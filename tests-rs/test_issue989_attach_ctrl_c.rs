// Issue #989: a psmux process attached to a pane console must survive a
// CTRL_C_EVENT that reaches that console while it is attached.
//
// conhost turns a typed 0x03 into a CTRL_C_EVENT for every process on a
// cooked console, on its own schedule.  psmux attaches to pane consoles for
// Ctrl+C, mouse and key injections, so the event can land while it is there.
// Measured: the handler `install_console_ctrl_handler` registers at startup
// is not called after AttachConsole, so the process ran the default handler
// and exited with 0xC000013A.  `free_and_attach` now protects every attach.
//
// The attach and the event happen in a CHILD copy of this test binary with
// its own hidden console, so nothing here can signal the console cargo runs
// in.  The child plays the server: it clears the inherited ignore bit, sets
// SERVER_PROCESS and installs the startup handler, exactly as run_server does.

use super::*;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

const CHILD_ENV: &str = "PSMUX_T989_ATTACH_CHILD";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[link(name = "kernel32")]
extern "system" {
    fn GenerateConsoleCtrlEvent(ctrl_event: u32, process_group_id: u32) -> i32;
}

fn test_name(leaf: &str) -> String {
    // libtest names a test by its module path without the crate name.
    let path = module_path!();
    let rest = path.splitn(2, "::").nth(1).unwrap_or(path);
    format!("{}::{}", rest, leaf)
}

/// Body of the child: attach the way the server does and take a CTRL_C_EVENT.
fn child_body() {
    crate::platform::clear_inherited_ctrl_c_ignore();
    SERVER_PROCESS.store(true, std::sync::atomic::Ordering::Relaxed);
    crate::platform::install_console_ctrl_handler();

    // The "pane": a native console program on a console of its own.
    let mut victim = Command::new(r"C:\Windows\System32\PING.EXE")
        .args(["-t", "127.0.0.1"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ping");
    std::thread::sleep(std::time::Duration::from_millis(700));

    let attached = {
        let _g = portable_pty::console_state_lock();
        unsafe {
            let ok = free_and_attach(victim.id(), "t989");
            if ok {
                // What conhost does for a typed Ctrl+C on a cooked console.
                GenerateConsoleCtrlEvent(0, 0);
                std::thread::sleep(std::time::Duration::from_millis(400));
                FreeConsole();
            }
            ok
        }
    };
    let _ = victim.kill();
    let _ = victim.wait();
    if let Some(marker) = std::env::var_os(CHILD_ENV) {
        let _ = std::fs::write(marker, if attached { "survived" } else { "no attach" });
    }
    // Reaching this line at all is the point: without the protection the
    // process exits with 0xC000013A inside the sleep above.
    std::process::exit(if attached { 0 } else { 3 });
}

#[test]
fn attached_process_survives_ctrl_c_on_the_pane_console() {
    if std::env::var_os(CHILD_ENV).is_some() {
        child_body();
        return;
    }
    let exe = std::env::current_exe().expect("current_exe");
    // The child writes here only if it ran the body and lived past the event,
    // so a test name that matched nothing cannot pass by exiting 0.
    let marker = std::env::temp_dir().join(format!("psmux_t989_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let status = Command::new(exe)
        .args([
            "--exact",
            &test_name("attached_process_survives_ctrl_c_on_the_pane_console"),
            "--test-threads=1",
            "--nocapture",
        ])
        .env(CHILD_ENV, &marker)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run child");
    let code = status.code().unwrap_or(-1) as u32;
    assert_ne!(code, 3, "the child could not attach to the victim console");
    assert_eq!(
        code, 0,
        "attached process died on the pane's CTRL_C_EVENT with exit 0x{:08X} (0xC000013A is STATUS_CONTROL_C_EXIT)",
        code
    );
    let said = std::fs::read_to_string(&marker).unwrap_or_default();
    let _ = std::fs::remove_file(&marker);
    assert_eq!(said, "survived", "the child did not run the attach body");
}

#[test]
fn survival_handler_keeps_ctrl_c_and_break_only() {
    unsafe {
        assert_eq!(survive_ctrl_c_while_attached(0), 1, "CTRL_C_EVENT handled");
        assert_eq!(survive_ctrl_c_while_attached(1), 1, "CTRL_BREAK_EVENT handled");
        assert_eq!(survive_ctrl_c_while_attached(2), 0, "CTRL_CLOSE_EVENT default");
        assert_eq!(survive_ctrl_c_while_attached(5), 0, "CTRL_LOGOFF_EVENT default");
        assert_eq!(survive_ctrl_c_while_attached(6), 0, "CTRL_SHUTDOWN_EVENT default");
    }
}
