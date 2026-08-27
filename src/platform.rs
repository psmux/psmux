// ---------------------------------------------------------------------------
// CREATE_NO_WINDOW for background subprocesses
// ---------------------------------------------------------------------------

/// Windows `CREATE_NO_WINDOW` flag (0x08000000).
///
/// When set on `CreateProcess`, the child process does not get a console
/// window allocated by conhost.  This is the correct flag for *helper*
/// subprocesses (format `#()` expansion, `run-shell`, `if-shell`, clipboard
/// pipes, plugin scripts) that only need stdin/stdout/stderr pipes.
///
/// **Important:** PTY/ConPTY child processes and psmux server processes must
/// NOT use this flag because they need a real console session.  Those use
/// `spawn_server_hidden()` (with `CREATE_NEW_CONSOLE` + `SW_HIDE`) instead.
///
/// On non-Windows platforms this is a no-op.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Extension trait that adds `.hide_window()` to `std::process::Command`.
///
/// Call this on any `Command` that spawns a background helper process.
/// On Windows it sets `CREATE_NO_WINDOW` so no cmd.exe / conhost.exe
/// window flashes on screen.  On other platforms it does nothing.
///
/// # Example
/// ```ignore
/// use crate::platform::HideWindowCommandExt;
/// std::process::Command::new("cmd")
///     .args(["/C", "echo hello"])
///     .hide_window()
///     .output();
/// ```
pub trait HideWindowCommandExt {
    fn hide_window(&mut self) -> &mut Self;
}

#[cfg(windows)]
impl HideWindowCommandExt for std::process::Command {
    fn hide_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }
}

#[cfg(not(windows))]
impl HideWindowCommandExt for std::process::Command {
    fn hide_window(&mut self) -> &mut Self {
        self // no-op
    }
}

// ---------------------------------------------------------------------------

/// Escape a single argument for a Windows command line per Microsoft's
/// `CommandLineToArgvW` parsing rules (the same algorithm Rust's
/// `std::process::Command` uses internally).
///
/// Rules: an argument is wrapped in `"..."` when it is empty or contains
/// whitespace / `"`. Inside the quotes, every embedded `"` is escaped as
/// `\"`, and any backslash run that immediately precedes a `"` (including
/// the closing quote) must be doubled. Backslashes in other positions
/// pass through unchanged — important on Windows where they are the path
/// separator (e.g. `C:\Program Files\...`).
///
/// Returns the argument verbatim when no quoting is needed.
#[cfg(windows)]
pub(crate) fn escape_arg_msvcrt(arg: &str) -> String {
    let needs_quoting = arg.is_empty()
        || arg.chars().any(|c| c == ' ' || c == '\t' || c == '\n' || c == '\x0b' || c == '"');
    if !needs_quoting {
        return arg.to_string();
    }

    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes: usize = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else if c == '"' {
            // 2N+1 backslashes followed by `"` = N literal backslashes + literal `"`
            for _ in 0..(backslashes * 2 + 1) { out.push('\\'); }
            out.push('"');
            backslashes = 0;
        } else {
            for _ in 0..backslashes { out.push('\\'); }
            out.push(c);
            backslashes = 0;
        }
    }
    // Closing quote: any trailing backslashes must be doubled so the
    // receiver does not see them as escaping the closing quote.
    for _ in 0..(backslashes * 2) { out.push('\\'); }
    out.push('"');
    out
}

/// Spawn a server process with a hidden console window on Windows.
///
/// Uses raw `CreateProcessW` with `STARTF_USESHOWWINDOW` + `SW_HIDE` and
/// `CREATE_NEW_CONSOLE` so that ConPTY has a real console session while the
/// window remains invisible.  This replicates the behaviour of
/// `Start-Process -WindowStyle Hidden` in PowerShell.
#[cfg(windows)]
pub fn spawn_server_hidden(exe: &std::path::Path, args: &[String]) -> std::io::Result<u32> {
    #[repr(C)]
    #[allow(non_snake_case)]
    struct STARTUPINFOW {
        cb: u32,
        lpReserved: *mut u16,
        lpDesktop: *mut u16,
        lpTitle: *mut u16,
        dwX: u32,
        dwY: u32,
        dwXSize: u32,
        dwYSize: u32,
        dwXCountChars: u32,
        dwYCountChars: u32,
        dwFillAttribute: u32,
        dwFlags: u32,
        wShowWindow: u16,
        cbReserved2: u16,
        lpReserved2: *mut u8,
        hStdInput: isize,
        hStdOutput: isize,
        hStdError: isize,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct PROCESS_INFORMATION {
        hProcess: isize,
        hThread: isize,
        dwProcessId: u32,
        dwThreadId: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateProcessW(
            lpApplicationName: *const u16,
            lpCommandLine: *mut u16,
            lpProcessAttributes: *const std::ffi::c_void,
            lpThreadAttributes: *const std::ffi::c_void,
            bInheritHandles: i32,
            dwCreationFlags: u32,
            lpEnvironment: *const std::ffi::c_void,
            lpCurrentDirectory: *const u16,
            lpStartupInfo: *const STARTUPINFOW,
            lpProcessInformation: *mut PROCESS_INFORMATION,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    const STARTF_USESHOWWINDOW: u32 = 0x00000001;
    const SW_HIDE: u16 = 0;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;

    // Build command line: "exe" arg1 arg2 ...
    // Each argument is escaped per Microsoft's CommandLineToArgvW rules
    // (see `escape_arg_msvcrt` below). The naive `arg.replace('"', "\\\"")`
    // approach mishandles values whose closing context is a backslash run
    // (e.g. `C:\Foo\` ends up serialised as `"C:\Foo\"` where the trailing
    // `\"` is interpreted by the receiver as an escaped quote, swallowing
    // the next argument). Issue #265.
    let mut cmdline = format!("\"{}\"", exe.display());
    for arg in args {
        cmdline.push(' ');
        cmdline.push_str(&escape_arg_msvcrt(arg));
    }
    let mut cmdline_wide: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESHOWWINDOW;
    si.wShowWindow = SW_HIDE;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // Try with CREATE_BREAKAWAY_FROM_JOB first so the server escapes the
    // parent's Job Object (e.g. sshd's kill-on-close job).  If the job
    // disallows breakaway the call fails with ERROR_ACCESS_DENIED; in
    // that case fall back without the flag.
    let base_flags = CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP;
    let mut ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmdline_wide.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0, // don't inherit handles
            base_flags | CREATE_BREAKAWAY_FROM_JOB,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        )
    };

    if ok == 0 {
        // Retry without breakaway (job may disallow it)
        // Re-encode cmdline_wide since CreateProcessW may have modified it
        cmdline_wide = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
        ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmdline_wide.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                base_flags,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            )
        };
    }

    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Capture the server PID before closing handles so callers can poll the
    // process for liveness (used by the new-session readiness gate to fail fast
    // if the server dies, instead of waiting out the readiness deadline).
    let server_pid = pi.dwProcessId;

    // Close handles – we don't need to wait for the child.
    unsafe {
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }

    Ok(server_pid)
}

/// Return true while the process with `pid` is still running.
///
/// Used by the new-session readiness gate as a fast-fail signal: if the freshly
/// spawned server process dies (hard kill, abrupt exit, or any path that skips
/// the server's panic hook and therefore does NOT remove the .port file), the
/// client stops waiting immediately rather than blocking until the readiness
/// deadline.
///
/// Conservative by design: only reports "dead" on a positive signal. The PID
/// belongs to a server we just spawned as the SAME user, so OpenProcess with
/// SYNCHRONIZE access is granted while it is alive; a failure to open the
/// (still very young) PID is the exit signal. WaitForSingleObject(0) avoids the
/// classic GetExitCodeProcess/STILL_ACTIVE(259) ambiguity. Callers only consult
/// this AFTER a readiness check fails for the current iteration, so a healthy,
/// reachable server is never declared dead.
#[cfg(windows)]
pub fn process_is_alive(pid: u32) -> bool {
    // OpenProcess / CloseHandle use the isize handle convention shared by the
    // rest of platform.rs; WaitForSingleObject uses the *mut c_void handle to
    // match ssh_input.rs's declaration of the same symbol (avoids a cross-module
    // clashing-extern warning). The two are ABI-identical, so we cast at the
    // call site.
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> isize;
        fn WaitForSingleObject(hHandle: *mut std::ffi::c_void, dwMilliseconds: u32) -> u32;
        fn CloseHandle(handle: isize) -> i32;
    }
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if h == 0 {
            // PID no longer openable -> the process has exited.
            return false;
        }
        let r = WaitForSingleObject(h as *mut std::ffi::c_void, 0);
        CloseHandle(h);
        // WAIT_TIMEOUT => handle not signaled => still running.
        // WAIT_OBJECT_0 (0) => signaled => the process has exited.
        r == WAIT_TIMEOUT
    }
}

#[cfg(not(windows))]
pub fn process_is_alive(_pid: u32) -> bool {
    // Non-Windows builds do not plumb the server PID into the readiness gate;
    // treat as alive so the gate falls back to the .port-vanish + deadline
    // signals rather than ever false-failing.
    true
}

/// Single-server-per-session-name lock (RAII). Holding the guard means this
/// process owns the right to be THE server for a given session name. Dropping it
/// (or the process exiting) releases the underlying Windows named mutex, which
/// the OS also auto-releases on a crash — so there is no stale-lock to reap.
#[cfg(windows)]
pub struct SessionMutex { handle: *mut std::ffi::c_void }
#[cfg(windows)]
unsafe impl Send for SessionMutex {}
#[cfg(windows)]
impl Drop for SessionMutex {
    fn drop(&mut self) {
        #[link(name = "kernel32")]
        extern "system" {
            fn ReleaseMutex(h: *mut std::ffi::c_void) -> i32;
            fn CloseHandle(h: isize) -> i32;
        }
        if !self.handle.is_null() {
            unsafe {
                // Give up ownership explicitly before closing. Closing alone frees
                // the name only when ours is the LAST handle; if any other process
                // happens to hold one open at that instant (a concurrent probe from
                // a starting server), the object outlives our close and would stay
                // owned by a thread that has moved on. Releasing first makes the
                // handover deterministic, which is what re-keying the guard across
                // a rename depends on (issue #505). Harmlessly returns 0 when this
                // thread does not own the mutex.
                ReleaseMutex(self.handle);
                CloseHandle(self.handle as isize);
            }
        }
    }
}

/// Name of the machine-wide mutex that guards one server per session base name.
///
/// Two parts carry meaning:
///
///   - the data-root tag (issue #599). The registry this guard protects is
///     rooted at `PSMUX_DATA_DIR`, but a kernel object name is machine-wide, so
///     without the root in the key two isolated registries collide on one
///     session name: the second root's `new-session -s X` is refused as a
///     duplicate of a server it cannot see, and the fixed `__warm__` name is
///     publishable by only the first root on the box. `-L` already reaches this
///     name through `port_file_base()`; the tag makes psmux's other namespace
///     mechanism agree.
///   - `base`, which is `{socket_name}__{session_name}` under `-L` and the bare
///     session name otherwise.
///
/// Backslash is the kernel-object namespace separator and must not appear in the
/// leaf name; path characters are mapped out. `Local\` scopes the object to this
/// logon session, matching the per-user scope of the data directory.
#[cfg(windows)]
pub fn session_mutex_name(base: &str) -> String {
    let sanitized: String = base.chars().map(|c| if c == '\\' || c == '/' { '_' } else { c }).collect();
    format!("Local\\psmux-session-{}-{}", crate::paths::data_root_tag(), sanitized)
}

/// Acquire the single-server lock for session `name` (P0: kill the duplicate-
/// same-name-server race). Returns `Some(guard)` when this process MAY run as
/// the server — because it now owns the mutex, or a previous owner died and left
/// it abandoned, or the FFI was unavailable (**fail-open**, so a legitimate start
/// is never blocked). Returns `None` ONLY when another LIVE process already owns
/// the name, i.e. this is a duplicate cold-spawn that must exit.
#[cfg(windows)]
pub fn acquire_session_mutex(name: &str) -> Option<SessionMutex> {
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attrs: *const std::ffi::c_void, initial_owner: i32, name: *const u16) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(h: *mut std::ffi::c_void, ms: u32) -> u32;
        fn CloseHandle(h: isize) -> i32;
    }
    const WAIT_OBJECT_0: u32 = 0x0000_0000;
    const WAIT_ABANDONED: u32 = 0x0000_0080; // prior owner died holding it -> ours now
    const WAIT_TIMEOUT: u32 = 0x0000_0102;   // another live process owns it
    let obj = session_mutex_name(name);
    let wide: Vec<u16> = obj.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, wide.as_ptr());
        if h.is_null() {
            return Some(SessionMutex { handle: std::ptr::null_mut() }); // fail-open
        }
        match WaitForSingleObject(h, 0) {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Some(SessionMutex { handle: h }),
            WAIT_TIMEOUT => { CloseHandle(h as isize); None }
            _ => Some(SessionMutex { handle: h }), // unknown -> fail-open
        }
    }
}

#[cfg(not(windows))]
pub struct SessionMutex;
/// Non-Windows: no cross-process named mutex plumbed; fail-open (never block a
/// legitimate start). psmux's duplicate-server race is a Windows-only concern.
#[cfg(not(windows))]
pub fn acquire_session_mutex(_name: &str) -> Option<SessionMutex> { Some(SessionMutex) }

/// Enable virtual terminal processing on Windows Console Host.
/// This is required for ANSI color codes to work in conhost.exe (legacy console).
#[cfg(windows)]
pub fn enable_virtual_terminal_processing() {
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    const CP_UTF8: u32 = 65001;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, dwMode: u32) -> i32;
        fn SetConsoleOutputCP(wCodePageID: u32) -> i32;
        fn SetConsoleCP(wCodePageID: u32) -> i32;
    }

    unsafe {
        // Set console code page to UTF-8 so multi-byte Unicode characters
        // (e.g. ▶ U+25B6, ◀ U+25C0) render correctly instead of as mojibake.
        SetConsoleOutputCP(CP_UTF8);
        SetConsoleCP(CP_UTF8);

        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if !handle.is_null() {
            let mut mode: u32 = 0;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn enable_virtual_terminal_processing() {
    // No-op on non-Windows platforms
}

/// Issue #473: query the HOST terminal for its colors (OSC 10/11 fg/bg, the
/// OSC 4 16-color palette, and the CSI ?996n light/dark scheme) at client
/// attach time, so the server can answer the same queries when pane
/// applications (GitHub Copilot CLI, vim, ...) issue them.
///
/// Writes the queries plus a DA1 (`CSI c`) sentinel to stdout and drains
/// console input until the DA1 reply arrives (every terminal answers DA1) or
/// a 500ms deadline passes.  Runs BEFORE the client's input pump starts, so
/// the replies cannot be misparsed as keystrokes.  Returns the colors in
/// `HostColors::to_spec` wire form, or None when stdin is not a console or
/// the host reported nothing useful.
///
/// The `PSMUX_HOST_COLORS` environment variable short-circuits the query and
/// is also the escape hatch for hosts that misreport.
pub fn query_host_terminal_colors() -> Option<String> {
    if let Ok(v) = std::env::var("PSMUX_HOST_COLORS") {
        let hc = crate::types::HostColors::from_spec(&v);
        if hc.has_any() || hc.dark.is_some() {
            return Some(hc.to_spec());
        }
    }
    // Never interrogate psmux itself.  When this client runs inside a psmux
    // pane or popup, the "host terminal" is psmux, and psmux answers these
    // queries by injecting the replies as console KEY_EVENT records into the
    // child's input buffer (server::helpers::answer_color_queries ->
    // send_vt_response).  That injection is asynchronous: it happens on a later
    // server tick, routinely after the 500ms drain below has given up, because
    // psmux never answers the DA1 sentinel that would end the drain early.
    // Whatever lands late stays queued in the console input buffer, and the
    // client's normal input pump then reads it as keystrokes and forwards it to
    // the session it is attached to, typing `ESC]10;rgb:...` garbage into that
    // pane.  The parent server plants the real terminal's colors in
    // PSMUX_HOST_COLORS instead (pane::set_host_colors_env), which the
    // short-circuit above picks up, so nesting keeps the right palette without
    // ever putting a query on the wire.
    if crate::util::psmux_drawn_terminal() {
        return None;
    }
    query_host_terminal_colors_impl()
}

#[cfg(not(windows))]
fn query_host_terminal_colors_impl() -> Option<String> { None }

#[cfg(windows)]
fn query_host_terminal_colors_impl() -> Option<String> {
    use std::io::Write as _;

    const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    const KEY_EVENT: u16 = 0x0001;

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KeyEventRecord {
        key_down: i32,
        repeat_count: u16,
        virtual_key_code: u16,
        virtual_scan_code: u16,
        u_char: u16,
        control_key_state: u32,
    }
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct InputRecord {
        event_type: u16,
        _padding: u16,
        event: KeyEventRecord,
        // KEY_EVENT_RECORD is the largest union member; no extra space needed.
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, dwMode: u32) -> i32;
        fn GetNumberOfConsoleInputEvents(hConsoleInput: *mut std::ffi::c_void, lpcNumberOfEvents: *mut u32) -> i32;
        // Buffer is untyped at the ABI: each module keeps its own view of
        // INPUT_RECORD, so every extern declaration of this function in the
        // crate uses *mut c_void (clashing_extern_declarations).
        fn ReadConsoleInputW(hConsoleInput: *mut std::ffi::c_void, lpBuffer: *mut std::ffi::c_void, nLength: u32, lpNumberOfEventsRead: *mut u32) -> i32;
    }

    unsafe {
        let h_in = GetStdHandle(STD_INPUT_HANDLE);
        if h_in.is_null() || h_in == (-1isize) as *mut std::ffi::c_void {
            return None;
        }
        let mut orig_mode: u32 = 0;
        if GetConsoleMode(h_in, &mut orig_mode) == 0 {
            return None; // stdin is not a console (e.g. SSH pipe)
        }
        // Raw + VTI: the host's reply bytes must arrive verbatim as KEY_EVENT
        // u_char records; without VTI conhost tries to translate the OSC
        // sequences into key encodings and mangles them.
        let raw_mode = (orig_mode & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        SetConsoleMode(h_in, raw_mode);

        let mut queries = String::from("\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
        for i in 0..16 {
            queries.push_str(&format!("\x1b]4;{};?\x1b\\", i));
        }
        queries.push_str("\x1b[?996n");
        queries.push_str("\x1b[c"); // DA1 sentinel: always answered, marks the end
        {
            let mut out = std::io::stdout();
            if out.write_all(queries.as_bytes()).is_err() || out.flush().is_err() {
                SetConsoleMode(h_in, orig_mode);
                return None;
            }
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut buf: Vec<u8> = Vec::with_capacity(1024);
        let mut records: [InputRecord; 64] = [InputRecord {
            event_type: 0, _padding: 0,
            event: KeyEventRecord { key_down: 0, repeat_count: 0, virtual_key_code: 0, virtual_scan_code: 0, u_char: 0, control_key_state: 0 },
        }; 64];
        'read: while std::time::Instant::now() < deadline {
            let mut avail: u32 = 0;
            if GetNumberOfConsoleInputEvents(h_in, &mut avail) == 0 { break; }
            if avail == 0 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            }
            let mut read: u32 = 0;
            if ReadConsoleInputW(h_in, records.as_mut_ptr() as *mut _, 64, &mut read) == 0 { break; }
            for rec in records.iter().take(read as usize) {
                if rec.event_type != KEY_EVENT || rec.event.key_down == 0 { continue; }
                let wch = rec.event.u_char;
                if wch == 0 { continue; }
                if wch < 0x80 {
                    buf.push(wch as u8);
                } else if let Some(c) = char::from_u32(wch as u32) {
                    let mut utf8 = [0u8; 4];
                    buf.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
                }
            }
            // Stop as soon as the DA1 reply (CSI ? ... c) is present.
            if find_csi_terminated(&buf, b'c') { break 'read; }
        }
        SetConsoleMode(h_in, orig_mode);

        let hc = parse_host_color_replies(&buf);
        if hc.has_any() || hc.dark.is_some() {
            Some(hc.to_spec())
        } else {
            None
        }
    }
}

/// True when `buf` contains a complete `CSI ? ... <final>` sequence with the
/// given final byte (used to spot the DA1 `\x1b[?...c` sentinel reply).
#[cfg(windows)]
fn find_csi_terminated(buf: &[u8], final_byte: u8) -> bool {
    let mut i = 0;
    while i + 2 < buf.len() {
        if buf[i] == 0x1b && buf[i + 1] == b'[' && buf[i + 2] == b'?' {
            let mut j = i + 3;
            while j < buf.len() {
                let b = buf[j];
                if b.is_ascii_alphabetic() {
                    if b == final_byte { return true; }
                    break;
                }
                j += 1;
            }
            i = j;
        }
        i += 1;
    }
    false
}

/// Parse the host terminal's replies to the color queries issued by
/// `query_host_terminal_colors`: OSC 10/11/4 color reports (BEL- or
/// ST-terminated) and the CSI ?997;1n / ?997;2n dark/light report.
pub fn parse_host_color_replies(buf: &[u8]) -> crate::types::HostColors {
    let mut hc = crate::types::HostColors::empty();
    let text = String::from_utf8_lossy(buf);
    // OSC replies: \x1b]<num>;[<idx>;]<color> terminated by BEL or ESC \
    let mut rest: &str = &text;
    while let Some(start) = rest.find("\x1b]") {
        let body_start = start + 2;
        let body = &rest[body_start..];
        let end = body.find('\x07')
            .into_iter()
            .chain(body.find("\x1b\\"))
            .min();
        let Some(end) = end else { break };
        let seq = &body[..end];
        if let Some(payload) = seq.strip_prefix("10;") {
            if let Some(rgb) = crate::types::parse_x11_color(payload) { hc.fg = Some(rgb); }
        } else if let Some(payload) = seq.strip_prefix("11;") {
            if let Some(rgb) = crate::types::parse_x11_color(payload) { hc.bg = Some(rgb); }
        } else if let Some(p) = seq.strip_prefix("4;") {
            if let Some((idx, payload)) = p.split_once(';') {
                if let (Ok(i), Some(rgb)) = (idx.parse::<usize>(), crate::types::parse_x11_color(payload)) {
                    if i < 16 { hc.palette[i] = Some(rgb); }
                }
            }
        }
        rest = &body[end..];
    }
    if text.contains("\x1b[?997;1n") { hc.dark = Some(true); }
    else if text.contains("\x1b[?997;2n") { hc.dark = Some(false); }
    hc
}

/// Clear `ENABLE_VIRTUAL_TERMINAL_INPUT` (VTI, 0x0200) from the console stdin.
///
/// crossterm 0.28's `enable_raw_mode()` sets VTI.  When psmux runs inside a
/// ConPTY-based terminal (e.g. WezTerm), VTI tells conhost to pass VT bytes
/// through as raw KEY_EVENT records instead of properly translating them to
/// INPUT_RECORDs with virtual-key codes.  This breaks crossterm's event parser
/// because it expects translated INPUT_RECORDs for regular key events.
///
/// For local (non-SSH) sessions, we do not need VTI — crossterm reads native
/// INPUT_RECORDs via `ReadConsoleInputW`.  The SSH input path has its OWN
/// `SetConsoleMode(+VTI)` call, so this only runs for local mode.
///
/// Windows Terminal is unaffected because it IS the console host (no ConPTY
/// pipe translation).  The fix specifically helps ConPTY-hosted terminals.
#[cfg(windows)]
pub fn disable_vti_on_stdin() {
    const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, dwMode: u32) -> i32;
    }

    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        if handle.is_null() || handle == (-1isize) as *mut std::ffi::c_void {
            return;
        }
        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &mut mode) != 0 {
            let had_vti = mode & ENABLE_VIRTUAL_TERMINAL_INPUT != 0;
            crate::debug_log::input_log("console", &format!(
                "stdin mode before: 0x{:04X} VTI={}", mode, had_vti
            ));
            if had_vti {
                let new_mode = mode & !ENABLE_VIRTUAL_TERMINAL_INPUT;
                SetConsoleMode(handle, new_mode);
                crate::debug_log::input_log("console", &format!(
                    "stdin mode after: 0x{:04X} (VTI cleared)", new_mode
                ));
            }
        }
    }
}

#[cfg(not(windows))]
pub fn disable_vti_on_stdin() {
    // No-op on non-Windows platforms
}

/// Install a console control handler on Windows to prevent termination on client detach.
#[cfg(windows)]
pub fn install_console_ctrl_handler() {
    type HandlerRoutine = unsafe extern "system" fn(u32) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<HandlerRoutine>, add: i32) -> i32;
    }

    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const CTRL_CLOSE_EVENT: u32 = 2;
    const CTRL_LOGOFF_EVENT: u32 = 5;
    const CTRL_SHUTDOWN_EVENT: u32 = 6;

    unsafe extern "system" fn handler(ctrl_type: u32) -> i32 {
        match ctrl_type {
            // Never let a Ctrl+C / Ctrl+Break signal terminate the server — that
            // would tear down every session at once.  When psmux relays such a
            // signal to a pane's child it briefly AttachConsole()s to the child's
            // console, which places the server in that console's process group;
            // a GenerateConsoleCtrlEvent(_, 0) broadcast would then kill the
            // server itself.  SetConsoleCtrlHandler(None,1) only suppresses
            // Ctrl+C, so Ctrl+Break needs this explicit survival (issue #454).
            CTRL_C_EVENT | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => 1,
            _ => 0,
        }
    }

    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(not(windows))]
pub fn install_console_ctrl_handler() {
    // No-op on non-Windows platforms
}

/// Set true by the client's console-control handler when a Ctrl+Break signal
/// is trapped, drained by the client's main loop.  (issue #454)
#[cfg(windows)]
static CLIENT_CTRL_BREAK_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Install a console control handler on the ATTACHED CLIENT so Ctrl+Break
/// interrupts the pane's foreground program instead of killing the client and
/// detaching the still-running session (issue #454).
///
/// Ctrl+Break is ALWAYS delivered as a CTRL_BREAK_EVENT console signal — it
/// cannot be read as a keystroke even in raw mode — so crossterm's key loop
/// never sees it.  With no handler installed the OS default terminates the
/// client, which is exactly the reported bug: the session survives on the
/// server while the attached window vanishes.  We trap the signal, return TRUE
/// to stay alive, and flag the main loop to forward a `send-key C-Break` to the
/// server, which interrupts the pane's foreground program via the reliable
/// Ctrl+C path (a real CTRL_BREAK_EVENT cannot be relayed into a ConPTY child).
/// A stray CTRL_C signal (rare in raw mode, where Ctrl+C arrives as a keystroke)
/// is likewise swallowed so it can never kill the client.
#[cfg(windows)]
pub fn install_client_console_ctrl_handler() {
    use std::sync::atomic::Ordering;
    type HandlerRoutine = unsafe extern "system" fn(u32) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<HandlerRoutine>, add: i32) -> i32;
    }

    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;

    unsafe extern "system" fn handler(ctrl_type: u32) -> i32 {
        match ctrl_type {
            CTRL_BREAK_EVENT => {
                CLIENT_CTRL_BREAK_PENDING.store(true, Ordering::SeqCst);
                1
            }
            // Never let a stray Ctrl+C signal terminate the client; normal
            // Ctrl+C is already handled via the keystroke path.
            CTRL_C_EVENT => 1,
            // Close / logoff / shutdown: let the OS proceed with cleanup.
            _ => 0,
        }
    }

    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

/// Returns true exactly once per trapped Ctrl+Break signal.  (issue #454)
#[cfg(windows)]
pub fn take_client_ctrl_break() -> bool {
    CLIENT_CTRL_BREAK_PENDING.swap(false, std::sync::atomic::Ordering::SeqCst)
}

#[cfg(not(windows))]
pub fn install_client_console_ctrl_handler() {
    // No-op on non-Windows platforms
}

#[cfg(not(windows))]
pub fn take_client_ctrl_break() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Windows Console API mouse injection
// ---------------------------------------------------------------------------
// ConPTY does NOT translate VT mouse escape sequences (e.g. SGR \x1b[<0;10;5M)
// into MOUSE_EVENT INPUT_RECORDs. Writing them to the PTY master appears as
// garbage text in the child app.
//
// The solution: use WriteConsoleInput to inject native MOUSE_EVENT records
// directly into the child's console input buffer.
//
// Flow:
//   1. On first mouse event targeting a pane, lazily acquire the console handle:
//      FreeConsole() → AttachConsole(child_pid) → CreateFileW("CONIN$") → FreeConsole()
//   2. The handle remains valid after FreeConsole on modern Windows (real kernel handles).
//   3. Use WriteConsoleInputW(handle, MOUSE_EVENT record) for each mouse event.
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod mouse_inject {
    use std::ffi::c_void;

    const GENERIC_READ: u32  = 0x80000000;
    const GENERIC_WRITE: u32 = 0x40000000;
    const FILE_SHARE_READ: u32  = 0x00000001;
    const FILE_SHARE_WRITE: u32 = 0x00000002;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE: isize = -1;

    const MOUSE_EVENT: u16 = 0x0002;
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;

    // dwButtonState flags
    pub const FROM_LEFT_1ST_BUTTON_PRESSED: u32 = 0x0001;
    pub const RIGHTMOST_BUTTON_PRESSED: u32     = 0x0002;
    pub const FROM_LEFT_2ND_BUTTON_PRESSED: u32 = 0x0004; // middle button

    // dwEventFlags
    pub const MOUSE_MOVED: u32       = 0x0001;
    pub const MOUSE_WHEELED: u32     = 0x0004;

    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST_DRAG_INJECT: Mutex<Option<Instant>> = Mutex::new(None);
    const DRAG_THROTTLE: Duration = Duration::from_millis(16); // ~60fps

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct COORD {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct MOUSE_EVENT_RECORD {
        mouse_position: COORD,
        button_state: u32,
        control_key_state: u32,
        event_flags: u32,
    }

    #[repr(C)]
    struct INPUT_RECORD {
        event_type: u16,
        _padding: u16,
        event: MOUSE_EVENT_RECORD,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> i32;
        fn AttachConsole(process_id: u32) -> i32;
        fn GetConsoleWindow() -> isize;
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *const c_void,
        ) -> isize;
        fn WriteConsoleInputW(
            console_input: isize,
            buffer: *const INPUT_RECORD,
            length: u32,
            events_written: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetProcessId(process: isize) -> u32;
        fn GetLastError() -> u32;
    }

    /// Console input mode flags
    const ENABLE_MOUSE_INPUT: u32         = 0x0010;
    const ENABLE_EXTENDED_FLAGS: u32      = 0x0080;
    const ENABLE_QUICK_EDIT_MODE: u32     = 0x0040;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

    #[inline]
    fn debug_log(msg: &str) {
        // Write to mouse_debug.log when PSMUX_MOUSE_DEBUG=1 is set.
        use std::sync::atomic::{AtomicBool, Ordering};
        static CHECKED: AtomicBool = AtomicBool::new(false);
        static ENABLED: AtomicBool = AtomicBool::new(false);

        if !CHECKED.swap(true, Ordering::Relaxed) {
            let on = std::env::var("PSMUX_MOUSE_DEBUG").map_or(false, |v| v == "1" || v == "true");
            ENABLED.store(on, Ordering::Relaxed);
        }
        if !ENABLED.load(Ordering::Relaxed) { return; }

        let path = format!("{}/mouse_debug.log", crate::paths::psmux_dir());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = writeln!(f, "[platform] {}", msg);
        }
    }

    /// Extract the process ID from a portable_pty::Child trait object.
    ///
    /// Uses the `Child::process_id()` trait method provided by portable-pty 0.9+.
    pub fn get_child_pid(child: &dyn portable_pty::Child) -> Option<u32> {
        child.process_id()
    }

    /// Query whether the child process's console input has
    /// ENABLE_VIRTUAL_TERMINAL_INPUT (0x0200) set.
    ///
    /// When this flag is ON, the process uses VT-based input processing
    /// (crossterm, ratatui apps).  VT mouse sequences written to the ConPTY
    /// input pipe are passed through as KEY_EVENT records, and the app's VT
    /// parser handles them.  If the flag is OFF (e.g. Node.js libuv raw mode
    /// which sets only ENABLE_WINDOW_INPUT), VT mouse sequences should NOT
    /// be written because the app cannot parse them and they appear as garbage.
    pub fn query_vti_enabled(child_pid: u32) -> Option<bool> {
        // Hold across the FreeConsole/AttachConsole dance so a concurrent
        // ConPTY spawn can't stamp freed std handles into a newborn shell
        // (issue #450).  Same guard in every dance function below.
        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = GetConsoleWindow() != 0;
            FreeConsole();

            if AttachConsole(child_pid) == 0 {
                debug_log(&format!("query_vti_enabled: AttachConsole({}) FAILED", child_pid));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return None;
            }

            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                debug_log("query_vti_enabled: CreateFileW(CONIN$) FAILED");
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return None;
            }

            #[link(name = "kernel32")]
            extern "system" {
                fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
            }
            let mut mode: u32 = 0;
            let ok = GetConsoleMode(handle as *mut c_void, &mut mode);

            CloseHandle(handle);
            FreeConsole();
            if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }

            if ok == 0 {
                debug_log("query_vti_enabled: GetConsoleMode FAILED");
                return None;
            }

            let vti = (mode & ENABLE_VIRTUAL_TERMINAL_INPUT) != 0;
            debug_log(&format!("query_vti_enabled: pid={} mode=0x{:04X} VTI={}", child_pid, mode, vti));
            Some(vti)
        }
    }

    /// Ensure the child process's console input has ENABLE_VIRTUAL_TERMINAL_INPUT
    /// (0x0200) set.
    ///
    /// Root cause of issue #277/#245 scroll-forwarding failure: SGR mouse
    /// escape sequences written to the ConPTY master pipe (`write_mouse_to_pty`
    /// in window_ops.rs) are silently swallowed by conhost's input engine and
    /// NEVER reach the child at all — not even as literal characters — unless
    /// the child's console already has VTI enabled.  Freshly spawned console
    /// apps (a plain shell, or a TUI app that hasn't gotten around to calling
    /// `SetConsoleMode` yet) default to VTI off, so every SGR wheel sequence
    /// psmux writes is dropped before the child ever sees it.
    ///
    /// `send_vt_sequence` (used for the WSL/SSH vt-bridge case, below) already
    /// force-enables VTI before writing for exactly this reason; this function
    /// does the same for the native-ConPTY path so `write_mouse_to_pty` callers
    /// can call it once before injecting.  Idempotent — a no-op if VTI is
    /// already on.
    pub fn ensure_vti_enabled(child_pid: u32) -> bool {
        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = GetConsoleWindow() != 0;
            FreeConsole();

            if AttachConsole(child_pid) == 0 {
                debug_log(&format!("ensure_vti_enabled: AttachConsole({}) FAILED", child_pid));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                debug_log("ensure_vti_enabled: CreateFileW(CONIN$) FAILED");
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            #[link(name = "kernel32")]
            extern "system" {
                fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
                fn SetConsoleMode(hConsoleHandle: *mut c_void, dwMode: u32) -> i32;
            }
            let h = handle as *mut c_void;
            let mut mode: u32 = 0;
            let mut ok = GetConsoleMode(h, &mut mode) != 0;
            if ok {
                let desired = mode | ENABLE_VIRTUAL_TERMINAL_INPUT;
                if desired != mode {
                    ok = SetConsoleMode(h, desired) != 0;
                    debug_log(&format!("ensure_vti_enabled: pid={} mode 0x{:04X} -> 0x{:04X} ok={}", child_pid, mode, desired, ok));
                }
            } else {
                debug_log("ensure_vti_enabled: GetConsoleMode FAILED");
            }

            CloseHandle(handle);
            FreeConsole();
            if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }

            ok
        }
    }

    /// Inject a mouse event into a child process's console input buffer.
    ///
    /// Performs the full cycle: FreeConsole → AttachConsole(pid) → open CONIN$
    /// → WriteConsoleInputW → CloseHandle → FreeConsole.
    ///
    /// Console handles are pseudo-handles that are invalidated by FreeConsole,
    /// so we must do the entire cycle atomically for each event.
    ///
    /// `reattach`: if true, re-attaches to original console after injection
    /// (needed for app/standalone mode where crossterm uses the console).
    /// Server mode should pass false to avoid conhost cycling.
    pub fn send_mouse_event(
        child_pid: u32,
        col: i16,
        row: i16,
        button_state: u32,
        event_flags: u32,
        reattach: bool,
    ) -> bool {
        // Throttle drag events to ~60fps to avoid excessive console attach/detach cycling
        if event_flags & MOUSE_MOVED != 0 {
            if let Ok(mut guard) = LAST_DRAG_INJECT.lock() {
                if let Some(t) = *guard {
                    if t.elapsed() < DRAG_THROTTLE {
                        return false;
                    }
                }
                *guard = Some(Instant::now());
            }
        }

        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            // Check if we currently own a console (app mode yes, server mode no after first call)
            let had_console = reattach && GetConsoleWindow() != 0;

            // Detach from current console (no-op if already detached)
            FreeConsole();

            // Attach to child's pseudo-console
            if AttachConsole(child_pid) == 0 {
                let err = GetLastError();
                debug_log(&format!("send_mouse_event: AttachConsole({}) FAILED err={}", child_pid, err));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Open the console input buffer
            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                let err = GetLastError();
                debug_log(&format!("send_mouse_event: CreateFileW(CONIN$) FAILED err={}", err));
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Temporarily ensure ENABLE_MOUSE_INPUT is set on the console so
            // mouse events are delivered to the foreground process.  Save and
            // restore original mode to prevent polluting the child's console
            // state (which would confuse query_mouse_input_enabled).
            {
                // Re-use the top-level GetConsoleMode/SetConsoleMode declarations
                // (they use *mut c_void for the handle parameter).
                #[link(name = "kernel32")]
                extern "system" {
                    fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
                    fn SetConsoleMode(hConsoleHandle: *mut c_void, dwMode: u32) -> i32;
                }
                let mut mode: u32 = 0;
                let h = handle as *mut c_void;
                if GetConsoleMode(h, &mut mode) != 0 {
                    let desired = (mode | ENABLE_MOUSE_INPUT | ENABLE_EXTENDED_FLAGS)
                                  & !ENABLE_QUICK_EDIT_MODE;
                    if desired != mode {
                        SetConsoleMode(h, desired);
                    }
                }
            }

            // Write the mouse event
            let record = INPUT_RECORD {
                event_type: MOUSE_EVENT,
                _padding: 0,
                event: MOUSE_EVENT_RECORD {
                    mouse_position: COORD { x: col, y: row },
                    button_state,
                    control_key_state: 0,
                    event_flags,
                },
            };
            let mut written: u32 = 0;
            let result = WriteConsoleInputW(handle, &record, 1, &mut written);
            let write_err = GetLastError();

            debug_log(&format!("send_mouse_event: pid={} ({},{}) btn=0x{:X} flags=0x{:X} => ok={} written={} err={}",
                child_pid, col, row, button_state, event_flags, result, written, write_err));

            // Clean up: close handle, detach from child's console
            CloseHandle(handle);
            FreeConsole();
            // Only re-attach if we had our own console (app/standalone mode)
            // Server mode: leave detached to avoid conhost cycling
            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            result != 0
        }
    }

    /// Query whether the child process's console input has
    /// ENABLE_MOUSE_INPUT (0x0010) set.
    ///
    /// When this flag is ON, the child uses ReadConsoleInputW to read
    /// MOUSE_EVENT INPUT_RECORDs (crossterm/ratatui apps).  When OFF, the
    /// child reads input as text (ReadConsole/ReadFile) and expects VT
    /// mouse sequences delivered as KEY_EVENT records (nvim, vim).
    pub fn query_mouse_input_enabled(child_pid: u32) -> Option<bool> {
        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = GetConsoleWindow() != 0;
            FreeConsole();

            if AttachConsole(child_pid) == 0 {
                debug_log(&format!("query_mouse_input_enabled: AttachConsole({}) FAILED", child_pid));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return None;
            }

            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                debug_log("query_mouse_input_enabled: CreateFileW(CONIN$) FAILED");
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return None;
            }

            #[link(name = "kernel32")]
            extern "system" {
                fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
            }
            let mut mode: u32 = 0;
            let ok = GetConsoleMode(handle as *mut c_void, &mut mode);

            CloseHandle(handle);
            FreeConsole();
            if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }

            if ok == 0 {
                debug_log("query_mouse_input_enabled: GetConsoleMode FAILED");
                return None;
            }

            let mouse_input = (mode & ENABLE_MOUSE_INPUT) != 0;
            debug_log(&format!("query_mouse_input_enabled: pid={} mode=0x{:04X} ENABLE_MOUSE_INPUT={}", child_pid, mode, mouse_input));
            Some(mouse_input)
        }
    }

    /// Inject a VT escape sequence into a child process's console input buffer
    /// as a series of KEY_EVENT records.
    ///
    /// This bypasses ConPTY's VT input parser entirely — the raw characters of
    /// the escape sequence are delivered directly to the foreground process
    /// (e.g. wsl.exe) as keyboard input.  wsl.exe forwards them to the Linux
    /// PTY, where the terminal application (e.g. htop) interprets them as
    /// mouse events.
    ///
    /// This is more reliable than writing to the PTY master pipe because
    /// ConPTY's input engine may not correctly handle SGR mouse sequences
    /// written to hInput.
    pub fn send_vt_sequence(child_pid: u32, sequence: &[u8]) -> bool {
        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = GetConsoleWindow() != 0;
            FreeConsole();

            if AttachConsole(child_pid) == 0 {
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Save original console mode, temporarily set VTI for injection,
            // then restore after writing.  This prevents mode pollution which
            // would confuse the query_mouse_input_enabled() heuristic used to
            // distinguish console-API apps (crossterm) from VT apps (nvim).
            #[link(name = "kernel32")]
            extern "system" {
                fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
                fn SetConsoleMode(hConsoleHandle: *mut c_void, dwMode: u32) -> i32;
            }
            let h = handle as *mut c_void;
            let mut original_mode: u32 = 0;
            let got_mode = GetConsoleMode(h, &mut original_mode) != 0;
            // Issue #604: touch ONLY ENABLE_VIRTUAL_TERMINAL_INPUT, and only
            // when it is actually missing.
            //
            // This used to also set ENABLE_EXTENDED_FLAGS and clear
            // ENABLE_QUICK_EDIT_MODE, then restore the saved mode after the
            // write.  Neither bit has anything to do with putting KEY_EVENT
            // records into an input buffer, and toggling quick edit is not
            // free: conhost derives "is this client tracking the mouse" from
            // the console input mode, so every set/restore pair was mirrored
            // back up the ConPTY into the PANE'S OUTPUT as `ESC[?1003;1006h`
            // immediately followed by `ESC[?1003;1006l`.  psmux's own vt100
            // parser applied both, and since DECRST 1003 clears the mouse
            // protocol (tmux does the same, input.c: `case 1000: case 1001:
            // case 1002: case 1003: screen_write_mode_clear(sctx,
            // ALL_MOUSE_MODES)`), the trailing DECRST wiped the mode the
            // application had really asked for.  nvim running under wsl.exe
            // enables 1002+1006, so from the very first forwarded mouse event
            // onward `mouse_protocol_mode()` read None and the bridge gate in
            // window_ops::inject_mouse_combined suppressed every later click:
            // clicking in nvim inside WSL stopped moving the cursor (#604).
            //
            // Measured on this tree: the pane raw stream (PSMUX_PANE_RAW=1)
            // showed exactly one `ESC[?1003;1006h ESC[?1003;1006l` pair per
            // injected event, and the pane's mouse mode went ButtonMotion ->
            // None across the first one.
            let mut restore_mode = false;
            if got_mode && (original_mode & 0x0200 /*ENABLE_VIRTUAL_TERMINAL_INPUT*/) == 0 {
                if SetConsoleMode(h, original_mode | 0x0200) != 0 {
                    restore_mode = true;
                }
            }

            // Build KEY_EVENT records for each byte of the VT sequence.
            // Each record is a "key down" event with the character set.
            const KEY_EVENT: u16 = 0x0001;

            #[repr(C)]
            #[derive(Copy, Clone)]
            struct KEY_EVENT_RECORD {
                key_down: i32,
                repeat_count: u16,
                virtual_key_code: u16,
                virtual_scan_code: u16,
                u_char: u16,       // UnicodeChar
                control_key_state: u32,
            }

            #[repr(C)]
            struct KEY_INPUT_RECORD {
                event_type: u16,
                _padding: u16,
                event: KEY_EVENT_RECORD,
            }

            // Build the array of input records
            let mut records: Vec<KEY_INPUT_RECORD> = Vec::with_capacity(sequence.len());
            for &byte in sequence {
                records.push(KEY_INPUT_RECORD {
                    event_type: KEY_EVENT,
                    _padding: 0,
                    event: KEY_EVENT_RECORD {
                        key_down: 1,
                        repeat_count: 1,
                        virtual_key_code: 0,
                        virtual_scan_code: 0,
                        u_char: byte as u16,
                        control_key_state: 0,
                    },
                });
            }

            let mut written: u32 = 0;
            let result = WriteConsoleInputW(
                handle,
                records.as_ptr() as *const INPUT_RECORD,
                records.len() as u32,
                &mut written,
            );

            // Restore original console mode to prevent pollution, but only if
            // we actually changed it (#604: a redundant SetConsoleMode is a
            // mouse-registration event as far as conhost is concerned).
            if restore_mode {
                SetConsoleMode(h, original_mode);
            }

            CloseHandle(handle);
            FreeConsole();
            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            result != 0
        }
    }

    /// Inject bracketed paste text into a child process's console input buffer.
    ///
    /// Sends `\x1b[200~` + text + `\x1b[201~` as KEY_EVENT records via
    /// WriteConsoleInputW, bypassing ConPTY's VT input parser entirely.
    /// ConPTY strips bracketed paste sequences written to the PTY master pipe,
    /// so this direct injection is the only way to deliver them to the child.
    ///
    /// The text is encoded as UTF-16 for proper Unicode support (file paths
    /// may contain non-ASCII characters).
    pub fn send_bracketed_paste(child_pid: u32, text: &str, bracket: bool) -> bool {
        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = GetConsoleWindow() != 0;
            FreeConsole();

            if AttachConsole(child_pid) == 0 {
                let err = GetLastError();
                debug_log(&format!("send_bracketed_paste: AttachConsole({}) FAILED err={}", child_pid, err));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                let err = GetLastError();
                debug_log(&format!("send_bracketed_paste: CreateFileW(CONIN$) FAILED err={}", err));
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            const KEY_EVENT: u16 = 0x0001;

            #[repr(C)]
            #[derive(Copy, Clone)]
            struct KEY_EVENT_RECORD {
                key_down: i32,
                repeat_count: u16,
                virtual_key_code: u16,
                virtual_scan_code: u16,
                u_char: u16,
                control_key_state: u32,
            }

            #[repr(C)]
            struct KEY_INPUT_RECORD {
                event_type: u16,
                _padding: u16,
                event: KEY_EVENT_RECORD,
            }

            // Build bracket-open, text, bracket-close as UTF-16 chars
            let bracket_open: &[u8] = b"\x1b[200~";
            let bracket_close: &[u8] = b"\x1b[201~";

            // Collect all UTF-16 code units to send
            let mut chars: Vec<u16> = Vec::new();
            if bracket {
                for &b in bracket_open {
                    chars.push(b as u16);
                }
            }
            // Encode paste text as UTF-16, normalizing \n → \r for the
            // console input buffer (Windows apps expect CR for line breaks;
            // PSReadLine and other readline implementations treat \r as Enter).
            let mut prev_cr = false;
            for c in text.chars() {
                if c == '\n' {
                    if !prev_cr {
                        // Bare \n → \r
                        chars.push('\r' as u16);
                    }
                    // If preceded by \r, the \r was already pushed; skip this \n
                    prev_cr = false;
                    continue;
                }
                prev_cr = c == '\r';
                let mut buf = [0u16; 2];
                let encoded = c.encode_utf16(&mut buf);
                for &unit in encoded.iter() {
                    chars.push(unit);
                }
            }
            if bracket {
                for &b in bracket_close {
                    chars.push(b as u16);
                }
            }

            // Build KEY_EVENT records (key-down only; key-up not needed for
            // console input injection — only key-down events carry characters).
            let mut records: Vec<KEY_INPUT_RECORD> = Vec::with_capacity(chars.len());
            for &wch in &chars {
                records.push(KEY_INPUT_RECORD {
                    event_type: KEY_EVENT,
                    _padding: 0,
                    event: KEY_EVENT_RECORD {
                        key_down: 1,
                        repeat_count: 1,
                        virtual_key_code: 0,
                        virtual_scan_code: 0,
                        u_char: wch,
                        control_key_state: 0,
                    },
                });
            }

            // WriteConsoleInputW can perform partial writes (returns fewer
            // records than requested).  Retry in a loop so that large pastes
            // are delivered in full; without this the closing bracket sequence
            // can be silently dropped, breaking bracket paste mode in the
            // child application.
            //
            // For very large pastes, the console input buffer may fill up.
            // We limit each write to CHUNK_SIZE records and yield briefly
            // between chunks to let the consumer (PSReadLine etc.) drain.
            const CHUNK_SIZE: usize = 2048;
            let mut offset: usize = 0;
            let mut last_result: i32 = 1;
            while offset < records.len() {
                let mut written: u32 = 0;
                let remaining = (records.len() - offset).min(CHUNK_SIZE);
                last_result = WriteConsoleInputW(
                    handle,
                    records[offset..].as_ptr() as *const INPUT_RECORD,
                    remaining as u32,
                    &mut written,
                );
                if last_result == 0 || written == 0 {
                    // Brief yield and retry once (buffer may temporarily be full)
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    last_result = WriteConsoleInputW(
                        handle,
                        records[offset..].as_ptr() as *const INPUT_RECORD,
                        remaining as u32,
                        &mut written,
                    );
                    if last_result == 0 || written == 0 {
                        break;
                    }
                }
                offset += written as usize;
                // Yield between chunks to let the consumer drain the buffer
                if offset < records.len() && remaining >= CHUNK_SIZE {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }

            debug_log(&format!("send_bracketed_paste: pid={} bracket={} text_len={} records={} written={} ok={}",
                child_pid, bracket, text.len(), records.len(), offset, last_result != 0));

            CloseHandle(handle);
            FreeConsole();
            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            last_result != 0 && offset == records.len()
        }
    }

    /// Issue #473: deliver a VT response string (e.g. OSC color-query replies)
    /// into a child's console input buffer via WriteConsoleInputW.
    ///
    /// ConPTY consumes complete OSC sequences written to the pseudoconsole
    /// input pipe before the child can read them, so `pane.writer` cannot
    /// carry OSC 4/10/11 replies.  Injecting the bytes as KEY_EVENT records
    /// bypasses ConPTY's VT input filter entirely — the same transport that
    /// makes bracketed paste work (`send_bracketed_paste`), which this reuses
    /// without the paste brackets.
    pub fn send_vt_response(child_pid: u32, text: &str) -> bool {
        send_bracketed_paste(child_pid, text, false)
    }

    /// Send a CTRL_C_EVENT to all processes on the child's console.
    ///
    /// TUI applications (pstop, btop, etc.) often disable ENABLE_PROCESSED_INPUT
    /// on the ConPTY console and fail to restore it on exit.  When this flag is
    /// off, writing 0x03 to the ConPTY input pipe no longer generates a
    /// CTRL_C_EVENT signal — the byte is delivered as a regular key event that
    /// most programs ignore.
    ///
    /// This function works around the issue by:
    ///   1. Attaching to the child's hidden ConPTY console
    ///   2. Re-enabling ENABLE_PROCESSED_INPUT if it was cleared
    ///   3. Calling GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)
    ///
    /// The combination ensures Ctrl+C delivers a signal to shells and cooked
    /// console apps regardless of what a previous TUI application did to the
    /// console mode.
    ///
    /// EXCEPTION: when the pane's foreground process is a *live* raw-mode TUI
    /// (e.g. Copilot CLI, vim) that has cleared ENABLE_PROCESSED_INPUT to read
    /// Ctrl+C itself, this function skips the signal so the raw 0x03 byte the
    /// caller writes to the PTY reaches the app, which decides copy-vs-interrupt.
    /// (Call sites write the raw 0x03 either just before or just after invoking
    /// this function; the skip behavior is correct regardless of that ordering.)
    /// See `process_info::foreground_is_shell`.
    pub fn send_ctrl_c_event(child_pid: u32, reattach: bool) -> bool {
        const CTRL_C_EVENT: u32 = 0;
        const ENABLE_PROCESSED_INPUT: u32 = 0x0001;

        type HandlerRoutine = unsafe extern "system" fn(u32) -> i32;

        #[link(name = "kernel32")]
        extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<HandlerRoutine>,
                add: i32,
            ) -> i32;
            fn GenerateConsoleCtrlEvent(
                ctrl_event: u32,
                process_group_id: u32,
            ) -> i32;
            fn GetConsoleMode(h: *mut c_void, mode: *mut u32) -> i32;
            fn SetConsoleMode(h: *mut c_void, mode: u32) -> i32;
            fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
        }

        // Always log to file for Ctrl+C events (critical signal path).
        fn log(msg: &str) {
            debug_log(&format!("ctrl_c: {}", msg));
        }

        // Decide up-front whether the pane's foreground process wants a console
        // interrupt (shells / VT bridges / bare prompt) or is a live raw-mode
        // TUI that should receive raw 0x03 itself (Copilot CLI, vim, ...).
        // Unknown (snapshot failure) falls back to `true` so we preserve the
        // established interrupt behavior (#338 line-cancel, #346 ping).  This
        // process-tree walk does not touch our console, so it is done before
        // the FreeConsole/AttachConsole dance below.
        let fg_is_shell = crate::platform::process_info::foreground_is_shell(child_pid)
            .unwrap_or(true);

        // Issue #491: a VT bridge (wsl.exe, ssh.exe) reads raw bytes from its
        // console and forwards 0x03 into the guest as SIGINT itself, so the
        // console-wide CTRL_C_EVENT broadcast below is redundant for it — and
        // fatal when the bridge was launched from a Cygwin/MSYS shell (Git
        // Bash, MSYS2, Cygwin): the shell reacts to the broadcast by
        // delivering SIGINT to its native foreground child, which the Cygwin
        // runtime implements as a hard kill of wsl.exe.  Deliver only the raw
        // 0x03 the call site writes and skip the signal.
        // When a bridge guard below decides "raw 0x03 only", the byte must
        // actually ARRIVE as input.  If the pane console is in cooked mode at
        // that moment (the shell restored PROCESSED_INPUT while waiting on an
        // external command — measured 0x01F7 during the WSL boot window),
        // conhost itself converts the delivered ^C into a console-wide
        // CTRL_C_EVENT and the shell aborts the launch it is waiting on: the
        // WSL session dies with no psmux broadcast involved (reproduced with
        // every broadcast suppressed).  Stripping ENABLE_PROCESSED_INPUT
        // before the call site writes the byte makes conhost hand it over as
        // an input record instead; the bridge's relay reads it once attached.
        // No restore: the shell re-arms its own mode at the next prompt, and
        // the bridge sets its own mode when it takes the console.
        fn strip_processed_input(child_pid: u32, log: &dyn Fn(&str)) {
            const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
            #[link(name = "kernel32")]
            extern "system" {
                fn GetConsoleMode(h: *mut c_void, mode: *mut u32) -> i32;
                fn SetConsoleMode(h: *mut c_void, mode: u32) -> i32;
                fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
            }
            let _console_guard = portable_pty::console_state_lock();
            unsafe {
                let had_console = GetConsoleWindow() != 0;
                FreeConsole();
                if AttachConsole(child_pid) == 0 {
                    if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                    return;
                }
                // A plain native app on the console (ping under Git Bash) is
                // the legitimate recipient of conhost's Ctrl+C conversion —
                // stripping the flag then silences its interrupt (measured).
                let mut pids = [0u32; 64];
                let n = GetConsoleProcessList(pids.as_mut_ptr(), 64) as usize;
                if n > 0 && crate::platform::process_info::console_has_plain_native_app(&pids[..n.min(64)]) {
                    log("plain native app on pane console: keep PROCESSED_INPUT so its interrupt still fires");
                    FreeConsole();
                    if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                    return;
                }
                let conin: [u16; 7] = [
                    'C' as u16, 'O' as u16, 'N' as u16,
                    'I' as u16, 'N' as u16, '$' as u16, 0,
                ];
                let handle = CreateFileW(
                    conin.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null(),
                );
                if handle != INVALID_HANDLE && handle != 0 {
                    let mut mode: u32 = 0;
                    if GetConsoleMode(handle as *mut c_void, &mut mode) != 0
                        && mode & ENABLE_PROCESSED_INPUT != 0
                    {
                        SetConsoleMode(handle as *mut c_void, mode & !ENABLE_PROCESSED_INPUT);
                        log(&format!("stripped PROCESSED_INPUT (was 0x{:04X}) so the raw 0x03 arrives as input", mode));
                    }
                    CloseHandle(handle);
                }
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
            }
        }

        if crate::platform::process_info::foreground_is_vt_bridge(child_pid) {
            log(&format!("vt-bridge foreground under pid={}: deliver raw 0x03 only, skip CTRL_C_EVENT", child_pid));
            strip_processed_input(child_pid, &log);
            return false;
        }

        // Issue #579, third guard: a bridge ANYWHERE in the pane's process
        // tree.  The leaf walk above resolves one chain by a highest-PID
        // heuristic, and Windows PIDs are not monotonic: prompt tooling
        // (oh-my-posh, starship) parks transient children under the pane
        // shell, so the walk can land on a sibling and classify a shell
        // while wsl.exe is live in the tree.  Native (non-Cygwin) chains
        // keep their PPID links intact, so the descendant BFS is reliable
        // exactly where the leaf heuristic is not.  A bridge in the tree
        // forwards the raw 0x03 into its guest itself; the broadcast is
        // redundant for it and fatal to it.
        if crate::platform::process_info::has_vt_bridge_descendant(child_pid) {
            log(&format!("vt-bridge descendant under pid={}: deliver raw 0x03 only, skip CTRL_C_EVENT", child_pid));
            strip_processed_input(child_pid, &log);
            return false;
        }

        // Issue #579, boot-window guard (the reproduced kill): while the WSL
        // VM boots (~1-2s cold), the live wsl.exe processes are parented by
        // wslservice — NOT children of the pane shell — and not yet attached
        // to the pane console, so the leaf walk, the descendant BFS, and the
        // console-membership check below are ALL structurally blind to them.
        // The pane shell meanwhile has no visible children, so the foreground
        // resolution falls back to "bare shell prompt" and the broadcast
        // fires — and the shell reacts to CTRL_C_EVENT by aborting the launch
        // it is waiting on: the WSL session dies and the prompt returns
        // (measured: at-inject two live wsl.exe, console = {server, shell}
        // only, fg_is_shell=true, after = zero).  When the resolution came
        // from that childless fallback — exactly the attribution-blind state
        // — treat any bridge alive anywhere on the system as potentially
        // ours and skip the broadcast.  Cost when it misfires: a legacy
        // cooked prompt loses the explicit line-cancel signal while some
        // unrelated WSL runs elsewhere; the raw 0x03 still reaches the pane.
        if crate::platform::process_info::foreground_fell_back_to_root(child_pid)
            && crate::platform::process_info::any_vt_bridge_running()
        {
            log(&format!("childless foreground fallback with a live system bridge (pid={}): deliver raw 0x03 only, skip CTRL_C_EVENT", child_pid));
            strip_processed_input(child_pid, &log);
            return false;
        }

        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = reattach && GetConsoleWindow() != 0;

            FreeConsole();

            log(&format!("called: pid={} reattach={} had_console={}", child_pid, reattach, had_console));

            if AttachConsole(child_pid) == 0 {
                let err = GetLastError();
                log(&format!("AttachConsole({}) FAILED err={}", child_pid, err));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Issue #579: GenerateConsoleCtrlEvent(_, 0) broadcasts to EVERY
            // process on this console, while the foreground_is_vt_bridge guard
            // above classified only the deepest leaf of one child chain.  A VT
            // bridge the leaf walk missed — a backgrounded `wsl.exe &` job, an
            // interop child below wsl.exe, or a mis-resolved sibling (Windows
            // PIDs are not monotonic, so the highest-PID-child heuristic can
            // descend the wrong subtree) — still dies to the broadcast: either
            // directly (bridge on this console) or through a Cygwin/MSYS shell
            // on this console whose runtime hard-kills its native descendants
            // on CTRL_C_EVENT, even background ones parked on hidden consoles.
            // Now that we are attached, classify the console's REAL membership
            // and skip the signal when the broadcast would hit a bridge.  The
            // raw 0x03 the call site writes still reaches the foreground app,
            // which is all tmux ever delivers.
            let mut console_pids = vec![0u32; 64];
            let mut n = GetConsoleProcessList(console_pids.as_mut_ptr(), console_pids.len() as u32);
            if n as usize > console_pids.len() {
                console_pids.resize(n as usize, 0);
                n = GetConsoleProcessList(console_pids.as_mut_ptr(), console_pids.len() as u32);
            }
            {
                let members: Vec<String> = console_pids[..(n as usize).min(console_pids.len())].iter()
                    .map(|p| crate::platform::process_info::classify_console_member(*p))
                    .collect();
                log(&format!("console process list n={} members=[{}]", n, members.join(" | ")));
            }
            if n > 0 && crate::platform::process_info::console_broadcast_hits_bridge(&console_pids[..(n as usize).min(console_pids.len())]) {
                log(&format!("vt bridge on pane console (pid={}): deliver raw 0x03 only, skip CTRL_C_EVENT broadcast", child_pid));
                // Same cooked-mode hazard as the earlier skip paths: make sure
                // the raw 0x03 arrives as input, not as a conhost-side signal
                // — UNLESS a plain native app (ping under Git Bash) is on the
                // console, in which case conhost's conversion IS its interrupt
                // and must stay armed (measured regression).
                if crate::platform::process_info::console_has_plain_native_app(&console_pids[..(n as usize).min(console_pids.len())]) {
                    log("plain native app on pane console: keep PROCESSED_INPUT so its interrupt still fires");
                } else {
                    let conin: [u16; 7] = [
                        'C' as u16, 'O' as u16, 'N' as u16,
                        'I' as u16, 'N' as u16, '$' as u16, 0,
                    ];
                    let h = CreateFileW(
                        conin.as_ptr(),
                        GENERIC_READ | GENERIC_WRITE,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        std::ptr::null(),
                        OPEN_EXISTING,
                        0,
                        std::ptr::null(),
                    );
                    if h != INVALID_HANDLE && h != 0 {
                        let mut mode: u32 = 0;
                        if GetConsoleMode(h as *mut c_void, &mut mode) != 0
                            && mode & ENABLE_PROCESSED_INPUT != 0
                        {
                            SetConsoleMode(h as *mut c_void, mode & !ENABLE_PROCESSED_INPUT);
                            log(&format!("stripped PROCESSED_INPUT (was 0x{:04X}) so the raw 0x03 arrives as input", mode));
                        }
                        CloseHandle(h);
                    }
                }
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Open the console input buffer to check / fix ENABLE_PROCESSED_INPUT
            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            // If we temporarily flip ENABLE_PROCESSED_INPUT on to make
            // GenerateConsoleCtrlEvent fire, remember the handle + original mode
            // so we can restore the shell's raw console state afterwards.
            //
            // Leaving PROCESSED_INPUT permanently ON corrupts PSReadLine, which
            // deliberately runs the console RAW (PROCESSED_INPUT cleared) so it
            // can read Ctrl+C as a key event and cancel the input line.  Once the
            // flag is stuck on, the raw 0x03 byte the caller writes is swallowed
            // by the console as a no-op CTRL_C_EVENT at a bare prompt instead of
            // reaching PSReadLine as a key, so only the *first* Ctrl+C cancels the
            // line and every subsequent press is silently dropped (repeated-Ctrl+C
            // regression).
            let mut restore_mode: Option<(isize, u32)> = None;

            if handle != INVALID_HANDLE && handle != 0 {
                let mut mode: u32 = 0;
                if GetConsoleMode(handle as *mut c_void, &mut mode) != 0 {
                    log(&format!("console mode=0x{:04X} PROCESSED_INPUT={} fg_is_shell={}", mode, mode & ENABLE_PROCESSED_INPUT != 0, fg_is_shell));
                    if mode & ENABLE_PROCESSED_INPUT == 0 {
                        if !fg_is_shell {
                            // Live raw-mode TUI (Copilot CLI, vim, ...): it
                            // cleared ENABLE_PROCESSED_INPUT to read raw 0x03
                            // itself and decide copy-vs-interrupt.  The call
                            // site writes raw 0x03 to the PTY (just before or
                            // just after this call); firing GenerateConsoleCtrlEvent
                            // would bypass the app and kill it.  Skip the signal
                            // and detach cleanly.  (We have not installed the
                            // ignore-handler yet, so there is nothing to restore.)
                            log(&format!("raw-mode non-shell foreground pid={}: deliver raw 0x03, skip CTRL_C_EVENT", child_pid));
                            CloseHandle(handle);
                            FreeConsole();
                            if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                            return false;
                        }
                        // Raw-mode shell prompt (e.g. PSReadLine).  Flip
                        // PROCESSED_INPUT on *only* for the duration of the
                        // signal, then restore the original raw mode below so the
                        // NEXT Ctrl+C is still delivered to the shell as a key
                        // event.  Keep the handle open until the restore.
                        log(&format!("re-enabling ENABLE_PROCESSED_INPUT (temporary) for pid={}", child_pid));
                        SetConsoleMode(handle as *mut c_void, mode | ENABLE_PROCESSED_INPUT);
                        restore_mode = Some((handle, mode));
                    } else {
                        CloseHandle(handle);
                    }
                } else {
                    CloseHandle(handle);
                }
            }

            // Ignore CTRL_C in our own process so GenerateConsoleCtrlEvent
            // doesn't kill psmux (we're temporarily on the child's console).
            // Passing None as handler with add=1 tells the system to ignore
            // Ctrl+C signals in this process.
            SetConsoleCtrlHandler(None, 1);

            let ok = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
            let err = GetLastError();

            log(&format!("GenerateConsoleCtrlEvent => ok={} err={}", ok, err));

            // GenerateConsoleCtrlEvent dispatches asynchronously via a system
            // thread pool.  Sleep while still attached so the signal has time
            // to propagate through the console subsystem before we detach.
            // psmux is protected by the preceding SetConsoleCtrlHandler(None, 1).
            std::thread::sleep(std::time::Duration::from_millis(5));

            // Restore the shell's original (raw) console input mode now that the
            // signal has been delivered.  This is what keeps *repeated* Ctrl+C
            // working: PSReadLine left PROCESSED_INPUT cleared so it could read
            // Ctrl+C as a key, and the next press must still arrive that way.
            // Leaving it cooked makes every Ctrl+C after the first a silent
            // no-op at a bare prompt.
            if let Some((h, orig)) = restore_mode {
                SetConsoleMode(h as *mut c_void, orig);
                CloseHandle(h);
            }

            // Detach from the child's console BEFORE restoring Ctrl+C handling.
            // If we restore the default handler while still attached, the async
            // handler thread might terminate psmux.  Detaching first ensures the
            // event only targets processes that remain on the console.
            FreeConsole();

            // Restore default Ctrl+C handling now that we're detached
            SetConsoleCtrlHandler(None, 0);

            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            ok != 0
        }
    }

    /// Send a genuine CTRL_BREAK_EVENT to the pane's ConPTY child (issue #454).
    ///
    /// Ctrl+Break exists precisely to stop a program that ignores Ctrl+C, so it
    /// must deliver a REAL break signal — not the Ctrl+C path a program can trap
    /// and swallow.  We briefly FreeConsole()/AttachConsole() onto the child's
    /// hidden ConPTY console and broadcast CTRL_BREAK_EVENT to process group 0,
    /// exactly what a terminal emulator does when the user presses Ctrl+Break.
    ///
    /// This DOES reach the ConPTY child: attaching to the child's console places
    /// us in its process group, and the broadcast reaches every process on it
    /// (proven to kill a Ctrl+C-immune program while the session survives). The
    /// temporarily-attached server survives because its own process-wide console
    /// control handler returns TRUE for CTRL_BREAK_EVENT (see
    /// `install_console_ctrl_handler`).
    ///
    /// Unlike Ctrl+C there is no copy-vs-interrupt negotiation: Ctrl+Break is an
    /// unconditional break in a native console, so we deliver it regardless of
    /// the foreground app's console input mode — its delivery is not gated by
    /// ENABLE_PROCESSED_INPUT.
    pub fn send_ctrl_break_event(child_pid: u32, reattach: bool) -> bool {
        const CTRL_BREAK_EVENT: u32 = 1;

        type HandlerRoutine = unsafe extern "system" fn(u32) -> i32;

        #[link(name = "kernel32")]
        extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<HandlerRoutine>,
                add: i32,
            ) -> i32;
            fn GenerateConsoleCtrlEvent(
                ctrl_event: u32,
                process_group_id: u32,
            ) -> i32;
        }

        fn log(msg: &str) {
            debug_log(&format!("ctrl_break: {}", msg));
        }

        // A handler that SURVIVES both Ctrl+C and Ctrl+Break.  We attach to the
        // child's console and broadcast CTRL_BREAK to group 0, which also targets
        // us (the server) since we are momentarily on that console.  Returning
        // TRUE for CTRL_BREAK_EVENT is what keeps the server — and therefore every
        // other session — alive through our own broadcast.  Registered AFTER
        // AttachConsole and torn down immediately after, exactly like a terminal
        // emulator's Ctrl+Break sender.  (The startup handler alone did not
        // protect the server here; a freshly-registered handler does.)
        unsafe extern "system" fn survive_break(ctrl_type: u32) -> i32 {
            match ctrl_type {
                0 | 1 => 1, // CTRL_C_EVENT | CTRL_BREAK_EVENT -> handled, survive
                _ => 0,
            }
        }

        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = reattach && GetConsoleWindow() != 0;

            FreeConsole();

            log(&format!("called: pid={} reattach={} had_console={}", child_pid, reattach, had_console));

            if AttachConsole(child_pid) == 0 {
                let err = GetLastError();
                log(&format!("AttachConsole({}) FAILED err={}", child_pid, err));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Install the survive-break handler now that we share the child's
            // console, so the broadcast below cannot terminate the server.
            SetConsoleCtrlHandler(Some(survive_break), 1);

            let ok = GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0);
            let err = GetLastError();
            log(&format!("GenerateConsoleCtrlEvent(CTRL_BREAK) => ok={} err={}", ok, err));

            // GenerateConsoleCtrlEvent dispatches asynchronously via a system
            // thread pool.  Sleep while still attached AND still protected so the
            // signal propagates through the console subsystem before we detach.
            std::thread::sleep(std::time::Duration::from_millis(20));

            // Detach from the child's console first, then remove our temporary
            // handler so a late async break can only target processes that remain
            // on the console.  The server's permanent startup handler remains.
            FreeConsole();
            SetConsoleCtrlHandler(Some(survive_break), 0);

            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            ok != 0
        }
    }

    pub fn char_to_vk(ch: char) -> u16 {
        match ch {
            '\x1b' => 0x1B,  // VK_ESCAPE — VkKeyScanW returns -1 for non-printable
            '\r'   => 0x0D,  // VK_RETURN
            _ => {
                #[link(name = "user32")]
                extern "system" {
                    fn VkKeyScanW(ch: u16) -> i16;
                }
                let mut buf = [0u16; 2];
                let wch = ch.to_ascii_lowercase().encode_utf16(&mut buf)[0];
                let result = unsafe { VkKeyScanW(wch) };
                if result == -1 { 0u16 } else { (result & 0xFF) as u16 }
            }
        }
    }

    /// Map a virtual key code to its scan code.
    pub fn vk_to_scan(vk: u16) -> u16 {
        #[link(name = "kernel32")]
        extern "system" {
            fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
        }
        // MAPVK_VK_TO_VSC = 0
        unsafe { MapVirtualKeyW(vk as u32, 0) as u16 }
    }

    /// Inject a modified key event into a child process's console input buffer.
    ///
    /// Uses WriteConsoleInputW with the appropriate control_key_state flags
    /// (LEFT_CTRL_PRESSED, LEFT_ALT_PRESSED, SHIFT_PRESSED) matching how
    /// Windows Terminal synthesises input events.
    ///
    /// This is necessary because ConPTY does NOT reassemble ESC+char into
    /// native Alt+key events — PSReadLine and other console apps receive
    /// them as separate key events.  Similarly, Ctrl+Alt+key written as
    /// ESC + control-char is not reassembled.
    ///
    /// For Ctrl+key: `u_char` = control character (ch & 0x1F); for Alt+key:
    /// `u_char` = the plain char; for Ctrl+Alt: `u_char` = control character.
    /// Sends both key-down and key-up events for proper event pairing.
    pub fn send_modified_key_event(child_pid: u32, ch: char, ctrl: bool, alt: bool, shift: bool) -> bool {
        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = GetConsoleWindow() != 0;
            FreeConsole();

            if AttachConsole(child_pid) == 0 {
                debug_log(&format!("send_modified_key_event: AttachConsole({}) FAILED", child_pid));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                debug_log(&format!("send_modified_key_event: CreateFileW(CONIN$) FAILED"));
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            const KEY_EVENT: u16 = 0x0001;
            const LEFT_ALT_PRESSED: u32 = 0x0002;
            const LEFT_CTRL_PRESSED: u32 = 0x0008;
            const SHIFT_PRESSED: u32 = 0x0010;

            #[repr(C)]
            #[derive(Copy, Clone)]
            struct KEY_EVENT_RECORD {
                key_down: i32,
                repeat_count: u16,
                virtual_key_code: u16,
                virtual_scan_code: u16,
                u_char: u16,
                control_key_state: u32,
            }

            #[repr(C)]
            struct KEY_INPUT_RECORD {
                event_type: u16,
                _padding: u16,
                event: KEY_EVENT_RECORD,
            }

            // Build control_key_state flags (matching Windows Terminal convention)
            let mut flags: u32 = 0;
            if ctrl { flags |= LEFT_CTRL_PRESSED; }
            if alt  { flags |= LEFT_ALT_PRESSED; }
            if shift { flags |= SHIFT_PRESSED; }

            let base_char = if shift && !ctrl { ch.to_ascii_uppercase() } else { ch };
            let u_char_value: u16 = if ctrl {
                (base_char.to_ascii_lowercase() as u16) & 0x1F
            } else {
                let mut buf = [0u16; 2];
                base_char.encode_utf16(&mut buf)[0]
            };

            let vk = char_to_vk(ch);
            let scan = vk_to_scan(vk);

            let records = [
                KEY_INPUT_RECORD {
                    event_type: KEY_EVENT,
                    _padding: 0,
                    event: KEY_EVENT_RECORD {
                        key_down: 1,
                        repeat_count: 1,
                        virtual_key_code: vk,
                        virtual_scan_code: scan,
                        u_char: u_char_value,
                        control_key_state: flags,
                    },
                },
                KEY_INPUT_RECORD {
                    event_type: KEY_EVENT,
                    _padding: 0,
                    event: KEY_EVENT_RECORD {
                        key_down: 0,
                        repeat_count: 1,
                        virtual_key_code: vk,
                        virtual_scan_code: scan,
                        u_char: u_char_value,
                        control_key_state: flags,
                    },
                },
            ];

            let mut written: u32 = 0;
            let result = WriteConsoleInputW(
                handle,
                records.as_ptr() as *const INPUT_RECORD,
                2,
                &mut written,
            );

            debug_log(&format!("send_modified_key_event: pid={} char='{}' ctrl={} alt={} shift={} vk=0x{:02X} scan=0x{:02X} u_char=0x{:04X} flags=0x{:04X} => ok={} written={}",
                child_pid, ch, ctrl, alt, shift, vk, scan, u_char_value, flags, result != 0, written));

            CloseHandle(handle);
            FreeConsole();
            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            result != 0 && written >= 1
        }
    }

    /// Convenience: inject Alt+key event.
    pub fn send_alt_key_event(child_pid: u32, ch: char) -> bool {
        send_modified_key_event(child_pid, ch, false, true, false)
    }

    /// Inject a modified Enter (VK_RETURN) event via WriteConsoleInputW.
    ///
    /// ConPTY cannot reconstruct Shift+Enter from VT sequences (\x1b\r is
    /// misinterpreted as Alt+Enter).  Native injection delivers the exact
    /// KEY_EVENT_RECORD with the correct modifier flags, so PSReadLine and
    /// other console-API-based readers see the true Shift/Ctrl/Alt+Enter.
    pub fn send_modified_enter_event(child_pid: u32, ctrl: bool, alt: bool, shift: bool) -> bool {
        let _console_guard = portable_pty::console_state_lock();
        unsafe {
            let had_console = GetConsoleWindow() != 0;
            FreeConsole();

            if AttachConsole(child_pid) == 0 {
                debug_log(&format!("send_modified_enter_event: AttachConsole({}) FAILED", child_pid));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                debug_log(&format!("send_modified_enter_event: CreateFileW(CONIN$) FAILED"));
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            const KEY_EVENT: u16 = 0x0001;
            const LEFT_ALT_PRESSED: u32 = 0x0002;
            const LEFT_CTRL_PRESSED: u32 = 0x0008;
            const SHIFT_PRESSED: u32 = 0x0010;
            const VK_RETURN: u16 = 0x0D;

            #[repr(C)]
            #[derive(Copy, Clone)]
            struct KEY_EVENT_RECORD {
                key_down: i32,
                repeat_count: u16,
                virtual_key_code: u16,
                virtual_scan_code: u16,
                u_char: u16,
                control_key_state: u32,
            }

            #[repr(C)]
            struct KEY_INPUT_RECORD {
                event_type: u16,
                _padding: u16,
                event: KEY_EVENT_RECORD,
            }

            #[link(name = "user32")]
            extern "system" {
                fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
            }

            let mut flags: u32 = 0;
            if ctrl  { flags |= LEFT_CTRL_PRESSED; }
            if alt   { flags |= LEFT_ALT_PRESSED; }
            if shift { flags |= SHIFT_PRESSED; }

            // MAPVK_VK_TO_VSC = 0
            let scan = MapVirtualKeyW(VK_RETURN as u32, 0) as u16;

            // Plain Ctrl+Enter carries LF (0x0A) as the character payload, matching
            // Windows Terminal's regular input encoder (TerminalInput::_encodeRegular).
            // The VK_RETURN + LEFT_CTRL metadata is preserved so Console-API readers
            // (PSReadLine) still see Ctrl+Enter, while VT/raw stdin readers (Node/libuv
            // apps like pi, Claude Code) receive LF instead of CR (#409).  Shift/Alt
            // Enter variants keep CR to preserve their existing behavior.
            let u_char = if ctrl && !alt && !shift { '\n' as u16 } else { '\r' as u16 };

            let records = [
                KEY_INPUT_RECORD {
                    event_type: KEY_EVENT,
                    _padding: 0,
                    event: KEY_EVENT_RECORD {
                        key_down: 1,
                        repeat_count: 1,
                        virtual_key_code: VK_RETURN,
                        virtual_scan_code: scan,
                        u_char,
                        control_key_state: flags,
                    },
                },
                KEY_INPUT_RECORD {
                    event_type: KEY_EVENT,
                    _padding: 0,
                    event: KEY_EVENT_RECORD {
                        key_down: 0,
                        repeat_count: 1,
                        virtual_key_code: VK_RETURN,
                        virtual_scan_code: scan,
                        u_char,
                        control_key_state: flags,
                    },
                },
            ];

            let mut written: u32 = 0;
            let result = WriteConsoleInputW(
                handle,
                records.as_ptr() as *const INPUT_RECORD,
                2,
                &mut written,
            );

            debug_log(&format!("send_modified_enter_event: pid={} ctrl={} alt={} shift={} scan=0x{:02X} u_char=0x{:04X} flags=0x{:04X} => ok={} written={}",
                child_pid, ctrl, alt, shift, scan, u_char, flags, result != 0, written));

            CloseHandle(handle);
            FreeConsole();
            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            result != 0 && written >= 1
        }
    }
}

#[cfg(not(windows))]
pub mod mouse_inject {
    // Win32 MOUSE_EVENT flag values (winuser.h). The stubs below ignore them,
    // but callers reference the constants directly so they must exist on
    // every platform.
    pub const FROM_LEFT_1ST_BUTTON_PRESSED: u32 = 0x0001;
    pub const RIGHTMOST_BUTTON_PRESSED: u32 = 0x0002;
    pub const FROM_LEFT_2ND_BUTTON_PRESSED: u32 = 0x0004;
    pub const MOUSE_MOVED: u32 = 0x0001;
    pub const MOUSE_WHEELED: u32 = 0x0004;
    pub fn get_child_pid(_child: &dyn portable_pty::Child) -> Option<u32> { None }
    pub fn send_mouse_event(_pid: u32, _col: i16, _row: i16, _btn: u32, _flags: u32, _reattach: bool) -> bool { false }
    pub fn send_vt_sequence(_pid: u32, _sequence: &[u8]) -> bool { false }
    pub fn query_vti_enabled(_pid: u32) -> Option<bool> { None }
    pub fn ensure_vti_enabled(_pid: u32) -> bool { false }
    pub fn send_ctrl_c_event(_pid: u32, _reattach: bool) -> bool { false }
    pub fn send_ctrl_break_event(_pid: u32, _reattach: bool) -> bool { false }
    pub fn query_mouse_input_enabled(_pid: u32) -> Option<bool> { None }
    pub fn send_bracketed_paste(_pid: u32, _text: &str, _bracket: bool) -> bool { false }
    pub fn send_vt_response(_pid: u32, _text: &str) -> bool { false }
    pub fn send_modified_key_event(_pid: u32, _ch: char, _ctrl: bool, _alt: bool, _shift: bool) -> bool { false }
    pub fn send_alt_key_event(_pid: u32, _ch: char) -> bool { false }
    pub fn send_modified_enter_event(_pid: u32, _ctrl: bool, _alt: bool, _shift: bool) -> bool { false }
    pub fn char_to_vk(_ch: char) -> u16 { 0 }
    pub fn vk_to_scan(_vk: u16) -> u16 { 0 }
}

// ---------------------------------------------------------------------------
// Process tree killing — ensures all descendant processes are terminated
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod process_kill {
    const TH32CS_SNAPPROCESS: u32 = 0x00000002;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const INVALID_HANDLE: isize = -1;

    #[repr(C)]
    struct PROCESSENTRY32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FILETIME {
        dwLowDateTime: u32,
        dwHighDateTime: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        fn Process32FirstW(h_snapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
        fn Process32NextW(h_snapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn TerminateProcess(h_process: isize, exit_code: u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetProcessTimes(
            h_process: isize,
            lp_creation: *mut FILETIME,
            lp_exit: *mut FILETIME,
            lp_kernel: *mut FILETIME,
            lp_user: *mut FILETIME,
        ) -> i32;
        fn GetSystemTimeAsFileTime(lp: *mut FILETIME);
    }

    #[inline]
    fn filetime_to_u64(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
    }

    /// Current system time as a 64-bit FILETIME (100ns ticks since 1601).
    /// Used as the "cutoff" for PID-reuse detection: any process whose
    /// creation time is LATER than the cutoff captured just before our
    /// snapshot cannot be a process we enumerated, so it must be a reused PID.
    fn now_filetime() -> u64 {
        unsafe {
            let mut ft = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            GetSystemTimeAsFileTime(&mut ft);
            filetime_to_u64(ft)
        }
    }

    /// Read a process's creation time (FILETIME) by PID. Returns None if the
    /// process cannot be opened or queried (already gone, or a different
    /// security context). The handle is opened with QUERY_LIMITED_INFORMATION
    /// which succeeds for same-user processes.
    fn process_creation_filetime(pid: u32) -> Option<u64> {
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h == 0 || h == INVALID_HANDLE {
                return None;
            }
            let mut creation = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let mut exit = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let mut kernel = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let mut user = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let ok = GetProcessTimes(h, &mut creation, &mut exit, &mut kernel, &mut user);
            CloseHandle(h);
            if ok == 0 {
                return None;
            }
            Some(filetime_to_u64(creation))
        }
    }

    /// Collect all descendant PIDs of `root_pid` (children, grandchildren, etc.).
    /// Uses a breadth-first traversal of the process tree snapshot.
    fn collect_descendants(root_pid: u32) -> Vec<u32> {
        let mut descendants = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE || snap == 0 { return descendants; }

            // Build full process table from snapshot
            let mut entries: Vec<(u32, u32)> = Vec::with_capacity(256); // (pid, parent_pid)
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snap, &mut pe) != 0 {
                entries.push((pe.th32_process_id, pe.th32_parent_process_id));
                while Process32NextW(snap, &mut pe) != 0 {
                    entries.push((pe.th32_process_id, pe.th32_parent_process_id));
                }
            }
            CloseHandle(snap);

            // BFS from root_pid. Every edge is validated against process
            // creation time before being followed: a stale ParentProcessId
            // link (the parent PID has been reused by an unrelated process
            // since the real parent exited) fails `edge_is_genuine` and is
            // not traversed, which is what keeps this BFS from walking out
            // of the pane's process tree into the OS process hierarchy.
            let mut creation_cache: std::collections::HashMap<u32, Option<u64>> =
                std::collections::HashMap::new();
            let mut creation_of = |pid: u32| -> Option<u64> {
                *creation_cache.entry(pid).or_insert_with(|| process_creation_filetime(pid))
            };
            let mut queue: Vec<u32> = vec![root_pid];
            let mut head = 0;
            while head < queue.len() {
                let parent = queue[head];
                head += 1;
                for &(pid, ppid) in &entries {
                    if ppid == parent && pid != root_pid && !queue.contains(&pid)
                        && edge_is_genuine(creation_of(parent), creation_of(pid))
                    {
                        queue.push(pid);
                        descendants.push(pid);
                    }
                }
            }
        }
        descendants
    }

    /// Executable base names (no extension, lowercase) that must never be
    /// force-killed by psmux under any circumstances. Every name on this list
    /// is a Windows critical-process image: `TerminateProcess`-ing any of
    /// them triggers bugcheck 0xEF `CRITICAL_PROCESS_DIED` and reboots the
    /// whole machine, not just the target process.
    pub(crate) fn is_protected_image(name: &str) -> bool {
        const PROTECTED: &[&str] = &[
            "csrss", "smss", "wininit", "winlogon", "services", "lsass",
            "lsaiso", "svchost", "dwm", "fontdrvhost",
        ];
        let lower = name.to_ascii_lowercase();
        let stripped = lower.strip_suffix(".exe").unwrap_or(&lower);
        PROTECTED.contains(&stripped)
    }

    /// A parent→child edge in the Toolhelp32 snapshot is only trustworthy if
    /// the child was actually created after the parent. Windows reuses PIDs;
    /// once a real parent process exits, its PID can be handed to an
    /// unrelated process, and any process that still lists the old (now
    /// reused) PID as its `ParentProcessId` produces a stale edge that BFS
    /// would otherwise happily walk into the OS process hierarchy. A real
    /// child is always created strictly after its parent, so this is a
    /// necessary (not just heuristic) property of a genuine edge. Either
    /// creation time being unknown fails safe to "not genuine" so an
    /// unqueryable process is never traversed into.
    pub(crate) fn edge_is_genuine(parent_creation: Option<u64>, child_creation: Option<u64>) -> bool {
        match (parent_creation, child_creation) {
            (Some(parent), Some(child)) => child >= parent,
            _ => false,
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn ProcessIdToSessionId(dw_process_id: u32, p_session_id: *mut u32) -> i32;
    }

    /// Terminal Services session ID that owns `pid`, or `None` if it cannot be
    /// determined (pid 0, already-exited process, or the API call fails).
    fn process_session_id(pid: u32) -> Option<u32> {
        if pid == 0 {
            return None;
        }
        unsafe {
            let mut session_id: u32 = 0;
            let ok = ProcessIdToSessionId(pid, &mut session_id);
            if ok == 0 {
                return None;
            }
            Some(session_id)
        }
    }

    /// Fail-safe refuse-to-kill verdict for a PID reached via process-tree
    /// traversal. Polarity is deliberately "unknown means protected": every
    /// pane descendant psmux legitimately tears down is a same-user child
    /// process, and `get_process_name`/`process_session_id` both use
    /// `PROCESS_QUERY_LIMITED_INFORMATION`, which succeeds for same-user
    /// processes regardless of elevation — so a genuine pane descendant is
    /// always queryable and this gate never blocks legitimate teardown. Only
    /// a misidentified target (recycled PID pointing at a system process, or
    /// an identity we can't confirm) trips it.
    pub fn is_protected_system_process(pid: u32) -> bool {
        if pid == 0 || pid == 4 {
            return true;
        }
        match super::process_info::get_process_name(pid) {
            None => return true,
            Some(name) => {
                if is_protected_image(&name.to_ascii_lowercase()) {
                    return true;
                }
            }
        }
        let target_session = process_session_id(pid);
        let our_session = process_session_id(std::process::id());
        match (target_session, our_session) {
            (Some(t), Some(o)) if t == o => false,
            _ => true,
        }
    }

    /// Force-terminate a single process by PID, guarded against PID reuse by
    /// process creation time (issue #447).
    ///
    /// `max_creation_ft` is `Some(cutoff)` for callers that identified `pid` from
    /// a process snapshot: they capture the cutoff (via `now_filetime()`)
    /// immediately BEFORE snapshotting, so any legitimately enumerated process
    /// was created at or before it. A pid since reused by an unrelated process
    /// was created AFTER the cutoff, so its creation time exceeds it and we refuse
    /// to kill; a pid we cannot query is skipped too (fail safe).
    ///
    /// `None` is for callers that have already settled identity another way — an
    /// exact creation-time match (kill-server's `confirms_identity`) or a
    /// first-class child handle — so no snapshot cutoff applies.
    fn terminate_pid(pid: u32, max_creation_ft: Option<u64>) {
        // PID-reuse guard: verify identity by creation time before killing.
        if let Some(cutoff) = max_creation_ft {
            match process_creation_filetime(pid) {
                // Created after our snapshot cutoff -> PID was reused. Do NOT kill.
                Some(created) if created > cutoff => return,
                // Could not confirm identity (gone / foreign context). Skip to
                // stay on the safe side of the false-kill race.
                None => return,
                _ => {}
            }
        }
        // Fail-safe protection gate: a stale/recycled parent-child edge in the
        // Toolhelp32 BFS (see `edge_is_genuine`) can walk traversal out of the
        // pane's process tree and into the OS process hierarchy (session-0
        // svchost.exe and friends). Terminating one of those triggers bugcheck
        // 0xEF CRITICAL_PROCESS_DIED and reboots the machine, so this check
        // runs on every terminate_pid call regardless of caller.
        if is_protected_system_process(pid) {
            if crate::debug_log::session_log_enabled() {
                crate::debug_log::session_log("process_kill", &format!(
                    "refused to terminate pid {} (protected system process guard)", pid));
            }
            return;
        }
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_INFORMATION, 0, pid);
            if h != 0 && h != INVALID_HANDLE {
                let _ = TerminateProcess(h, 1);
                CloseHandle(h);
            }
        }
    }

    /// Look up the parent process ID of the calling process via the snapshot
    /// table.  Returns None if the snapshot fails or the current PID isn't
    /// found (extremely unlikely).  Used by `detach-client -P` (issue #275).
    pub fn current_parent_pid() -> Option<u32> {
        unsafe {
            let cur_pid = GetCurrentProcessIdSafe();
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE || snap == 0 { return None; }
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found: Option<u32> = None;
            if Process32FirstW(snap, &mut pe) != 0 {
                if pe.th32_process_id == cur_pid {
                    found = Some(pe.th32_parent_process_id);
                }
                while found.is_none() && Process32NextW(snap, &mut pe) != 0 {
                    if pe.th32_process_id == cur_pid {
                        found = Some(pe.th32_parent_process_id);
                    }
                }
            }
            CloseHandle(snap);
            found
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        #[link_name = "GetCurrentProcessId"]
        fn GetCurrentProcessIdSafe() -> u32;
    }

    /// Forcefully terminate the calling process's parent.  Used to implement
    /// `detach-client -P` parity with tmux (which sends SIGHUP to the parent
    /// shell on POSIX).  Returns true if the parent was located and a
    /// termination request was issued.
    pub fn kill_parent_process() -> bool {
        if let Some(ppid) = current_parent_pid() {
            // Sanity check: don't terminate PID 0 / 4 (System / kernel).
            if ppid == 0 || ppid == 4 { return false; }
            // detach-client -P intentionally targets the caller's own parent
            // shell; there is no snapshot cutoff to verify against, so kill
            // unconditionally.
            terminate_pid(ppid, None);
            true
        } else {
            false
        }
    }

    /// Kill an entire process tree: all descendants first (leaves → root order),
    /// then the root process itself.  Calls `child.kill()` via portable_pty as a
    /// fallback.  Does NOT call `child.wait()` so `try_wait()` still works for
    /// the reaper (`prune_exited`), which will detect the dead process and clean
    /// up the tree node.
    ///
    /// This mirrors how tmux on Linux sends SIGKILL to the pane's process group.
    pub fn kill_process_tree(child: &mut Box<dyn portable_pty::Child>) {
        // Try to get the PID
        let pid = super::mouse_inject::get_child_pid(child.as_ref());

        if let Some(root_pid) = pid {
            // root is still alive here (we just read its PID from the live child
            // handle), so its creation time is <= this cutoff. Used to guard the
            // root PID-kill below against reuse.
            let entry_cutoff = now_filetime();
            // Sweep descendants leaf-first. We run the sweep TWICE: the second
            // pass (a fresh snapshot) catches children the tree spawned AFTER
            // the first snapshot but before we tore it down (issue #447 race #2
            // "missed children"). Each pass captures its own creation-time
            // cutoff BEFORE snapshotting so the PID-reuse guard in terminate_pid
            // rejects any PID reused by a process created after that snapshot.
            for _ in 0..2 {
                let cutoff = now_filetime();
                let mut descs = collect_descendants(root_pid);
                if descs.is_empty() {
                    break;
                }
                descs.reverse();
                for &dpid in &descs {
                    terminate_pid(dpid, Some(cutoff));
                }
            }
            // Kill the root process last. Its PID also gets the reuse guard,
            // gated on the entry cutoff captured while root was still alive.
            terminate_pid(root_pid, Some(entry_cutoff));
        }

        // Fallback: tell portable_pty to kill the direct child process.
        // Do NOT call child.wait() here — the reaper (prune_exited) needs
        // try_wait() to detect the dead process and remove the tree node.
        let _ = child.kill();
    }

    /// Kill multiple process trees using a SINGLE process snapshot.
    /// Much faster than calling `kill_process_tree` N times when
    /// killing an entire session (avoids N separate system snapshots).
    pub fn kill_process_trees_batch(children: &mut [&mut Box<dyn portable_pty::Child>]) {
        // Collect all root PIDs
        let root_pids: Vec<Option<u32>> = children.iter()
            .map(|c| super::mouse_inject::get_child_pid(c.as_ref()))
            .collect();

        // Capture the reuse-guard cutoff BEFORE taking the snapshot: every PID
        // enumerated below was created at or before this instant, so any PID
        // reused by a process created afterwards is rejected by terminate_pid
        // (issue #447 PID-reuse guard).
        let cutoff = now_filetime();

        // Take ONE process snapshot for all trees
        let entries = snapshot_process_table();

        // For each root PID, find descendants using the shared snapshot
        for (i, root_pid_opt) in root_pids.iter().enumerate() {
            if let Some(root_pid) = root_pid_opt {
                let mut descs = collect_descendants_from_table(&entries, *root_pid);
                descs.reverse();
                for &dpid in &descs {
                    terminate_pid(dpid, Some(cutoff));
                }
                terminate_pid(*root_pid, Some(cutoff));
            }
            let _ = children[i].kill();
        }
    }

    /// Take a system-wide process snapshot and return the process table.
    fn snapshot_process_table() -> Vec<(u32, u32)> {
        let mut entries: Vec<(u32, u32)> = Vec::with_capacity(256);
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE || snap == 0 { return entries; }

            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snap, &mut pe) != 0 {
                entries.push((pe.th32_process_id, pe.th32_parent_process_id));
                while Process32NextW(snap, &mut pe) != 0 {
                    entries.push((pe.th32_process_id, pe.th32_parent_process_id));
                }
            }
            CloseHandle(snap);
        }
        entries
    }

    /// BFS from root_pid using a pre-built process table. Same edge-validation
    /// rule as `collect_descendants`: a parent→child edge is only followed if
    /// `edge_is_genuine` confirms the child's creation time is not older than
    /// the parent's, which rejects stale ParentProcessId links from PID reuse.
    fn collect_descendants_from_table(entries: &[(u32, u32)], root_pid: u32) -> Vec<u32> {
        let mut creation_cache: std::collections::HashMap<u32, Option<u64>> =
            std::collections::HashMap::new();
        let mut creation_of = |pid: u32| -> Option<u64> {
            *creation_cache.entry(pid).or_insert_with(|| process_creation_filetime(pid))
        };
        collect_descendants_from_table_with(entries, root_pid, &mut creation_of)
    }

    /// Core of `collect_descendants_from_table` with the creation-time source
    /// injected, so tests can model PID reuse and snapshot timing with fully
    /// synthetic process tables (live-process lookups would return None for
    /// synthetic PIDs and the edge guard would reject every edge).
    fn collect_descendants_from_table_with(
        entries: &[(u32, u32)],
        root_pid: u32,
        creation_of: &mut dyn FnMut(u32) -> Option<u64>,
    ) -> Vec<u32> {
        let mut descendants = Vec::new();
        let mut queue: Vec<u32> = vec![root_pid];
        let mut head = 0;
        while head < queue.len() {
            let parent = queue[head];
            head += 1;
            for &(pid, ppid) in entries {
                if ppid == parent && pid != root_pid && !queue.contains(&pid)
                    && edge_is_genuine(creation_of(parent), creation_of(pid))
                {
                    queue.push(pid);
                    descendants.push(pid);
                }
            }
        }
        descendants
    }

    // ── Orphaned-server reaper support (issue #448) ───────────────────────
    //
    // The stale-port cleanup only removes registry *files* for servers proven
    // dead; a live server whose registry entry was lost (a spawn-race duplicate,
    // or a crashed client's headless server) keeps running forever, invisible to
    // that file-driven pass. These helpers let the reaper enumerate live psmux
    // server processes by identity (loopback TCP listener + image name + creation
    // time) so an untracked one can be terminated at startup.

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetExtendedTcpTable(
            p_tcp_table: *mut u8,
            pdw_size: *mut u32,
            b_order: i32,
            ul_af: u32,
            table_class: u32,
            reserved: u32,
        ) -> u32;
    }

    const AF_INET: u32 = 2;
    const TCP_TABLE_OWNER_PID_LISTENER: u32 = 3;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    /// 127.0.0.1 as a native-endian u32 (bytes 127,0,0,1 in the on-wire order the
    /// TCP table stores dwLocalAddr in). psmux servers always bind 127.0.0.1, so
    /// this is the only address we consider a server listener.
    const LOOPBACK_ADDR: u32 = 0x0100_007F;

    /// Enumerate every 127.0.0.1 TCP *listener* as `(owning_pid, port)`.
    ///
    /// Uses `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_LISTENER)` so only listening
    /// sockets are returned — a psmux *client* never listens, so clients can never
    /// appear here and are structurally safe from the reaper.
    pub fn loopback_listener_pids() -> Vec<(u32, u16)> {
        let mut out = Vec::new();
        unsafe {
            let mut size: u32 = 0;
            // First call sizes the buffer.
            let _ = GetExtendedTcpTable(
                std::ptr::null_mut(), &mut size, 0, AF_INET,
                TCP_TABLE_OWNER_PID_LISTENER, 0,
            );
            if size == 0 { return out; }
            let mut buf = vec![0u8; size as usize];
            let mut attempts = 0;
            let mut ret = GetExtendedTcpTable(
                buf.as_mut_ptr(), &mut size, 0, AF_INET,
                TCP_TABLE_OWNER_PID_LISTENER, 0,
            );
            // The table can grow between the sizing and filling calls; retry a
            // couple of times on ERROR_INSUFFICIENT_BUFFER with the new size.
            while ret == ERROR_INSUFFICIENT_BUFFER && attempts < 3 {
                buf.resize(size as usize, 0);
                ret = GetExtendedTcpTable(
                    buf.as_mut_ptr(), &mut size, 0, AF_INET,
                    TCP_TABLE_OWNER_PID_LISTENER, 0,
                );
                attempts += 1;
            }
            if ret != 0 { return out; }

            // MIB_TCPTABLE_OWNER_PID: u32 dwNumEntries, then rows.
            // MIB_TCPROW_OWNER_PID (24 bytes): state, localAddr, localPort,
            // remoteAddr, remotePort, owningPid — each a u32.
            let base = buf.as_ptr();
            let num = (base as *const u32).read_unaligned() as usize;
            const ROW: usize = 24;
            for i in 0..num {
                let row = base.add(4 + i * ROW);
                if 4 + i * ROW + ROW > buf.len() { break; }
                let local_addr = (row.add(4) as *const u32).read_unaligned();
                if local_addr != LOOPBACK_ADDR { continue; }
                let local_port_raw = (row.add(8) as *const u32).read_unaligned();
                // dwLocalPort is network byte order in the low 16 bits.
                let port = (((local_port_raw & 0xff) << 8) | ((local_port_raw >> 8) & 0xff)) as u16;
                let pid = (row.add(20) as *const u32).read_unaligned();
                out.push((pid, port));
            }
        }
        out
    }

    /// Current system time as a FILETIME (100ns ticks). Callers capture this
    /// BEFORE enumerating processes and pass it to `terminate_server_pid` as the
    /// PID-reuse cutoff (see `terminate_pid`).
    pub fn now_process_filetime() -> u64 {
        now_filetime()
    }

    /// Creation time (FILETIME) of a process by PID, or None if it can't be read.
    pub fn process_creation_time(pid: u32) -> Option<u64> {
        process_creation_filetime(pid)
    }

    /// Terminate a psmux server PID, guarded against PID reuse by process
    /// creation time (issue #447). Pass `Some(cutoff)` (the reaper's path) to
    /// reject a pid reused by a process created after the snapshot that found it;
    /// pass `None` when the caller has already matched creation time exactly
    /// (kill-server's `confirms_identity` against the stored `.pid` value), which
    /// settles identity and makes the cutoff heuristic redundant.
    pub fn terminate_server_pid(pid: u32, max_creation_ft: Option<u64>) {
        terminate_pid(pid, max_creation_ft);
    }

    #[cfg(test)]
    #[path = "../../../tests-rs/test_issue447_kill_pid_reuse.rs"]
    mod tests_issue447_kill_pid_reuse;

    #[cfg(test)]
    #[path = "../../../tests-rs/test_bsod_kill_guard.rs"]
    mod tests_bsod_kill_guard;
}

#[cfg(not(windows))]
pub mod process_kill {
    /// On non-Windows, fall back to simple kill (no wait — let the reaper handle it).
    pub fn kill_process_tree(child: &mut Box<dyn portable_pty::Child>) {
        let _ = child.kill();
    }

    /// Batch kill — on non-Windows, just kill each child individually.
    pub fn kill_process_trees_batch(children: &mut [&mut Box<dyn portable_pty::Child>]) {
        for child in children.iter_mut() {
            let _ = child.kill();
        }
    }

    // Orphaned-server reaper stubs (issue #448) — no-ops off Windows.
    pub fn loopback_listener_pids() -> Vec<(u32, u16)> { Vec::new() }
    pub fn now_process_filetime() -> u64 { 0 }
    pub fn process_creation_time(_pid: u32) -> Option<u64> { None }
    pub fn terminate_server_pid(_pid: u32, _max_creation_ft: Option<u64>) {}
}

// ---------------------------------------------------------------------------
// Process info queries — get CWD and process name from PID (for format vars)
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod process_info {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;
    const MAX_PATH: usize = 260;
    const TH32CS_SNAPPROCESS: u32 = 0x00000002;
    const INVALID_HANDLE: isize = -1;

    #[allow(non_snake_case)]
    #[repr(C)]
    struct PROCESS_BASIC_INFORMATION {
        Reserved1: isize,
        PebBaseAddress: isize, // pointer to PEB
        Reserved2: [isize; 2],
        UniqueProcessId: isize,
        Reserved3: isize,
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    struct UNICODE_STRING {
        Length: u16,
        MaximumLength: u16,
        Buffer: isize, // pointer to wide string
    }

    #[repr(C)]
    struct PROCESSENTRY32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn QueryFullProcessImageNameW(h: isize, flags: u32, name: *mut u16, size: *mut u32) -> i32;
        fn ReadProcessMemory(
            h_process: isize,
            base_address: isize,
            buffer: *mut u8,
            size: usize,
            bytes_read: *mut usize,
        ) -> i32;
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        fn Process32FirstW(h_snapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
        fn Process32NextW(h_snapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process_handle: isize,
            process_information_class: u32,
            process_information: *mut u8,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    /// Get the executable name of a process by PID (e.g. "pwsh" or "vim").
    pub fn get_process_name(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h == 0 || h == -1 { return None; }
            let mut buf = [0u16; 1024];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
            CloseHandle(h);
            if ok == 0 { return None; }
            let full_path = OsString::from_wide(&buf[..size as usize])
                .to_string_lossy()
                .into_owned();
            let name = std::path::Path::new(&full_path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())?;
            Some(name)
        }
    }

    /// Get the current working directory of a process by PID.
    /// Reads the PEB → ProcessParameters → CurrentDirectory from the target process.
    pub fn get_process_cwd(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
            if h == 0 || h == -1 { return None; }
            let result = read_process_cwd(h);
            CloseHandle(h);
            result
        }
    }

    /// Read CWD from a process handle via NtQueryInformationProcess + ReadProcessMemory.
    unsafe fn read_process_cwd(h: isize) -> Option<String> {
        // Step 1: Get PEB address
        let mut pbi: PROCESS_BASIC_INFORMATION = std::mem::zeroed();
        let mut ret_len: u32 = 0;
        let status = NtQueryInformationProcess(
            h,
            0, // ProcessBasicInformation
            &mut pbi as *mut _ as *mut u8,
            std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut ret_len,
        );
        if status != 0 { return None; }
        let peb_addr = pbi.PebBaseAddress;
        if peb_addr == 0 { return None; }

        // Step 2: Read ProcessParameters pointer from PEB.
        // PEB layout (x64): offset 0x20 = ProcessParameters pointer
        // PEB layout (x86): offset 0x10 = ProcessParameters pointer
        let params_ptr_offset = if std::mem::size_of::<usize>() == 8 { 0x20 } else { 0x10 };
        let mut process_params_ptr: isize = 0;
        let mut bytes_read: usize = 0;
        let ok = ReadProcessMemory(
            h,
            peb_addr + params_ptr_offset,
            &mut process_params_ptr as *mut isize as *mut u8,
            std::mem::size_of::<isize>(),
            &mut bytes_read,
        );
        if ok == 0 || process_params_ptr == 0 { return None; }

        // Step 3: Read CurrentDirectory.DosPath (UNICODE_STRING) from RTL_USER_PROCESS_PARAMETERS.
        // x64 offset: 0x38 = CurrentDirectory.DosPath
        // x86 offset: 0x24 = CurrentDirectory.DosPath
        let cwd_offset = if std::mem::size_of::<usize>() == 8 { 0x38 } else { 0x24 };
        let mut cwd_ustr: UNICODE_STRING = std::mem::zeroed();
        let ok = ReadProcessMemory(
            h,
            process_params_ptr + cwd_offset,
            &mut cwd_ustr as *mut UNICODE_STRING as *mut u8,
            std::mem::size_of::<UNICODE_STRING>(),
            &mut bytes_read,
        );
        if ok == 0 || cwd_ustr.Length == 0 || cwd_ustr.Buffer == 0 { return None; }

        // Step 4: Read the actual CWD wide string
        let char_count = (cwd_ustr.Length / 2) as usize;
        let mut wchars: Vec<u16> = vec![0u16; char_count];
        let ok = ReadProcessMemory(
            h,
            cwd_ustr.Buffer,
            wchars.as_mut_ptr() as *mut u8,
            cwd_ustr.Length as usize,
            &mut bytes_read,
        );
        if ok == 0 { return None; }

        let path = OsString::from_wide(&wchars)
            .to_string_lossy()
            .into_owned();
        // Remove trailing backslash (tmux convention) — but keep it for a
        // drive root: "C:\" trimmed to "C:" is a drive-RELATIVE path on
        // Windows, not the root (surfaced by #547's drive-root repro).
        let trimmed = path.trim_end_matches('\\');
        if trimmed.ends_with(':') {
            Some(format!("{}\\", trimmed))
        } else {
            Some(trimmed.to_string())
        }
    }

    /// Append a line to ~/.psmux/autorename.log (first 100 entries only).
    ///
    /// Gated on `PSMUX_AUTORENAME_DEBUG=1`, like every other logger in
    /// `src/debug_log.rs`. It was previously UNGATED — the only logger in the
    /// tree that was — so every psmux server ever run wrote up to 100 lines of
    /// process-tree tracing from this hot path, with an open+append syscall per
    /// line. On this developer's machine it had accumulated a 703KB file. The
    /// 100-entry cap is per-process, so the file grows without bound across
    /// server restarts and the cap gives no protection against that.
    fn autorename_log(msg: &str) {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::OnceLock;
        static ENABLED: OnceLock<bool> = OnceLock::new();
        if !*ENABLED.get_or_init(|| {
            // Accept "1" or "true", matching debug_log.rs's env_enabled so every
            // debug gate in the tree behaves the same way.
            std::env::var("PSMUX_AUTORENAME_DEBUG")
                .map_or(false, |v| v == "1" || v.eq_ignore_ascii_case("true"))
        }) {
            return;
        }
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        // fetch_add returns the PREVIOUS value, so `>= 100` caps at exactly 100
        // writes (n = 0..=99); `> 100` allowed 101. This cap predates the branch;
        // aligned with the "first 100 entries" doc while touching the function.
        if n >= 100 { return; }
        let path = format!("{}/autorename.log", crate::paths::psmux_dir());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%H:%M:%S%.3f"), msg);
        }
    }

    /// Get the name of the foreground process in the pane.
    /// Walks the process tree from the shell PID to find the deepest
    /// non-system descendant (the user's foreground command).
    pub fn get_foreground_process_name(pid: u32) -> Option<String> {
        // Walk the process tree to find the foreground child.
        let result = find_foreground_child_pid(pid);
        match result {
            Some(target) if target != pid => {
                let name = get_process_name(target);
                autorename_log(&format!("pid={} fg_child={} name={:?}", pid, target, name));
                if let Some(n) = name {
                    return Some(n);
                }
            }
            Some(_) => {
                autorename_log(&format!("pid={} fg_child=self (no children)", pid));
            }
            None => {
                autorename_log(&format!("pid={} fg_child=None (BFS found nothing)", pid));
            }
        }
        // No foreground child found.  Return None so the caller can
        // preserve the current window name instead of briefly flashing
        // to the shell name before the child process has spawned
        // (issue #229).
        autorename_log(&format!("pid={} no_foreground_child", pid));
        None
    }

    /// Name of the pane's DEEPEST foreground descendant, for
    /// `#{pane_current_command}`.
    ///
    /// tmux answers that format with the program in front of the tty, so a
    /// pane sitting at `pwsh -> bash -> cat` must report `cat`.
    /// `get_foreground_process_name` cannot: it looks at the immediate child
    /// and steps one further only for a known wrapper name, so the real
    /// Windows shell chain (git bash alone is `bash.exe -> bash.exe`) already
    /// exhausts its budget and the answer freezes at the shell. Control tools
    /// that key off this format then mis-detect nesting and never see a command
    /// finish.
    ///
    /// Resolution is on demand, off the shared render-path snapshot, and every
    /// failure (snapshot unavailable, root has no descendants, the leaf's
    /// executable path unreadable) degrades quietly so the caller can fall back
    /// to the pane's own process name.
    pub fn get_deepest_foreground_process_name(pid: u32) -> Option<String> {
        let entries = process_table(RENDER_PATH_TTL)?;
        let (leaf_pid, snapshot_name) = deepest_descendant(&entries, pid)?;
        // The snapshot name is lowercased and carries `.exe`; the live query
        // gives the real casing. Fall back to the snapshot when the leaf is
        // gone or unopenable (an elevated process) rather than reporting the
        // shell.
        get_process_name(leaf_pid).or_else(|| {
            Some(snapshot_name.strip_suffix(".exe").unwrap_or(&snapshot_name).to_string())
        })
    }

    /// Follow the highest-PID non-system child at each level from `root_pid`
    /// down to the deepest descendant, returning `(pid, snapshot_name)`.
    /// `None` when the root has no descendants at all.
    ///
    /// Highest PID is this module's established "most recently created"
    /// heuristic (Windows exposes no console foreground process group, and
    /// Toolhelp32 carries no creation time). The iteration guard stops a
    /// pathological loop from PID reuse inside one snapshot.
    fn deepest_descendant(
        entries: &[(u32, u32, String)],
        root_pid: u32,
    ) -> Option<(u32, String)> {
        let mut cur = root_pid;
        let mut leaf: Option<(u32, String)> = None;
        for _ in 0..64 {
            let next = entries.iter()
                .filter(|(pid, ppid, name)| *ppid == cur && *pid != cur && !is_system_exe(name))
                .max_by_key(|(pid, _, _)| *pid);
            match next {
                Some((pid, _, name)) => {
                    cur = *pid;
                    leaf = Some((*pid, name.clone()));
                }
                None => break,
            }
        }
        leaf
    }

    /// Get the CWD of the foreground process in the pane.
    pub fn get_foreground_cwd(pid: u32) -> Option<String> {
        if let Some(target) = find_foreground_child_pid(pid) {
            if target != pid {
                if let Some(cwd) = get_process_cwd(target) {
                    return Some(cwd);
                }
            }
        }
        get_process_cwd(pid)
    }

    /// Known system/infrastructure processes that should be skipped when
    /// walking the process tree to find the user's foreground command.
    fn is_system_exe(name: &str) -> bool {
        matches!(name,
            "conhost.exe" | "csrss.exe" | "dwm.exe" | "services.exe"
            | "svchost.exe" | "wininit.exe" | "winlogon.exe"
            | "openconsole.exe" | "runtimebroker.exe"
        )
    }

    /// Known shell/wrapper executables where the meaningful foreground
    /// command is one level deeper (e.g. `cmd /c foo`, `bash -c foo`,
    /// `npx tool`).  When the immediate child is one of these, we look
    /// at *its* immediate child instead.
    fn is_wrapper_exe(name: &str) -> bool {
        let stem = name.strip_suffix(".exe").unwrap_or(name);
        matches!(stem,
            "cmd" | "bash" | "sh" | "dash" | "zsh" | "fish"
            | "npx" | "npm" | "pnpm" | "yarn" | "bunx"
            | "env" | "sudo" | "runas"
        )
    }

    /// Walk the process tree from `root_pid` downward and return the PID of
    /// the process most likely to be the user's foreground command.
    ///
    /// Strategy: pick the immediate non-system child of `root_pid`.  This
    /// matches tmux's effective behaviour (`tcgetpgrp` returns the process
    /// that took TTY foreground, which is the program the user launched from
    /// the shell).  For known wrapper processes (cmd, bash, npx, ...) we
    /// look one level deeper so the meaningful program is returned instead
    /// of the wrapper.
    fn find_foreground_child_pid(root_pid: u32) -> Option<u32> {
        // Render-path caller: reuse a recent snapshot rather than walking every
        // process on the machine per repaint. See `process_table`.
        let entries = match process_table(RENDER_PATH_TTL) {
            Some(t) => t,
            None => {
                autorename_log(&format!("root={} SNAPSHOT FAILED", root_pid));
                return None;
            }
        };

        autorename_log(&format!("root={} snapshot_entries={}", root_pid, entries.len()));

        // Immediate children of root_pid, skipping system processes.
        let direct: Vec<(u32, String)> = entries.iter()
            .filter(|(_, ppid, name)| *ppid == root_pid && !is_system_exe(name))
            .map(|(pid, _, name)| (*pid, name.clone()))
            .collect();

        for (pid, name) in &direct {
            autorename_log(&format!("  direct_child: pid={} name={}", pid, name));
        }

        if direct.is_empty() {
            autorename_log(&format!("root={} no_direct_children", root_pid));
            return None;
        }

        // Pick the immediate child.  When multiple exist, prefer the
        // largest PID (most recently created).
        let (mut chosen_pid, chosen_name) = direct.iter()
            .max_by_key(|(pid, _)| *pid)
            .map(|(pid, name)| (*pid, name.clone()))
            .unwrap();

        autorename_log(&format!("root={} immediate_child={} name={}", root_pid, chosen_pid, chosen_name));

        // If the immediate child is a known wrapper (cmd, bash, npx, ...),
        // look one level deeper for the real program.
        if is_wrapper_exe(&chosen_name) {
            let grandchildren: Vec<(u32, String)> = entries.iter()
                .filter(|(_, ppid, name)| *ppid == chosen_pid && !is_system_exe(name))
                .map(|(pid, _, name)| (*pid, name.clone()))
                .collect();

            if let Some((gc_pid, gc_name)) = grandchildren.iter()
                .max_by_key(|(pid, _)| *pid)
            {
                autorename_log(&format!(
                    "root={} wrapper={} skip_to_grandchild={} name={}",
                    root_pid, chosen_name, gc_pid, gc_name
                ));
                chosen_pid = *gc_pid;
            }
        }

        autorename_log(&format!("root={} selected={}", root_pid, chosen_pid));
        Some(chosen_pid)
    }

    /// One `(pid, ppid, lowercased_exe_name)` row per process on the machine.
    type ProcTable = std::sync::Arc<Vec<(u32, u32, String)>>;

    static PROC_TABLE_CACHE: std::sync::LazyLock<
        std::sync::Mutex<Option<(std::time::Instant, ProcTable)>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

    thread_local! {
        /// Count of REAL `CreateToolhelp32Snapshot` walks performed by THIS
        /// thread, as opposed to cache hits. The whole point of this module's
        /// caching is a number that does not scale with the frame rate, so the
        /// number is made observable rather than inferred from a timing
        /// measurement, which would be flaky on a loaded machine.
        ///
        /// Per-thread rather than global specifically so the tests are not at
        /// the mercy of the parallel test suite: several other test modules
        /// reach this code through `#{pane_current_command}` and friends, and a
        /// global counter would let their walks inflate another test's delta.
        pub(crate) static PROC_TABLE_WALKS: std::cell::Cell<u64> =
            const { std::cell::Cell::new(0) };
    }

    /// Enumerate every process on the machine, with an optional freshness bound.
    ///
    /// `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` walks the whole system —
    /// ~340 processes on a normal desktop — and three separate functions here
    /// used to do it independently, with their own identical copy of the
    /// enumeration loop and no caching between them.
    ///
    /// That was fine when the callers were rare (a Ctrl+C, a mouse click) and
    /// catastrophic once `#{pane_current_command}` and `#{pane_current_path}`
    /// reached it: both are expanded on the server's per-output render path, so
    /// a status bar or window title referencing them took TWO full snapshots per
    /// repaint — i.e. per keystroke, on the same thread that delivers keystrokes
    /// to ConPTY. `window_ops::scroll_foreground_classify` had already hit this
    /// and worked around it with its own 2s per-pane cache; the format path had
    /// no such guard.
    ///
    /// `max_age` is per-caller on purpose rather than a single global TTL:
    /// reusing a snapshot changes what a caller can observe, and the paths that
    /// route Ctrl+C and mouse events are not places to introduce staleness for a
    /// performance win they do not need. They pass `Duration::ZERO` and always
    /// enumerate fresh; only the render-path callers opt into reuse.
    ///
    /// Returns `None` only when the snapshot itself fails. Failures are never
    /// cached.
    fn process_table(max_age: std::time::Duration) -> Option<ProcTable> {
        if !max_age.is_zero() {
            // Recover a poisoned lock rather than skipping the cache. The inner
            // value is only ever a fully-published entry or None (the critical
            // sections are a single assignment / a clone), so `into_inner` is
            // safe, and silently disabling the cache for the rest of the process
            // after some unrelated panic is the outcome worth avoiding. Matches
            // the recovery the tests use.
            let guard = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, table)) = guard.as_ref() {
                if at.elapsed() < max_age {
                    return Some(std::sync::Arc::clone(table));
                }
            }
        }

        PROC_TABLE_WALKS.with(|c| c.set(c.get() + 1));
        let entries = unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE || snap == 0 {
                return None;
            }
            let mut entries: Vec<(u32, u32, String)> = Vec::with_capacity(512);
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut pe) != 0 {
                entries.push((pe.th32_process_id, pe.th32_parent_process_id, exe_name_from_entry(&pe)));
                while Process32NextW(snap, &mut pe) != 0 {
                    entries.push((pe.th32_process_id, pe.th32_parent_process_id, exe_name_from_entry(&pe)));
                }
            }
            CloseHandle(snap);
            entries
        };

        let table: ProcTable = std::sync::Arc::new(entries);
        // Publish even for a ZERO-max_age caller: it paid for the walk, so a
        // later render-path caller may as well reuse it. Recover a poisoned lock
        // here too (see the read site above) so a panic elsewhere cannot leave
        // the cache permanently unpopulated.
        {
            let mut guard = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some((std::time::Instant::now(), std::sync::Arc::clone(&table)));
        }
        Some(table)
    }

    /// Freshness bound for the render path (`#{pane_current_command}`,
    /// `#{pane_current_path}`, automatic-rename).
    ///
    /// These feed a status bar and a window title, where a fraction of a second
    /// of lag is invisible, and they are re-expanded on every frame. 250ms caps
    /// the cost at 4 snapshots/second no matter how fast a pane is drawing,
    /// while still updating a window title fast enough to look instant.
    /// automatic-rename separately throttles itself to 1/s per pane, so it is
    /// unaffected by this bound in practice.
    const RENDER_PATH_TTL: std::time::Duration = std::time::Duration::from_millis(250);

    /// Extract the lowercased executable name from a PROCESSENTRY32W.
    fn exe_name_from_entry(pe: &PROCESSENTRY32W) -> String {
        let nul = pe.sz_exe_file.iter().position(|&c| c == 0).unwrap_or(pe.sz_exe_file.len());
        String::from_utf16_lossy(&pe.sz_exe_file[..nul]).to_lowercase()
    }

    /// Check if an executable name is a VT bridge process (WSL, SSH, etc.)
    /// that requires VT mouse injection instead of Win32 console injection.
    fn is_vt_bridge_exe(name: &str) -> bool {
        let stem = name.strip_suffix(".exe").unwrap_or(name);
        matches!(stem, "wsl" | "ssh" | "ubuntu" | "debian" | "kali"
                      | "fedoraremix" | "opensuse-leap" | "sles" | "arch")
            || stem.starts_with("wsl")
    }

    /// Native Windows shell executables.  Used by the Ctrl+C router to decide
    /// whether a pane's foreground process expects a console interrupt signal
    /// (shells) or should instead receive raw 0x03 and handle Ctrl+C itself
    /// (live raw-mode TUIs like Copilot CLI, vim, nvim).
    pub fn is_shell_exe(name: &str) -> bool {
        let stem = name.strip_suffix(".exe").unwrap_or(name);
        matches!(stem,
            "pwsh" | "powershell" | "cmd" | "command"
            | "bash" | "sh" | "dash" | "zsh" | "fish"
            | "ksh" | "tcsh" | "csh" | "nu" | "elvish" | "xonsh" | "busybox"
        )
    }

    /// Classify the foreground process of the pane rooted at `root_pid` for the
    /// purpose of Ctrl+C routing.
    ///
    /// Walks the process tree from `root_pid` down to the deepest foreground
    /// leaf (the highest-PID child at each level — a most-recently-created
    /// heuristic, since Windows exposes no real console foreground group), so
    /// nested wrapper chains such as `pwsh -> cmd -> node` resolve to the
    /// actual running program rather than stopping at the first wrapper.
    ///
    /// If `root_pid` has no non-system children, the root process itself is
    /// classified.  This covers both a bare shell prompt (root is pwsh/cmd ->
    /// shell) and a pane spawned via `create_window_raw` that directly exec'd a
    /// program with no shell wrapper (root may be a live TUI -> not a shell).
    ///
    /// Returns:
    ///   `Some(true)`  — the foreground is a shell or a VT bridge (wsl/ssh).
    ///                   These expect a console `CTRL_C_EVENT`.
    ///   `Some(false)` — a live non-shell program (Copilot CLI, vim, ...) owns
    ///                   the console; it should receive raw 0x03 and decide for
    ///                   itself (copy selection vs. interrupt).
    ///   `None`        — the process snapshot could not be taken; the caller
    ///                   should fall back to its default behavior.
    pub fn foreground_is_shell(root_pid: u32) -> Option<bool> {
        match foreground_leaf_name(root_pid)? {
            Some(name) => Some(is_shell_exe(&name) || is_vt_bridge_exe(&name)),
            // Root not present in the snapshot (rare race): default to shell
            // so the established interrupt behavior is preserved.
            None => Some(true),
        }
    }

    /// True when the pane's deepest foreground process is a VT bridge
    /// (wsl.exe, ssh.exe, ...).  Used by the Ctrl+C router (issue #491):
    /// bridges read raw bytes from their console and forward 0x03 into the
    /// guest as SIGINT themselves, so they must NOT be hit with a
    /// console-wide CTRL_C_EVENT broadcast.
    pub fn foreground_is_vt_bridge(root_pid: u32) -> bool {
        matches!(foreground_leaf_name(root_pid), Some(Some(name)) if is_vt_bridge_exe(&name))
    }

    /// Snapshot the process table and resolve the deepest foreground leaf
    /// under `root_pid`.  Outer `None` = snapshot failed; inner `None` =
    /// root absent from the snapshot (rare race).
    ///
    /// Routes through the shared `process_table()` cache like the other walkers
    /// in this module. Both callers (`foreground_is_shell`,
    /// `foreground_is_vt_bridge`) run on the Ctrl+C path — real user input, not
    /// per frame — so this passes `Duration::ZERO` and always enumerates fresh:
    /// misrouting an interrupt off a stale process tree would be a real bug.
    fn foreground_leaf_name(root_pid: u32) -> Option<Option<String>> {
        let entries = process_table(std::time::Duration::ZERO)?;

        // Descend to the deepest foreground leaf, skipping system processes
        // (see `deepest_descendant`).
        let leaf_name = deepest_descendant(&entries, root_pid).map(|(_, name)| name);

        // The process whose Ctrl+C behavior matters is the deepest
        // foreground leaf.  If the root has no children, classify the root
        // itself — a bare shell prompt resolves to pwsh/cmd (shell), while a
        // directly-exec'd pane (create_window_raw) resolves to the program
        // it ran, which may be a live TUI that must NOT be force-signalled.
        Some(leaf_name.or_else(|| {
            entries.iter()
                .find(|(pid, _, _)| *pid == root_pid)
                .map(|(_, _, name)| name.clone())
        }))
    }

    /// True when the pane root has no visible non-system children in the
    /// process table, i.e. `foreground_leaf_name` would fall back to
    /// classifying the root itself.  Used by the Ctrl+C router's boot-window
    /// guard (issue #579): this childless state is exactly when the walk
    /// cannot attribute a service-parented bridge (a booting wsl.exe) to the
    /// pane, so a "bare shell prompt" classification is untrustworthy.
    pub fn foreground_fell_back_to_root(root_pid: u32) -> bool {
        let entries = match process_table(std::time::Duration::ZERO) {
            Some(t) => t,
            None => return false,
        };
        !entries.iter().any(|(pid, ppid, name)| {
            *ppid == root_pid && *pid != root_pid && !is_system_exe(name)
        })
    }

    /// True when a VT bridge CLIENT executable (wsl.exe, ssh.exe,
    /// wslhost.exe, ...) is alive anywhere on the system.  Deliberately
    /// unscoped: during the WSL boot window the bridge is parented by
    /// wslservice and attached to no console, so no pane-scoped attribution
    /// can see it (issue #579).
    ///
    /// MUST exclude `wslservice.exe`: it is a Windows service that stays
    /// resident forever once WSL has run, and `is_vt_bridge_exe`'s `wsl*`
    /// prefix matches it.  Counting it made this check permanently true on
    /// any WSL-installed machine, which made the childless-fallback guard
    /// fire on every bare-prompt Ctrl+C and (with the PROCESSED_INPUT strip)
    /// broke cancelling an in-process cmdlet like `Start-Sleep`
    /// (measured: test_issue231's inter-test C-c stopped working).
    pub fn any_vt_bridge_running() -> bool {
        let entries = match process_table(std::time::Duration::ZERO) {
            Some(t) => t,
            None => return false,
        };
        entries.iter().any(|(_, _, name)| {
            let stem = name.strip_suffix(".exe").unwrap_or(name.as_str());
            stem != "wslservice" && is_vt_bridge_exe(name)
        })
    }

    /// True when the console holding `console_pids` contains a plain native
    /// application — a member that is not psmux itself, not console
    /// infrastructure, not a shell, and not a VT bridge (e.g. a foreground
    /// `ping.exe` under Git Bash).  Such a member is the legitimate recipient
    /// of conhost's PROCESSED_INPUT Ctrl+C conversion, so the router must
    /// NOT strip the flag then: stripping it silenced the ping interrupt
    /// (measured, issue #579 regression during the boot-window fix).  With
    /// only infrastructure/shells/bridges on the console, the conversion's
    /// sole victim is a shell waiting on a bridge launch, and stripping is
    /// what keeps a booting WSL alive.
    pub fn console_has_plain_native_app(console_pids: &[u32]) -> bool {
        let entries = match process_table(std::time::Duration::ZERO) {
            Some(t) => t,
            None => return false,
        };
        entries.iter().any(|(pid, _, name)| {
            console_pids.contains(pid) && {
                let stem = name.strip_suffix(".exe").unwrap_or(name.as_str());
                !matches!(stem, "psmux" | "tmux" | "pmux" | "conhost" | "openconsole")
                    && !is_shell_exe(name)
                    && !is_vt_bridge_exe(name)
            }
        })
    }

    /// True when any of the given PIDs names a VT bridge executable
    /// (wsl.exe, ssh.exe, ...).  Part of the Ctrl+C router's console-scoped
    /// guard (issue #579); see `console_broadcast_hits_bridge`.
    pub fn any_vt_bridge(pids: &[u32]) -> bool {
        let entries = match process_table(std::time::Duration::ZERO) {
            Some(t) => t,
            None => return false,
        };
        entries.iter().any(|(pid, _, name)| pids.contains(pid) && is_vt_bridge_exe(name))
    }

    /// Unix-family shells whose Windows builds are Cygwin/MSYS-based (Git
    /// Bash, MSYS2, Cygwin).  Their runtime reacts to a console CTRL_C_EVENT
    /// by delivering SIGINT to native child processes, which Cygwin
    /// implements as a hard kill — the lethal half of issue #491/#579.
    /// PowerShell/cmd are deliberately excluded: under them the broadcast is
    /// harmless to bridges and still needed to interrupt cooked console apps
    /// (ping, #346).
    fn is_unix_shell_exe(name: &str) -> bool {
        let stem = name.strip_suffix(".exe").unwrap_or(name);
        matches!(stem,
            "bash" | "sh" | "dash" | "zsh" | "fish"
            | "ksh" | "tcsh" | "csh" | "busybox"
        )
    }

    /// Issue #579: decide whether a CTRL_C_EVENT broadcast to the console
    /// holding `console_pids` is unsafe, i.e. can kill a VT bridge
    /// (wsl.exe, ssh.exe, ...).
    ///
    /// Two lethal shapes, both proven by reproduction:
    ///   (a) the bridge is itself on the console — the broadcast reaches it
    ///       directly and its default handler terminates it;
    ///   (b) a Cygwin/MSYS shell (Git Bash, MSYS2, Cygwin) is on the console.
    ///       Its runtime reacts to the broadcast by hard-killing the native
    ///       children its OWN bookkeeping tracks — including a backgrounded
    ///       `wsl.exe &` job.  This cannot be narrowed with a Windows
    ///       process-tree walk: the MSYS fork stub that spawned the native
    ///       child exits, leaving the child's Windows PPID pointing at a dead
    ///       PID, so no PPID-based descendant scan can see the bridge
    ///       (measured: the chain is intact seconds earlier and severed by
    ///       Ctrl+C time), while Cygwin's internal process table still
    ///       delivers the kill.
    ///
    /// A Cygwin shell needs no broadcast anyway: its pty line discipline turns
    /// the raw 0x03 the call site writes into SIGINT for its foreground
    /// process group, natives included — the same thing a plain mintty or
    /// conhost Git Bash session does, and all tmux ever delivers.
    ///
    /// Fresh snapshot for the same reason as `foreground_is_shell`: this runs
    /// on real user input, and a stale table could miss a just-launched
    /// bridge.
    pub fn console_broadcast_hits_bridge(console_pids: &[u32]) -> bool {
        let entries = match process_table(std::time::Duration::ZERO) {
            Some(t) => t,
            None => return false,
        };
        entries.iter().any(|(pid, _, name)| {
            console_pids.contains(pid) && (is_vt_bridge_exe(name) || is_unix_shell_exe(name))
        })
    }

    /// Diagnostic used by the Ctrl+C router's debug log: classify one console
    /// member the same way `console_broadcast_hits_bridge` does.
    pub fn classify_console_member(pid: u32) -> String {
        let entries = match process_table(std::time::Duration::ZERO) {
            Some(t) => t,
            None => return format!("{}: no-table", pid),
        };
        match entries.iter().find(|(p, _, _)| *p == pid) {
            Some((_, ppid, name)) => format!(
                "{}={} ppid={} bridge={} unix_shell={}",
                pid, name, ppid,
                is_vt_bridge_exe(name),
                is_unix_shell_exe(name),
            ),
            None => format!("{}: not-in-table", pid),
        }
    }

    /// Walk the process tree from `root_pid` and check if any descendant
    /// is a VT bridge process (wsl.exe, ssh.exe, etc.).
    /// This is used for mouse injection: VT bridge processes need VT mouse
    /// sequences written to the PTY master, not Win32 MOUSE_EVENT records.
    pub fn has_vt_bridge_descendant(root_pid: u32) -> bool {
        // ZERO max_age, same reasoning as foreground_is_shell: this picks the
        // mouse-injection transport for a real click, and its callers in
        // window_ops already keep their own 2s per-pane cache in front of it.
        let entries = match process_table(std::time::Duration::ZERO) {
            Some(t) => t,
            None => return false,
        };

        // BFS from root_pid to check all descendants
        let mut queue: Vec<u32> = vec![root_pid];
        let mut head = 0;
        while head < queue.len() {
            let parent = queue[head];
            head += 1;
            for (pid, ppid, name) in entries.iter() {
                if *ppid == parent && *pid != root_pid
                    && !queue.contains(pid)
                {
                    if is_vt_bridge_exe(name) {
                        return true;
                    }
                    queue.push(*pid);
                }
            }
        }
        false
    }

    // Path is relative to src/platform/process_info/ — an inline module inside a
    // file-based module adds a directory level, so this needs one more `..` than
    // the equivalent declaration in src/server/helpers.rs.
    #[cfg(test)]
    #[path = "../../../tests-rs/test_proc_table_cache.rs"]
    mod tests_proc_table_cache;

    #[cfg(test)]
    #[path = "../../../tests-rs/test_issue579_any_vt_bridge.rs"]
    mod tests_issue579_any_vt_bridge;
}

#[cfg(not(windows))]
pub mod process_info {
    pub fn get_process_name(_pid: u32) -> Option<String> { None }
    pub fn get_process_cwd(_pid: u32) -> Option<String> { None }
    pub fn get_foreground_process_name(_pid: u32) -> Option<String> { None }
    pub fn get_deepest_foreground_process_name(_pid: u32) -> Option<String> { None }
    pub fn get_foreground_cwd(_pid: u32) -> Option<String> { None }
    pub fn has_vt_bridge_descendant(_root_pid: u32) -> bool { false }
    pub fn foreground_is_shell(_root_pid: u32) -> Option<bool> { None }
    pub fn foreground_is_vt_bridge(_root_pid: u32) -> bool { false }
}

// ─── UTF-16 Console Writer (Windows) ────────────────────────────────────
//
// On Windows, Rust's `Stdout::write()` uses `WriteFile` which sends raw
// bytes to the console.  The console interprets those bytes according to
// the *output code page* (typically 437 or 1252, **not** UTF-8).  Even
// after calling `SetConsoleOutputCP(65001)`, ConPTY has incomplete support
// for multi-byte UTF-8 sequences delivered through `WriteFile`, causing
// characters like ▶ (U+25B6, 3 bytes: E2 96 B6) to render as mojibake
// (e.g. `â¶`).
//
// The fix is to bypass `WriteFile` entirely and use `WriteConsoleW`, which
// accepts UTF-16 wide strings and renders them correctly regardless of
// the console codepage.  This wrapper converts incoming UTF-8 bytes to
// UTF-16 on the fly and writes them with `WriteConsoleW`.

/// A [`std::io::Write`] implementation that renders Unicode correctly on
/// Windows by converting UTF-8 → UTF-16 and calling `WriteConsoleW`.
///
/// Crucially, this buffers incomplete trailing UTF-8 sequences between
/// `write()` calls.  `write_all()` may split a buffer at any byte
/// boundary — including in the middle of a multi-byte character like
/// `▶` (U+25B6, bytes E2 96 B6).  Without buffering, each orphaned byte
/// would be emitted as a Latin-1 code point (`â`, `¶`), producing the
/// exact garbling the user sees.
#[cfg(windows)]
pub struct Utf16ConsoleWriter {
    handle: *mut std::ffi::c_void,
    /// True when stdout is NOT a console (a Cygwin/MSYS pty pipe under
    /// mintty, issue #474). `WriteConsoleW` fails with ERROR_INVALID_FUNCTION
    /// on a pipe handle; flush() writes raw UTF-8 via `WriteFile` instead so
    /// the byte stream reaches the terminal emulator on the other side.
    pipe_output: bool,
    /// Frame buffer: accumulates all `write()` output so that `flush()`
    /// can emit the complete frame as a single `WriteConsoleW` call.
    /// This eliminates the visible top-to-bottom "curtain" repaint that
    /// occurs when ratatui's many small per-cell writes are each sent to
    /// the console individually.
    frame_buf: Vec<u8>,
}

#[cfg(windows)]
unsafe impl Send for Utf16ConsoleWriter {}

#[cfg(windows)]
impl Utf16ConsoleWriter {
    pub fn new() -> Self {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        }
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        #[link(name = "kernel32")]
        extern "system" {
            fn GetConsoleMode(h: *mut std::ffi::c_void, mode: *mut u32) -> i32;
        }
        let mut mode: u32 = 0;
        let pipe_output = handle.is_null()
            || handle == (-1isize) as *mut std::ffi::c_void
            || unsafe { GetConsoleMode(handle, &mut mode) } == 0;
        // Pre-allocate ~128KB for the frame buffer — large enough for a
        // typical full-screen frame's escape sequences without reallocation.
        Self { handle, pipe_output, frame_buf: Vec::with_capacity(131072) }
    }

    /// Write raw UTF-8 bytes via `WriteFile` — the output path when stdout is
    /// a pipe (mintty / Cygwin pty) rather than a console.
    fn write_raw(&self, bytes: &[u8]) -> std::io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn WriteFile(
                h: *mut std::ffi::c_void,
                buf: *const u8,
                len: u32,
                written: *mut u32,
                overlapped: *mut std::ffi::c_void,
            ) -> i32;
        }
        let mut total: usize = 0;
        while total < bytes.len() {
            let mut written: u32 = 0;
            let ok = unsafe {
                WriteFile(
                    self.handle,
                    bytes.as_ptr().add(total),
                    (bytes.len() - total) as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if written == 0 {
                break;
            }
            total += written as usize;
        }
        Ok(())
    }

    /// Write a valid UTF-8 string via `WriteConsoleW`.
    fn write_wide(&self, s: &str) -> std::io::Result<()> {
        if s.is_empty() {
            return Ok(());
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn WriteConsoleW(
                hConsoleOutput: *mut std::ffi::c_void,
                lpBuffer: *const u16,
                nNumberOfCharsToWrite: u32,
                lpNumberOfCharsWritten: *mut u32,
                lpReserved: *mut std::ffi::c_void,
            ) -> i32;
        }

        let wide: Vec<u16> = s.encode_utf16().collect();
        let mut total: u32 = 0;
        let len = wide.len() as u32;
        while total < len {
            let mut written: u32 = 0;
            let ok = unsafe {
                WriteConsoleW(
                    self.handle,
                    wide.as_ptr().add(total as usize),
                    len - total,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if written == 0 {
                break;
            }
            total += written;
        }
        Ok(())
    }
}

// ─── "Bold is bright" SGR restoration (issue #425) ──────────────────────────
//
// crossterm 0.29 serialises every one of the 16 basic ANSI colors as a
// 256-indexed sequence (`38;5;N` for foreground, `48;5;N` for background —
// see crossterm's `Colored` Display impl).  The 256-indexed form suppresses
// the outer terminal's "bold is bright" behaviour: a bare shell emitting
// `ESC[32;1m` reaches Windows Terminal as `ESC[32m`+`ESC[1m` and renders as
// *bright* green, but the same text routed through psmux reached WT as
// `ESC[38;5;2m`+`ESC[1m`, which WT renders as muted green with a heavier
// font.  Restoring the standard SGR codes (30-37/90-97 fg, 40-47/100-107 bg)
// for palette indices 0-15 makes psmux match a bare shell.

/// Global toggle for the "bold is bright" SGR rewrite (issue #425 option
/// `bold-is-bright`, default on).  The console writer is a detached singleton
/// with no access to `AppState`, so the option is mirrored into this atomic by
/// whichever process applies the option (config parse or `set-option`).  When
/// off, `flush()` skips the rewrite and passes crossterm's output through
/// untouched.
pub static BOLD_IS_BRIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Set the `bold-is-bright` rewrite toggle (cross-platform; only read by the
/// Windows console writer).
pub fn set_bold_is_bright(on: bool) {
    BOLD_IS_BRIGHT.store(on, std::sync::atomic::Ordering::Relaxed);
}

// ── Process priority (issue #608) ───────────────────────────────────────────
//
// Windows hands its foreground boost to whoever owns the foreground WINDOW.
// The psmux server owns no window at all, and the attach client draws inside a
// console window that the terminal host owns, so neither process is ever the
// one Windows decides to favour. On a box that is merely busy that costs
// nothing. On a heavily oversubscribed one it means the keystroke path queues
// behind every compute job on the machine, which is the lag reported in #608.
//
// tmux has no equivalent. It never calls setpriority or nice anywhere in its
// source, because on Linux the CFS scheduler already favours a process that
// spends its life blocked on a read: interactivity is inferred from the sleep
// pattern rather than granted to a window. This knob is therefore a Windows
// specific extension, not a tmux parity item.

/// Priority class psmux's own processes run at unless something overrides it.
pub const DEFAULT_PRIORITY: &str = "above-normal";

/// The values `priority` and `PSMUX_PRIORITY` accept, in the order shown to a
/// user who got one wrong.
pub const PRIORITY_VALUES: &[&str] = &["normal", "above-normal", "high"];

const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;
const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;

/// Canonical spelling of an accepted priority value, or `None` if it is not one.
///
/// Deliberately narrow. `realtime` is not offered at any spelling: it outranks
/// most of the kernel's own threads and a wedged psmux at that class can make a
/// machine unusable. `idle` and `below-normal` are not offered either, because
/// they would make the reported symptom worse rather than better.
pub fn normalize_priority(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "normal" => Some("normal"),
        "above-normal" | "above_normal" | "abovenormal" => Some("above-normal"),
        "high" => Some("high"),
        _ => None,
    }
}

fn priority_class(value: &str) -> Option<u32> {
    match normalize_priority(value)? {
        "normal" => Some(NORMAL_PRIORITY_CLASS),
        "above-normal" => Some(ABOVE_NORMAL_PRIORITY_CLASS),
        "high" => Some(HIGH_PRIORITY_CLASS),
        _ => None,
    }
}

/// `PSMUX_PRIORITY` as a canonical value, or `None` when it is unset, empty or
/// unusable. Silent: the one place that warns about a bad value is startup.
pub fn env_priority() -> Option<&'static str> {
    let raw = std::env::var("PSMUX_PRIORITY").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    normalize_priority(&raw)
}

/// Resolve the class this process should run at.
///
/// `PSMUX_PRIORITY` outranks the `priority` option, so a user can climb out of
/// a bad configured value from the shell they start psmux in without editing a
/// config file. `warn` is for the startup call only: the option arms re-resolve
/// on every `set -g priority` and must not print on each one.
pub fn resolve_priority(from_option: Option<&str>, warn: bool) -> String {
    if let Ok(raw) = std::env::var("PSMUX_PRIORITY") {
        if !raw.trim().is_empty() {
            match normalize_priority(&raw) {
                Some(v) => return v.to_string(),
                None if warn => eprintln!(
                    "psmux: PSMUX_PRIORITY: unknown value '{}' (expected {}); ignoring it",
                    raw.trim(),
                    PRIORITY_VALUES.join(", ")
                ),
                None => {}
            }
        }
    }
    from_option
        .and_then(normalize_priority)
        .unwrap_or(DEFAULT_PRIORITY)
        .to_string()
}

/// The class a claiming client wants its server to end up at, resolved under
/// the documented precedence (env, then the `priority` option in the config
/// file, then the default).
///
/// This exists so `claim-session -p <value>` is built the same way at all three
/// send sites (#608). The resolution happens on the CLIENT because that is the
/// only process that can see the user's shell environment: a warm standby was
/// spawned by an earlier server generation and inherits ITS environment, never
/// the claimant's.
pub fn claim_priority_arg() -> String {
    resolve_priority(crate::config::priority_from_config().as_deref(), false)
}

/// Set THIS process's priority class. Never raises pane children: a Windows
/// child created without an explicit class flag gets NORMAL_PRIORITY_CLASS
/// unless its creator is idle or below-normal, so the shells and programs psmux
/// spawns stay where the user expects them.
///
/// Fail open. A refused SetPriorityClass (a restricted token, a job object that
/// caps the class) returns false and is otherwise ignored, because losing a few
/// milliseconds of scheduling is never a reason to refuse to start.
#[cfg(windows)]
pub fn set_process_priority(value: &str) -> bool {
    let Some(class) = priority_class(value) else {
        return false;
    };
    #[link(name = "kernel32")]
    extern "system" {
        // *mut c_void to match the other GetCurrentProcess declaration in the
        // crate (src/paths.rs); a second shape trips clashing_extern_declarations.
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn SetPriorityClass(handle: *mut std::ffi::c_void, class: u32) -> i32;
    }
    unsafe { SetPriorityClass(GetCurrentProcess(), class) != 0 }
}

#[cfg(not(windows))]
pub fn set_process_priority(value: &str) -> bool {
    priority_class(value).is_some()
}

/// This process's current class as one of the accepted names, or `None` when it
/// is something psmux never sets. Used by the tests and by `#{priority}`-style
/// introspection rather than by the runtime itself.
#[cfg(windows)]
pub fn current_process_priority() -> Option<&'static str> {
    #[link(name = "kernel32")]
    extern "system" {
        // *mut c_void to match the other GetCurrentProcess declaration in the
        // crate (src/paths.rs); a second shape trips clashing_extern_declarations.
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetPriorityClass(handle: *mut std::ffi::c_void) -> u32;
    }
    let class = unsafe { GetPriorityClass(GetCurrentProcess()) };
    match class {
        NORMAL_PRIORITY_CLASS => Some("normal"),
        ABOVE_NORMAL_PRIORITY_CLASS => Some("above-normal"),
        HIGH_PRIORITY_CLASS => Some("high"),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn current_process_priority() -> Option<&'static str> {
    None
}

/// Returns true when an SGR parameter list contains only ASCII digits and
/// `;` separators (the shape crossterm emits).  Anything else (`:` subparams,
/// private markers) is left untouched.
#[cfg(windows)]
fn sgr_params_simple(params: &[u8]) -> bool {
    params.iter().all(|&c| c.is_ascii_digit() || c == b';')
}

/// Parse a short decimal token (0-999) into a u16, rejecting empty/oversized.
#[cfg(windows)]
fn parse_dec_u16(t: &[u8]) -> Option<u16> {
    if t.is_empty() || t.len() > 3 {
        return None;
    }
    let mut v: u16 = 0;
    for &c in t {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as u16;
    }
    Some(v)
}

/// Append the decimal representation of `v` to `out` without allocating.
#[cfg(windows)]
fn push_dec_u16(out: &mut Vec<u8>, mut v: u16) {
    if v == 0 {
        out.push(b'0');
        return;
    }
    let mut buf = [0u8; 5];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

/// Rewrite the parameter list of a single SGR sequence, converting
/// `38;5;N`/`48;5;N` (N <= 15) into the standard basic/bright color codes.
/// Truecolor (`38;2;r;g;b`), 256-indexed N >= 16, and underline color
/// (`58;...`) are copied through verbatim.
#[cfg(windows)]
fn rewrite_sgr_params(params: &[u8], out: &mut Vec<u8>) {
    let tokens: Vec<&[u8]> = params.split(|&c| c == b';').collect();
    let mut first = true;
    let push = |tok: &[u8], out: &mut Vec<u8>, first: &mut bool| {
        if !*first {
            out.push(b';');
        }
        out.extend_from_slice(tok);
        *first = false;
    };
    let mut k = 0;
    while k < tokens.len() {
        let t = tokens[k];
        // fg/bg 256-indexed: 38;5;N / 48;5;N
        if (t == b"38" || t == b"48") && k + 2 < tokens.len() && tokens[k + 1] == b"5" {
            if let Some(n) = parse_dec_u16(tokens[k + 2]) {
                if n < 16 {
                    let code = if t == b"38" {
                        if n < 8 { 30 + n } else { 90 + (n - 8) }
                    } else if n < 8 {
                        40 + n
                    } else {
                        100 + (n - 8)
                    };
                    if !first {
                        out.push(b';');
                    }
                    push_dec_u16(out, code);
                    first = false;
                    k += 3;
                    continue;
                }
            }
            // N >= 16 or unparsable — copy the three tokens verbatim.
            push(tokens[k], out, &mut first);
            push(tokens[k + 1], out, &mut first);
            push(tokens[k + 2], out, &mut first);
            k += 3;
            continue;
        }
        // fg/bg/underline truecolor: 38;2;r;g;b — skip past all 5 tokens.
        if (t == b"38" || t == b"48" || t == b"58") && k + 1 < tokens.len() && tokens[k + 1] == b"2" {
            let end = (k + 5).min(tokens.len());
            for m in k..end {
                push(tokens[m], out, &mut first);
            }
            k = end;
            continue;
        }
        // underline 256-indexed: 58;5;N — leave untouched, skip 3 tokens.
        if t == b"58" && k + 2 < tokens.len() && tokens[k + 1] == b"5" {
            for m in k..k + 3 {
                push(tokens[m], out, &mut first);
            }
            k += 3;
            continue;
        }
        push(t, out, &mut first);
        k += 1;
    }
}

/// Scan `buf`, copying it into `out` while rewriting the basic-color SGR
/// sequences (see [`rewrite_sgr_params`]).  Only complete escape sequences
/// are processed; a trailing incomplete `ESC[...` (or lone `ESC`) is left
/// unconsumed so the caller can defer it to the next flush.  Returns the
/// number of input bytes consumed into `out`.
#[cfg(windows)]
fn rewrite_sgr_basic_colors(buf: &[u8], out: &mut Vec<u8>) -> usize {
    let n = buf.len();
    let mut i = 0;
    while i < n {
        let b = buf[i];
        if b == 0x1B {
            if i + 1 >= n {
                return i; // lone trailing ESC — defer
            }
            if buf[i + 1] == b'[' {
                // CSI: scan for the final byte in 0x40..=0x7E.
                let mut j = i + 2;
                while j < n && !(0x40..=0x7E).contains(&buf[j]) {
                    j += 1;
                }
                if j >= n {
                    return i; // incomplete CSI — defer
                }
                let final_byte = buf[j];
                let params = &buf[i + 2..j];
                if final_byte == b'm' && sgr_params_simple(params) {
                    out.extend_from_slice(b"\x1b[");
                    rewrite_sgr_params(params, out);
                    out.push(b'm');
                } else {
                    out.extend_from_slice(&buf[i..=j]);
                }
                i = j + 1;
                continue;
            }
            // Non-CSI escape (OSC/DCS/etc.): copy the ESC and continue.  Its
            // payload is copied verbatim byte-by-byte and never matched as a
            // CSI, so any embedded "38;5;" text is left untouched.
            out.push(b);
            i += 1;
            continue;
        }
        out.push(b);
        i += 1;
    }
    n
}

#[cfg(windows)]
impl std::io::Write for Utf16ConsoleWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Append to the frame buffer — actual console output is deferred
        // until flush(), so all of ratatui's per-cell writes within a
        // single draw() call are batched into one atomic WriteConsoleW.
        self.frame_buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.frame_buf.is_empty() {
            return Ok(());
        }

        // Restore standard SGR codes for the 16 basic ANSI colors so the
        // outer terminal's "bold is bright" rendering works (issue #425).
        // Only pay the rewrite cost when a candidate `8;5;` token is present
        // (covers both `38;5;` and `48;5;`); otherwise the buffer is used
        // as-is.  `deferred` holds a trailing incomplete escape sequence to
        // carry to the next flush (empty in the common case).
        let needs_rewrite = BOLD_IS_BRIGHT.load(std::sync::atomic::Ordering::Relaxed)
            && self.frame_buf.windows(4).any(|w| w == b"8;5;");
        let mut rewritten: Vec<u8> = Vec::new();
        let (processed, deferred): (&[u8], &[u8]) = if needs_rewrite {
            rewritten.reserve(self.frame_buf.len() + 16);
            let consumed = rewrite_sgr_basic_colors(&self.frame_buf, &mut rewritten);
            (&rewritten[..], &self.frame_buf[consumed..])
        } else {
            (&self.frame_buf[..], &[][..])
        };

        // Convert the buffered UTF-8 to a valid string, handling any
        // incomplete trailing multi-byte sequence.
        let (valid, remainder) = match std::str::from_utf8(processed) {
            Ok(s) => (s.len(), 0),
            Err(e) => {
                let valid_end = e.valid_up_to();
                // If error_len is None, trailing bytes are an incomplete
                // sequence — they'll be completed by the next write.
                // If it's Some, those bytes are genuinely invalid — skip.
                let skip = e.error_len().unwrap_or(0);
                (valid_end, processed.len() - valid_end - skip)
            }
        };

        if valid > 0 {
            if self.pipe_output {
                // Pipe (Cygwin pty) output: the terminal on the other side
                // consumes raw UTF-8; WriteConsoleW would fail on this handle.
                self.write_raw(&processed[..valid])?;
            } else {
                // Safety: we just validated this range is valid UTF-8.
                let s = unsafe { std::str::from_utf8_unchecked(&processed[..valid]) };
                self.write_wide(s)?;
            }
        }

        // Rebuild the frame buffer: any pending UTF-8 tail first, then the
        // deferred incomplete escape sequence.
        let utf8_tail_start = processed.len() - remainder;
        let mut next = Vec::with_capacity(remainder + deferred.len());
        if remainder > 0 {
            next.extend_from_slice(&processed[utf8_tail_start..]);
        }
        next.extend_from_slice(deferred);
        self.frame_buf = next;

        Ok(())
    }
}

/// Platform-independent writer type for the TUI backend.
///
/// On Windows this uses [`Utf16ConsoleWriter`] (WriteConsoleW) so that
/// multi-byte UTF-8 characters render correctly.  On other platforms it
/// is simply [`std::io::Stdout`].
#[cfg(windows)]
pub type PsmuxWriter = Utf16ConsoleWriter;
#[cfg(not(windows))]
pub type PsmuxWriter = std::io::Stdout;

/// Create a new [`PsmuxWriter`].
pub fn create_writer() -> PsmuxWriter {
    #[cfg(windows)]
    { Utf16ConsoleWriter::new() }
    #[cfg(not(windows))]
    { std::io::stdout() }
}

#[cfg(all(test, windows))]
mod bold_is_bright_tests {
    // Issue #425: crossterm serialises the 16 basic colors as 256-indexed
    // `38;5;N`, which suppresses the outer terminal's "bold is bright".  These
    // tests lock in the byte-level rewrite that restores the standard codes.
    use super::{rewrite_sgr_basic_colors, rewrite_sgr_params};

    fn rewrite(input: &str) -> String {
        let mut out = Vec::new();
        let consumed = rewrite_sgr_basic_colors(input.as_bytes(), &mut out);
        assert_eq!(consumed, input.len(), "expected full consumption");
        String::from_utf8(out).unwrap()
    }

    fn params(input: &str) -> String {
        let mut out = Vec::new();
        rewrite_sgr_params(input.as_bytes(), &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn basic_fg_becomes_standard() {
        // 38;5;0..7 -> 30..37
        for n in 0u8..8 {
            assert_eq!(params(&format!("38;5;{n}")), format!("{}", 30 + n));
        }
        // 38;5;8..15 -> 90..97
        for n in 8u8..16 {
            assert_eq!(params(&format!("38;5;{n}")), format!("{}", 90 + (n - 8)));
        }
    }

    #[test]
    fn basic_bg_becomes_standard() {
        for n in 0u8..8 {
            assert_eq!(params(&format!("48;5;{n}")), format!("{}", 40 + n));
        }
        for n in 8u8..16 {
            assert_eq!(params(&format!("48;5;{n}")), format!("{}", 100 + (n - 8)));
        }
    }

    #[test]
    fn green_bold_matches_bare_shell() {
        // The exact issue scenario: crossterm emits fg then bold as separate
        // SGRs.  After rewrite the green must be the standard `32` so WT
        // brightens it, and the bold `1` must survive.
        assert_eq!(rewrite("\x1b[38;5;2m\x1b[1m"), "\x1b[32m\x1b[1m");
        // Combined form (color + bold in one SGR) is handled too.
        assert_eq!(params("38;5;2;1"), "32;1");
    }

    #[test]
    fn indexed_over_15_preserved() {
        assert_eq!(params("38;5;240"), "38;5;240");
        assert_eq!(params("48;5;250"), "48;5;250");
        assert_eq!(params("38;5;16"), "38;5;16");
    }

    #[test]
    fn truecolor_preserved() {
        assert_eq!(params("38;2;255;0;0"), "38;2;255;0;0");
        assert_eq!(params("48;2;1;2;3"), "48;2;1;2;3");
        // A blue channel equal to "5" must not be misread as the 256 selector.
        assert_eq!(params("38;2;5;5;5"), "38;2;5;5;5");
    }

    #[test]
    fn underline_color_untouched() {
        assert_eq!(params("58;5;2"), "58;5;2");
        assert_eq!(params("58;2;1;2;3"), "58;2;1;2;3");
    }

    #[test]
    fn already_standard_and_attrs_untouched() {
        assert_eq!(params("32"), "32");
        assert_eq!(params("1"), "1");
        assert_eq!(params("0"), "0");
        assert_eq!(params("32;1;4"), "32;1;4");
        assert_eq!(params(""), "");
    }

    #[test]
    fn non_sgr_sequences_and_text_preserved() {
        // Cursor move ends in 'H', not 'm' — leave alone.
        assert_eq!(rewrite("\x1b[10;20H"), "\x1b[10;20H");
        // Private CSI (show cursor) untouched.
        assert_eq!(rewrite("\x1b[?25h"), "\x1b[?25h");
        // OSC payload containing "38;5;" text must not be rewritten.
        assert_eq!(rewrite("\x1b]0;38;5;2\x07"), "\x1b]0;38;5;2\x07");
        // Plain text passes through.
        assert_eq!(rewrite("hello \x1b[38;5;1mred\x1b[0m"), "hello \x1b[31mred\x1b[0m");
    }

    #[test]
    fn incomplete_trailing_escape_deferred() {
        // A split CSI at the buffer end is left unconsumed for the next flush.
        let input = b"ok\x1b[38;5";
        let mut out = Vec::new();
        let consumed = rewrite_sgr_basic_colors(input, &mut out);
        assert_eq!(consumed, 2, "should defer from the ESC");
        assert_eq!(out, b"ok");
        // Lone trailing ESC deferred too.
        let mut out2 = Vec::new();
        assert_eq!(rewrite_sgr_basic_colors(b"hi\x1b", &mut out2), 2);
        assert_eq!(out2, b"hi");
    }
}

// ---------------------------------------------------------------------------
// Win32 System Caret — Accessibility / Speech-to-Text support
// ---------------------------------------------------------------------------
// Speech-to-text tools like Wispr Flow use GetGUIThreadInfo() to locate the
// system caret.  When psmux enters raw mode + alternate screen, the default
// console caret is hidden and accessibility tools lose track of the text
// insertion point.
//
// By creating a Win32 caret on the console window and updating its position
// every frame, accessibility tools can detect the active text input context
// and inject transcribed text.
//
// These functions are safe to call on all platforms; non-Windows builds are
// no-ops.  SSH sessions should skip calling these (no local console window).
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod caret {
    use std::sync::atomic::{AtomicBool, Ordering};

    static CARET_CREATED: AtomicBool = AtomicBool::new(false);

    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> isize;
        fn GetCurrentConsoleFontEx(
            hConsoleOutput: *mut std::ffi::c_void,
            bMaximumWindow: i32,
            lpConsoleCurrentFontEx: *mut CONSOLE_FONT_INFOEX,
        ) -> i32;
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
    }

    #[link(name = "user32")]
    extern "system" {
        fn CreateCaret(hWnd: isize, hBitmap: isize, nWidth: i32, nHeight: i32) -> i32;
        fn SetCaretPos(x: i32, y: i32) -> i32;
        fn ShowCaret(hWnd: isize) -> i32;
        fn DestroyCaret() -> i32;
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct CONSOLE_FONT_INFOEX {
        cbSize: u32,
        nFont: u32,
        dwFontSize_X: i16,
        dwFontSize_Y: i16,
        FontFamily: u32,
        FontWeight: u32,
        FaceName: [u16; 32],
    }

    /// Query the current console font cell size in pixels.
    /// Returns (cell_width, cell_height).  Falls back to (8, 16) on failure.
    fn console_cell_size() -> (i32, i32) {
        const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;
        unsafe {
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            if handle.is_null() || handle == (-1isize) as *mut std::ffi::c_void {
                return (8, 16);
            }
            let mut info: CONSOLE_FONT_INFOEX = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<CONSOLE_FONT_INFOEX>() as u32;
            if GetCurrentConsoleFontEx(handle, 0, &mut info) != 0 {
                let w = if info.dwFontSize_X > 0 { info.dwFontSize_X as i32 } else { 8 };
                let h = if info.dwFontSize_Y > 0 { info.dwFontSize_Y as i32 } else { 16 };
                (w, h)
            } else {
                (8, 16)
            }
        }
    }

    /// Create the system caret on the console window (if not already created)
    /// and update its position to the given terminal cell coordinates.
    ///
    /// `col` and `row` are 0-based terminal cell coordinates (the same values
    /// used for VT CUP positioning).
    pub fn update(col: u16, row: u16) {
        unsafe {
            let hwnd = GetConsoleWindow();
            if hwnd == 0 {
                return;
            }
            if !CARET_CREATED.load(Ordering::Relaxed) {
                let (cw, ch) = console_cell_size();
                if CreateCaret(hwnd, 0, cw.max(1), ch.max(1)) != 0 {
                    CARET_CREATED.store(true, Ordering::Relaxed);
                    ShowCaret(hwnd);
                }
            }
            let (cw, ch) = console_cell_size();
            SetCaretPos(col as i32 * cw, row as i32 * ch);
        }
    }

    /// Hide and destroy the system caret.  Call on exit.
    pub fn destroy() {
        if CARET_CREATED.swap(false, Ordering::Relaxed) {
            unsafe { DestroyCaret(); }
        }
    }
}

#[cfg(not(windows))]
pub mod caret {
    pub fn update(_col: u16, _row: u16) {}
    pub fn destroy() {}
}

/// On Windows ConPTY, Shift+Enter is misreported by crossterm:
///
/// VS Code's xterm.js sends `\x1b\r` (ESC + CR) for Shift+Enter.
/// ConPTY interprets the ESC prefix as Alt, so crossterm reports
/// `KeyModifiers::ALT` instead of `KeyModifiers::SHIFT`.
///
/// This function polls the physical keyboard state to detect the real
/// modifiers and remaps accordingly.
#[cfg(windows)]
pub fn augment_enter_shift(key: &mut crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode, KeyModifiers};

    if !matches!(key.code, KeyCode::Enter) {
        return;
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        return;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(vKey: i32) -> i16;
    }

    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12; // Alt

    unsafe {
        let shift_down = GetAsyncKeyState(VK_SHIFT) < 0;
        let ctrl_down = GetAsyncKeyState(VK_CONTROL) < 0;
        let alt_down = GetAsyncKeyState(VK_MENU) < 0;

        if shift_down {
            key.modifiers.insert(KeyModifiers::SHIFT);
            // Windows Terminal + crossterm sometimes reports a phantom CONTROL
            // modifier on the Press event for Shift+Enter while the physical
            // Ctrl key is not held.  Remove it.
            if !ctrl_down && key.modifiers.contains(KeyModifiers::CONTROL) {
                key.modifiers.remove(KeyModifiers::CONTROL);
            }
            if !alt_down && key.modifiers.contains(KeyModifiers::ALT) {
                key.modifiers.remove(KeyModifiers::ALT);
            }
        } else if !shift_down && !ctrl_down && !alt_down {
            // No physical modifiers held; ConPTY may have injected a phantom
            // ALT from ESC+CR.  Already handled by the early return for SHIFT
            // above, but guard plain Enter too.
        } else if !shift_down && alt_down {
            // Physical Alt is held, leave as is.
        }
    }
}

// ---------------------------------------------------------------------------
// IME (Input Method Editor) management for prefix mode (issue #286)
// ---------------------------------------------------------------------------
//
// When an IME (e.g. Japanese, Chinese, Korean) is active, alphabetic
// keystrokes after the prefix key get intercepted by the IME composition
// engine instead of reaching psmux as raw key events.  We suppress the
// IME while in prefix mode and restore it afterwards.

/// Disable the IME on the console window.  Returns `true` if the IME was
/// previously open (so the caller knows whether to restore it later).
#[cfg(windows)]
pub fn ime_disable() -> bool {
    #[link(name = "imm32")]
    extern "system" {
        fn ImmGetContext(hWnd: isize) -> isize;
        fn ImmGetOpenStatus(hIMC: isize) -> i32;
        fn ImmSetOpenStatus(hIMC: isize, fOpen: i32) -> i32;
        fn ImmReleaseContext(hWnd: isize, hIMC: isize) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> isize;
    }
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd == 0 { return false; }
        let himc = ImmGetContext(hwnd);
        if himc == 0 { return false; }
        let was_open = ImmGetOpenStatus(himc) != 0;
        if was_open {
            ImmSetOpenStatus(himc, 0);
        }
        ImmReleaseContext(hwnd, himc);
        was_open
    }
}

/// Restore (re-open) the IME on the console window.
#[cfg(windows)]
pub fn ime_restore() {
    #[link(name = "imm32")]
    extern "system" {
        fn ImmGetContext(hWnd: isize) -> isize;
        fn ImmSetOpenStatus(hIMC: isize, fOpen: i32) -> i32;
        fn ImmReleaseContext(hWnd: isize, hIMC: isize) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> isize;
    }
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd == 0 { return; }
        let himc = ImmGetContext(hwnd);
        if himc == 0 { return; }
        ImmSetOpenStatus(himc, 1);
        ImmReleaseContext(hwnd, himc);
    }
}

#[cfg(test)]
#[cfg(windows)]
#[path = "../tests-rs/test_issue265_argv_backslash.rs"]
mod tests_issue265_argv_backslash;

#[cfg(test)]
#[cfg(windows)]
#[path = "../tests-rs/test_char_to_vk.rs"]
mod tests_char_to_vk;

#[cfg(test)]
#[cfg(windows)]
#[path = "../tests-rs/test_ctrlc_shell_classify.rs"]
mod tests_ctrlc_shell_classify;

// ─── Cygwin/MSYS pty (pipe) client support — issue #474 ─────────────────────
//
// When the psmux client runs under mintty (Git Bash, MSYS2) or any other
// Cygwin-style pty, stdin/stdout are named pipes, not a console. Console
// size APIs cannot see the real terminal there; the size instead comes from
// XTWINOPS (`CSI 18 t` query → `CSI 8 ; rows ; cols t` reply) parsed by the
// VT input reader, which stores it here for the TUI backend to consume.

/// Terminal size override for pipe-mode clients, packed as `cols << 16 | rows`.
/// Zero means "not in pipe mode / not yet known" and the backend falls through
/// to the console size APIs.
static PIPE_TERM_SIZE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Record the real terminal size reported over a raw VT pipe.
pub fn set_pipe_term_size(cols: u16, rows: u16) {
    if cols == 0 || rows == 0 {
        return;
    }
    PIPE_TERM_SIZE.store(((cols as u32) << 16) | rows as u32, std::sync::atomic::Ordering::SeqCst);
}

/// The pipe-mode terminal size, when one has been reported.
pub fn pipe_term_size() -> Option<(u16, u16)> {
    let v = PIPE_TERM_SIZE.load(std::sync::atomic::Ordering::SeqCst);
    if v == 0 {
        None
    } else {
        Some(((v >> 16) as u16, (v & 0xFFFF) as u16))
    }
}

/// Maps a ratatui colour onto the crossterm colour crossterm 0.29 writes.
fn to_crossterm_color(c: ratatui::style::Color) -> crossterm::style::Color {
    use crossterm::style::Color as C;
    use ratatui::style::Color as R;
    match c {
        R::Reset => C::Reset,
        R::Black => C::Black,
        R::Red => C::DarkRed,
        R::Green => C::DarkGreen,
        R::Yellow => C::DarkYellow,
        R::Blue => C::DarkBlue,
        R::Magenta => C::DarkMagenta,
        R::Cyan => C::DarkCyan,
        R::Gray => C::Grey,
        R::DarkGray => C::DarkGrey,
        R::LightRed => C::Red,
        R::LightGreen => C::Green,
        R::LightYellow => C::Yellow,
        R::LightBlue => C::Blue,
        R::LightMagenta => C::Magenta,
        R::LightCyan => C::Cyan,
        R::White => C::White,
        R::Indexed(i) => C::AnsiValue(i),
        R::Rgb(r, g, b) => C::Rgb { r, g, b },
    }
}

/// A copy of ratatui-crossterm's private `ModifierDiff::queue`, needed because
/// [`PsmuxBackend::draw`] runs its own cell loop to add the extended
/// underline styles ratatui cannot express (issue #589).
fn queue_modifier_diff<W: std::io::Write>(
    w: &mut W,
    from: ratatui::style::Modifier,
    to: ratatui::style::Modifier,
) -> std::io::Result<()> {
    use crossterm::queue;
    use crossterm::style::{Attribute as A, SetAttribute};
    use ratatui::style::Modifier as M;

    let removed = from - to;
    if removed.contains(M::REVERSED) {
        queue!(w, SetAttribute(A::NoReverse))?;
    }
    let reset_intensity =
        removed.contains(M::BOLD) || removed.contains(M::DIM);
    if reset_intensity {
        queue!(w, SetAttribute(A::NormalIntensity))?;
        if to.contains(M::DIM) {
            queue!(w, SetAttribute(A::Dim))?;
        }
        if to.contains(M::BOLD) {
            queue!(w, SetAttribute(A::Bold))?;
        }
    }
    if removed.contains(M::ITALIC) {
        queue!(w, SetAttribute(A::NoItalic))?;
    }
    if removed.contains(M::UNDERLINED) {
        queue!(w, SetAttribute(A::NoUnderline))?;
    }
    if removed.contains(M::CROSSED_OUT) {
        queue!(w, SetAttribute(A::NotCrossedOut))?;
    }
    if removed.contains(M::HIDDEN) {
        queue!(w, SetAttribute(A::NoHidden))?;
    }
    if removed.contains(M::SLOW_BLINK) || removed.contains(M::RAPID_BLINK) {
        queue!(w, SetAttribute(A::NoBlink))?;
    }

    let added = to - from;
    if added.contains(M::REVERSED) {
        queue!(w, SetAttribute(A::Reverse))?;
    }
    if added.contains(M::BOLD) && !reset_intensity {
        queue!(w, SetAttribute(A::Bold))?;
    }
    if added.contains(M::ITALIC) {
        queue!(w, SetAttribute(A::Italic))?;
    }
    if added.contains(M::UNDERLINED) {
        queue!(w, SetAttribute(A::Underlined))?;
    }
    if added.contains(M::DIM) && !reset_intensity {
        queue!(w, SetAttribute(A::Dim))?;
    }
    if added.contains(M::CROSSED_OUT) {
        queue!(w, SetAttribute(A::CrossedOut))?;
    }
    if added.contains(M::HIDDEN) {
        queue!(w, SetAttribute(A::Hidden))?;
    }
    if added.contains(M::SLOW_BLINK) {
        queue!(w, SetAttribute(A::SlowBlink))?;
    }
    if added.contains(M::RAPID_BLINK) {
        queue!(w, SetAttribute(A::RapidBlink))?;
    }
    Ok(())
}

/// Same cell loop as `CrosstermBackend::draw`, with one addition: psmux
/// smuggles the extended underline style (SGR `4:0` .. `4:5`) through three
/// unused high bits of ratatui's `Modifier` (see `rendering::UL_STYLE_SHIFT`),
/// because ratatui has no modifier for double, curly, dotted or dashed
/// underscores.  Those bits are masked off here and turned into the matching
/// crossterm attribute, which emits `CSI 4:N m`, the same bytes tmux writes
/// through `Smulx` (tmux `tty.c` `tty_attributes`).  Issue #589.
///
/// Kept as a free function over any `Write` so it can be driven from a test
/// with a plain byte buffer; `PsmuxWriter` is a real console handle.
pub fn draw_cells<'a, W, I>(w: &mut W, content: I) -> std::io::Result<()>
where
    W: std::io::Write,
    I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
{
    use crossterm::cursor::MoveTo;
    use crossterm::queue;
    use crossterm::style::{
        Attribute as CtAttribute, Color as CtColor, Print, SetAttribute,
        SetBackgroundColor, SetForegroundColor,
    };
    use ratatui::style::Modifier;

    let mut fg = ratatui::style::Color::Reset;
    let mut bg = ratatui::style::Color::Reset;
    let mut underline_color = ratatui::style::Color::Reset;
    let mut modifier = Modifier::empty();
    let mut ul_style: u8 = 0;
    let mut last_pos: Option<(u16, u16)> = None;
    for (x, y, cell) in content {
        if !matches!(last_pos, Some((px, py)) if x == px + 1 && y == py) {
            queue!(*w, MoveTo(x, y))?;
        }
        last_pos = Some((x, y));

        let cell_modifier = crate::rendering::strip_ul_style(cell.modifier);
        if cell_modifier != modifier {
            queue_modifier_diff(&mut *w, modifier, cell_modifier)?;
            modifier = cell_modifier;
        }

        // The extended style is emitted after the modifier diff on
        // purpose: the diff may have written a plain `4`, and `4:3` must
        // come last to win.  A style of 0 means "no underline" and is
        // already covered by the diff's `24`.
        let cell_ul = if cell_modifier.contains(Modifier::UNDERLINED) {
            crate::rendering::ul_style_of(cell.modifier).max(1)
        } else {
            0
        };
        if cell_ul != ul_style {
            match cell_ul {
                1 => queue!(*w, SetAttribute(CtAttribute::Underlined))?,
                2 => queue!(*w, SetAttribute(CtAttribute::DoubleUnderlined))?,
                3 => queue!(*w, SetAttribute(CtAttribute::Undercurled))?,
                4 => queue!(*w, SetAttribute(CtAttribute::Underdotted))?,
                5 => queue!(*w, SetAttribute(CtAttribute::Underdashed))?,
                _ => {}
            }
            ul_style = cell_ul;
        }

        if cell.fg != fg {
            queue!(*w, SetForegroundColor(to_crossterm_color(cell.fg)))?;
            fg = cell.fg;
        }
        if cell.bg != bg {
            queue!(*w, SetBackgroundColor(to_crossterm_color(cell.bg)))?;
            bg = cell.bg;
        }
        if cell.underline_color != underline_color {
            write_underline_color(w, cell.underline_color)?;
            underline_color = cell.underline_color;
        }

        queue!(*w, Print(cell.symbol()))?;
    }

    queue!(
        *w,
        SetForegroundColor(CtColor::Reset),
        SetBackgroundColor(CtColor::Reset),
    )?;
    write_underline_color(w, ratatui::style::Color::Reset)?;
    queue!(*w, SetAttribute(CtAttribute::Reset))
}

/// Writes the SGR 58 underline colour in the COLON subparameter form, and SGR
/// 59 to clear it.
///
/// crossterm's `SetUnderlineColor` emits the semicolon form (`58;5;9`), which
/// the Windows system conhost does not understand: it skips the 58, then reads
/// the remaining arguments as ordinary SGR parameters, so `58;5;9` arrives at
/// the outer terminal as blink plus strikethrough and `58;2;255;0;0` ends in a
/// `0` that resets every attribute.  The colon form is passed through
/// untouched by conhost, is what tmux's own `Setulc`/`Setulc1` capabilities
/// use (tmux `tty-features.c:157`), and is what Windows Terminal, `WezTerm`
/// and `kitty` accept.  Issue #589.
fn write_underline_color<W: std::io::Write>(
    w: &mut W,
    color: ratatui::style::Color,
) -> std::io::Result<()> {
    use ratatui::style::Color as R;
    match color {
        R::Reset => w.write_all(b"\x1b[59m"),
        R::Rgb(r, g, b) => {
            write!(w, "\x1b[58:2::{r}:{g}:{b}m")
        }
        other => {
            let idx = match other {
                R::Black => 0,
                R::Red => 1,
                R::Green => 2,
                R::Yellow => 3,
                R::Blue => 4,
                R::Magenta => 5,
                R::Cyan => 6,
                R::Gray => 7,
                R::DarkGray => 8,
                R::LightRed => 9,
                R::LightGreen => 10,
                R::LightYellow => 11,
                R::LightBlue => 12,
                R::LightMagenta => 13,
                R::LightCyan => 14,
                R::White => 15,
                R::Indexed(i) => i,
                _ => return Ok(()),
            };
            write!(w, "\x1b[58:5:{idx}m")
        }
    }
}

/// TUI backend for the psmux client: [`ratatui::backend::CrosstermBackend`]
/// over [`PsmuxWriter`], with one twist — `size()`/`window_size()` consult the
/// pipe-mode override first so a client attached over a Cygwin pty or a
/// no-ConPTY SSH channel renders at the real terminal size even though the
/// console size APIs cannot see that terminal. Outside pipe mode the override
/// is never set and every call delegates.
pub struct PsmuxBackend {
    inner: ratatui::backend::CrosstermBackend<PsmuxWriter>,
}

impl PsmuxBackend {
    pub fn new(writer: PsmuxWriter) -> Self {
        Self { inner: ratatui::backend::CrosstermBackend::new(writer) }
    }
}

// The client's shutdown path drives the backend directly as an `io::Write`
// (crossterm `execute!` for SGR/cursor resets) — delegate to the inner
// CrosstermBackend, which forwards to the writer.
impl std::io::Write for PsmuxBackend {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::io::Write::write(&mut self.inner, buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.inner)
    }
}

impl ratatui::backend::Backend for PsmuxBackend {
    type Error = std::io::Error;

    /// Same cell loop as `CrosstermBackend::draw`, with one addition: psmux
    /// smuggles the extended underline style (SGR `4:0` .. `4:5`) through
    /// three unused high bits of ratatui's `Modifier` (see
    /// `rendering::UL_STYLE_SHIFT`), because ratatui has no modifier for
    /// double, curly, dotted or dashed underscores.  Those bits are masked off
    /// here and turned into the matching crossterm attribute, which emits
    /// `CSI 4:N m`, the same bytes tmux writes through `Smulx` (tmux `tty.c`
    /// `tty_attributes`).  Issue #589.
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        draw_cells(&mut self.inner, content)
    }

    fn hide_cursor(&mut self) -> std::io::Result<()> {
        ratatui::backend::Backend::hide_cursor(&mut self.inner)
    }

    fn show_cursor(&mut self) -> std::io::Result<()> {
        ratatui::backend::Backend::show_cursor(&mut self.inner)
    }

    fn get_cursor_position(&mut self) -> std::io::Result<ratatui::layout::Position> {
        ratatui::backend::Backend::get_cursor_position(&mut self.inner)
    }

    fn set_cursor_position<P: Into<ratatui::layout::Position>>(&mut self, position: P) -> std::io::Result<()> {
        ratatui::backend::Backend::set_cursor_position(&mut self.inner, position)
    }

    fn clear(&mut self) -> std::io::Result<()> {
        ratatui::backend::Backend::clear(&mut self.inner)
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> std::io::Result<()> {
        ratatui::backend::Backend::clear_region(&mut self.inner, clear_type)
    }

    fn append_lines(&mut self, n: u16) -> std::io::Result<()> {
        ratatui::backend::Backend::append_lines(&mut self.inner, n)
    }

    fn size(&self) -> std::io::Result<ratatui::layout::Size> {
        if let Some((cols, rows)) = pipe_term_size() {
            return Ok(ratatui::layout::Size::new(cols, rows));
        }
        ratatui::backend::Backend::size(&self.inner)
    }

    fn window_size(&mut self) -> std::io::Result<ratatui::backend::WindowSize> {
        if let Some((cols, rows)) = pipe_term_size() {
            return Ok(ratatui::backend::WindowSize {
                columns_rows: ratatui::layout::Size::new(cols, rows),
                pixels: ratatui::layout::Size::new(0, 0),
            });
        }
        ratatui::backend::Backend::window_size(&mut self.inner)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        ratatui::backend::Backend::flush(&mut self.inner)
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue589_undercurl.rs"]
mod tests_issue589_undercurl;

#[cfg(all(test, windows))]
#[path = "../tests-rs/test_issue599_data_root_mutex.rs"]
mod tests_issue599_data_root_mutex;

#[cfg(test)]
#[path = "../tests-rs/test_issue608_priority.rs"]
mod tests_issue608_priority;
