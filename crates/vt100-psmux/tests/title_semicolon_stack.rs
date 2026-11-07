// Titles from OSC 0 and OSC 2 keep every `;` after the first (input_exit_osc,
// input.c:2714-2731 at tag 3.8), and CSI 22 ; 0 t / CSI 23 ; 0 t save and
// restore the title on a stack of at most ten (input.c:2187-2209, screen.c:
// 292-337). vt100 dropped a title holding a `;` (vte splits the string on
// every `;`) and ignored the stack. Every expectation is what tmux 3.8 showed
// for the same bytes.

use vt100_psmux as vt100;

fn parser(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(5, 40, 0);
    parser.process(bytes.as_bytes());
    parser
}

fn title(bytes: &str) -> String {
    parser(bytes).screen().title().to_string()
}

const PUSH: &str = "\x1b[22;0t";
const POP: &str = "\x1b[23;0t";

fn set(t: &str) -> String {
    format!("\x1b]2;{}\x07", t)
}

#[test]
fn a_title_keeps_its_semicolons() {
    assert_eq!(title("\x1b]0;a;b\x07"), "a;b");
    assert_eq!(title("\x1b]2;x;y;z\x1b\\"), "x;y;z");
    assert_eq!(title("\x1b]2;x;\x07"), "x;");
    assert_eq!(title("\x1b]2;;lead\x07"), ";lead");
    assert_eq!(title("\x1b]2;plain\x07"), "plain");
}

#[test]
fn the_icon_name_alone_leaves_the_title() {
    assert_eq!(title(&format!("{}\x1b]1;a;b\x07", set("kept"))), "kept");
}

#[test]
fn save_and_restore_the_title() {
    assert_eq!(title(&format!("{}{}{}{}", set("outer"), PUSH, set("inner"), POP)), "outer");
    assert_eq!(title(&format!("{}\x1b[22;2t{}\x1b[23;2t", set("outer"), set("inner"))), "outer");
    // Two deep.
    let two = format!("{}{}{}{}{}{}", set("a"), PUSH, set("b"), PUSH, set("c"), POP);
    assert_eq!(title(&two), "b");
    assert_eq!(title(&format!("{}{}", two, POP)), "a");
}

#[test]
fn the_icon_name_or_no_second_parameter_does_nothing() {
    assert_eq!(title(&format!("{}\x1b[22;1t{}\x1b[23;1t", set("outer"), set("inner"))), "inner");
    assert_eq!(title(&format!("{}\x1b[22t{}\x1b[23t", set("outer"), set("inner"))), "inner");
}

#[test]
fn restoring_with_nothing_saved_keeps_the_title() {
    assert_eq!(title(&format!("{}{}", set("only"), POP)), "only");
}

#[test]
fn ten_titles_are_kept_and_the_oldest_goes() {
    let mut s = String::new();
    for i in 0..=10 {
        s.push_str(&set(&format!("t{}", i)));
        s.push_str(PUSH);
    }
    s.push_str(&set("t11"));
    for _ in 0..=10 {
        s.push_str(POP);
    }
    assert_eq!(title(&s), "t1");
}

#[test]
fn restoring_the_title_saved_before_any_asks_for_the_first_one_once() {
    let mut p = parser(&format!("{}{}{}", PUSH, set("app"), POP));
    assert_eq!(p.screen().title(), "");
    assert!(p.screen_mut().take_title_reset());
    assert!(!p.screen_mut().take_title_reset());
    // A title restored over it clears the request.
    let mut p = parser(&format!("{}{}{}{}{}", set("x"), PUSH, PUSH, set("y"), POP));
    assert_eq!(p.screen().title(), "x");
    assert!(!p.screen_mut().take_title_reset());
}

#[test]
fn ris_keeps_the_title_and_the_saved_ones() {
    // tmux's RIS goes through screen_write_reset, which touches neither.
    assert_eq!(title(&format!("{}\x1bc", set("kept"))), "kept");
    assert_eq!(title(&format!("{}{}{}\x1bc{}", set("outer"), PUSH, set("inner"), POP)), "outer");
    assert_eq!(title(&format!("{}\x1bc{}{}{}", set("a"), PUSH, set("b"), POP)), "a");
}
