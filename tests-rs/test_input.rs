use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

/// Helper: build a KeyEvent with the given code and modifiers.
fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

// ── DECCKM (application cursor keys): arrows/Home/End use SS3 in app mode ──

#[test]
fn cursor_keys_keep_csi_when_app_cursor_off() {
    // Not in application-cursor mode: None => caller writes the CSI form verbatim.
    for csi in [b"\x1b[A", b"\x1b[B", b"\x1b[C", b"\x1b[D", b"\x1b[H", b"\x1b[F"] {
        assert_eq!(csi_cursor_to_ss3(csi, false), None, "app-cursor off must keep CSI");
    }
}

#[test]
fn cursor_keys_become_ss3_when_app_cursor_on() {
    assert_eq!(csi_cursor_to_ss3(b"\x1b[A", true), Some(*b"\x1bOA")); // up
    assert_eq!(csi_cursor_to_ss3(b"\x1b[B", true), Some(*b"\x1bOB")); // down
    assert_eq!(csi_cursor_to_ss3(b"\x1b[C", true), Some(*b"\x1bOC")); // right
    assert_eq!(csi_cursor_to_ss3(b"\x1b[D", true), Some(*b"\x1bOD")); // left
    assert_eq!(csi_cursor_to_ss3(b"\x1b[H", true), Some(*b"\x1bOH")); // home
    assert_eq!(csi_cursor_to_ss3(b"\x1b[F", true), Some(*b"\x1bOF")); // end
}

#[test]
fn non_cursor_and_modified_keys_never_ss3() {
    // App-cursor mode must NOT touch tilde keys or modified arrows: xterm modified
    // cursor keys stay CSI (ESC [ 1 ; mod x) and are longer than 3 bytes.
    assert_eq!(csi_cursor_to_ss3(b"\x1b[5~", true), None);   // PageUp
    assert_eq!(csi_cursor_to_ss3(b"\x1b[3~", true), None);   // Delete
    assert_eq!(csi_cursor_to_ss3(b"\x1b[1;5A", true), None); // Ctrl+Up
    assert_eq!(csi_cursor_to_ss3(b"\x1bOA", true), None);    // already SS3
    assert_eq!(csi_cursor_to_ss3(b"x", true), None);         // not an escape
    assert_eq!(csi_cursor_to_ss3(b"\x1b[Z", true), None);    // BackTab: 3-byte CSI, final byte outside A-D/H/F
    assert_eq!(csi_cursor_to_ss3(b"", true), None);          // empty
    assert_eq!(csi_cursor_to_ss3(b"\x1b[", true), None);     // truncated (len 2)
}

// ── Integration: parser DECCKM state drives the encoder (no pane/PTY/server) ──

#[test]
fn decckm_parser_state_drives_arrow_encoding() {
    // Walk one terminal through the timeline the encoder observes at keypress time.
    let mut term = vt100::Parser::new(24, 80, 0);

    // Fresh screen defaults to off, so Up stays CSI.
    assert!(!term.screen().application_cursor(), "fresh screen defaults to off");
    assert_eq!(csi_cursor_to_ss3(b"\x1b[A", term.screen().application_cursor()), None);

    // PSReadLine enables DECCKM, so Up becomes SS3.
    term.process(b"\x1b[?1h");
    assert!(term.screen().application_cursor());
    assert_eq!(csi_cursor_to_ss3(b"\x1b[A", term.screen().application_cursor()), Some(*b"\x1bOA"));

    // Mode survives later output (the encoder reads it long after the app set it).
    term.process(b"PS C:\\> \x1b[32mgreen\x1b[m");
    assert!(term.screen().application_cursor());

    // Reset restores CSI.
    term.process(b"\x1b[?1l");
    assert!(!term.screen().application_cursor());
    assert_eq!(csi_cursor_to_ss3(b"\x1b[A", term.screen().application_cursor()), None);
}

// ── AltGr characters (Ctrl+Alt on Windows) should be forwarded verbatim ──

#[test]
fn altgr_backslash_german_layout() {
    // German: AltGr+ß → '\'   reported as Ctrl+Alt+'\'
    let ev = key(KeyCode::Char('\\'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\\", "AltGr+backslash must produce literal backslash");
}

#[test]
fn altgr_at_sign_german_layout() {
    // German: AltGr+Q → '@'   reported as Ctrl+Alt+'@'
    let ev = key(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"@", "AltGr+@ must produce literal @");
}

#[test]
fn altgr_open_curly_brace() {
    // German: AltGr+7 → '{'   reported as Ctrl+Alt+'{'
    let ev = key(KeyCode::Char('{'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"{", "AltGr+{{ must produce literal {{");
}

#[test]
fn altgr_close_curly_brace() {
    // German: AltGr+0 → '}'
    let ev = key(KeyCode::Char('}'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"}", "AltGr+}} must produce literal }}");
}

#[test]
fn altgr_open_bracket() {
    // German: AltGr+8 → '['
    let ev = key(KeyCode::Char('['), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"[", "AltGr+[ must produce literal [");
}

#[test]
fn altgr_close_bracket() {
    // German: AltGr+9 → ']'
    let ev = key(KeyCode::Char(']'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"]", "AltGr+] must produce literal ]");
}

#[test]
fn altgr_pipe() {
    // German: AltGr+< → '|'
    let ev = key(KeyCode::Char('|'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"|", "AltGr+| must produce literal |");
}

#[test]
fn altgr_tilde() {
    // German: AltGr++ → '~'
    let ev = key(KeyCode::Char('~'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"~", "AltGr+~ must produce literal ~");
}

#[test]
fn altgr_euro_sign() {
    // German: AltGr+E → '€'   (multi-byte UTF-8)
    let ev = key(KeyCode::Char('€'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, "€".as_bytes(), "AltGr+euro must produce UTF-8 euro sign");
}

#[test]
fn altgr_dollar_czech_layout() {
    // Czech: AltGr produces '$'
    let ev = key(KeyCode::Char('$'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"$", "AltGr+$ must produce literal $");
}

// ── Genuine Ctrl+Alt+letter must still produce ESC + ctrl-char ──

#[test]
fn ctrl_alt_a_is_esc_ctrl_a() {
    let ev = key(KeyCode::Char('a'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, vec![0x1b, 0x01], "Ctrl+Alt+a → ESC + ^A");
}

#[test]
fn ctrl_alt_c_is_esc_ctrl_c() {
    let ev = key(KeyCode::Char('c'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, vec![0x1b, 0x03], "Ctrl+Alt+c → ESC + ^C");
}

#[test]
fn ctrl_alt_z_is_esc_ctrl_z() {
    let ev = key(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, vec![0x1b, 0x1a], "Ctrl+Alt+z → ESC + ^Z");
}

// ── Plain characters / other modifier combos (regression checks) ──

#[test]
fn plain_char_no_modifiers() {
    let ev = key(KeyCode::Char('a'), KeyModifiers::NONE);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"a");
}

#[test]
fn alt_a_produces_esc_a() {
    let ev = key(KeyCode::Char('a'), KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\x1ba");
}

#[test]
fn ctrl_a_produces_soh() {
    let ev = key(KeyCode::Char('a'), KeyModifiers::CONTROL);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, vec![0x01]); // ^A = SOH
}

#[test]
fn ctrl_c_key_event_detects_uppercase_control_c() {
    let ev = key(KeyCode::Char('C'), KeyModifiers::CONTROL);
    assert!(is_ctrl_c_key_event(&ev), "Ctrl+C must be recognized as interrupt key");
}

#[test]
fn ctrl_c_key_event_detects_raw_etx() {
    let ev = key(KeyCode::Char('\u{0003}'), KeyModifiers::NONE);
    assert!(is_ctrl_c_key_event(&ev), "raw ETX (0x03) must be recognized as Ctrl+C");
}

#[test]
fn ctrl_c_key_event_rejects_alt_modified_c() {
    let ev = key(KeyCode::Char('c'), KeyModifiers::CONTROL | KeyModifiers::ALT);
    assert!(!is_ctrl_c_key_event(&ev), "Ctrl+Alt+C must not be treated as plain Ctrl+C");
}

#[test]
fn plain_backslash_no_modifiers() {
    let ev = key(KeyCode::Char('\\'), KeyModifiers::NONE);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\\");
}

// ── Modified Enter key tests (PR #115) ──

#[test]
fn plain_enter_produces_cr() {
    let ev = key(KeyCode::Enter, KeyModifiers::NONE);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\r", "plain Enter must produce CR");
}

#[test]
fn shift_enter_produces_correct_encoding() {
    let ev = key(KeyCode::Enter, KeyModifiers::SHIFT);
    let bytes = encode_key_event(&ev).unwrap();
    #[cfg(windows)]
    assert_eq!(bytes, b"\x1b\r", "Shift+Enter on Windows must produce ESC+CR for ConPTY");
    #[cfg(not(windows))]
    assert_eq!(bytes, b"\x1b[13;2~", "Shift+Enter must produce CSI 13;2~");
}

#[test]
fn ctrl_enter_produces_csi_13_5() {
    let ev = key(KeyCode::Enter, KeyModifiers::CONTROL);
    let bytes = encode_key_event(&ev).unwrap();
    // #409: On Windows, plain Ctrl+Enter is LF (0x0A) to match Windows Terminal's
    // regular input encoder; other platforms keep xterm CSI 13;5~.
    #[cfg(windows)]
    assert_eq!(bytes, b"\n", "Ctrl+Enter on Windows must produce LF");
    #[cfg(not(windows))]
    assert_eq!(bytes, b"\x1b[13;5~", "Ctrl+Enter must produce CSI 13;5~");
}

#[test]
fn ctrl_shift_enter_produces_csi_13_6() {
    let ev = key(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\x1b[13;6~", "Ctrl+Shift+Enter must produce CSI 13;6~");
}

#[test]
fn alt_enter_produces_correct_encoding() {
    let ev = key(KeyCode::Enter, KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    #[cfg(windows)]
    assert_eq!(bytes, b"\x1b\r", "Alt+Enter on Windows must produce ESC+CR for ConPTY");
    #[cfg(not(windows))]
    assert_eq!(bytes, b"\x1b[13;3~", "Alt+Enter must produce CSI 13;3~");
}

// ── parse_modified_special_key tests (PR #115) ──

// `send-keys S-Enter` has to put the same bytes in the pane as a real
// Shift+Enter keystroke does, and on Windows the CSI form puts NOTHING there:
// feeding a real pseudoconsole ESC [ 13;2 ~ and ESC [ 13;3 ~ produced zero
// input records, while 1b 0d produced VK_RETURN with LEFT_ALT_PRESSED
// (issue #611).  So these mirror `encode_key_event` exactly.
#[test]
fn parse_shift_enter() {
    #[cfg(windows)]
    assert_eq!(parse_modified_special_key("S-Enter"), Some("\x1b\r".to_string()));
    #[cfg(not(windows))]
    assert_eq!(parse_modified_special_key("S-Enter"), Some("\x1b[13;2~".to_string()));
}

#[test]
fn parse_ctrl_enter() {
    // #409: plain Ctrl+Enter is LF on Windows, CSI 13;5~ elsewhere.
    #[cfg(windows)]
    assert_eq!(parse_modified_special_key("C-Enter"), Some("\n".to_string()));
    #[cfg(not(windows))]
    assert_eq!(parse_modified_special_key("C-Enter"), Some("\x1b[13;5~".to_string()));
}

#[test]
fn parse_alt_enter() {
    #[cfg(windows)]
    assert_eq!(parse_modified_special_key("M-Enter"), Some("\x1b\r".to_string()));
    #[cfg(not(windows))]
    assert_eq!(parse_modified_special_key("M-Enter"), Some("\x1b[13;3~".to_string()));
}

#[test]
fn parse_ctrl_shift_enter() {
    assert_eq!(parse_modified_special_key("C-S-Enter"), Some("\x1b[13;6~".to_string()));
}

#[test]
fn parse_plain_enter_returns_none() {
    assert_eq!(parse_modified_special_key("enter"), None, "no modifiers should return None");
}

#[test]
fn parse_shift_left_works() {
    // Regression: S-Left was broken because m started at 1 and S- did m|=1 (no-op)
    assert_eq!(parse_modified_special_key("S-Left"), Some("\x1b[1;2D".to_string()));
}

#[test]
fn parse_ctrl_tab_unchanged() {
    assert_eq!(parse_modified_special_key("C-Tab"), Some("\x1b[9;5~".to_string()));
}

#[test]
fn parse_ctrl_left_unchanged() {
    assert_eq!(parse_modified_special_key("C-Left"), Some("\x1b[1;5D".to_string()));
}

// ── PR #131: paste line-ending normalization tests ──

/// Helper: capture what write_paste_chunked writes to a Vec<u8>.
fn capture_paste(text: &[u8], bracket: bool) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    super::write_paste_chunked(&mut buf, text, bracket);
    buf
}

#[test]
fn paste_lf_normalized_to_cr() {
    // Multi-line paste with LF line endings should produce CR
    let input = b"line1\nline2\nline3";
    let output = capture_paste(input, false);
    assert_eq!(output, b"line1\rline2\rline3",
        "bare LF must be normalized to CR for ConPTY; got {:?}", String::from_utf8_lossy(&output));
}

#[test]
fn paste_crlf_normalized_to_cr() {
    // Multi-line paste with CRLF line endings should produce CR (not CRLF)
    let input = b"line1\r\nline2\r\nline3";
    let output = capture_paste(input, false);
    assert_eq!(output, b"line1\rline2\rline3",
        "CRLF must be normalized to CR for ConPTY; got {:?}", String::from_utf8_lossy(&output));
}

#[test]
fn paste_mixed_endings_normalized() {
    // Mixed: some lines LF, some CRLF
    let input = b"a\nb\r\nc";
    let output = capture_paste(input, false);
    assert_eq!(output, b"a\rb\rc",
        "mixed line endings must all become CR; got {:?}", String::from_utf8_lossy(&output));
}

#[test]
fn paste_no_line_endings_unchanged() {
    // Text without newlines should pass through unchanged
    let input = b"hello world";
    let output = capture_paste(input, false);
    assert_eq!(output, b"hello world");
}

#[test]
fn paste_bracket_markers_with_normalization() {
    // Bracketed paste should still wrap with markers AND normalize
    let input = b"a\nb";
    let output = capture_paste(input, true);
    assert_eq!(output, b"\x1b[200~a\rb\x1b[201~",
        "bracketed paste must normalize line endings; got {:?}", String::from_utf8_lossy(&output));
}

// ── PR #132: Shift+Enter ConPTY encoding tests ──

#[cfg(windows)]
#[test]
fn shift_enter_encoding_for_conpty() {
    // On Windows, Shift+Enter should produce \x1b\r (ESC+CR) instead of
    // \x1b[13;2~ which ConPTY drops (code 13 is non-standard).
    let ev = key(KeyCode::Enter, KeyModifiers::SHIFT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\x1b\r",
        "Shift+Enter on Windows must produce ESC+CR for ConPTY compatibility; got {:?}", bytes);
}

#[cfg(windows)]
#[test]
fn alt_enter_encoding_for_conpty() {
    // Alt+Enter should also produce \x1b\r on Windows
    let ev = key(KeyCode::Enter, KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\x1b\r",
        "Alt+Enter on Windows must produce ESC+CR for ConPTY; got {:?}", bytes);
}

// ── Issue #121 (whil0012 follow-up): PSReadLine Shift+Enter via native injection ──

/// augment_enter_shift must remap Alt+Enter → Shift+Enter when physical Shift
/// is held (ConPTY misreports Shift+Enter as Alt+Enter).
#[cfg(windows)]
#[test]
fn augment_enter_shift_noop_when_already_shift() {
    use crossterm::event::KeyModifiers;
    let mut ev = key(KeyCode::Enter, KeyModifiers::SHIFT);
    crate::platform::augment_enter_shift(&mut ev);
    assert!(ev.modifiers.contains(KeyModifiers::SHIFT),
        "augment_enter_shift must preserve existing SHIFT modifier");
}

#[cfg(windows)]
#[test]
fn augment_enter_shift_ignores_non_enter() {
    use crossterm::event::KeyModifiers;
    let mut ev = key(KeyCode::Char('a'), KeyModifiers::ALT);
    crate::platform::augment_enter_shift(&mut ev);
    assert!(ev.modifiers.contains(KeyModifiers::ALT),
        "augment_enter_shift must not change non-Enter keys");
    assert!(!ev.modifiers.contains(KeyModifiers::SHIFT),
        "augment_enter_shift must not add SHIFT to non-Enter keys");
}

/// Issue #121 follow-up Bug #3: Shift/Alt+Enter (no Ctrl) must use VT encoding
/// only; Ctrl combos should still use CSI encoding (and native injection in the
/// live code path).  Verify encode_key_event produces the right sequences.
#[cfg(windows)]
#[test]
fn shift_enter_no_ctrl_uses_vt_not_csi() {
    // Shift+Enter → \x1b\r (VT), NOT \x1b[13;2~ (CSI)
    let ev = key(KeyCode::Enter, KeyModifiers::SHIFT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\x1b\r",
        "Shift+Enter (no Ctrl) on Windows must use VT encoding (ESC+CR); got {:?}", bytes);
}

#[cfg(windows)]
#[test]
fn alt_enter_no_ctrl_uses_vt_not_csi() {
    // Alt+Enter → \x1b\r (VT), NOT \x1b[13;3~ (CSI)
    let ev = key(KeyCode::Enter, KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\x1b\r",
        "Alt+Enter (no Ctrl) on Windows must use VT encoding (ESC+CR); got {:?}", bytes);
}

#[test]
fn ctrl_enter_uses_platform_encoding() {
    // #409: plain Ctrl+Enter is LF on Windows Terminal, CSI 13;5~ elsewhere.
    let ev = key(KeyCode::Enter, KeyModifiers::CONTROL);
    let bytes = encode_key_event(&ev).unwrap();
    #[cfg(windows)]
    assert_eq!(bytes, b"\n", "Ctrl+Enter must use LF on Windows; got {:?}", bytes);
    #[cfg(not(windows))]
    assert_eq!(bytes, b"\x1b[13;5~",
        "Ctrl+Enter must use CSI encoding; got {:?}", bytes);
}

/// VT fallback encoding for modified Enter still works (encode_key_event path).
#[test]
fn ctrl_shift_enter_vt_encoding_works() {
    let ev = key(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\x1b[13;6~",
        "Ctrl+Shift+Enter VT encoding must be CSI 13;6~");
}

#[test]
fn ctrl_alt_enter_vt_encoding_works() {
    let ev = key(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    assert_eq!(bytes, b"\x1b[13;7~",
        "Ctrl+Alt+Enter VT encoding must be CSI 13;7~");
}

#[test]
fn shift_alt_enter_on_non_windows_produces_csi() {
    // On non-Windows, Shift+Alt+Enter should use CSI encoding
    let ev = key(KeyCode::Enter, KeyModifiers::SHIFT | KeyModifiers::ALT);
    let bytes = encode_key_event(&ev).unwrap();
    #[cfg(windows)]
    assert_eq!(bytes, b"\x1b\r", "Shift+Alt+Enter on Windows → ESC+CR");
    #[cfg(not(windows))]
    assert_eq!(bytes, b"\x1b[13;4~", "Shift+Alt+Enter on non-Windows → CSI 13;4~");
}

/// Issue #121 Bug #3 double-delivery proof: verify that VT-encoded Shift+Enter
/// is distinct from plain CR (which is what native WriteConsoleInputW injection
/// produces after ConPTY translation).  Before the fix, forward_key_to_active
/// sent BOTH \x1b\r (VT) and a native VK_RETURN injection for Shift+Enter,
/// causing the child process to receive two Enter events.  After the fix,
/// only VT encoding is used for Shift/Alt+Enter (no Ctrl), preventing double
/// delivery.  Plain Ctrl+Enter uses native injection with an LF fallback (#409).
#[cfg(windows)]
#[test]
fn bug3_double_delivery_prevention() {
    // Native injection produces a KEY_EVENT_RECORD → ConPTY translates to \r.
    // VT encoding for Shift+Enter is \x1b\r (ESC + CR).
    // If both paths fire, child sees: ESC + CR (VT) + CR (native) = 2 Enters.
    // The fix ensures only ONE path fires for each modifier combination.

    let shift_enter = key(KeyCode::Enter, KeyModifiers::SHIFT);
    let alt_enter = key(KeyCode::Enter, KeyModifiers::ALT);
    let ctrl_enter = key(KeyCode::Enter, KeyModifiers::CONTROL);

    let shift_bytes = encode_key_event(&shift_enter).unwrap();
    let alt_bytes = encode_key_event(&alt_enter).unwrap();
    let ctrl_bytes = encode_key_event(&ctrl_enter).unwrap();

    // VT path (Shift/Alt+Enter): produces \x1b\r
    assert_eq!(shift_bytes, b"\x1b\r");
    assert_eq!(alt_bytes, b"\x1b\r");

    // Plain Ctrl+Enter (#409): LF byte payload.  The live Windows path injects a
    // VK_RETURN KEY_EVENT with this same LF payload; encode_key_event is the byte
    // fallback.  LF stays distinct from Shift/Alt+Enter's ESC+CR, so no combination
    // collapses into a plain CR double-delivery.
    assert_eq!(ctrl_bytes, b"\n");

    // The critical guard in forward_key_to_active:
    //   let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    //   if ctrl { /* native injection */ }
    //   // else: fall through to encode_key_event (VT)
    assert!(!shift_enter.modifiers.contains(KeyModifiers::CONTROL),
        "Shift+Enter must NOT trigger the ctrl guard (no native injection)");
    assert!(!alt_enter.modifiers.contains(KeyModifiers::CONTROL),
        "Alt+Enter must NOT trigger the ctrl guard (no native injection)");
    assert!(ctrl_enter.modifiers.contains(KeyModifiers::CONTROL),
        "Ctrl+Enter MUST trigger the ctrl guard (native injection allowed)");
}

// ── Issue #134: wrapped directional navigation geometry tests ──

/// Build a two-pane horizontal layout (left | right) for geometry tests.
fn two_pane_h_rects() -> Vec<(Vec<usize>, ratatui::layout::Rect)> {
    use ratatui::layout::Rect;
    vec![
        (vec![0], Rect { x: 0,  y: 0, width: 40, height: 24 }), // left
        (vec![1], Rect { x: 40, y: 0, width: 40, height: 24 }), // right
    ]
}

#[test]
fn issue134_wrap_right_from_rightmost_pane() {
    // From the rightmost pane (index 1), going Right should find no direct
    // neighbor but find a wrap target (the leftmost pane, index 0).
    let rects = two_pane_h_rects();
    let ai = 1; // rightmost pane
    let arect = &rects[ai].1;
    let direct = find_best_pane_in_direction(
        &rects, ai, arect, crate::types::FocusDir::Right, &[], &[],
    );
    assert!(direct.is_none(), "rightmost pane should have no direct Right neighbor");
    let wrap = find_wrap_target(
        &rects, ai, arect, crate::types::FocusDir::Right, &[], &[],
    );
    assert_eq!(wrap, Some(0), "wrap Right from rightmost should reach leftmost (index 0)");
}

#[test]
fn issue134_wrap_left_from_leftmost_pane() {
    let rects = two_pane_h_rects();
    let ai = 0; // leftmost pane
    let arect = &rects[ai].1;
    let direct = find_best_pane_in_direction(
        &rects, ai, arect, crate::types::FocusDir::Left, &[], &[],
    );
    assert!(direct.is_none(), "leftmost pane should have no direct Left neighbor");
    let wrap = find_wrap_target(
        &rects, ai, arect, crate::types::FocusDir::Left, &[], &[],
    );
    assert_eq!(wrap, Some(1), "wrap Left from leftmost should reach rightmost (index 1)");
}

#[test]
fn issue134_direct_neighbor_takes_priority_over_wrap() {
    // From left pane (index 0), going Right should find a direct neighbor (index 1),
    // ensuring wrap is NOT used when a direct neighbor exists.
    let rects = two_pane_h_rects();
    let ai = 0;
    let arect = &rects[ai].1;
    let direct = find_best_pane_in_direction(
        &rects, ai, arect, crate::types::FocusDir::Right, &[], &[],
    );
    assert_eq!(direct, Some(1), "left pane should have direct Right neighbor (right pane)");
}

// ── Issue #141: wrapped nav must not jump columns/rows ──

/// Build a three-pane horizontal layout (left | center | right) for issue #141.
fn three_pane_h_rects() -> Vec<(Vec<usize>, ratatui::layout::Rect)> {
    use ratatui::layout::Rect;
    vec![
        (vec![0], Rect { x: 0,  y: 0, width: 60, height: 30 }), // %1 left
        (vec![1], Rect { x: 61, y: 0, width: 29, height: 30 }), // %2 center
        (vec![2], Rect { x: 91, y: 0, width: 30, height: 30 }), // %3 right
    ]
}

#[test]
fn issue141_wrap_up_single_row_stays_on_self() {
    // Three panes in a single row. From %2 (center), select-pane -U should
    // not jump to %3 or %1. There is no pane above or below, so wrapping
    // should return None (stay on the current pane).
    let rects = three_pane_h_rects();
    let ai = 1; // center pane
    let arect = &rects[ai].1;
    let direct = find_best_pane_in_direction(
        &rects, ai, arect, crate::types::FocusDir::Up, &[], &[],
    );
    assert!(direct.is_none(), "no pane above center in single row");
    let wrap = find_wrap_target(
        &rects, ai, arect, crate::types::FocusDir::Up, &[], &[],
    );
    assert!(wrap.is_none(), "wrap Up in single row must not jump columns (issue #141)");
}

#[test]
fn issue141_wrap_down_single_row_stays_on_self() {
    let rects = three_pane_h_rects();
    let ai = 1;
    let arect = &rects[ai].1;
    let direct = find_best_pane_in_direction(
        &rects, ai, arect, crate::types::FocusDir::Down, &[], &[],
    );
    assert!(direct.is_none(), "no pane below center in single row");
    let wrap = find_wrap_target(
        &rects, ai, arect, crate::types::FocusDir::Down, &[], &[],
    );
    assert!(wrap.is_none(), "wrap Down in single row must not jump columns (issue #141)");
}

/// Build a three-pane vertical layout (top / middle / bottom) for issue #141.
fn three_pane_v_rects() -> Vec<(Vec<usize>, ratatui::layout::Rect)> {
    use ratatui::layout::Rect;
    vec![
        (vec![0], Rect { x: 0, y: 0,  width: 80, height: 10 }), // top
        (vec![1], Rect { x: 0, y: 11, width: 80, height: 10 }), // middle
        (vec![2], Rect { x: 0, y: 22, width: 80, height: 10 }), // bottom
    ]
}

#[test]
fn issue141_wrap_left_single_column_stays_on_self() {
    // Three panes stacked vertically. From middle, select-pane -L should
    // stay on self since there are no panes to the left or right.
    let rects = three_pane_v_rects();
    let ai = 1;
    let arect = &rects[ai].1;
    let direct = find_best_pane_in_direction(
        &rects, ai, arect, crate::types::FocusDir::Left, &[], &[],
    );
    assert!(direct.is_none(), "no pane left of middle in single column");
    let wrap = find_wrap_target(
        &rects, ai, arect, crate::types::FocusDir::Left, &[], &[],
    );
    assert!(wrap.is_none(), "wrap Left in single column must not jump rows (issue #141)");
}

#[test]
fn issue141_wrap_right_single_column_stays_on_self() {
    let rects = three_pane_v_rects();
    let ai = 1;
    let arect = &rects[ai].1;
    let direct = find_best_pane_in_direction(
        &rects, ai, arect, crate::types::FocusDir::Right, &[], &[],
    );
    assert!(direct.is_none(), "no pane right of middle in single column");
    let wrap = find_wrap_target(
        &rects, ai, arect, crate::types::FocusDir::Right, &[], &[],
    );
    assert!(wrap.is_none(), "wrap Right in single column must not jump rows (issue #141)");
}

#[test]
fn issue141_wrap_up_still_works_with_column_overlap() {
    // Two panes stacked vertically. Wrap Up from bottom should still reach top
    // because they overlap on the perpendicular (x) axis.
    use ratatui::layout::Rect;
    let rects: Vec<(Vec<usize>, Rect)> = vec![
        (vec![0], Rect { x: 0, y: 0,  width: 80, height: 12 }),
        (vec![1], Rect { x: 0, y: 13, width: 80, height: 12 }),
    ];
    let ai = 0; // top pane
    let arect = &rects[ai].1;
    let wrap = find_wrap_target(
        &rects, ai, arect, crate::types::FocusDir::Up, &[], &[],
    );
    assert_eq!(wrap, Some(1), "wrap Up from top should reach bottom when they share a column");
}
