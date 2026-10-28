// The terminal's cell size in pixels, from its replies to `CSI 16 t` and
// `CSI 14 t` at attach.  tmux reads `CSI 4 ; h ; w t` and divides by the
// terminal's size (tty_keys_winsz, tty-keys.c:726-735 in tmux 3.8).
use super::{draw_cell_px, host_cell_px, note_host_cell_px, parse_cell_size};

#[test]
fn windows_terminal_reports_its_10_by_20_cell() {
    // Measured on Windows Terminal 1.24 at 120 by 30.
    let drain = b"\x1b[?61;4;6;7;14;21;22;23;24;28;32;42;52c\x1b[4;600;1200t\x1b[6;20;10t";
    assert_eq!(parse_cell_size(drain, 120, 30), Some((10, 20)));
}

#[test]
fn the_cell_report_wins_over_the_window_report() {
    assert_eq!(parse_cell_size(b"\x1b[4;480;640t\x1b[6;17;8t", 80, 24), Some((8, 17)));
}

#[test]
fn the_window_report_alone_is_divided_by_the_terminal_size() {
    assert_eq!(parse_cell_size(b"\x1b[4;480;640t", 80, 24), Some((8, 20)));
    assert_eq!(parse_cell_size(b"\x1b[4;480;640t", 0, 24), None, "no size, no division");
}

#[test]
fn zero_or_malformed_reports_say_nothing() {
    assert_eq!(parse_cell_size(b"", 80, 24), None);
    assert_eq!(parse_cell_size(b"\x1b[6;0;0t", 80, 24), None);
    assert_eq!(parse_cell_size(b"\x1b[6;20t", 80, 24), None);
    assert_eq!(parse_cell_size(b"\x1b[6;20;10;1t", 80, 24), None);
    assert_eq!(parse_cell_size(b"\x1b[4;10;10t", 80, 24), None, "less than a pixel a cell");
    // Other replies in the drain are not cell reports.
    assert_eq!(parse_cell_size(b"\x1b]4;6;rgb:0000/0000/0000\x1b\\\x1b[?61;4c", 80, 24), None);
}

#[test]
fn without_a_report_images_are_drawn_at_the_pane_cell() {
    note_host_cell_px(None);
    assert_eq!(host_cell_px(), None);
    assert_eq!(draw_cell_px(), (10, 20));
    note_host_cell_px(Some((8, 16)));
    assert_eq!(draw_cell_px(), (8, 16));
    note_host_cell_px(Some((0, 16)));
    assert_eq!(host_cell_px(), None);
    note_host_cell_px(None);
}
