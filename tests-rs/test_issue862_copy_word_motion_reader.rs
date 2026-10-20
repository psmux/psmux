// Issue #862: copy mode word motions follow tmux's grid reader
// (grid-reader.c, tag 3.7c).
//
// Every expected position below is what tmux 3.7c reported in
// `#{copy_cursor_y}:#{copy_cursor_x}` for the same lines in a pane of the
// same size. Like tmux's, a motion can stop on the second cell of a wide
// character (2:15 and 2:23 below).
//
// Registered from src/grid_reader.rs.

use super::*;

/// The lines the tmux measurements were made on, printed into a 60x10 pane.
const LINES: [&str; 4] = [
    "foo-bar_baz qux.quux  a@b:c",
    "path/to/file.txt (x+y)=z  ~/dir",
    "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} abc\u{65e5}\u{672c} def",
    "  lead  spaces, \"quoted\" end.",
];

fn parser(lines: &[&str], rows: u16, cols: u16, scrollback: usize) -> vt100::Parser {
    let mut p = vt100::Parser::new(rows, cols, scrollback);
    for line in lines {
        p.process(format!("{line}\r\n").as_bytes());
    }
    p
}

fn pos(s: &str) -> (usize, usize) {
    let (y, x) = s.split_once(':').expect("y:x");
    (y.parse().expect("y"), x.parse().expect("x"))
}

#[derive(Clone, Copy)]
enum Verb {
    NextWord,
    NextWordEnd,
    PreviousWord,
    NextSpace,
    NextSpaceEnd,
    PreviousSpace,
}

/// Run `verb` `steps` times from (`x`, `y`) the way copy mode does, and give
/// each position it stops at.
fn walk(grid: &mut ScreenGrid<'_>, verb: Verb, vi: bool, x: usize, y: usize, steps: usize) -> Vec<String> {
    let mut gr = GridReader::new(grid, x, y);
    let mut out = Vec::new();
    for _ in 0..steps {
        match verb {
            Verb::NextWord => gr.cursor_next_word(DEFAULT_WORD_SEPARATORS),
            Verb::NextWordEnd => gr.cursor_next_word_end_keys(DEFAULT_WORD_SEPARATORS, vi),
            Verb::PreviousWord => gr.cursor_previous_word(DEFAULT_WORD_SEPARATORS, true, !vi),
            Verb::NextSpace => gr.cursor_next_word(""),
            Verb::NextSpaceEnd => gr.cursor_next_word_end_keys("", vi),
            Verb::PreviousSpace => gr.cursor_previous_word("", true, !vi),
        }
        out.push(format!("{}:{}", gr.cy, gr.cx));
    }
    out
}

/// One measured walk: keys, line, verb, and where each step stopped.
/// Forward walks start at the start of the line, backward walks at tmux's
/// end-of-line (the last character with vi keys, the cell after it with
/// emacs keys).
const WALKS: &[(&str, usize, &str, &str)] = &[
    ("vi", 0, "next-word", "0:3 0:4 0:12 0:15 0:16 0:22 0:23"),
    ("vi", 0, "next-word-end", "0:2 0:3 0:10 0:14 0:15 0:19 0:22"),
    ("vi", 0, "previous-word", "0:25 0:24 0:23 0:22 0:16 0:15 0:12"),
    ("vi", 0, "next-space", "0:12 0:22 1:0 1:17"),
    ("vi", 0, "next-space-end", "0:10 0:19 0:26 0:26"),
    ("vi", 0, "previous-space", "0:22 0:12 0:0 0:0"),
    ("vi", 1, "next-word", "1:4 1:5 1:7 1:8 1:12 1:13 1:17"),
    ("vi", 1, "next-word-end", "1:3 1:4 1:6 1:7 1:11 1:12 1:15"),
    ("vi", 1, "previous-word", "1:28 1:26 1:23 1:21 1:20 1:19 1:18"),
    ("vi", 1, "next-space", "1:17 1:26 2:0 2:17"),
    ("vi", 1, "next-space-end", "1:15 1:23 1:30 1:30"),
    ("vi", 1, "previous-space", "1:26 1:17 1:0 0:22"),
    ("vi", 2, "next-word", "2:17 2:25 3:2 3:8 3:14 3:16 3:17"),
    ("vi", 2, "next-word-end", "2:15 2:23 2:27 2:27 2:27 2:27 2:27"),
    ("vi", 2, "previous-word", "2:25 2:17 2:0 1:28 1:26 1:23 1:21"),
    ("vi", 2, "next-space", "2:17 2:25 3:2 3:8"),
    ("vi", 2, "next-space-end", "2:15 2:23 2:27 2:27"),
    ("vi", 2, "previous-space", "2:25 2:17 2:0 1:26"),
    ("vi", 3, "next-word", "3:2 3:8 3:14 3:16 3:17 3:23 3:25"),
    ("vi", 3, "next-word-end", "3:5 3:13 3:14 3:16 3:22 3:23 3:27"),
    ("vi", 3, "previous-word", "3:25 3:23 3:17 3:16 3:14 3:8 3:2"),
    ("vi", 3, "next-space", "3:2 3:8 3:16 3:25"),
    ("vi", 3, "next-space-end", "3:5 3:14 3:23 3:28"),
    ("vi", 3, "previous-space", "3:25 3:16 3:8 3:2"),
    ("emacs", 0, "next-word", "0:3 0:4 0:12 0:15 0:16 0:22 0:23"),
    ("emacs", 0, "next-word-end", "0:3 0:4 0:11 0:15 0:16 0:20 0:23"),
    ("emacs", 0, "previous-word", "0:26 0:25 0:24 0:23 0:22 0:16 0:15"),
    ("emacs", 0, "next-space", "0:12 0:22 1:0 1:17"),
    ("emacs", 0, "next-space-end", "0:11 0:20 0:27 1:16"),
    ("emacs", 0, "previous-space", "0:22 0:12 0:0 0:0"),
    ("emacs", 1, "next-word", "1:4 1:5 1:7 1:8 1:12 1:13 1:17"),
    ("emacs", 1, "next-word-end", "1:4 1:5 1:7 1:8 1:12 1:13 1:16"),
    ("emacs", 1, "previous-word", "1:28 1:26 1:23 1:21 1:20 1:19 1:18"),
    ("emacs", 1, "next-space", "1:17 1:26 2:0 2:17"),
    ("emacs", 1, "next-space-end", "1:16 1:24 1:31 2:16"),
    ("emacs", 1, "previous-space", "1:26 1:17 1:0 0:22"),
    ("emacs", 2, "next-word", "2:17 2:25 3:2 3:8 3:14 3:16 3:17"),
    ("emacs", 2, "next-word-end", "2:16 2:24 2:28 3:6 3:14 3:15 3:17"),
    ("emacs", 2, "previous-word", "2:25 2:17 2:0 1:28 1:26 1:23 1:21"),
    ("emacs", 2, "next-space", "2:17 2:25 3:2 3:8"),
    ("emacs", 2, "next-space-end", "2:16 2:24 2:28 3:6"),
    ("emacs", 2, "previous-space", "2:25 2:17 2:0 1:26"),
    ("emacs", 3, "next-word", "3:2 3:8 3:14 3:16 3:17 3:23 3:25"),
    ("emacs", 3, "next-word-end", "3:6 3:14 3:15 3:17 3:23 3:24 3:28"),
    ("emacs", 3, "previous-word", "3:28 3:25 3:23 3:17 3:16 3:14 3:8"),
    ("emacs", 3, "next-space", "3:2 3:8 3:16 3:25"),
    ("emacs", 3, "next-space-end", "3:6 3:15 3:24 3:29"),
    ("emacs", 3, "previous-space", "3:25 3:16 3:8 3:2"),
];

fn verb(name: &str) -> Verb {
    match name {
        "next-word" => Verb::NextWord,
        "next-word-end" => Verb::NextWordEnd,
        "previous-word" => Verb::PreviousWord,
        "next-space" => Verb::NextSpace,
        "next-space-end" => Verb::NextSpaceEnd,
        "previous-space" => Verb::PreviousSpace,
        _ => panic!("unknown verb {name}"),
    }
}

#[test]
fn every_measured_walk_stops_where_tmux_does() {
    let mut p = parser(&LINES, 10, 60, 0);
    let mut grid = ScreenGrid::new(p.screen_mut());
    let mut failures = Vec::new();
    for &(keys, line, name, want) in WALKS {
        let vi = keys == "vi";
        let want: Vec<String> = want.split(' ').map(str::to_string).collect();
        let len = GridReader::new(&mut grid, 0, line).line_length(line);
        let backward = name.starts_with("previous");
        let x = if !backward { 0 } else if vi { len - 1 } else { len };
        let got = walk(&mut grid, verb(name), vi, x, line, want.len());
        if got != want {
            failures.push(format!("{keys} L{line} {name}: got {} want {}", got.join(" "), want.join(" ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_default_separators_are_tmuxs() {
    // options-table.c: the printable ASCII characters that are neither
    // letters nor digits, minus the underscore.
    let want: String = (0x21u8..0x7f)
        .map(char::from)
        .filter(|c| !c.is_ascii_alphanumeric() && *c != '_')
        .collect();
    assert_eq!(DEFAULT_WORD_SEPARATORS, want);
}

#[test]
fn cjk_and_letters_make_one_word_and_padding_is_part_of_it() {
    let mut p = parser(&LINES, 10, 60, 0);
    let mut grid = ScreenGrid::new(p.screen_mut());
    let mut gr = GridReader::new(&mut grid, 0, 2);
    assert_eq!(gr.kind_at(1, 2, DEFAULT_WORD_SEPARATORS), 2, "the second cell of 日 is part of the word");
    assert_eq!(gr.kind_at(16, 2, DEFAULT_WORD_SEPARATORS), 0);
    assert_eq!(gr.kind_at(3, 0, DEFAULT_WORD_SEPARATORS), 1, "- is a separator");
    assert_eq!(gr.kind_at(7, 0, DEFAULT_WORD_SEPARATORS), 2, "_ is not");
}

/// The 20 column pane of the wrapped line measurements: the first line runs
/// on to row 1, then `x-y  end`.
fn wrapped_parser() -> vt100::Parser {
    parser(&["alpha beta gamma delta epsilon zeta", "x-y  end"], 8, 20, 0)
}

#[test]
fn a_wrapped_line_is_walked_as_one_line() {
    let mut p = wrapped_parser();
    let mut grid = ScreenGrid::new(p.screen_mut());
    assert!(GridReader::new(&mut grid, 0, 0).line_length(0) == 20);
    let cases: &[(&str, Verb, usize, usize, &str)] = &[
        ("vi", Verb::NextWord, 0, 0, "0:6 0:11 0:17 1:3 1:11 2:0 2:1 2:2"),
        ("vi", Verb::NextWordEnd, 0, 0, "0:4 0:9 0:15 1:1 1:9 1:14 1:14 1:14"),
        ("vi", Verb::PreviousWord, 7, 2, "2:5 2:2 2:1 2:0 1:11 1:3 0:17 0:11"),
        ("emacs", Verb::NextWord, 0, 0, "0:6 0:11 0:17 1:3 1:11 2:0 2:1 2:2"),
        ("emacs", Verb::NextWordEnd, 0, 0, "0:5 0:10 0:16 1:2 1:10 1:15 2:1 2:2"),
        ("emacs", Verb::PreviousWord, 8, 2, "2:5 2:2 2:1 2:0 1:11 1:3 0:17 0:11"),
    ];
    for &(keys, v, x, y, want) in cases {
        let want: Vec<String> = want.split(' ').map(str::to_string).collect();
        assert_eq!(walk(&mut grid, v, keys == "vi", x, y, want.len()), want, "{keys} from {y}:{x}");
    }
}

#[test]
fn the_word_under_a_cell_is_tmuxs_format_grid_word() {
    let mut p = parser(&LINES, 10, 60, 0);
    let mut grid = ScreenGrid::new(p.screen_mut());
    // tmux 3.7c `#{copy_cursor_word}` at each cell. On a separator it is the
    // word after it, and on whitespace too.
    let cases: &[(usize, usize, &str)] = &[
        (0, 0, "foo"),
        (3, 0, "bar_baz"),
        (5, 0, "bar_baz"),
        (12, 0, "qux"),
        (15, 0, "quux"),
        (21, 0, "a"),
        (22, 0, "a"),
        (24, 0, "b"),
        (4, 1, "to"),
        (17, 1, "x"),
        (23, 1, "z"),
        (0, 2, "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}"),
        (6, 2, "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}"),
        (17, 2, "abc\u{65e5}\u{672c}"),
        (20, 2, "abc\u{65e5}\u{672c}"),
        (0, 3, ""),
        (4, 3, "lead"),
        (16, 3, "quoted"),
        (26, 3, "end"),
    ];
    for &(x, y, want) in cases {
        assert_eq!(format_grid_word(&mut grid, x, y, DEFAULT_WORD_SEPARATORS), want, "at {y}:{x}");
    }
    drop(grid);
    // Across a wrap: `delta` starts on row 0 and ends on row 1.
    let mut p = wrapped_parser();
    let mut grid = ScreenGrid::new(p.screen_mut());
    assert_eq!(format_grid_word(&mut grid, 18, 0, DEFAULT_WORD_SEPARATORS), "delta");
}

#[test]
fn select_word_ends_where_tmux_does() {
    let mut p = parser(&LINES, 10, 60, 0);
    let mut grid = ScreenGrid::new(p.screen_mut());
    // (x, y, vi end, emacs end), as tmux 3.7c reported them. The emacs end is
    // the cell after the word, which an emacs selection leaves out, except
    // for a one-character word, which ends where it starts. A vi end can be
    // the second cell of a wide character.
    let cases: &[(usize, usize, &str, &str)] = &[
        (0, 0, "0:2", "0:3"),
        (3, 0, "0:10", "0:4"),
        (5, 0, "0:10", "0:11"),
        (12, 0, "0:14", "0:15"),
        (15, 0, "0:19", "0:16"),
        (21, 0, "0:19", "0:20"),
        (22, 0, "0:23", "0:23"),
        (24, 0, "0:25", "0:25"),
        (4, 1, "1:6", "1:5"),
        (17, 1, "1:18", "1:18"),
        (23, 1, "1:23", "1:23"),
        (0, 2, "2:15", "2:16"),
        (6, 2, "2:15", "2:16"),
        (17, 2, "2:23", "2:24"),
        (20, 2, "2:23", "2:24"),
        (0, 3, "2:27", "2:28"),
        (4, 3, "3:5", "3:6"),
        (16, 3, "3:22", "3:17"),
        (26, 3, "3:27", "3:28"),
    ];
    for &(x, y, vi_end, emacs_end) in cases {
        for (vi, want) in [(true, vi_end), (false, emacs_end)] {
            let mut gr = GridReader::new(&mut grid, x, y);
            gr.cursor_previous_word(DEFAULT_WORD_SEPARATORS, false, !vi);
            gr.select_word_end(DEFAULT_WORD_SEPARATORS, vi);
            assert_eq!((gr.cy, gr.cx), pos(want), "{} at {y}:{x}", if vi { "vi" } else { "emacs" });
        }
    }
}

#[test]
fn a_word_motion_reads_the_scrollback_and_puts_the_view_back() {
    // `qux.quux` scrolls into the scrollback under 20 filler lines.
    let mut lines = vec!["qux.quux"];
    let fillers: Vec<String> = (0..20).map(|i| format!("f{i}")).collect();
    lines.extend(fillers.iter().map(String::as_str));
    let mut p = parser(&lines, 10, 60, 100);
    p.screen_mut().set_scrollback(3);
    {
        let mut grid = ScreenGrid::new(p.screen_mut());
        // 21 lines and the empty one the cursor is on, 10 of them on screen.
        assert_eq!(grid.history(), 12);
        assert_eq!(grid.top(), 9);
        // From line 1 (`f0`), the word before is `quux` on line 0.
        let got = walk(&mut grid, Verb::PreviousWord, true, 0, 1, 3);
        assert_eq!(got, vec!["0:4", "0:3", "0:0"]);
    }
    assert_eq!(p.screen().scrollback(), 3, "the view is where it was");
}

#[test]
fn the_alternate_screen_has_no_lines_above_it() {
    // 30 lines go into the main grid's scrollback, then an app takes the
    // alternate screen. The alternate grid keeps no scrollback, so there is
    // nothing above its top row: previous-word from 0:0 stays there, and the
    // top row is not read again as if it were the line above.
    let fillers: Vec<String> = (0..30).map(|i| format!("word{i:02} aa")).collect();
    let lines: Vec<&str> = fillers.iter().map(String::as_str).collect();
    let mut p = parser(&lines, 8, 40, 100);
    p.process(b"\x1b[?1049h\x1b[HALT top line");
    assert!(p.screen().scrollback_filled() > 0, "the main grid has history");
    let mut grid = ScreenGrid::new(p.screen_mut());
    assert_eq!(grid.history(), 0);
    let top = grid.abs_of_row(0);
    assert_eq!(top, 0);
    let got = walk(&mut grid, Verb::PreviousWord, true, 0, top, 2);
    assert_eq!(got, vec!["0:0", "0:0"]);
    let got = walk(&mut grid, Verb::PreviousWord, false, 0, top, 1);
    assert_eq!(got, vec!["0:0"]);
    assert_eq!(format_grid_word(&mut grid, 0, top, DEFAULT_WORD_SEPARATORS), "ALT");
}
