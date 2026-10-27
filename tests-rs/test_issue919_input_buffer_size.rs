// Issue #919: input-buffer-size, tmux's server option for the most bytes one
// input string may hold before it is dropped (options-table.c:434, NUMBER,
// server scope, minimum and default INPUT_BUF_DEFAULT_SIZE = 1048576, maximum
// UINT_MAX).
// When it changes, tmux's options.c:1258 calls input_set_buffer_size, which
// sets the static every pane's input_input reads (input.c:767, :3367).
//
// psmux held a sixel DCS to a fixed 1 MiB in the vt100 parser and had no such
// option: `set -g input-buffer-size N` was kept as text and changed nothing.
// The option now hands its value to vt100::set_input_buffer_size, which the
// parser reads in place of the fixed limit. These tests pin the option side:
// default, range and wording as tmux 3.7c gives them, the parser limit after
// set and after -u, and the config file route. The parser side is covered by
// crates/vt100-psmux/tests/issue919_input_buffer_size.rs.

use crate::server::option_catalog::{default_for, server_option_names, validate_option_value};
use crate::server::options::{apply_set_option, get_option_value, reset_option_to_default};

const DEFAULT: usize = 1 << 20;

fn fresh_app() -> crate::types::AppState {
    crate::types::AppState::new("input_buffer_size".to_string())
}

/// The limit is process wide, so the tests here take turns and each puts the
/// default back when it ends, pass or fail.
struct LimitGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl LimitGuard {
    fn take() -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        vt100::set_input_buffer_size(DEFAULT);
        Self { _lock: lock }
    }
}

impl Drop for LimitGuard {
    fn drop(&mut self) {
        vt100::set_input_buffer_size(DEFAULT);
    }
}

#[test]
fn the_default_is_tmux_input_buf_default_size() {
    let _guard = LimitGuard::take();
    assert_eq!(vt100::DEFAULT_INPUT_BUFFER_SIZE, DEFAULT);
    assert_eq!(default_for("input-buffer-size"), Some("1048576"));
    assert_eq!(get_option_value(&fresh_app(), "input-buffer-size"), "1048576");
}

#[test]
fn it_is_a_server_option() {
    assert!(server_option_names().contains(&"input-buffer-size"));
}

#[test]
fn set_changes_what_show_reads_and_the_parser_limit() {
    let _guard = LimitGuard::take();
    let mut app = fresh_app();
    apply_set_option(&mut app, "input-buffer-size", "2097152", false).unwrap();
    assert_eq!(get_option_value(&app, "input-buffer-size"), "2097152");
    assert_eq!(vt100::input_buffer_size(), 2097152);

    apply_set_option(&mut app, "input-buffer-size", "4294967295", false).unwrap();
    assert_eq!(vt100::input_buffer_size(), 4294967295);
}

#[test]
fn out_of_range_values_are_refused_with_tmux_wording() {
    // tmux 3.7c: `set -g input-buffer-size 1048575` prints
    // "value is too small: 1048575", and 4294967296 prints
    // "value is too large: 4294967296". Neither changes the value.
    let _guard = LimitGuard::take();
    let mut app = fresh_app();
    for (value, wording) in [
        ("100", "value is too small: 100"),
        ("1048575", "value is too small: 1048575"),
        ("4294967296", "value is too large: 4294967296"),
    ] {
        let error = apply_set_option(&mut app, "input-buffer-size", value, false).unwrap_err();
        assert!(error.contains(wording), "{} gave {:?}", value, error);
        assert_eq!(get_option_value(&app, "input-buffer-size"), "1048576");
        assert_eq!(vt100::input_buffer_size(), DEFAULT);
    }
    assert!(validate_option_value("input-buffer-size", "1048576").is_ok());
    assert!(validate_option_value("input-buffer-size", "abc").is_err());
}

#[test]
fn unset_restores_the_default_and_leaves_no_override() {
    let _guard = LimitGuard::take();
    let mut app = fresh_app();
    apply_set_option(&mut app, "input-buffer-size", "8388608", false).unwrap();
    reset_option_to_default(&mut app, "input-buffer-size");
    assert_eq!(get_option_value(&app, "input-buffer-size"), "1048576");
    assert_eq!(vt100::input_buffer_size(), DEFAULT);
    assert!(!app.user_options.contains_key("input-buffer-size"));
}

#[test]
fn a_config_file_line_lands_where_the_runtime_set_does() {
    let _guard = LimitGuard::take();
    let mut app = fresh_app();
    crate::config::parse_config_content(&mut app, "set -g input-buffer-size 3145728\n");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert_eq!(get_option_value(&app, "input-buffer-size"), "3145728");
    assert_eq!(vt100::input_buffer_size(), 3145728);

    // A line out of range warns as tmux does and leaves the limit alone.
    let mut app = fresh_app();
    crate::config::parse_config_content(&mut app, "set -g input-buffer-size 1000\n");
    assert!(
        app.config_warnings.iter().any(|w| w.contains("value is too small: 1000")),
        "{:?}",
        app.config_warnings
    );
    assert_eq!(get_option_value(&app, "input-buffer-size"), "1048576");
    assert_eq!(vt100::input_buffer_size(), 3145728);
}
