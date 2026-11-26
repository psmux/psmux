// Issue #470: display-popup invoked from display-menu did not run its command.
//
// Root cause: execute_command_string_single() split the command with
// split_whitespace(), which does NOT strip shell quotes. A menu item whose
// command was `display-popup -E 'lazygit'` therefore executed the literal
// `'lazygit'` (with quotes), which is not a runnable program, so the popup
// shell exited immediately and close-on-exit closed the popup. The fix
// re-tokenizes with the quote-aware parse_command_line() so the popup command
// matches the CLI/TCP dispatch path.
//
// These tests assert the parsed popup command (stored in Mode::PopupMode.command)
// has its surrounding quotes stripped. They use short/harmless inner commands so
// no long-lived child processes linger after the test run.

use super::*;

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn popup_command(app: &AppState) -> Option<String> {
    match &app.mode {
        Mode::PopupMode { command, .. } => Some(command.clone()),
        _ => None,
    }
}

#[test]
fn display_popup_strips_single_quotes_from_command() {
    let mut app = mock_app();
    // Reporter's exact shape (single-word inner command). `lazygit` is usually
    // absent, so no process is spawned; only the parsed command string matters.
    execute_command_string(&mut app, "display-popup -E 'lazygit'").unwrap();
    let cmd = popup_command(&app).expect("display-popup must enter PopupMode");
    assert_eq!(cmd, "lazygit", "single quotes must be stripped from the popup command");
}

#[test]
fn display_popup_strips_quotes_from_multiword_command() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -E 'cmd /c ver'").unwrap();
    let cmd = popup_command(&app).expect("display-popup must enter PopupMode");
    assert_eq!(cmd, "cmd /c ver", "quoted multi-word command must survive intact WITHOUT the quotes");
}

#[test]
fn display_popup_strips_double_quotes() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -E \"cmd /c ver\"").unwrap();
    let cmd = popup_command(&app).expect("display-popup must enter PopupMode");
    assert_eq!(cmd, "cmd /c ver", "double quotes must be stripped too");
}

#[test]
fn display_popup_unquoted_single_word_still_works() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -E whoami").unwrap();
    let cmd = popup_command(&app).expect("display-popup must enter PopupMode");
    assert_eq!(cmd, "whoami", "an unquoted command must be unchanged");
}

#[test]
fn display_popup_e_flag_sets_close_on_exit() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -E 'whoami'").unwrap();
    match &app.mode {
        Mode::PopupMode { close, .. } => assert!(close.closes_on_exit(0), "-E must close the popup when its command exits"),
        _ => panic!("expected PopupMode"),
    }
}

#[test]
fn display_popup_from_menu_item_command_runs_popup() {
    // Simulate the full display-menu -> menu-select dispatch: the stored item
    // command is executed via execute_command_string, exactly as the server's
    // MenuSelect handler does.
    let mut app = mock_app();
    let menu_item_command = "display-popup -E 'lazygit'";
    execute_command_string(&mut app, menu_item_command).unwrap();
    match &app.mode {
        Mode::PopupMode { command, .. } => {
            assert_eq!(command, "lazygit");
            assert!(!command.contains('\''), "the literal quotes that broke #470 must be gone");
        }
        _ => panic!("menu-fired display-popup must enter PopupMode, not stay in a non-popup mode"),
    }
}

#[test]
fn display_popup_regression_quotes_are_not_left_literal() {
    // The exact regression guard: BEFORE the fix, the command retained quotes
    // (e.g. "'lazygit'"), which is what caused the popup to die instantly.
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -E 'lazygit'").unwrap();
    let cmd = popup_command(&app).unwrap();
    assert!(!cmd.starts_with('\''), "command must NOT start with a literal quote (that was the bug)");
    assert!(!cmd.ends_with('\''), "command must NOT end with a literal quote (that was the bug)");
}

#[test]
fn display_popup_preserves_width_height_flags_with_quoted_command() {
    // Ensure the quote-aware re-tokenization still parses -w/-h correctly.
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -w 50 -h 10 -E 'lazygit'").unwrap();
    match &app.mode {
        Mode::PopupMode { command, width, height, .. } => {
            assert_eq!(command, "lazygit", "command still stripped with -w/-h present");
            assert_eq!(*width, 50, "-w must be honored");
            assert_eq!(*height, 10, "-h must be honored");
        }
        _ => panic!("expected PopupMode"),
    }
}
