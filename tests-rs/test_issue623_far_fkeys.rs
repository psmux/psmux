//! Issue #623, third report: in Far Manager 3.0.6364 inside psmux the first
//! F10 inserted a `\` into the command line instead of asking "Do you want to
//! quit Far?", and only the second F10 worked.  The reporter saw the same with
//! Alt+F8, Ctrl+Alt+F8 and F12.  Same report: Ctrl+1 in the drives menu
//! (Alt+F1) still opened the Temporary panel.
//!
//! Measured on 26200 with WriteConsoleInput into the attached client and
//! `capture-pane`, 3 runs each, against the same Far in its own console:
//!
//! ```text
//!   native Far 6364, first F10             quit dialog 3/3, command line empty
//!   psmux a7ab8ea, before any key          command line already ends in `\`
//!   psmux a7ab8ea, first F10               no dialog 0/3 (it closed the
//!                                          autocompletion list the `\` opened)
//!   psmux a7ab8ea, second F10              quit dialog 3/3
//!   psmux a7ab8ea, first F12               no Screens list 0/3
//! ```
//!
//! The bytes psmux wrote to the pane for the first and the second F10 were
//! identical, `1b 5b 32 31 7e`.  The `\` came from somewhere else: Far reads
//! its palette with ONE write of `CSI 0c`, `OSC 4;0;?;4;?;...;255;? ST`,
//! `CSI 0c`, turning VT input on for the read and off after it, and stops at
//! the second DA1 reply.  ConPTY answers both DA1s itself while it processes
//! that write, so Far's read is over before psmux ever sees the OSC.  psmux
//! then injected `ESC ]10;rgb:cccc/cccc/cccc ESC \ ESC ]11;... ESC \ ESC
//! ]4;0;... ESC \` as key records into a console back in `0x01B8`.  Far took
//! them as typing: each ESC cleared the command line and the last ST left its
//! backslash, which opened the autocompletion list that ate the next key.
//! tmux keeps such replies in order (input.c `input_reply` queues a DA answer
//! behind pending palette requests); psmux cannot, because ConPTY owns DA1.
//!
//! Ctrl+1 is a different gate: Far with its mouse support switched off runs in
//! `0x01E8`, which has no `ENABLE_MOUSE_INPUT`, so the Ctrl+digit record path
//! (gated on "mouse set and cooked clear") skipped it and Far got a bare `1`.

use crate::input::{
    ctrl_key_win32_seq, function_key_seq, parse_modified_special_key, win32_input_key_seq,
};
use crate::window_ops::{mode_reads_key_records, mode_reads_vt_replies};

/// Far Manager in its normal loop (mouse support on).
const FAR: u32 = 0x01B8;
/// Far Manager with Options, Interface settings, Mouse switched off.
const FAR_NO_MOUSE: u32 = 0x01E8;
/// Far inside `query_vt`, where `scoped_vt_input` has added VT input.
const FAR_READING_REPLY: u32 = FAR | 0x0200;
/// yazi (crossterm), which also took the injected XTVERSION reply as keys.
const YAZI: u32 = 0x0098;
/// node in raw mode (libuv sets ENABLE_VIRTUAL_TERMINAL_INPUT).
const NODE_RAW: u32 = 0x0208;
/// The mode every pane child inherits.
const INHERITED_DEFAULT: u32 = 0x01F7;
/// pwsh at its prompt (PSReadLine).
const PWSH_PROMPT: u32 = 0x01E4;

#[test]
fn first_and_second_f10_are_the_same_bytes() {
    let first = function_key_seq(10);
    let second = function_key_seq(10);
    assert_eq!(first.as_bytes(), b"\x1b[21~");
    assert_eq!(first, second, "psmux must not encode a repeated F10 differently");
}

#[test]
fn f12_and_the_modified_f8_forms_are_pinned() {
    assert_eq!(function_key_seq(12).as_bytes(), b"\x1b[24~");
    assert_eq!(function_key_seq(1).as_bytes(), b"\x1bOP");
    assert_eq!(function_key_seq(13), "");
    // tmux input-keys.c: modifier parameter 1 + (Shift 1 | Meta 2 | Ctrl 4).
    assert_eq!(parse_modified_special_key("M-F8").as_deref(), Some("\x1b[19;3~"));
    assert_eq!(parse_modified_special_key("C-M-F8").as_deref(), Some("\x1b[19;7~"));
}

#[test]
fn ctrl_1_is_one_win32_press_and_release_with_left_ctrl() {
    // win32-input-mode.md: ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _, one record per
    // key event, the release included.
    let seq = ctrl_key_win32_seq('1', false).expect("Ctrl+1 needs a record");
    assert_eq!(seq, "\x1b[49;2;0;1;8;1_\x1b[49;2;0;0;8;1_");
    assert_eq!(seq, win32_input_key_seq(0x31, 0x02, 0, 0x0008));
}

#[test]
fn far_with_its_mouse_off_still_gets_ctrl_digit_as_a_record() {
    assert!(mode_reads_key_records(FAR));
    assert!(
        mode_reads_key_records(FAR_NO_MOUSE),
        "0x{FAR_NO_MOUSE:04X} reads records; a bare '1' opens the Temporary panel"
    );
    assert!(mode_reads_key_records(YAZI));
    // The mouse gate this path used to share cannot see it, which is the bug.
    assert!(!crate::window_ops::mode_is_deliberate_record_reader(FAR_NO_MOUSE));
}

#[test]
fn shells_and_vt_readers_keep_the_tmux_byte() {
    assert!(!mode_reads_key_records(INHERITED_DEFAULT));
    assert!(!mode_reads_key_records(PWSH_PROMPT));
    assert!(!mode_reads_key_records(NODE_RAW));
}

#[test]
fn a_late_reply_is_withheld_from_a_console_not_reading_vt() {
    assert!(
        !mode_reads_vt_replies(FAR),
        "Far back in 0x{FAR:04X} would type the reply into its command line"
    );
    assert!(!mode_reads_vt_replies(FAR_NO_MOUSE));
    assert!(!mode_reads_vt_replies(YAZI));
    assert!(!mode_reads_vt_replies(PWSH_PROMPT));
}

#[test]
fn a_console_reading_vt_still_gets_its_reply() {
    // #473 (Copilot CLI, node) and #597 (XTVERSION to Claude Code) stay answered.
    assert!(mode_reads_vt_replies(NODE_RAW));
    assert!(mode_reads_vt_replies(FAR_READING_REPLY));
}
