// Issue #1049: over ssh, holding `j` in Neovim's normal mode now and then typed
// `jjjj` into the buffer. The keyboard repeats a held key evenly, but the
// network hands several repeats over in one read when it stalls, and the
// client took three or more characters inside its 20 ms window for a paste and
// sent them to the pane bracketed. Neovim inserts a bracketed paste in any mode.
//
// A burst that is one key repeated is now forwarded as keys, unless a Ctrl+V
// press is in flight or the clipboard holds the same run.
use super::*;

fn flush(text: &str, stage2: bool, gesture: &mut PasteGesture) -> Vec<String> {
    let mut buf = String::from(text);
    let mut start = Some(std::time::Instant::now());
    let mut st2 = stage2;
    let mut cmds = Vec::new();
    flush_paste_pend_as_text(&mut buf, &mut start, &mut st2, &mut cmds, gesture);
    assert!(buf.is_empty() && start.is_none() && !st2, "the flush must reset the pending state");
    cmds
}

#[test]
fn a_held_key_cut_short_by_another_key_goes_out_as_keys() {
    // `jjjj` then Esc inside the window: the four repeats are motions.
    let cmds = flush("jjjj", false, &mut PasteGesture::default());
    assert_eq!(cmds, vec!["send-text \"j\"\n"; 4], "a held key must not become a send-paste");
}

#[test]
fn a_held_space_goes_out_as_spaces() {
    let cmds = flush("    ", false, &mut PasteGesture::default());
    assert_eq!(cmds, vec!["send-key space\n"; 4]);
}

#[test]
fn a_repeated_run_with_a_ctrl_v_press_in_flight_is_still_a_paste() {
    let mut g = PasteGesture::default();
    g.start();
    let cmds = flush("----------", false, &mut g);
    assert_eq!(cmds.len(), 1);
    assert!(cmds[0].starts_with("send-paste "), "got {:?}", cmds);
}

#[test]
fn a_repeated_run_already_in_stage_two_is_still_a_paste() {
    let cmds = flush("jjjj", true, &mut PasteGesture::default());
    assert_eq!(cmds.len(), 1);
    assert!(cmds[0].starts_with("send-paste "));
}

#[test]
fn a_burst_of_different_characters_is_unchanged() {
    // Out of scope for #1049 and kept as before: a mixed burst is a paste.
    let cmds = flush("abc", false, &mut PasteGesture::default());
    assert_eq!(cmds.len(), 1);
    assert!(cmds[0].starts_with("send-paste "));
}

#[test]
fn one_key_repeated_shape() {
    assert!(is_one_key_repeated("jjjj"));
    assert!(is_one_key_repeated("j"));
    assert!(!is_one_key_repeated(""));
    assert!(!is_one_key_repeated("jjjk"));
    assert!(!is_one_key_repeated("kjjj"));
    assert!(is_one_key_repeated("ééé"));
}

#[test]
fn the_clipboard_decides_for_a_repeated_run() {
    // No gesture, nothing on the clipboard: a held key.
    assert!(!repeated_key_is_a_paste("jjjj", false, None));
    // The clipboard holds something else: a held key.
    assert!(!repeated_key_is_a_paste("jjjj", false, Some("hello world")));
    // The clipboard holds the run, or the run is its head, or the run goes
    // past the 64 character head: a paste.
    assert!(repeated_key_is_a_paste("----", false, Some("----")));
    assert!(repeated_key_is_a_paste("----", false, Some("----------\nnext line")));
    let long = "-".repeat(100);
    let head: String = long.chars().take(ClipboardHeadCache::HEAD_CHARS).collect();
    assert!(repeated_key_is_a_paste(&long, false, Some(&head)));
    // A clipboard of one or two characters does not turn a held key into a
    // paste: `jj` on the clipboard and `j` held is still the key.
    assert!(!repeated_key_is_a_paste("jjjj", false, Some("jj")));
    // A Ctrl+V press in flight wins over the clipboard.
    assert!(repeated_key_is_a_paste("jjjj", true, None));
}
