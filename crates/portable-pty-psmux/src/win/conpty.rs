use crate::cmdbuilder::CommandBuilder;
use crate::win::psuedocon::PsuedoCon;
use crate::{Child, MasterPty, PtyPair, PtySize, PtySystem, SlavePty};
use anyhow::Error;
use filedescriptor::FileDescriptor;
use std::sync::{Arc, Mutex};
use winapi::um::wincon::COORD;

/// Create a pipe pair with an explicit buffer size.
///
/// Windows Terminal uses 128 KB pipe buffers for ConPTY I/O.  The default
/// `CreatePipe(..., 0)` typically gets 4 KB, which forces more frequent
/// kernel transitions during high-throughput output (e.g. `cat large_file`).
/// Using 64 KB matches Windows Terminal's approach and reduces syscall
/// overhead for both input (mouse/keyboard) and output.
///
/// Both ends are created NON inheritable.  Nothing needs them inheritable:
/// CreatePseudoConsole duplicates the conhost side ends into the conhost it
/// starts, and pane children are created with bInheritHandles=FALSE.  What
/// an inheritable end did do is ride along into every child the server
/// spawns with bInheritHandles=TRUE (std::process::Command always passes
/// TRUE: pipe-pane, run-shell, if-shell, `#()` status jobs, hooks), so a
/// long running job child held the master ends of every pane that existed
/// when it started, and passed them on to its own children.  Measured on
/// cb783dc: a server with three panes held 22 inheritable pipe handles and
/// a `pipe-pane` child (pwsh, then its PING) held copies of 6 of them.
/// The same rule as the socket fix: every handle the server keeps must
/// have HANDLE_FLAG_INHERIT clear.
#[doc(hidden)]
pub fn create_pipe_with_buffer(size: u32) -> anyhow::Result<(FileDescriptor, FileDescriptor)> {
    use std::os::windows::io::FromRawHandle;
    use std::ptr;
    use winapi::shared::minwindef::FALSE;
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::minwinbase::SECURITY_ATTRIBUTES;
    use winapi::um::namedpipeapi::CreatePipe;
    use winapi::um::winnt::HANDLE;

    let mut sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: FALSE as _,
    };
    let mut read: HANDLE = INVALID_HANDLE_VALUE;
    let mut write: HANDLE = INVALID_HANDLE_VALUE;
    if unsafe { CreatePipe(&mut read, &mut write, &mut sa, size) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe {(
        FileDescriptor::from_raw_handle(read as _),
        FileDescriptor::from_raw_handle(write as _),
    )})
}

#[derive(Default)]
pub struct ConPtySystem {}

impl PtySystem for ConPtySystem {
    fn openpty(&self, size: PtySize) -> anyhow::Result<PtyPair> {
        // Use 64KB pipe buffers (Windows Terminal uses 128KB).
        // Default CreatePipe(..., 0) = ~4KB, causing frequent kernel round-trips.
        const PIPE_BUF: u32 = 64 * 1024;
        let t_pipes = super::spawn_trace::now_us();
        let (stdin_read, stdin_write) = create_pipe_with_buffer(PIPE_BUF)?;
        let (stdout_read, stdout_write) = create_pipe_with_buffer(PIPE_BUF)?;
        super::spawn_trace::step(t_pipes, || "openpty.pipes");

        let t_con = super::spawn_trace::now_us();
        let con = PsuedoCon::new(
            COORD {
                X: size.cols as i16,
                Y: size.rows as i16,
            },
            stdin_read,
            stdout_write,
        )?;
        super::spawn_trace::step(t_con, || "openpty.CreatePseudoConsole");

        let master = ConPtyMasterPty {
            inner: Arc::new(Mutex::new(Inner {
                con,
                readable: stdout_read,
                writable: Some(stdin_write),
                size,
                resize_prime: None,
            })),
        };

        let slave = ConPtySlavePty {
            inner: master.inner.clone(),
        };

        Ok(PtyPair {
            master: Box::new(master),
            slave: Box::new(slave),
        })
    }
}

struct Inner {
    con: PsuedoCon,
    readable: FileDescriptor,
    writable: Option<FileDescriptor>,
    size: PtySize,
    /// A second handle on the input pipe, held only when the child runs on
    /// the Cygwin console runtime (see [`CYGWIN_RESIZE_PRIME`]).
    resize_prime: Option<FileDescriptor>,
}

/// One Shift key press and release, in the win32-input-mode form conhost
/// parses on a pseudoconsole's input pipe.  It produces no character.
///
/// The Cygwin console runtime (Git for Windows bash, MSYS2, Cygwin) loses the
/// first input record that wakes its read after the console was resized.
/// Measured on a bare ConPTY with no psmux in the loop, Git bash on MSYS
/// 3.6.7: after ResizePseudoConsole the next typed key went missing in 4/15,
/// 8/20 and 7/15 runs (0/15 without the resize), at any delay from 0 to
/// 1000 ms, because nothing looks at the new size until input arrives.  With
/// this Shift press written right after the resize, the Shift is the record
/// that goes and the typed key survives: 0/20 and 0/20.  A tty delivers
/// SIGWINCH outside the input queue, so tmux never meets this.
const CYGWIN_RESIZE_PRIME: &[u8] = b"\x1b[16;42;0;1;16;1_\x1b[16;42;0;0;16;1_";

/// Whether `program` runs on the Cygwin console runtime: its directory holds
/// msys-2.0.dll or cygwin1.dll, or it is one of Git for Windows' `bin\`
/// launchers whose real program sits in `..\usr\bin` beside msys-2.0.dll.
/// PowerShell and cmd are answered without touching the disk.
pub(crate) fn is_cygwin_runtime_program(program: &std::ffi::OsStr) -> bool {
    let path = std::path::Path::new(program);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(stem.as_str(), "pwsh" | "powershell" | "cmd" | "") {
        return false;
    }
    let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) else {
        return false;
    };
    dir.join("msys-2.0.dll").is_file()
        || dir.join("cygwin1.dll").is_file()
        || dir.join("..").join("usr").join("bin").join("msys-2.0.dll").is_file()
}

impl Inner {
    pub fn resize(
        &mut self,
        num_rows: u16,
        num_cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), Error> {
        self.con.resize(COORD {
            X: num_cols as i16,
            Y: num_rows as i16,
        })?;
        if self.size.rows != num_rows || self.size.cols != num_cols {
            if let Some(w) = self.resize_prime.as_mut() {
                use std::io::Write;
                let _ = w.write_all(CYGWIN_RESIZE_PRIME);
            }
        }
        self.size = PtySize {
            rows: num_rows,
            cols: num_cols,
            pixel_width,
            pixel_height,
        };
        Ok(())
    }
}

#[derive(Clone)]
pub struct ConPtyMasterPty {
    inner: Arc<Mutex<Inner>>,
}

pub struct ConPtySlavePty {
    inner: Arc<Mutex<Inner>>,
}

impl MasterPty for ConPtyMasterPty {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.resize(size.rows, size.cols, size.pixel_width, size.pixel_height)
    }

    fn get_size(&self) -> Result<PtySize, Error> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.size)
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn std::io::Read + Send>> {
        Ok(Box::new(self.inner.lock().unwrap().readable.try_clone()?))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn std::io::Write + Send>> {
        Ok(Box::new(
            self.inner
                .lock()
                .unwrap()
                .writable
                .take()
                .ok_or_else(|| anyhow::anyhow!("writer already taken"))?,
        ))
    }

    fn conpty_passthrough_mode(&self) -> Option<bool> {
        Some(self.inner.lock().unwrap().con.used_passthrough)
    }
}

impl SlavePty for ConPtySlavePty {
    fn spawn_command(&self, cmd: CommandBuilder) -> anyhow::Result<Box<dyn Child + Send + Sync>> {
        crate::reset_spawn_create_us();
        let t_lock = super::spawn_trace::now_us();
        let mut inner = self.inner.lock().unwrap();
        super::spawn_trace::step(t_lock, || "spawn.inner_mutex_wait");
        let cygwin = cmd
            .get_argv()
            .first()
            .map(|p| is_cygwin_runtime_program(p))
            .unwrap_or(false);
        let spawned = spawn_in(&mut inner, cmd);
        if cygwin && spawned.is_ok() {
            inner.resize_prime = inner.writable.as_ref().and_then(|w| w.try_clone().ok());
        }
        spawned
    }
}

fn spawn_in(inner: &mut Inner, cmd: CommandBuilder) -> anyhow::Result<Box<dyn Child + Send + Sync>> {
    {
        match inner.con.spawn_command(cmd.clone()) {
            Ok(child) => Ok(Box::new(child)),
            Err(e) if inner.con.used_passthrough && is_invalid_parameter(&e) => {
                // CreateProcessW rejected the ConPTY handle that was created
                // with PSEUDOCONSOLE_PASSTHROUGH_MODE.  Some Windows 11 builds
                // (notably Insider/Canary builds like 26200) accept the flag
                // during CreatePseudoConsole but later fail in CreateProcessW
                // with ERROR_INVALID_PARAMETER (87).
                //
                // Recovery: recreate the ConPTY without passthrough mode and
                // create fresh pipe pairs for the new pseudo-console.
                log::warn!(
                    "CreateProcessW failed with ERROR_INVALID_PARAMETER while using \
                     ConPTY passthrough mode; retrying without passthrough"
                );
                const PIPE_BUF: u32 = 64 * 1024;
                let (stdin_read, stdin_write) = create_pipe_with_buffer(PIPE_BUF)?;
                let (stdout_read, stdout_write) = create_pipe_with_buffer(PIPE_BUF)?;

                let new_con = PsuedoCon::new_without_passthrough(
                    COORD {
                        X: inner.size.cols as i16,
                        Y: inner.size.rows as i16,
                    },
                    stdin_read,
                    stdout_write,
                )?;

                // Replace the ConPTY and pipe endpoints inside Inner.
                // At this point nobody has cloned the reader or taken the
                // writer yet (pane.rs acquires them after spawn_command),
                // so the old FileDescriptors are dropped cleanly.
                inner.con = new_con;
                inner.readable = stdout_read;
                inner.writable = Some(stdin_write);

                let child = inner.con.spawn_command(cmd)?;
                Ok(Box::new(child))
            }
            Err(e) => Err(e),
        }
    }
}

/// Check if an error chain contains Windows ERROR_INVALID_PARAMETER (87).
/// The OS error number is locale-independent; the textual message varies
/// (e.g. "Falscher Parameter" in German).
fn is_invalid_parameter(e: &anyhow::Error) -> bool {
    let msg = format!("{}", e);
    msg.contains("os error 87")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const CHILD_ENV: &str = "PORTABLE_PTY_CONPTY_PASSTHROUGH_TEST_CHILD";

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pp_cyg_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cygwin_runtime_program_is_found_by_its_runtime_dll() {
        let msys = scratch("msys");
        std::fs::write(msys.join("msys-2.0.dll"), b"").unwrap();
        assert!(is_cygwin_runtime_program(msys.join("bash.exe").as_os_str()));
        assert!(is_cygwin_runtime_program(msys.join("zsh.exe").as_os_str()));

        let cyg = scratch("cyg");
        std::fs::write(cyg.join("cygwin1.dll"), b"").unwrap();
        assert!(is_cygwin_runtime_program(cyg.join("fish.exe").as_os_str()));

        // Git for Windows: bin\bash.exe is a launcher, the runtime is in usr\bin.
        let git = scratch("git");
        std::fs::create_dir_all(git.join("bin")).unwrap();
        std::fs::create_dir_all(git.join("usr").join("bin")).unwrap();
        std::fs::write(git.join("usr").join("bin").join("msys-2.0.dll"), b"").unwrap();
        assert!(is_cygwin_runtime_program(git.join("bin").join("bash.exe").as_os_str()));

        // PowerShell and cmd never qualify, even beside the runtime dll.
        assert!(!is_cygwin_runtime_program(msys.join("pwsh.exe").as_os_str()));
        assert!(!is_cygwin_runtime_program(msys.join("powershell.exe").as_os_str()));
        assert!(!is_cygwin_runtime_program(msys.join("cmd.exe").as_os_str()));
        // A program with no runtime dll (WSL's bash.exe), or a bare name.
        let plain = scratch("plain");
        assert!(!is_cygwin_runtime_program(plain.join("bash.exe").as_os_str()));
        assert!(!is_cygwin_runtime_program(std::ffi::OsStr::new("bash")));

        for d in [msys, cyg, git, plain] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// A resize that changes the size writes the Shift press to the input pipe
    /// of a Cygwin runtime pane, once per change; an unchanged size writes
    /// nothing, and a pane without the prime handle never gets one.
    #[test]
    fn resize_primes_the_input_pipe_only_on_a_size_change() {
        use std::io::Read;
        let size = PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 };
        let (stdin_read, stdin_write) = create_pipe_with_buffer(4096).unwrap();
        let (_stdout_read, stdout_write) = create_pipe_with_buffer(4096).unwrap();
        let con = PsuedoCon::new(COORD { X: 80, Y: 24 }, stdin_read, stdout_write).unwrap();
        // Read what reaches the input pipe through a second pipe standing in
        // for it: the prime handle writes there.
        let (mut probe_read, probe_write) = create_pipe_with_buffer(4096).unwrap();
        let mut inner = Inner {
            con,
            readable: _stdout_read,
            writable: Some(stdin_write),
            size,
            resize_prime: Some(probe_write),
        };
        inner.resize(24, 80, 0, 0).unwrap();
        inner.resize(24, 40, 0, 0).unwrap();
        inner.resize(24, 40, 0, 0).unwrap();
        inner.resize(12, 40, 0, 0).unwrap();
        inner.resize_prime = None;
        inner.resize(24, 80, 0, 0).unwrap();
        let mut got = vec![0u8; CYGWIN_RESIZE_PRIME.len() * 2];
        probe_read.read_exact(&mut got).unwrap();
        assert_eq!(&got[..CYGWIN_RESIZE_PRIME.len()], CYGWIN_RESIZE_PRIME);
        assert_eq!(&got[CYGWIN_RESIZE_PRIME.len()..], CYGWIN_RESIZE_PRIME);
        // Nothing more: the handle is gone, so the read end now sees EOF.
        let mut rest = Vec::new();
        probe_read.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty(), "unexpected extra input: {:?}", rest);
    }

    #[test]
    fn reports_when_passthrough_is_disabled_by_environment() {
        if std::env::var_os(CHILD_ENV).is_some() {
            let pair = ConPtySystem::default().openpty(PtySize::default()).unwrap();

            assert_eq!(pair.master.conpty_passthrough_mode(), Some(false));
            return;
        }

        let exe = std::env::current_exe().unwrap();
        let output = Command::new(exe)
            .args([
                "--exact",
                "win::conpty::tests::reports_when_passthrough_is_disabled_by_environment",
            ])
            .env("PSMUX_NO_PASSTHROUGH", "1")
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "child test failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
