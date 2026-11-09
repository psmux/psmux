// Mouse input is delivered to a console, not to every descendant process.
use super::*;

fn processes() -> Vec<ProcRowT> {
    vec![
        (10, 1, "cmd.exe".into()),
        (20, 10, "native-tui.exe".into()),
        (30, 20, "powershell.exe".into()),
        (40, 30, "ssh.exe".into()),
    ]
}

#[test]
fn private_console_ssh_descendant_does_not_change_native_mouse_transport() {
    // Same ancestry as a TUI -> tool subprocess -> ssh -N. The native TUI's
    // console contains only cmd and the TUI; the SSH helper owns another one.
    assert!(!console_contains_vt_bridge(&processes(), &[10, 20]));
}

#[test]
fn interactive_ssh_on_the_input_console_retains_vt_transport() {
    assert!(console_contains_vt_bridge(&processes(), &[10, 40]));
}

#[test]
fn directly_spawned_bridge_is_a_console_member_too() {
    assert!(console_contains_vt_bridge(&processes(), &[40]));
}

#[test]
fn wsl_bridge_still_uses_vt_transport() {
    let entries = vec![(50, 10, "wsl.exe".into())];
    assert!(console_contains_vt_bridge(&entries, &[10, 50]));
    assert!(!console_contains_vt_bridge(&entries, &[10]));
}

#[test]
fn pane_without_bridge_descendant_never_queries_its_console() {
    // The console query attaches the server to the pane console; a tree with
    // no bridge in it must answer without that.
    let mut queried = false;
    let hit = vt_bridge_on_pane_console(std::process::id(), || { queried = true; Some(vec![]) });
    assert!(!hit);
    assert!(!queried, "console membership was queried for a tree with no bridge");
}
