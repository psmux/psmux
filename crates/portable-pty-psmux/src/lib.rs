//! This crate provides a cross platform API for working with the
//! psuedo terminal (pty) interfaces provided by the system.
//! Unlike other crates in this space, this crate provides a set
//! of traits that allow selecting from different implementations
//! at runtime.
//! This crate is part of [wezterm](https://github.com/wezterm/wezterm).
//!
//! ```no_run
//! use portable_pty::{CommandBuilder, PtySize, native_pty_system, PtySystem};
//! use anyhow::Error;
//!
//! // Use the native pty implementation for the system
//! let pty_system = native_pty_system();
//!
//! // Create a new pty
//! let mut pair = pty_system.openpty(PtySize {
//!     rows: 24,
//!     cols: 80,
//!     // Not all systems support pixel_width, pixel_height,
//!     // but it is good practice to set it to something
//!     // that matches the size of the selected font.  That
//!     // is more complex than can be shown here in this
//!     // brief example though!
//!     pixel_width: 0,
//!     pixel_height: 0,
//! })?;
//!
//! // Spawn a shell into the pty
//! let cmd = CommandBuilder::new("bash");
//! let child = pair.slave.spawn_command(cmd)?;
//!
//! // Read and parse output from the pty with reader
//! let mut reader = pair.master.try_clone_reader()?;
//!
//! // Send data to the pty by writing to the master
//! writeln!(pair.master.take_writer()?, "ls -l\r\n")?;
//! # Ok::<(), Error>(())
//! ```
//!
use anyhow::Error;
use downcast_rs::{impl_downcast, Downcast};
#[cfg(unix)]
use libc;
#[cfg(feature = "serde_support")]
use serde_derive::*;
use std::io::Result as IoResult;
#[cfg(windows)]
use std::os::windows::prelude::{AsRawHandle, RawHandle};

pub mod cmdbuilder;
pub use cmdbuilder::CommandBuilder;

#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod win;

pub mod serial;

/// Represents the size of the visible display area in the pty
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde_support", derive(Serialize, Deserialize))]
pub struct PtySize {
    /// The number of lines of text
    pub rows: u16,
    /// The number of columns of text
    pub cols: u16,
    /// The width of a cell in pixels.  Note that some systems never
    /// fill this value and ignore it.
    pub pixel_width: u16,
    /// The height of a cell in pixels.  Note that some systems never
    /// fill this value and ignore it.
    pub pixel_height: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// Represents the master/control end of the pty
pub trait MasterPty: Downcast + Send {
    /// Inform the kernel and thus the child process that the window resized.
    /// It will update the winsize information maintained by the kernel,
    /// and generate a signal for the child to notice and update its state.
    fn resize(&self, size: PtySize) -> Result<(), Error>;
    /// Retrieves the size of the pty as known by the kernel
    fn get_size(&self) -> Result<PtySize, Error>;
    /// Obtain a readable handle; output from the slave(s) is readable
    /// via this stream.
    fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, Error>;
    /// Obtain a writable handle; writing to it will send data to the
    /// slave end.
    /// Dropping the writer will send EOF to the slave end.
    /// It is invalid to take the writer more than once.
    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, Error>;

    /// Whether this is a ConPTY created with passthrough mode enabled.
    ///
    /// `None` means that the pty implementation is not ConPTY, or does not
    /// expose this implementation-specific detail.  The result should be
    /// queried after spawning a child: ConPTY can fall back to a newly-created
    /// non-passthrough console when process creation rejects passthrough mode.
    fn conpty_passthrough_mode(&self) -> Option<bool> {
        None
    }

    /// Install `hook`, called right after every resize that changes the
    /// size, when the spawned child needs input to follow a resize (a ConPTY
    /// child on the Cygwin console runtime; see `win::conpty`).  Returns
    /// whether the hook was kept.  The hook should queue the input behind
    /// whatever the pane already has queued, never write it out of order.
    /// Call it after spawning.
    fn set_resize_hook(&self, _hook: Box<dyn FnMut() + Send>) -> bool {
        false
    }

    /// If applicable to the type of the tty, return the local process id
    /// of the process group or session leader
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<libc::pid_t>;

    /// If get_termios() and process_group_leader() are both implemented and
    /// return Some, then as_raw_fd() should return the same underlying fd
    /// associated with the stream. This is to enable applications that
    /// "know things" to query similar information for themselves.
    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<unix::RawFd>;

    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf>;

    /// If applicable to the type of the tty, return the termios
    /// associated with the stream
    #[cfg(unix)]
    fn get_termios(&self) -> Option<nix::sys::termios::Termios> {
        None
    }
}
impl_downcast!(MasterPty);

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn non_conpty_master_reports_no_passthrough_mode() {
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();

        assert_eq!(pair.master.conpty_passthrough_mode(), None);
    }
}

/// Represents a child process spawned into the pty.
/// This handle can be used to wait for or terminate that child process.
pub trait Child: std::fmt::Debug + ChildKiller + Downcast + Send {
    /// Poll the child to see if it has completed.
    /// Does not block.
    /// Returns None if the child has not yet terminated,
    /// else returns its exit status.
    fn try_wait(&mut self) -> IoResult<Option<ExitStatus>>;
    /// Blocks execution until the child process has completed,
    /// yielding its exit status.
    fn wait(&mut self) -> IoResult<ExitStatus>;
    /// Returns the process identifier of the child process,
    /// if applicable
    fn process_id(&self) -> Option<u32>;
    /// Returns the process handle of the child process, if applicable.
    /// Only available on Windows.
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle>;
}
impl_downcast!(Child);

/// Represents the ability to signal a Child to terminate
pub trait ChildKiller: std::fmt::Debug + Downcast + Send {
    /// Terminate the child process
    fn kill(&mut self) -> IoResult<()>;

    /// Clone an object that can be split out from the Child in order
    /// to send it signals independently from a thread that may be
    /// blocked in `.wait`.
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync>;
}
impl_downcast!(ChildKiller);

/// Represents the slave side of a pty.
/// Can be used to spawn processes into the pty.
pub trait SlavePty {
    /// Spawns the command specified by the provided CommandBuilder
    fn spawn_command(&self, cmd: CommandBuilder) -> Result<Box<dyn Child + Send + Sync>, Error>;
}

/// Represents the exit status of a child process.
#[derive(Debug, Clone)]
pub struct ExitStatus {
    code: u32,
    signal: Option<String>,
}

impl ExitStatus {
    /// Construct an ExitStatus from a process return code
    pub fn with_exit_code(code: u32) -> Self {
        Self { code, signal: None }
    }

    /// Construct an ExitStatus from a signal name
    pub fn with_signal(signal: &str) -> Self {
        Self {
            code: 1,
            signal: Some(signal.to_string()),
        }
    }

    /// Returns true if the status indicates successful completion
    pub fn success(&self) -> bool {
        match self.signal {
            None => self.code == 0,
            Some(_) => false,
        }
    }

    /// Returns the exit code that this ExitStatus was constructed with
    pub fn exit_code(&self) -> u32 {
        self.code
    }

    /// Returns the signal if present that this ExitStatus was constructed with
    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }
}

impl From<std::process::ExitStatus> for ExitStatus {
    fn from(status: std::process::ExitStatus) -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;

            if let Some(signal) = status.signal() {
                let signame = unsafe { libc::strsignal(signal) };
                let signal = if signame.is_null() {
                    format!("Signal {}", signal)
                } else {
                    let signame = unsafe { std::ffi::CStr::from_ptr(signame) };
                    signame.to_string_lossy().to_string()
                };

                return ExitStatus {
                    code: status.code().map(|c| c as u32).unwrap_or(1),
                    signal: Some(signal),
                };
            }
        }

        let code =
            status
                .code()
                .map(|c| c as u32)
                .unwrap_or_else(|| if status.success() { 0 } else { 1 });

        ExitStatus { code, signal: None }
    }
}

impl std::fmt::Display for ExitStatus {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.success() {
            write!(fmt, "Success")
        } else {
            match &self.signal {
                Some(sig) => write!(fmt, "Terminated by {}", sig),
                None => write!(fmt, "Exited with code {}", self.code),
            }
        }
    }
}

pub struct PtyPair {
    // slave is listed first so that it is dropped first.
    // The drop order is stable and specified by rust rfc 1857
    pub slave: Box<dyn SlavePty + Send>,
    pub master: Box<dyn MasterPty + Send>,
}

/// The `PtySystem` trait allows an application to work with multiple
/// possible Pty implementations at runtime.  This is important on
/// Windows systems which have a variety of implementations.
pub trait PtySystem: Downcast {
    /// Create a new Pty instance with the window size set to the specified
    /// dimensions.  Returns a (master, slave) Pty pair.  The master side
    /// is used to drive the slave side.
    fn openpty(&self, size: PtySize) -> anyhow::Result<PtyPair>;
}
impl_downcast!(PtySystem);

impl Child for std::process::Child {
    fn try_wait(&mut self) -> IoResult<Option<ExitStatus>> {
        std::process::Child::try_wait(self).map(|status| status.map(Into::into))
    }

    fn wait(&mut self) -> IoResult<ExitStatus> {
        std::process::Child::wait(self).map(Into::into)
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.id())
    }

    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        Some(std::os::windows::io::AsRawHandle::as_raw_handle(self))
    }
}

#[derive(Debug)]
struct ProcessSignaller {
    pid: Option<u32>,

    #[cfg(windows)]
    handle: Option<filedescriptor::OwnedHandle>,
}

#[cfg(windows)]
impl ChildKiller for ProcessSignaller {
    fn kill(&mut self) -> IoResult<()> {
        if let Some(handle) = &self.handle {
            unsafe {
                if winapi::um::processthreadsapi::TerminateProcess(handle.as_raw_handle() as _, 127)
                    == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
        }
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Self {
            pid: self.pid,
            handle: self.handle.as_ref().and_then(|h| h.try_clone().ok()),
        })
    }
}

#[cfg(unix)]
impl ChildKiller for ProcessSignaller {
    fn kill(&mut self) -> IoResult<()> {
        if let Some(pid) = self.pid {
            let result = unsafe { libc::kill(pid as i32, libc::SIGHUP) };
            if result != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Self { pid: self.pid })
    }
}

impl ChildKiller for std::process::Child {
    fn kill(&mut self) -> IoResult<()> {
        #[cfg(unix)]
        {
            // On unix, we send the SIGHUP signal instead of trying to kill
            // the process. The default behavior of a process receiving this
            // signal is to be killed unless it configured a signal handler.
            let result = unsafe { libc::kill(self.id() as i32, libc::SIGHUP) };
            if result != 0 {
                return Err(std::io::Error::last_os_error());
            }

            // We successfully delivered SIGHUP, but the semantics of Child::kill
            // are that on success the process is dead or shortly about to
            // terminate.  Since SIGUP doesn't guarantee termination, we
            // give the process a bit of a grace period to shutdown or do whatever
            // it is doing in its signal handler befre we proceed with the
            // full on kill.
            for attempt in 0..5 {
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }

                if let Ok(Some(_)) = self.try_wait() {
                    // It completed, so report success!
                    return Ok(());
                }
            }

            // it's still alive after a grace period, so proceed with a kill
        }

        std::process::Child::kill(self)
    }

    #[cfg(windows)]
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        struct RawDup(RawHandle);
        impl AsRawHandle for RawDup {
            fn as_raw_handle(&self) -> RawHandle {
                self.0
            }
        }

        Box::new(ProcessSignaller {
            pid: self.process_id(),
            handle: Child::as_raw_handle(self)
                .as_ref()
                .and_then(|h| filedescriptor::OwnedHandle::dup(&RawDup(*h)).ok()),
        })
    }

    #[cfg(unix)]
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(ProcessSignaller {
            pid: self.process_id(),
        })
    }
}

pub fn native_pty_system() -> Box<dyn PtySystem + Send> {
    Box::new(NativePtySystem::default())
}

/// The one process-global lock that orders console identity changes against
/// ConPTY spawns.
///
/// On Windows, FreeConsole/AttachConsole swap the whole process's console
/// connection and its std handle slots.  CreateProcessW for a ConPTY child
/// (bInheritHandles=FALSE, no STARTF_USESTDHANDLES) stamps the parent's
/// std handle *values* into the child's ProcessParameters at that instant;
/// a spawn landing inside another thread's FreeConsole/AttachConsole window
/// gives the child freed, recycled handle values and the shell dies at its
/// first console read (psmux issue #450).  Every FreeConsole/AttachConsole
/// dance and every ConPTY spawn must hold this lock.
///
/// Identity changes take it EXCLUSIVELY ([`console_state_lock`]); spawns take
/// it SHARED ([`conpty_spawn_guard`]).  See `conpty_spawn_guard` for why two
/// spawns may safely overlap.
static CONSOLE_STATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Exclusive hold on the process console identity: FreeConsole/AttachConsole,
/// and anything else that swaps the process's console connection or its std
/// handle slots.  Blocks, and is blocked by, every in flight ConPTY spawn.
///
/// A panic while holding the lock leaves console state possibly odd but the
/// lock itself must keep working (see psmux issue #446).
pub fn console_state_lock() -> std::sync::RwLockWriteGuard<'static, ()> {
    CONSOLE_STATE.write().unwrap_or_else(|e| e.into_inner())
}

/// How long the last ConPTY spawn on THIS thread spent waiting to get into the
/// console state, in microseconds.  Diagnostics only: psmux reads it for its
/// `PSMUX_WARM_TRACE` spawn line so a serialised surge is visible as wait time
/// rather than guessed at.
pub fn last_spawn_console_wait_us() -> u64 {
    SPAWN_WAIT_US.with(|c| c.get())
}

/// How long the last ConPTY spawn on THIS thread spent inside CreateProcessW
/// itself (every attempt it made), in microseconds.  Diagnostics only: the
/// rest of a spawn is psmux's own work and waits, this part is the operating
/// system's.  They must be told apart because the OS part does not scale with
/// concurrency for a Store packaged shell: measured on Windows 11 26200 from an
/// unpackaged caller with no psmux code and no job object at all, eight
/// concurrent CreateProcessW calls of the Store pwsh each took about 300 ms
/// (one alone takes about 50 ms) and returned in batches at the same instant,
/// with a straggler finishing 90 to 250 ms after the rest, while eight of
/// cmd.exe took 2 to 17 ms.  A surge of spares therefore shows a spread of
/// whole spawn times that psmux neither causes nor can remove, and only the
/// time OUTSIDE CreateProcessW says whether psmux serialised its spawns
/// (issue #686).
pub fn last_spawn_create_us() -> u64 {
    SPAWN_CREATE_US.with(|c| c.get())
}

/// Start a new spawn's CreateProcessW account on this thread.
pub fn reset_spawn_create_us() {
    SPAWN_CREATE_US.with(|c| c.set(0));
}

thread_local! {
    static SPAWN_WAIT_US: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static SPAWN_CREATE_US: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Adds the time until it drops to this thread's CreateProcessW account.
#[cfg(windows)]
pub(crate) struct SpawnCreateTimer(std::time::Instant);

#[cfg(windows)]
impl SpawnCreateTimer {
    pub(crate) fn start() -> Self {
        Self(std::time::Instant::now())
    }
}

#[cfg(windows)]
impl Drop for SpawnCreateTimer {
    fn drop(&mut self) {
        let us = self.0.elapsed().as_micros() as u64;
        SPAWN_CREATE_US.with(|c| c.set(c.get().saturating_add(us)));
    }
}

/// Shared, refcounted entry into the console state for a ConPTY spawn.
///
/// Two ConPTY spawns do NOT conflict with each other.  Both want the exact
/// same process-global state for the duration of their `CreateProcessW`: the
/// three std handle slots parked on NULL, so the child is born with the
/// GUI-parent handle set that conhost fills in itself (issue #450).  Holding
/// them apart therefore bought nothing and cost everything: a surge of eight
/// spares queued one whole `CreateProcessW` behind another, so the eighth paid
/// about 45ms times eight before its shell even began starting (issue #686).
///
/// What must still be exclusive is an *identity change* (FreeConsole /
/// AttachConsole), because that one wants the slots to hold something else.
/// So identity changes take [`console_state_lock`] for write and spawns take
/// the read side here, and the NULL parking is refcounted: the first spawn in
/// saves the real handles and parks the slots, the last one out restores them.
/// While any spawn is inside, an identity change is blocked, so nothing can
/// alter the slots underneath the refcount.
pub struct ConPtySpawnGuard {
    _shared: std::sync::RwLockReadGuard<'static, ()>,
}

/// Saved std handles plus how many spawns are currently parked on NULL.
/// Handles are kept as `usize` because a raw `HANDLE` is not `Send`.
static STD_PARK: std::sync::Mutex<(usize, [usize; 3])> = std::sync::Mutex::new((0, [0; 3]));

impl ConPtySpawnGuard {
    /// Enter the console state for a spawn and park the std handle slots on
    /// NULL.  Blocks only while an identity change is in flight.
    pub fn acquire() -> Self {
        let t0 = std::time::Instant::now();
        let shared = CONSOLE_STATE.read().unwrap_or_else(|e| e.into_inner());
        SPAWN_WAIT_US.with(|c| c.set(t0.elapsed().as_micros() as u64));
        #[cfg(windows)]
        {
            let mut park = STD_PARK.lock().unwrap_or_else(|e| e.into_inner());
            if park.0 == 0 {
                park.1 = unsafe { save_and_null_std_handles() };
            }
            park.0 += 1;
        }
        Self { _shared: shared }
    }
}

impl Drop for ConPtySpawnGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let mut park = STD_PARK.lock().unwrap_or_else(|e| e.into_inner());
            park.0 = park.0.saturating_sub(1);
            if park.0 == 0 {
                unsafe { restore_std_handles(park.1) };
            }
        }
    }
}

#[cfg(windows)]
unsafe fn save_and_null_std_handles() -> [usize; 3] {
    use winapi::um::processenv::{GetStdHandle, SetStdHandle};
    use winapi::um::winbase::{STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    let saved = [
        GetStdHandle(STD_INPUT_HANDLE) as usize,
        GetStdHandle(STD_OUTPUT_HANDLE) as usize,
        GetStdHandle(STD_ERROR_HANDLE) as usize,
    ];
    SetStdHandle(STD_INPUT_HANDLE, std::ptr::null_mut());
    SetStdHandle(STD_OUTPUT_HANDLE, std::ptr::null_mut());
    SetStdHandle(STD_ERROR_HANDLE, std::ptr::null_mut());
    saved
}

#[cfg(windows)]
unsafe fn restore_std_handles(saved: [usize; 3]) {
    use winapi::um::processenv::SetStdHandle;
    use winapi::um::winbase::{STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    SetStdHandle(STD_INPUT_HANDLE, saved[0] as _);
    SetStdHandle(STD_OUTPUT_HANDLE, saved[1] as _);
    SetStdHandle(STD_ERROR_HANDLE, saved[2] as _);
}

#[cfg(unix)]
pub type NativePtySystem = unix::UnixPtySystem;
#[cfg(windows)]
pub type NativePtySystem = win::conpty::ConPtySystem;
