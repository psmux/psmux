use crate::{Child, ChildKiller, ExitStatus};
use anyhow::Context as _;
use std::io::{Error as IoError, Result as IoResult};
use std::os::windows::io::{AsRawHandle, RawHandle};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use winapi::shared::minwindef::DWORD;
use winapi::um::minwinbase::STILL_ACTIVE;
use winapi::um::processthreadsapi::*;
use winapi::um::synchapi::WaitForSingleObject;
use winapi::um::winbase::INFINITE;

pub mod conpty;
mod procthreadattr;
mod psuedocon;
pub(crate) mod spawn_trace;

/// Which ConPTY implementation this process loaded: the system one, or a
/// conpty.dll named by `PSMUX_CONPTY_DIR`.  Diagnostics only.
pub use psuedocon::{conpty_source, ConPtySource};

use filedescriptor::OwnedHandle;

#[derive(Debug)]
pub struct WinChild {
    proc: Mutex<OwnedHandle>,
    /// The pane process's own job; see [`PaneJob`].
    #[allow(dead_code)]
    job: Option<PaneJob>,
}

/// A job object holding one pane's process and everything it starts, armed
/// with KILL_ON_JOB_CLOSE while psmux holds it.
///
/// tmux closes a pane's pty master when the pane or the server goes away, so
/// the pane's processes get SIGHUP (window.c window_pane_destroy, server exit).
/// On Windows the pseudoconsole's conhost plays that part when its owner
/// closes it or exits.  It does not when the owner dies while that conhost is
/// still starting: measured on cb783dc, a server TerminateProcess'd 0 to 150 ms
/// after `new-session -d` left the warm pool's freshly spawned shells alive,
/// each with its conhost, in 25 of 30 rounds (a sweep collected 51 of them).
/// The handle lives in the server, so when the server ends in ANY way the
/// kernel closes it and ends every process still in the job, and the conhost,
/// left without clients, exits on its own.
///
/// A job of its own per pane that the process enters at creation (the job list
/// attribute, or for an image that refuses it, created suspended and assigned
/// before it runs), rather than one inherited from the server: measured, a
/// server's children are born outside its job (the server's own job allows
/// silent breakaway) and a Store packaged shell did not reliably stay in an
/// inherited job, so a server wide job let pane processes escape.  Nor is the
/// conhost put in a job: a conhost the kernel terminates skips the CTRL_CLOSE
/// round it does when it exits on its own, and a client that ignores console
/// errors (`ping -t`) then ran on.
///
/// BREAKAWAY_OK lets a process the pane starts leave on purpose
/// (CREATE_BREAKAWAY_FROM_JOB): a psmux server started from inside a pane does,
/// and survives, as `tmux new -d` inside tmux does.
///
/// When psmux lets go of a pane (its `WinChild` is dropped: the pane is
/// killed, exits, or a warm spare is retired), the handle closes ARMED, so
/// whatever is left of that pane's tree ends with it, as closing a pty master
/// hangs up its process group.  kill-pane, kill-window, kill-session and
/// kill-server already ended the whole tree (measured on cb783dc, a program
/// the pane started with Start-Process included); this makes a pane psmux
/// drops in any other way behave the same.
///
/// Residual, measured: an image that refuses the job list (the alias) is
/// created suspended, and a server killed while that CreateProcessW is in the
/// kernel leaves the new shell suspended and outside the job (0 to 4 of 20
/// rounds when the server is killed 0 to 150 ms after `new-session -d`, where
/// cb783dc left a RUNNING shell and its conhost in 11 to 14 of 15).  Such a
/// shell never ran and holds no conhost.  A graceful shutdown (kill-server,
/// the last client leaving a destroy-unattached session) closes this window
/// for spare spawns: psmux's teardown reaper waits for every spare still
/// inside CreateProcessW to return its pid before the process exits (#686,
/// `INFLIGHT_REAP_BUDGET`), so only a server terminated from outside can still
/// leave one.  Launching the alias's real image path would not close it: from an
/// unpackaged caller such as the server, the real path under Program
/// Files\WindowsApps refuses the job list too (measured, see spawn_command).
///
/// Cost, measured with `PSMUX_SPAWN_TRACE=1` on the Store pwsh: creating and
/// arming the job ~22 us, assigning the suspended process ~30 us, resuming it
/// ~5 us, against a CreateProcessW of 50 to 300 ms that is the same with the
/// job off; interleaved cold `new-session -d` to prompt, 45 pairs, job on
/// minus job off had a median of -12.6 and 3.1 ms in two batches.
/// `PSMUX_NO_PANE_JOB=1` skips the job, for diagnosis.
#[derive(Debug)]
pub(crate) struct PaneJob(OwnedHandle);

extern "system" {
    fn CreateJobObjectW(attrs: *mut winapi::shared::minwindef::LPVOID, name: *const u16) -> winapi::um::winnt::HANDLE;
    fn SetInformationJobObject(job: winapi::um::winnt::HANDLE, class: i32, info: winapi::shared::minwindef::LPVOID, len: DWORD) -> i32;
    fn AssignProcessToJobObject(job: winapi::um::winnt::HANDLE, process: winapi::um::winnt::HANDLE) -> i32;
}

/// JobObjectExtendedLimitInformation.
const JOB_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
pub(crate) const PANE_JOB_ARMED: DWORD =
    winapi::um::winnt::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | winapi::um::winnt::JOB_OBJECT_LIMIT_BREAKAWAY_OK;

fn set_job_limits(job: winapi::um::winnt::HANDLE, flags: DWORD) -> bool {
    let mut info: winapi::um::winnt::JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = flags;
    unsafe {
        SetInformationJobObject(
            job,
            JOB_EXTENDED_LIMIT_INFORMATION_CLASS,
            &mut info as *mut _ as winapi::shared::minwindef::LPVOID,
            std::mem::size_of::<winapi::um::winnt::JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as DWORD,
        ) != 0
    }
}

impl PaneJob {
    /// A new, empty, armed job for one pane process, which enters it at
    /// creation (see spawn_command).  `None` when the job could not be made;
    /// the process then runs as it did before.
    pub(crate) fn create() -> Option<PaneJob> {
        if std::env::var_os("PSMUX_NO_PANE_JOB").is_some_and(|v| v == "1") {
            return None;
        }
        unsafe {
            // NULL security attributes: the handle is not inheritable, so no
            // child can keep the job open after the server is gone.
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let owned = <OwnedHandle as std::os::windows::io::FromRawHandle>::from_raw_handle(job as _);
            if !set_job_limits(job, PANE_JOB_ARMED) {
                return None;
            }
            Some(PaneJob(owned))
        }
    }

    pub(crate) fn handle(&self) -> winapi::um::winnt::HANDLE {
        self.0.as_raw_handle() as _
    }

    /// Put a process created suspended into the job (the image refused the
    /// job list attribute).
    pub(crate) fn assign(&self, process: winapi::um::winnt::HANDLE) -> bool {
        unsafe { AssignProcessToJobObject(self.handle(), process) != 0 }
    }
}



impl WinChild {
    fn is_complete(&mut self) -> IoResult<Option<ExitStatus>> {
        let mut status: DWORD = 0;
        let proc = self.proc.lock().unwrap_or_else(|e| e.into_inner()).try_clone().unwrap();
        let res = unsafe { GetExitCodeProcess(proc.as_raw_handle() as _, &mut status) };
        if res != 0 {
            if status == STILL_ACTIVE {
                Ok(None)
            } else {
                Ok(Some(ExitStatus::with_exit_code(status)))
            }
        } else {
            Ok(None)
        }
    }

    fn do_kill(&mut self) -> IoResult<()> {
        let proc = self.proc.lock().unwrap_or_else(|e| e.into_inner()).try_clone().unwrap();
        let res = unsafe { TerminateProcess(proc.as_raw_handle() as _, 1) };
        let err = IoError::last_os_error();
        // TerminateProcess returns nonzero on SUCCESS, zero on failure.
        if res == 0 {
            Err(err)
        } else {
            Ok(())
        }
    }
}

impl ChildKiller for WinChild {
    fn kill(&mut self) -> IoResult<()> {
        self.do_kill().ok();
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        let proc = self.proc.lock().unwrap_or_else(|e| e.into_inner()).try_clone().unwrap();
        Box::new(WinChildKiller { proc })
    }
}

#[derive(Debug)]
pub struct WinChildKiller {
    proc: OwnedHandle,
}

impl ChildKiller for WinChildKiller {
    fn kill(&mut self) -> IoResult<()> {
        let res = unsafe { TerminateProcess(self.proc.as_raw_handle() as _, 1) };
        let err = IoError::last_os_error();
        // TerminateProcess returns nonzero on SUCCESS, zero on failure.
        if res == 0 {
            Err(err)
        } else {
            Ok(())
        }
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        let proc = self.proc.try_clone().unwrap();
        Box::new(WinChildKiller { proc })
    }
}

impl Child for WinChild {
    fn try_wait(&mut self) -> IoResult<Option<ExitStatus>> {
        self.is_complete()
    }

    fn wait(&mut self) -> IoResult<ExitStatus> {
        if let Ok(Some(status)) = self.try_wait() {
            return Ok(status);
        }
        let proc = self.proc.lock().unwrap_or_else(|e| e.into_inner()).try_clone().unwrap();
        unsafe {
            WaitForSingleObject(proc.as_raw_handle() as _, INFINITE);
        }
        let mut status: DWORD = 0;
        let res = unsafe { GetExitCodeProcess(proc.as_raw_handle() as _, &mut status) };
        if res != 0 {
            Ok(ExitStatus::with_exit_code(status))
        } else {
            Err(IoError::last_os_error())
        }
    }

    fn process_id(&self) -> Option<u32> {
        let res = unsafe { GetProcessId(self.proc.lock().unwrap_or_else(|e| e.into_inner()).as_raw_handle() as _) };
        if res == 0 {
            None
        } else {
            Some(res)
        }
    }

    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        let proc = self.proc.lock().unwrap_or_else(|e| e.into_inner());
        Some(proc.as_raw_handle())
    }
}

impl std::future::Future for WinChild {
    type Output = anyhow::Result<ExitStatus>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<anyhow::Result<ExitStatus>> {
        match self.is_complete() {
            Ok(Some(status)) => Poll::Ready(Ok(status)),
            Err(err) => Poll::Ready(Err(err).context("Failed to retrieve process exit status")),
            Ok(None) => {
                struct PassRawHandleToWaiterThread(pub RawHandle);
                unsafe impl Send for PassRawHandleToWaiterThread {}

                let proc = self.proc.lock().unwrap_or_else(|e| e.into_inner()).try_clone()?;
                let handle = PassRawHandleToWaiterThread(proc.as_raw_handle());

                let waker = cx.waker().clone();
                std::thread::spawn(move || {
                    unsafe {
                        WaitForSingleObject(handle.0 as _, INFINITE);
                    }
                    waker.wake();
                });
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests_issue446 {
    // Issue #446: a panic while holding WinChild's `proc` mutex poisons it, and
    // every method then does `self.proc.lock().unwrap_or_else(|e| e.into_inner())` which panics on a
    // poisoned mutex. Result: the pane's child can no longer be queried, waited
    // on, or killed, so it leaks and becomes permanently un-killable.
    //
    // The poisoning path is reachable from this very file: e.g.
    // `self.proc.lock().unwrap_or_else(|e| e.into_inner()).try_clone().unwrap()` keeps the guard alive
    // across the `try_clone().unwrap()`, so if `try_clone` fails (handle
    // exhaustion) the panic unwinds while the lock is held and poisons it.
    //
    // These tests build a WinChild over a REAL, live OS process, poison the
    // exact mutex the methods lock, then prove the child stays queryable and
    // killable. They PANIC (fail) before the fix and pass after it.
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::process::{Command, Stdio};

    // Spawn a genuinely long-lived process and wrap a duplicated handle in a
    // WinChild, exactly the kind of full-access handle spawn_command produces.
    fn spawn_live_winchild() -> (WinChild, std::process::Child) {
        let child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ping test child");
        let proc = OwnedHandle::dup(&child).expect("duplicate process handle");
        (
            WinChild {
                proc: Mutex::new(proc),
                job: None,
            },
            child,
        )
    }

    // Poison `proc` the way an unrelated teardown panic would: hold the guard
    // and unwind through it.
    fn poison_proc(win: &WinChild) {
        let res = catch_unwind(AssertUnwindSafe(|| {
            let _guard = win.proc.lock().unwrap();
            panic!("simulated panic while holding proc lock (try_clone().unwrap() failure)");
        }));
        assert!(res.is_err(), "the panic must unwind so the lock is poisoned");
        assert!(win.proc.is_poisoned(), "proc mutex must be poisoned now");
    }

    #[test]
    fn poisoned_mutex_still_kills_and_waits() {
        let (mut win, mut sys_child) = spawn_live_winchild();

        // Baseline: process is alive and identifiable.
        let pid_before = win.process_id();
        assert!(
            pid_before.is_some(),
            "process_id should resolve for a live child"
        );

        poison_proc(&win);

        // Each of these panics on master (BUG). After the fix they must not.
        let pid = catch_unwind(AssertUnwindSafe(|| win.process_id()))
            .expect("BUG #446: process_id() panicked on a poisoned mutex");
        assert_eq!(
            pid, pid_before,
            "process_id must still resolve after poisoning"
        );

        let killed = catch_unwind(AssertUnwindSafe(|| win.kill()))
            .expect("BUG #446: kill() panicked on a poisoned mutex -> pane un-killable");
        assert!(killed.is_ok(), "kill() should report success");

        let status = catch_unwind(AssertUnwindSafe(|| win.wait()))
            .expect("BUG #446: wait() panicked on a poisoned mutex");
        assert!(
            status.is_ok(),
            "wait() should return an exit status after kill"
        );

        // Prove the real OS process is actually gone (not merely that we did
        // not crash): reaping the system handle must succeed.
        let reaped = sys_child.wait().expect("reap the underlying OS process");
        // A TerminateProcess(1) exit is a non-zero code; either way the process
        // has ended, which is the whole point.
        assert!(
            reaped.code().is_some(),
            "the underlying process must have terminated"
        );
    }

    #[test]
    fn poisoned_mutex_try_wait_raw_handle_and_clone_killer_survive() {
        let (mut win, mut sys_child) = spawn_live_winchild();
        poison_proc(&win);

        let tw = catch_unwind(AssertUnwindSafe(|| win.try_wait()))
            .expect("BUG #446: try_wait() panicked on a poisoned mutex");
        assert!(tw.is_ok(), "try_wait should return Ok(...)");

        let raw = catch_unwind(AssertUnwindSafe(|| win.as_raw_handle()))
            .expect("BUG #446: as_raw_handle() panicked on a poisoned mutex");
        assert!(raw.is_some(), "as_raw_handle should return the handle");

        // clone_killer is how the pane hands a killer to another thread; it must
        // survive poisoning too, and the cloned killer must actually kill.
        let mut killer = catch_unwind(AssertUnwindSafe(|| win.clone_killer()))
            .expect("BUG #446: clone_killer() panicked on a poisoned mutex");
        killer.kill().ok();
        let _ = win.wait();
        let reaped = sys_child.wait().expect("reap the underlying OS process");
        assert!(
            reaped.code().is_some(),
            "cloned killer must have terminated the process"
        );
    }
}
