// Issue #792: display-popup frame parity with tmux.
//
// Before the fix the attached client drew every popup with a yellow plain
// frame titled with the command, never read popup-border-style,
// popup-border-lines or popup-style, and the server's display-popup arm only
// knew -w -h -d/-c -E -K, so `-T title`, `-B`, `-b`, `-S` and `-s` were glued
// onto the command string. Now one parser, one frame resolver and one drawing
// routine (all in popup.rs) serve the server, the attached client and the
// in-process renderer. These tests pin each piece.
//
// No test here starts a process: every display-popup below has no command, and
// the command.rs path only spawns a PTY when a command is given.

use super::*;
use crate::commands::execute_command_string;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;

fn args(s: &str) -> Vec<String> {
    s.split_whitespace().map(|w| w.to_string()).collect()
}

fn parse(s: &str) -> DisplayPopupArgs {
    parse_display_popup_args(&args(s)).expect("parse")
}

fn mock_app() -> AppState {
    let mut app = AppState::new("i792".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn look_of(app: &AppState) -> PopupLook {
    match &app.mode {
        Mode::PopupMode { look, .. } => look.clone(),
        _ => panic!("expected PopupMode"),
    }
}

fn row(buf: &Buffer, y: u16, x0: u16, x1: u16) -> String {
    (x0..x1).map(|x| buf[(x, y)].symbol().to_string()).collect()
}

// ── Parsing ────────────────────────────────────────────────────────────────

#[test]
fn title_flag_is_parsed_not_part_of_command() {
    let p = parse("-w 40 -h 10 -T MyTitle ping -n 6 127.0.0.1");
    assert_eq!(p.title.as_deref(), Some("MyTitle"));
    assert_eq!(p.command, "ping -n 6 127.0.0.1");
    assert_eq!(p.width.as_deref(), Some("40"));
    assert_eq!(p.height.as_deref(), Some("10"));
}

#[test]
fn no_border_flag_is_parsed_not_part_of_command() {
    let p = parse("-B -E lazygit");
    assert!(p.no_border);
    assert_eq!(p.close_on_exit, 1);
    assert_eq!(p.command, "lazygit");
}

#[test]
fn border_lines_and_styles_are_parsed() {
    let p = parse("-b double -S fg=red -s bg=green htop");
    assert_eq!(p.border_lines.as_deref(), Some("double"));
    assert_eq!(p.border_style.as_deref(), Some("fg=red"));
    assert_eq!(p.style.as_deref(), Some("bg=green"));
    assert_eq!(p.command, "htop");
}

#[test]
fn clustered_and_attached_values_parse_like_getopt() {
    let p = parse("-BE -TTitle -w50 cmd");
    assert!(p.no_border);
    assert_eq!(p.close_on_exit, 1);
    assert_eq!(p.title.as_deref(), Some("Title"));
    assert_eq!(p.width.as_deref(), Some("50"));
    assert_eq!(p.command, "cmd");
    let p = parse("-EE cmd");
    assert_eq!(p.close_on_exit, 2);
}

#[test]
fn flags_after_the_command_belong_to_the_command() {
    // tmux getopt stops at the first non flag word.
    let p = parse("-E ping -w 5 -h host");
    assert_eq!(p.command, "ping -w 5 -h host");
    assert_eq!(p.width, None);
    assert_eq!(p.height, None);
    let p = parse("-E -- -T notatitle");
    assert_eq!(p.title, None);
    // After `--` the words are an argument vector, carried in the argv form
    // build_command runs directly, as tmux's job_run execvp's it (#794).
    assert_eq!(p.command, crate::pane::encode_argv_form(&["-T", "notatitle"]));
}

#[test]
fn target_position_and_env_flags_never_reach_the_command() {
    let p = parse("-t %1 -x 3 -y 4 -e FOO=bar -c C:\\dir -k -N cmd");
    assert_eq!(p.command, "cmd");
    assert_eq!(p.env, vec![("FOO".to_string(), "bar".to_string())]);
    assert_eq!(p.start_dir.as_deref(), Some("C:\\dir"));
}

#[test]
fn close_flag_and_keep_open_flag() {
    assert!(parse("-C").close_existing);
    assert!(parse("-K cmd").keep_open);
}

#[test]
fn bad_border_lines_and_dangling_values_are_errors() {
    let e = parse_display_popup_args(&args("-b wobbly cmd")).unwrap_err();
    assert_eq!(e, "popup-border-lines unknown value: wobbly");
    let e = parse_display_popup_args(&args("-T")).unwrap_err();
    assert_eq!(e, "-T expects an argument");
    for v in POPUP_BORDER_LINES {
        assert_eq!(parse(&format!("-b {} x", v)).border_lines.as_deref(), Some(v));
    }
}

// ── Resolution ─────────────────────────────────────────────────────────────

#[test]
fn border_lines_precedence_no_border_then_flag_then_option() {
    assert_eq!(resolve_popup_border_lines(true, Some("double"), Some("rounded")), "none");
    assert_eq!(resolve_popup_border_lines(false, Some("double"), Some("rounded")), "double");
    assert_eq!(resolve_popup_border_lines(false, None, Some("rounded")), "rounded");
    assert_eq!(resolve_popup_border_lines(false, None, None), "single");
    assert_eq!(resolve_popup_border_lines(false, None, Some("bogus")), "single");
}

#[test]
fn content_size_is_full_without_border_and_two_less_with_one() {
    assert_eq!(popup_content_size(40, 10, "none"), (40, 10));
    for l in ["single", "double", "heavy", "simple", "rounded", "padded"] {
        assert_eq!(popup_content_size(40, 10, l), (38, 8), "{}", l);
    }
}

#[test]
fn style_flags_win_over_options_and_unset_border_is_yellow() {
    let none = PopupLook::default();
    assert_eq!(popup_effective_styles(&none, None, None), ("fg=yellow".to_string(), String::new()));
    assert_eq!(
        popup_effective_styles(&none, Some("fg=#3b434d"), Some("bg=blue")),
        ("fg=#3b434d".to_string(), "bg=blue".to_string())
    );
    let flags = PopupLook { border_style: Some("fg=red".into()), style: Some("bg=green".into()), ..Default::default() };
    let (bs, s) = popup_effective_styles(&flags, Some("fg=#3b434d"), Some("bg=blue"));
    let f = PopupFrame::new("", "single", &bs, &s);
    assert_eq!(f.border_style.fg, Some(Color::Red));
    assert_eq!(f.style.bg, Some(Color::Green));
    // -S names only fg, so the option's bg survives (tmux style_parse over
    // the option cell).
    let (bs, _) = popup_effective_styles(&flags, Some("fg=blue,bg=white"), None);
    let f = PopupFrame::new("", "single", &bs, "");
    assert_eq!(f.border_style.fg, Some(Color::Red));
    assert_eq!(f.border_style.bg, Some(Color::White));
}

#[test]
fn frame_keeps_colours_only() {
    let f = PopupFrame::new("t", "rounded", "fg=#3b434d,bold", "bg=blue,italics");
    assert_eq!(f.border_style, Style { fg: Some(Color::Rgb(0x3b, 0x43, 0x4d)), ..Default::default() });
    assert_eq!(f.style, Style { bg: Some(Color::Blue), ..Default::default() });
    assert!(!f.border_style.add_modifier.contains(Modifier::BOLD));
}

// ── Drawing (shared by the attached client and the in-process renderer) ───

fn draw(frame: &PopupFrame, content: &str, w: u16, h: u16) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, w + 4, h + 4));
    draw_popup(&mut buf, Rect::new(2, 2, w, h), frame, Text::from(content.to_string()), 0);
    buf
}

#[test]
fn rounded_frame_with_option_colour_and_no_title() {
    let f = PopupFrame::new("", "rounded", "fg=#3b434d", "");
    let buf = draw(&f, "body", 12, 4);
    assert_eq!(row(&buf, 2, 2, 14), "╭──────────╮");
    assert_eq!(row(&buf, 5, 2, 14), "╰──────────╯");
    assert_eq!(row(&buf, 3, 2, 8), "│body ");
    assert_eq!(buf[(2, 2)].fg, Color::Rgb(0x3b, 0x43, 0x4d));
}

#[test]
fn title_starts_two_cells_in_and_is_cut_to_width_minus_four() {
    let f = PopupFrame::new("MyTitle", "single", "fg=yellow", "");
    let buf = draw(&f, "", 12, 3);
    assert_eq!(row(&buf, 2, 2, 14), "┌─MyTitle──┐");
    let f = PopupFrame::new("ABCDEFGHIJKLMNOP", "single", "fg=yellow", "");
    let buf = draw(&f, "", 12, 3);
    assert_eq!(row(&buf, 2, 2, 14), "┌─ABCDEFGH─┐");
}

#[test]
fn every_line_set_draws_its_glyphs() {
    let cases = [
        ("single", "┌──┐"),
        ("double", "╔══╗"),
        ("heavy", "┏━━┓"),
        ("rounded", "╭──╮"),
        ("simple", "+--+"),
        ("padded", "    "),
    ];
    for (lines, top) in cases {
        let buf = draw(&PopupFrame::new("", lines, "fg=yellow", ""), "", 4, 3);
        assert_eq!(row(&buf, 2, 2, 6), top, "{}", lines);
    }
}

#[test]
fn no_border_gives_content_the_whole_area() {
    let f = PopupFrame::new("ignored", "none", "fg=yellow", "");
    let buf = draw(&f, "ABCDEFGHIJ\nrow2", 10, 3);
    assert_eq!(row(&buf, 2, 2, 12), "ABCDEFGHIJ");
    assert_eq!(row(&buf, 3, 2, 6), "row2");
    assert_eq!(popup_inner_rect(Rect::new(2, 2, 10, 3), false), Rect::new(2, 2, 10, 3));
    assert_eq!(popup_inner_rect(Rect::new(2, 2, 10, 3), true), Rect::new(3, 3, 8, 1));
}

#[test]
fn popup_style_colours_default_cells_only() {
    let f = PopupFrame::new("", "single", "fg=yellow", "bg=blue,fg=white");
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
    let content = Text::from(vec![ratatui::text::Line::from(vec![
        ratatui::text::Span::raw("a"),
        ratatui::text::Span::styled("b", Style::default().fg(Color::Red).bg(Color::Black)),
    ])]);
    draw_popup(&mut buf, Rect::new(0, 0, 6, 3), &f, content, 0);
    assert_eq!((buf[(1, 1)].fg, buf[(1, 1)].bg), (Color::White, Color::Blue), "default cell takes popup-style");
    assert_eq!((buf[(2, 1)].fg, buf[(2, 1)].bg), (Color::Red, Color::Black), "explicit colours stay");
    assert_eq!(buf[(4, 1)].bg, Color::Blue, "empty content cell is filled too");
    assert_eq!(buf[(0, 0)].bg, Color::Reset, "the frame is not popup-style");
}

#[test]
fn client_cursor_follows_the_frame() {
    let area = Rect::new(0, 0, 100, 30);
    let r = popup_rect(area, 40, 10);
    let framed = crate::client::popup_cursor_screen_pos_framed(area, 40, 10, 0, (0, 0), true).unwrap();
    assert_eq!(framed, (r.x + 1, r.y + 1));
    let bare = crate::client::popup_cursor_screen_pos_framed(area, 40, 10, 0, (0, 0), false).unwrap();
    assert_eq!(bare, (r.x, r.y));
    assert!(crate::client::popup_cursor_screen_pos_framed(area, 40, 10, 0, (39, 9), false).is_some());
    assert!(crate::client::popup_cursor_screen_pos_framed(area, 40, 10, 0, (39, 9), true).is_none());
}

// ── Command dispatch and serialisation ────────────────────────────────────

#[test]
fn display_popup_command_stores_the_look() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -B -T Hello -S fg=red -s bg=green -w 30 -h 8").unwrap();
    let look = look_of(&app);
    assert_eq!(look.title, "Hello");
    assert_eq!(look.border_lines.as_deref(), Some("none"));
    assert_eq!(look.border_style.as_deref(), Some("fg=red"));
    assert_eq!(look.style.as_deref(), Some("bg=green"));
    match &app.mode {
        Mode::PopupMode { command, width, height, .. } => {
            assert_eq!(command, "");
            assert_eq!((*width, *height), (30, 8));
        }
        _ => unreachable!(),
    }
}

#[test]
fn display_popup_without_title_has_none_and_reads_line_option() {
    let mut app = mock_app();
    app.user_options.insert("popup-border-lines".into(), "rounded".into());
    execute_command_string(&mut app, "display-popup -w 30 -h 8").unwrap();
    let look = look_of(&app);
    assert_eq!(look.title, "", "tmux draws no title without -T");
    assert_eq!(look.border_lines.as_deref(), Some("rounded"));
}

#[test]
fn display_popup_bad_border_lines_opens_nothing() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -b wobbly").unwrap();
    assert!(!matches!(app.mode, Mode::PopupMode { .. }));
}

#[test]
fn display_popup_minus_c_closes_the_open_popup() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -w 30 -h 8").unwrap();
    assert!(matches!(app.mode, Mode::PopupMode { .. }));
    execute_command_string(&mut app, "display-popup -C").unwrap();
    assert!(!matches!(app.mode, Mode::PopupMode { .. }));
}

#[test]
fn serialised_popup_carries_the_resolved_frame() {
    let mut app = mock_app();
    app.user_options.insert("popup-border-style".into(), "fg=#3b434d".into());
    app.user_options.insert("popup-style".into(), "bg=blue".into());
    execute_command_string(&mut app, "display-popup -T T1 -b heavy -s fg=white -w 30 -h 8").unwrap();
    let json = serialize_popup_overlay(&app);
    assert!(json.contains("\"popup_title\":\"T1\""), "{}", json);
    assert!(json.contains("\"popup_border_lines\":\"heavy\""), "{}", json);
    assert!(json.contains("\"popup_border_style\":\"fg=#3b434d\""), "{}", json);
    assert!(json.contains("\"popup_style\":\"bg=blue,fg=white\""), "{}", json);
}

#[test]
fn text_popups_keep_their_title() {
    // psmux's own list output popups are titled with what they show.
    let look = PopupLook::titled("list-keys");
    assert_eq!(look.title, "list-keys");
    assert_eq!(look.border_lines, None);
    let app = mock_app();
    let f = popup_frame_for(&app, &look);
    assert_eq!(f.title, "list-keys");
    assert_eq!(f.border_lines, "single");
    assert_eq!(f.border_style.fg, Some(Color::Yellow));
}
