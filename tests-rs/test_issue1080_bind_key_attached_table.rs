// Found while checking #1080 (`-T` before the command): bind-key's own `-T`
// in the attached spelling tmux's getopt reads ("nrN:T:", cmd-bind-key.c).
// A config `bind -Troot F5 ...` read the `r` of `root` as -r and bound
// `-r -T prefix F5`; from the CLI, `bind-key -Troot F5 ...` bound nothing.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};
use crate::config::parse_bind_key;

fn mock_app() -> AppState {
    AppState::new("test_session".to_string())
}

fn find(app: &AppState, table: &str, key: KeyCode) -> Option<bool> {
    app.key_tables
        .get(table)
        .and_then(|t| t.iter().find(|b| b.key == (key, KeyModifiers::NONE)))
        .map(|b| b.repeat)
}

#[test]
fn config_attached_table_binds_in_that_table() {
    let mut app = mock_app();
    parse_bind_key(&mut app, "bind -Troot F5 display-message x");
    assert_eq!(find(&app, "root", KeyCode::F(5)), Some(false));
    assert_eq!(find(&app, "prefix", KeyCode::F(5)), None);
}

#[test]
fn config_cluster_with_attached_table() {
    let mut app = mock_app();
    parse_bind_key(&mut app, "bind -rTcopy-mode-vi F4 send-keys -X cancel");
    assert_eq!(find(&app, "copy-mode-vi", KeyCode::F(4)), Some(true));
}

#[test]
fn config_detached_and_cluster_forms_unchanged() {
    let mut app = mock_app();
    parse_bind_key(&mut app, "bind -T root F3 display-message x");
    parse_bind_key(&mut app, "bind -rT root F2 display-message x");
    parse_bind_key(&mut app, "bind -nr F1 display-message x");
    parse_bind_key(&mut app, "bind -r F6 display-message x");
    assert_eq!(find(&app, "root", KeyCode::F(3)), Some(false));
    assert_eq!(find(&app, "root", KeyCode::F(2)), Some(true));
    assert_eq!(find(&app, "root", KeyCode::F(1)), Some(true));
    assert_eq!(find(&app, "prefix", KeyCode::F(6)), Some(true));
}

#[test]
fn config_attached_note_is_read_whole() {
    let mut app = mock_app();
    // The `n` in `note` is part of the value, not -n (root table).
    parse_bind_key(&mut app, "bind -Nnote F7 display-message x");
    assert_eq!(find(&app, "prefix", KeyCode::F(7)), Some(false));
    assert_eq!(find(&app, "root", KeyCode::F(7)), None);
}

#[test]
fn cli_clusters_of_key_commands_are_split() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        crate::cli::split_command_flag_clusters("bind-key", &["-Troot", "F5", "display-message", "-p", "x"]),
        Some(s(&["-T", "root", "F5", "display-message", "-p", "x"]))
    );
    assert_eq!(
        crate::cli::split_command_flag_clusters("bind", &["-rT", "root", "F5", "a"]),
        Some(s(&["-r", "-T", "root", "F5", "a"]))
    );
    assert_eq!(
        crate::cli::split_command_flag_clusters("unbind-key", &["-Troot", "F5"]),
        Some(s(&["-T", "root", "F5"]))
    );
    assert_eq!(
        crate::cli::split_command_flag_clusters("list-keys", &["-Tcopy-mode-vi"]),
        Some(s(&["-T", "copy-mode-vi"]))
    );
    // The bound command after the key is never touched.
    assert_eq!(
        crate::cli::split_command_flag_clusters("bind-key", &["-n", "F5", "split-window", "-hc", "#{pane_current_path}"]),
        None
    );
}
