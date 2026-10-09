use super::WinChild;
use crate::cmdbuilder::CommandBuilder;
use crate::win::procthreadattr::ProcThreadAttributeList;
use anyhow::{bail, ensure, Error};
use filedescriptor::{FileDescriptor, OwnedHandle};
use lazy_static::lazy_static;
use shared_library::shared_library;
use std::ffi::OsString;
use std::io::Error as IoError;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::{mem, ptr};
use winapi::shared::minwindef::DWORD;
use winapi::shared::winerror::{HRESULT, S_OK};
use winapi::um::handleapi::*;
use winapi::um::processthreadsapi::*;
use winapi::um::winbase::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, STARTUPINFOEXW,
};
use winapi::um::wincon::COORD;
use winapi::um::winnt::HANDLE;

#[allow(clippy::upper_case_acronyms)]
pub type HPCON = HANDLE;

/// Images for which CreateProcessW refused a job list (see spawn_command).
static JOBLIST_REFUSED: Mutex<Vec<Vec<u16>>> = Mutex::new(Vec::new());

fn joblist_refused(exe: &[u16]) -> bool {
    // A Store packaged image (a WindowsApps path: the execution alias the
    // Store pwsh is launched by, or the package's own copy) is known to refuse
    // when the caller has no package identity, as the server never does: skip
    // the failing CreateProcessW, which runs the whole package activation
    // before it refuses, about 160 ms on the first spawn of every server.
    let lower: Vec<u16> = exe.iter().map(|&c| if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c }).collect();
    let needle: Vec<u16> = "\\windowsapps\\".encode_utf16().collect();
    if lower.windows(needle.len()).any(|w| w == needle.as_slice()) {
        return true;
    }
    JOBLIST_REFUSED.lock().map(|v| v.iter().any(|e| e.as_slice() == exe)).unwrap_or(false)
}

/// Test hook (#686): `PSMUX_TEST_SLOW_CREATE_MS=<n>` holds every successful
/// CreateProcessW for n ms before the process is assigned to its job and
/// resumed, which is where a CreateProcessW that Windows is slow to return
/// leaves a spawner thread: the shell exists, suspended and outside its job,
/// and nothing has its pid yet. Measured in a sweep, the Store pwsh's
/// CreateProcessW had a median of 736 ms, so this window is real; the hook
/// makes it reproducible. Unset (the default) costs one cached env read.
fn slow_create_hook() {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let ms = *MS.get_or_init(|| {
        std::env::var("PSMUX_TEST_SLOW_CREATE_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0)
    });
    if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

fn remember_joblist_refused(exe: &[u16]) {
    if let Ok(mut v) = JOBLIST_REFUSED.lock() {
        if !v.iter().any(|e| e.as_slice() == exe) {
            v.push(exe.to_vec());
        }
    }
}

/// Deliberately absent from base_flags() (see the doc comment there): only the
/// regression test asserting that absence references it, hence test-gated.
#[cfg(test)]
pub const PSUEDOCONSOLE_INHERIT_CURSOR: DWORD = 0x1;
pub const PSEUDOCONSOLE_RESIZE_QUIRK: DWORD = 0x2;
pub const PSEUDOCONSOLE_WIN32_INPUT_MODE: DWORD = 0x4;
pub const PSEUDOCONSOLE_PASSTHROUGH_MODE: DWORD = 0x8;

shared_library!(ConPtyFuncs,
    pub fn CreatePseudoConsole(
        size: COORD,
        hInput: HANDLE,
        hOutput: HANDLE,
        flags: DWORD,
        hpc: *mut HPCON
    ) -> HRESULT,
    pub fn ResizePseudoConsole(hpc: HPCON, size: COORD) -> HRESULT,
    pub fn ClosePseudoConsole(hpc: HPCON),
);

/// Where the three ConPTY entry points came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConPtySource {
    /// The system implementation in kernel32.dll, which drives the inbox
    /// conhost.exe.  This is the default and the only source unless the user
    /// explicitly names a directory.
    Kernel32,
    /// A conpty.dll the user pointed at with `PSMUX_CONPTY_DIR`, which drives
    /// the `OpenConsole.exe` sitting next to it.
    Directory,
}

/// The three ConPTY entry points plus a note of where they came from.
///
/// The library handle behind the pointers is deliberately never freed: this
/// lives in a `lazy_static` for the life of the process and panes hold HPCONs
/// created by it.
/// The three entry points as the loader hands them out.  Named so the
/// transmutes below can say what they turn into (clippy 1.98's
/// `missing_transmute_annotations` refuses an unannotated one under
/// `-D warnings`).
pub type CreatePseudoConsoleFn =
    unsafe extern "system" fn(COORD, HANDLE, HANDLE, DWORD, *mut HPCON) -> HRESULT;
pub type ResizePseudoConsoleFn = unsafe extern "system" fn(HPCON, COORD) -> HRESULT;
pub type ClosePseudoConsoleFn = unsafe extern "system" fn(HPCON);

#[allow(non_snake_case)]
pub struct ConPtyApi {
    pub CreatePseudoConsole: CreatePseudoConsoleFn,
    pub ResizePseudoConsole: ResizePseudoConsoleFn,
    pub ClosePseudoConsole: ClosePseudoConsoleFn,
    /// Holds the kernel32 `DynamicLibrary` guard alive on the default path.
    _guard: Option<ConPtyFuncs>,
    pub source: ConPtySource,
}

impl std::fmt::Debug for ConPtyApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConPtyApi")
            .field("source", &self.source)
            .finish()
    }
}

unsafe impl Send for ConPtyApi {}
unsafe impl Sync for ConPtyApi {}

fn load_conpty_kernel32() -> ConPtyApi {
    // The system kernel32.dll ConPTY implementation: the default, and the only
    // one unless the user opts in by naming a directory (see below).
    //
    // Historically this loader was hard wired here because a conpty.dll picked
    // up ACCIDENTALLY off the DLL search order (terminal emulators such as
    // WezTerm ship their own conpty.dll + OpenConsole.exe, and psmux running
    // inside such a terminal could resolve theirs) gave blank panes and broken
    // I/O: their OpenConsole.exe need not accept our flag set
    // (PASSTHROUGH_MODE, WIN32_INPUT_MODE, ...).  That hazard is about an
    // IMPLICIT pickup.  `PSMUX_CONPTY_DIR` is the opposite: nothing but
    // kernel32 is ever loaded unless the user names the directory themselves.
    let funcs = ConPtyFuncs::open(Path::new("kernel32.dll")).expect(
        "this system does not support conpty.  Windows 10 October 2018 or newer is required",
    );
    // Fn-pointer transmute: the macro declares `extern "C"`, which is the same
    // ABI as `extern "system"` on the x86_64 Windows target psmux ships.
    unsafe {
        ConPtyApi {
            CreatePseudoConsole: mem::transmute::<
                unsafe extern "C" fn(COORD, HANDLE, HANDLE, DWORD, *mut HPCON) -> HRESULT,
                CreatePseudoConsoleFn,
            >(funcs.CreatePseudoConsole),
            ResizePseudoConsole: mem::transmute::<
                unsafe extern "C" fn(HPCON, COORD) -> HRESULT,
                ResizePseudoConsoleFn,
            >(funcs.ResizePseudoConsole),
            ClosePseudoConsole: mem::transmute::<unsafe extern "C" fn(HPCON), ClosePseudoConsoleFn>(
                funcs.ClosePseudoConsole,
            ),
            _guard: Some(funcs),
            source: ConPtySource::Kernel32,
        }
    }
}

fn wide_nul(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Load `<dir>\conpty.dll` by absolute path with `LOAD_WITH_ALTERED_SEARCH_PATH`,
/// so that the DLL's own directory (and therefore the `OpenConsole.exe` sitting
/// next to it, which conpty.dll spawns relative to its own module path) is what
/// gets used.  Returns a human readable reason on failure; the caller logs it
/// and falls back to kernel32.
fn load_conpty_from_dir(dir: &Path) -> Result<ConPtyApi, String> {
    use winapi::um::libloaderapi::{GetProcAddress, LoadLibraryExW};

    const LOAD_WITH_ALTERED_SEARCH_PATH: DWORD = 0x0000_0008;

    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let dll: PathBuf = dir.join("conpty.dll");
    if !dll.is_file() {
        return Err(format!("{} does not exist", dll.display()));
    }
    // LOAD_WITH_ALTERED_SEARCH_PATH requires an absolute path to mean anything.
    let abs = std::fs::canonicalize(&dll)
        .map_err(|e| format!("cannot resolve {}: {}", dll.display(), e))?;
    let wide = wide_nul(abs.as_os_str());
    let module =
        unsafe { LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
    if module.is_null() {
        return Err(format!(
            "LoadLibraryExW({}) failed: {}",
            abs.display(),
            IoError::last_os_error()
        ));
    }
    let resolve = |name: &[u8]| -> Result<*mut (), String> {
        let p = unsafe { GetProcAddress(module, name.as_ptr() as *const i8) };
        if p.is_null() {
            Err(format!(
                "{} exports no {}",
                abs.display(),
                String::from_utf8_lossy(&name[..name.len() - 1])
            ))
        } else {
            Ok(p as *mut ())
        }
    };
    let create = resolve(b"CreatePseudoConsole\0")?;
    let resize = resolve(b"ResizePseudoConsole\0")?;
    let close = resolve(b"ClosePseudoConsole\0")?;
    // The module handle is intentionally leaked: see ConPtyApi's doc comment.
    Ok(unsafe {
        ConPtyApi {
            CreatePseudoConsole: mem::transmute::<*mut (), CreatePseudoConsoleFn>(create),
            ResizePseudoConsole: mem::transmute::<*mut (), ResizePseudoConsoleFn>(resize),
            ClosePseudoConsole: mem::transmute::<*mut (), ClosePseudoConsoleFn>(close),
            _guard: None,
            source: ConPtySource::Directory,
        }
    })
}

/// Pick the ConPTY implementation.  `dir` is the value of `PSMUX_CONPTY_DIR`;
/// `None` (the default, unset) is kernel32 only, exactly as before.  A named
/// directory that cannot be loaded logs the reason and falls back to kernel32
/// rather than failing the pane.
fn resolve_conpty(dir: Option<PathBuf>) -> ConPtyApi {
    if let Some(dir) = dir {
        match load_conpty_from_dir(&dir) {
            Ok(api) => {
                log::info!(
                    "ConPTY loaded from PSMUX_CONPTY_DIR {} (conpty.dll + its OpenConsole.exe)",
                    dir.display()
                );
                return api;
            }
            Err(reason) => {
                log::warn!(
                    "PSMUX_CONPTY_DIR {} not used ({}); falling back to kernel32 ConPTY",
                    dir.display(),
                    reason
                );
            }
        }
    }
    load_conpty_kernel32()
}

fn conpty_dir_from_env() -> Option<PathBuf> {
    let v = std::env::var_os("PSMUX_CONPTY_DIR")?;
    if v.is_empty() {
        return None;
    }
    Some(PathBuf::from(v))
}

fn load_conpty() -> ConPtyApi {
    resolve_conpty(conpty_dir_from_env())
}

lazy_static! {
    static ref CONPTY: ConPtyApi = load_conpty();
}

/// Which ConPTY implementation this process ended up with.  Diagnostics only.
pub fn conpty_source() -> ConPtySource {
    CONPTY.source
}

pub struct PsuedoCon {
    con: HPCON,
    /// Whether this ConPTY was created with PSEUDOCONSOLE_PASSTHROUGH_MODE.
    /// Used by the retry logic in ConPtySlavePty::spawn_command to decide
    /// whether a fallback without passthrough is worth attempting.
    pub used_passthrough: bool,
}

unsafe impl Send for PsuedoCon {}
unsafe impl Sync for PsuedoCon {}

impl Drop for PsuedoCon {
    fn drop(&mut self) {
        unsafe { (CONPTY.ClosePseudoConsole)(self.con) };
    }
}

/// Returns true if the current Windows build supports ConPTY passthrough mode.
/// PSEUDOCONSOLE_PASSTHROUGH_MODE requires Windows 11 22H2 (build 22621+).
/// On older Windows versions, the flag may be silently accepted but produce
/// broken ConPTY output (no Win32 Console API translation).
///
/// Respects `PSMUX_NO_PASSTHROUGH=1` environment variable to let users
/// force-disable passthrough mode on builds where it causes CreateProcessW
/// to fail with ERROR_INVALID_PARAMETER (87).
fn supports_passthrough_mode() -> bool {
    if std::env::var("PSMUX_NO_PASSTHROUGH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        log::info!("ConPTY passthrough mode disabled via PSMUX_NO_PASSTHROUGH");
        return false;
    }
    let ver = unsafe {
        let mut info: winapi::um::winnt::OSVERSIONINFOW = mem::zeroed();
        info.dwOSVersionInfoSize = mem::size_of::<winapi::um::winnt::OSVERSIONINFOW>() as u32;
        // RtlGetVersion is used because GetVersionEx lies on Windows 10+
        // unless the application has a compatibility manifest.
        type RtlGetVersionFn = unsafe extern "system" fn(*mut winapi::um::winnt::OSVERSIONINFOW) -> i32;
        let ntdll = winapi::um::libloaderapi::GetModuleHandleW(
            ['n' as u16, 't' as u16, 'd' as u16, 'l' as u16, 'l' as u16, '.' as u16,
             'd' as u16, 'l' as u16, 'l' as u16, 0].as_ptr()
        );
        if ntdll.is_null() {
            return false;
        }
        let func = winapi::um::libloaderapi::GetProcAddress(
            ntdll,
            b"RtlGetVersion\0".as_ptr() as *const i8,
        );
        if func.is_null() {
            return false;
        }
        let rtl_get_version: RtlGetVersionFn = mem::transmute(func);
        rtl_get_version(&mut info);
        info
    };
    // Windows 11 22H2 = build 22621
    ver.dwBuildNumber >= 22621
}

/// The flag set passed to every CreatePseudoConsole call (passthrough is
/// OR'ed on separately where supported).
///
/// PSUEDOCONSOLE_INHERIT_CURSOR is deliberately NOT set. With it, conhost emits
/// an ESC[6n cursor-position request at startup and will not service a child's
/// console connection until the host answers it. So if that reply is sent later
/// than the child's connect attempt, the child blocks in
/// ConsoleCreateConnectionObject during process initialization (a single
/// thread, before any user code runs) until the reply arrives: a temporary
/// stall if it is merely late, indefinite if it never comes. A multiplexer pane
/// always starts on a fresh screen, so inheriting the host cursor row buys
/// nothing here.
fn base_flags() -> DWORD {
    PSEUDOCONSOLE_RESIZE_QUIRK | PSEUDOCONSOLE_WIN32_INPUT_MODE
}

impl PsuedoCon {
    pub fn new(size: COORD, input: FileDescriptor, output: FileDescriptor) -> Result<Self, Error> {
        let mut con: HPCON = INVALID_HANDLE_VALUE;
        let base_flags = base_flags();

        // Use PSEUDOCONSOLE_PASSTHROUGH_MODE on Windows 11 22H2+ to relay
        // VT sequences (including DECSCUSR cursor shapes) from child processes
        // directly through the output pipe.  On older Windows, this flag is
        // silently accepted but breaks Win32 Console API translation, so we
        // only attempt it on known-good builds.
        if supports_passthrough_mode() {
            let result = unsafe {
                (CONPTY.CreatePseudoConsole)(
                    size,
                    input.as_raw_handle() as _,
                    output.as_raw_handle() as _,
                    base_flags | PSEUDOCONSOLE_PASSTHROUGH_MODE,
                    &mut con,
                )
            };

            if result == S_OK {
                return Ok(Self { con, used_passthrough: true });
            }
            // If the API call failed despite being on a supported build,
            // fall through to the standard path.
            con = INVALID_HANDLE_VALUE;
        }

        let result = unsafe {
            (CONPTY.CreatePseudoConsole)(
                size,
                input.as_raw_handle() as _,
                output.as_raw_handle() as _,
                base_flags,
                &mut con,
            )
        };
        ensure!(
            result == S_OK,
            "failed to create psuedo console: HRESULT {}",
            result
        );
        Ok(Self { con, used_passthrough: false })
    }

    /// Create a ConPTY explicitly without passthrough mode, regardless of
    /// Windows build version.  Used by the retry logic when CreateProcessW
    /// rejects the passthrough ConPTY handle.
    pub fn new_without_passthrough(size: COORD, input: FileDescriptor, output: FileDescriptor) -> Result<Self, Error> {
        let mut con: HPCON = INVALID_HANDLE_VALUE;
        let base_flags = base_flags();

        let result = unsafe {
            (CONPTY.CreatePseudoConsole)(
                size,
                input.as_raw_handle() as _,
                output.as_raw_handle() as _,
                base_flags,
                &mut con,
            )
        };
        ensure!(
            result == S_OK,
            "failed to create psuedo console (no passthrough): HRESULT {}",
            result
        );
        Ok(Self { con, used_passthrough: false })
    }

    pub fn resize(&self, size: COORD) -> Result<(), Error> {
        let result = unsafe { (CONPTY.ResizePseudoConsole)(self.con, size) };
        ensure!(
            result == S_OK,
            "failed to resize console to {}x{}: HRESULT: {}",
            size.X,
            size.Y,
            result
        );
        Ok(())
    }

    pub fn spawn_command(&self, cmd: CommandBuilder) -> anyhow::Result<WinChild> {
        let mut si: STARTUPINFOEXW = unsafe { mem::zeroed() };
        si.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
        // Note: we deliberately do NOT set STARTF_USESTDHANDLES with
        // INVALID_HANDLE_VALUE for stdio.  MSDN explicitly requires
        // STARTF_USESTDHANDLES to be paired with bInheritHandles=TRUE,
        // and we use bInheritHandles=FALSE below.  Most Windows builds
        // tolerate the combination silently (because INVALID_HANDLE_VALUE
        // is a sentinel rather than a real handle), but newer/restricted
        // configurations — Win 11 26200, Microsoft-account profiles with
        // tighter token policies, certain WDAC/AppLocker rule sets — now
        // enforce the contract strictly and reject the call with
        // ERROR_INVALID_PARAMETER (87).  See psmux issue #167.
        //
        // ConPTY routes stdio through the PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE
        // attribute on the attribute list, so the child gets correct stdio
        // regardless of dwFlags.  bInheritHandles=FALSE prevents leaking
        // any other inheritable handles.

        // The pane process is created INSIDE a job of its own (see PaneJob),
        // atomically: no window exists in which it, or anything it starts,
        // lives outside that job.
        use super::spawn_trace as st;
        let t_total = st::now_us();
        let t = st::now_us();
        let mut job = super::PaneJob::create();
        st::step(t, || format!("spawn.job_create ok={}", job.is_some()));
        let t = st::now_us();
        let mut attrs = ProcThreadAttributeList::with_capacity(2)?;
        attrs.set_pty(self.con)?;
        if let Some(j) = job.as_ref() {
            if attrs.set_job(j.handle()).is_err() {
                job = None;
                attrs = ProcThreadAttributeList::with_capacity(1)?;
                attrs.set_pty(self.con)?;
            }
        }
        si.lpAttributeList = attrs.as_mut_ptr();
        st::step(t, || "spawn.attr_list");

        let mut pi: PROCESS_INFORMATION = unsafe { mem::zeroed() };

        let t = st::now_us();
        let (mut exe, mut cmdline) = cmd.cmdline()?;
        if st::enabled() {
            st::step(t, || format!("spawn.cmdline exe={}", String::from_utf16_lossy(&exe).trim_end_matches('\0')));
        }
        let cmd_os = OsString::from_wide(&cmdline);

        let cwd = cmd.current_directory();

        // The child's ProcessParameters std handles are stamped from this
        // process's std handle slots at CreateProcessW time.  Enter the console
        // state so no FreeConsole/AttachConsole dance (Ctrl+C delivery,
        // mouse/VT injection) is mid-flight on another thread, and park the
        // slots on NULL for the duration of the call: after any dance the
        // slots of a headless server dangle on freed, recycled handle values,
        // and a child born from them dies at its first console read
        // (issue #450).  A NULL-std parent is the GUI-parent case, for which
        // conhost always hands the child fresh handles to its own console.
        //
        // The guard is SHARED between concurrent spawns and refcounts the
        // parking, because every spawn wants the same parked state; only an
        // identity change is exclusive.  Serialising spawns against each other
        // is what made a surge of spares cost one CreateProcessW after another
        // (issue #686).
        let t = st::now_us();
        let _console_guard = crate::ConPtySpawnGuard::acquire();
        st::step(t, || format!("spawn.console_guard_acquire shared_wait_us={}", crate::last_spawn_console_wait_us()));

        let exe_key = exe.clone();
        let t = st::now_us();
        let mut env_block = cmd.environment_block();
        st::step(t, || "spawn.env_block");
        // Every CreateProcessW is timed into `last_spawn_create_us`, so the
        // time the OS spends creating the process can be told apart from the
        // time psmux spends around it (see that function).
        let mut create = |si: &mut STARTUPINFOEXW, pi: &mut PROCESS_INFORMATION, extra: DWORD| unsafe {
            let _timed = crate::SpawnCreateTimer::start();
            CreateProcessW(
                exe.as_mut_slice().as_mut_ptr(),
                cmdline.as_mut_slice().as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | extra,
                env_block.as_mut_slice().as_mut_ptr() as *mut _,
                cwd.as_ref()
                    .map(|c| c.as_slice().as_ptr())
                    .unwrap_or(ptr::null()),
                &mut si.StartupInfo,
                pi,
            )
        };
        // Three ways in, best first:
        //  1. the job list attribute: the process is born in its pane job.
        //  2. Windows refuses a job list with ERROR_ACCESS_DENIED when an
        //     UNPACKAGED caller, as the server is, creates a Store packaged
        //     image: the pwsh execution alias and its real path under
        //     Program Files\WindowsApps alike, measured from an unpackaged C#
        //     probe with or without a console or an OS compatibility manifest.
        //     The same probe hosted in the Store pwsh (a caller WITH package
        //     identity) is accepted for both, which is why a pane's shell or a
        //     run-shell child never sees the refusal.  cmd.exe is accepted.
        //     Such an image is created suspended, assigned to the job that
        //     already exists, then resumed, so the window outside the job is
        //     the one assign call.  The refusal is remembered per image, so it
        //     is paid once per process.  Measured, the suspended path costs
        //     nothing the user sees: AssignProcessToJobObject ~30 us,
        //     ResumeThread ~5 us, and launch to prompt of the shell is the same
        //     as an unsuspended create.
        //  3. Without a job, exactly as before.
        let mut res = 0;
        let mut create_err = IoError::from_raw_os_error(0);
        let t = st::now_us();
        let refused = joblist_refused(&exe_key);
        st::step(t, || format!("spawn.joblist_refused_check refused={}", refused));
        if job.is_some() && !refused {
            let t = st::now_us();
            res = create(&mut si, &mut pi, 0);
            create_err = IoError::last_os_error();
            st::step(t, || format!("spawn.CreateProcessW kind=joblist ok={} err={}", res != 0, create_err.raw_os_error().unwrap_or(0)));
            if res == 0 {
                remember_joblist_refused(&exe_key);
                // The refused attempt leaves THAT job unusable for the assign
                // below (measured: the first spawn of every server failed to
                // assign), so the suspended path gets a fresh one.
                job = super::PaneJob::create();
            }
        }
        let mut assign_after = false;
        if res == 0 {
            attrs = ProcThreadAttributeList::with_capacity(1)?;
            attrs.set_pty(self.con)?;
            si.lpAttributeList = attrs.as_mut_ptr();
            let suspended = job.is_some();
            let t = st::now_us();
            res = create(&mut si, &mut pi, if suspended { CREATE_SUSPENDED } else { 0 });
            create_err = IoError::last_os_error();
            st::step(t, || format!("spawn.CreateProcessW kind={} ok={} err={}", if suspended { "suspended" } else { "plain" }, res != 0, create_err.raw_os_error().unwrap_or(0)));
            assign_after = suspended && res != 0;
            if res == 0 && suspended {
                job = None;
                let t = st::now_us();
                res = create(&mut si, &mut pi, 0);
                create_err = IoError::last_os_error();
                st::step(t, || format!("spawn.CreateProcessW kind=retry_plain ok={}", res != 0));
            }
        }
        if res != 0 {
            slow_create_hook();
        }
        if assign_after {
            let t = st::now_us();
            if !job.as_ref().is_some_and(|j| j.assign(pi.hProcess)) {
                job = None;
            }
            st::step(t, || format!("spawn.AssignProcessToJobObject ok={}", job.is_some()));
            let t = st::now_us();
            unsafe { ResumeThread(pi.hThread) };
            st::step(t, || "spawn.ResumeThread");
        }
        st::step(t_total, || format!("spawn.total pid={}", pi.dwProcessId));
        // The std handle slots are restored when `_console_guard` drops, by the
        // last spawn still inside the console state.
        if res == 0 {
            let err = create_err;
            let msg = format!(
                "CreateProcessW `{:?}` in cwd `{:?}` failed: {}",
                cmd_os,
                cwd.as_ref().map(|c| OsString::from_wide(c)),
                err
            );
            log::error!("{}", msg);
            bail!("{}", msg);
        }

        // Make sure we close out the thread handle so we don't leak it;
        // we do this simply by making it owned
        let _main_thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread as _) };
        let proc = unsafe { OwnedHandle::from_raw_handle(pi.hProcess as _) };

        Ok(WinChild {
            proc: Mutex::new(proc),
            job,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conpty_flags_do_not_include_inherit_cursor() {
        // PSUEDOCONSOLE_INHERIT_CURSOR makes the new conhost emit ESC[6n at
        // startup and block its own initialization until the host replies; an
        // unanswered query leaves conhost unable to service the child's
        // console connect, so the child hangs inside process initialization
        // (single thread parked in ConsoleCreateConnectionObject). A
        // multiplexer pane always starts on a fresh screen, so inheriting the
        // host cursor row has no value here.
        assert_eq!(
            base_flags() & PSUEDOCONSOLE_INHERIT_CURSOR,
            0,
            "INHERIT_CURSOR must not be set: it makes conhost block startup waiting for an ESC[6n reply"
        );
        // The other flags are load-bearing and must stay.
        assert_ne!(base_flags() & PSEUDOCONSOLE_RESIZE_QUIRK, 0);
        assert_ne!(base_flags() & PSEUDOCONSOLE_WIN32_INPUT_MODE, 0);
    }

    /// Unset PSMUX_CONPTY_DIR is the default and must still be kernel32.
    #[test]
    fn conpty_defaults_to_kernel32() {
        assert_eq!(resolve_conpty(None).source, ConPtySource::Kernel32);
    }

    /// A directory that does not exist must not fail the pane: it logs and
    /// falls back to the system implementation.
    #[test]
    fn conpty_dir_missing_falls_back_to_kernel32() {
        let missing = std::env::temp_dir().join("psmux-no-such-conpty-dir-3f9a1c");
        assert!(!missing.exists(), "test fixture directory must not exist");
        assert_eq!(
            resolve_conpty(Some(missing.clone())).source,
            ConPtySource::Kernel32
        );
        assert!(load_conpty_from_dir(&missing).is_err());
    }

    /// A directory with no conpty.dll in it falls back too.
    #[test]
    fn conpty_dir_without_dll_falls_back_to_kernel32() {
        let dir = std::env::temp_dir().join("psmux-conpty-empty-3f9a1c");
        std::fs::create_dir_all(&dir).unwrap();
        let err = load_conpty_from_dir(&dir).unwrap_err();
        assert!(err.contains("does not exist"), "unexpected reason: {}", err);
        assert_eq!(resolve_conpty(Some(dir)).source, ConPtySource::Kernel32);
    }

    /// A conpty.dll that is not a loadable DLL at all falls back with the
    /// LoadLibraryExW error rather than propagating a failure.
    #[test]
    fn conpty_dir_with_bad_dll_falls_back_to_kernel32() {
        let dir = std::env::temp_dir().join("psmux-conpty-bad-3f9a1c");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("conpty.dll"), b"not a PE image at all").unwrap();
        let err = load_conpty_from_dir(&dir).unwrap_err();
        assert!(
            err.contains("LoadLibraryExW"),
            "unexpected reason: {}",
            err
        );
        assert_eq!(resolve_conpty(Some(dir)).source, ConPtySource::Kernel32);
    }
}
