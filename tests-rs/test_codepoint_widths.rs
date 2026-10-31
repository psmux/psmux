//! `codepoint-widths`: tmux's server option for overriding how many columns a
//! Unicode codepoint occupies (`options-table.c`, parsed by
//! `utf8_add_to_width_cache` in `utf8.c`, applied by `utf8_width`).
//!
//! Why it exists, and why psmux needs it. psmux resolves East Asian AMBIGUOUS
//! characters to width 1, exactly as tmux does. If the user's OUTER terminal
//! draws them as two columns instead, the two disagree about where every
//! following cell on the row starts, and cells get stranded on screen. That is
//! the mechanism reproduced while investigating issue #639, whose reporter saw
//! leftover glyphs after quitting `tig` -- whose commit graph is built from
//! precisely those ambiguous width characters. This option is the escape
//! hatch: tell psmux to agree with the terminal.
//!
//! The tests below are layered the same way the #639 harness is: parser first,
//! then the shared width function, then the emulator grid, because a width
//! override that is honoured when reserving columns but not when erasing them
//! would CREATE stranded cells rather than fix them.
//!
//! # Serialisation
//!
//! The override table is process global (matching tmux's own global
//! `utf8_width_cache`), so these tests must not run concurrently with each
//! other. Every test takes `WIDTH_LOCK` and restores the empty default on the
//! way out.

use std::sync::{Mutex, MutexGuard};

/// Serialises access to the process-global width table.
static WIDTH_LOCK: Mutex<()> = Mutex::new(());

/// Take the lock and guarantee the table starts and ends empty, so one test
/// can never leak an override into another (and a panicking test cannot leave
/// the table dirty for the rest of the binary).
struct WidthGuard(#[allow(dead_code)] MutexGuard<'static, ()>);

impl WidthGuard {
    fn new() -> Self {
        // A poisoned lock just means an earlier test panicked; the Drop below
        // still cleared the table, so the guard is safe to reuse.
        let guard = WIDTH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        vt100::clear_codepoint_widths();
        Self(guard)
    }

    /// Apply a `codepoint-widths` value the way `set -s` would.
    fn set(&self, value: &str) {
        let entries: Vec<String> = value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        vt100::set_codepoint_widths(&entries);
    }
}

impl Drop for WidthGuard {
    fn drop(&mut self) {
        vt100::clear_codepoint_widths();
    }
}

/// U+2502 BOX DRAWINGS LIGHT VERTICAL. East Asian AMBIGUOUS, so
/// `unicode-width` reports 1 column and so does tmux by default. This is the
/// character class `tig` draws its commit graph with, and the one a
/// CJK-configured terminal is most likely to draw as two columns.
const AMBIGUOUS: char = '\u{2502}';

/// U+4E2D, an unambiguously double width Han character, as a control.
const WIDE: char = '\u{4E2D}';

// ---------------------------------------------------------------------------
// Layer 1: the entry parser, matched against tmux's utf8_add_to_width_cache
// ---------------------------------------------------------------------------

#[test]
fn parses_the_hex_codepoint_form() {
    let parsed = vt100::parse_codepoint_width_entry("U+2502=2").expect("U+XXXX=N is valid");
    assert_eq!(parsed.start, 0x2502);
    assert_eq!(parsed.end, 0x2502);
    assert_eq!(parsed.width, 2);
}

#[test]
fn parses_the_inclusive_range_form() {
    // tmux loops `for (wc = wc_start; wc <= wc_end; wc++)`, so both ends are
    // included.
    let parsed =
        vt100::parse_codepoint_width_entry("U+2500-U+257F=2").expect("range form is valid");
    assert_eq!(parsed.start, 0x2500);
    assert_eq!(parsed.end, 0x257F);
    assert_eq!(parsed.width, 2);
}

#[test]
fn parses_the_literal_character_form() {
    // tmux's else branch: anything not starting with "U+" is decoded as a
    // single literal character.
    let parsed = vt100::parse_codepoint_width_entry("\u{2502}=2").expect("literal char is valid");
    assert_eq!(parsed.start, 0x2502);
    assert_eq!(parsed.end, 0x2502);
    assert_eq!(parsed.width, 2);
}

#[test]
fn accepts_only_widths_zero_one_and_two() {
    // tmux validates with strtonum(cp, 0, 2, &errstr) and drops the entry when
    // errstr is set, so 3 and above are not merely clamped, they are ignored.
    for width in 0u8..=2 {
        assert_eq!(
            vt100::parse_codepoint_width_entry(&format!("U+2502={width}"))
                .map(|o| o.width),
            Some(width),
            "width {width} is inside tmux's 0..=2 range"
        );
    }
    for bad in ["3", "9", "-1", "", "x", "2x", "1.5"] {
        assert_eq!(
            vt100::parse_codepoint_width_entry(&format!("U+2502={bad}")),
            None,
            "width {bad:?} is outside tmux's 0..=2 range or not a number"
        );
    }
    // tmux's strtonum is strtoll plus a `*ep != '\0'` check
    // (compat/strtonum.c:52-54), so whatever strtoll swallows at the FRONT is
    // accepted: a leading '+' and leading whitespace both parse. Trailing text
    // is still rejected (covered by "2x" above), and a negative parses but is
    // then refused by the range check (covered by "-1").
    for ok in ["+1", " 1", "+2"] {
        assert!(
            vt100::parse_codepoint_width_entry(&format!("U+2502={ok}")).is_some(),
            "tmux's strtonum accepts width {ok:?}"
        );
    }
}

#[test]
fn rejects_malformed_entries() {
    let cases: &[(&str, &str)] = &[
        ("U+2502", "no '=' separator at all"),
        ("U+=2", "no hex digits after the prefix"),
        ("U+ZZZZ=2", "non hex digits"),
        ("U+0=2", "tmux rejects codepoint zero (n == 0)"),
        ("U+110000=2", "above the Unicode maximum"),
        ("U+2500-2502=2", "tmux requires U+ on the range END too"),
        ("U+2502-U+2500=2", "descending range"),
        ("ab=2", "literal form takes exactly one character"),
        ("=2", "empty spec"),
    ];
    for (entry, why) in cases {
        assert_eq!(
            vt100::parse_codepoint_width_entry(entry),
            None,
            "{entry:?} should be rejected: {why}"
        );
    }
}

#[test]
fn malformed_entries_do_not_discard_the_good_ones() {
    // tmux parses each array item independently and simply returns early on a
    // bad one, so a typo costs you that entry and nothing else.
    let guard = WidthGuard::new();
    guard.set("U+2502=2,this-is-junk,U+4E2D=1");
    assert_eq!(vt100::char_width(AMBIGUOUS), Some(2), "good entry before the junk");
    assert_eq!(vt100::char_width(WIDE), Some(1), "good entry after the junk");
}

// ---------------------------------------------------------------------------
// Layer 2: the shared width function
// ---------------------------------------------------------------------------

#[test]
fn default_resolution_is_unchanged() {
    // The guard rail for this feature: the override is opt in, and with none
    // configured psmux must resolve exactly as it did before. Ambiguous stays
    // narrow (tmux's default), genuinely wide stays wide.
    let _guard = WidthGuard::new();
    assert!(!vt100::has_overrides());
    assert_eq!(vt100::char_width(AMBIGUOUS), Some(1));
    assert_eq!(vt100::char_width(WIDE), Some(2));
    assert_eq!(vt100::char_width('a'), Some(1));
    assert_eq!(vt100::str_width("ab\u{4E2D}"), 4);
}

#[test]
fn an_override_changes_the_reported_width() {
    let guard = WidthGuard::new();
    guard.set("U+2502=2");
    assert!(vt100::has_overrides());
    assert_eq!(vt100::char_width(AMBIGUOUS), Some(2), "overridden to 2");
    assert_eq!(vt100::char_width('a'), Some(1), "untouched codepoint");
    assert_eq!(vt100::str_width("a\u{2502}b"), 4, "str width sees it too");
}

#[test]
fn an_override_can_narrow_a_wide_character() {
    // Width 2 is the interesting direction, but the option is symmetric and
    // tmux accepts 0 and 1 as well.
    let guard = WidthGuard::new();
    guard.set("U+4E2D=1");
    assert_eq!(vt100::char_width(WIDE), Some(1));
    guard.set("U+4E2D=0");
    assert_eq!(vt100::char_width(WIDE), Some(0));
}

#[test]
fn a_range_covers_every_codepoint_in_it() {
    let guard = WidthGuard::new();
    guard.set("U+2500-U+257F=2");
    for cp in [0x2500u32, 0x2502, 0x2540, 0x257F] {
        let c = char::from_u32(cp).unwrap();
        assert_eq!(vt100::char_width(c), Some(2), "U+{cp:04X} is inside the range");
    }
    // Just outside each end of the range, still at their default widths.
    for cp in [0x24FFu32, 0x2580] {
        let c = char::from_u32(cp).unwrap();
        assert_eq!(
            vt100::char_width(c),
            Some(1),
            "U+{cp:04X} is outside the range and must keep its default width"
        );
    }
}

#[test]
fn a_later_entry_wins_over_an_earlier_one() {
    // tmux's utf8_insert_width_cache removes and replaces an existing node, so
    // duplicates resolve to the last one written.
    let guard = WidthGuard::new();
    guard.set("U+2502=1,U+2502=2");
    assert_eq!(vt100::char_width(AMBIGUOUS), Some(2), "second entry wins");
    guard.set("U+2500-U+257F=2,U+2502=1");
    assert_eq!(
        vt100::char_width(AMBIGUOUS),
        Some(1),
        "a single entry after a range overrides just that codepoint"
    );
    assert_eq!(
        vt100::char_width('\u{2500}'),
        Some(2),
        "the rest of the range survives"
    );
}

#[test]
fn clearing_restores_the_default() {
    let guard = WidthGuard::new();
    guard.set("U+2502=2");
    assert_eq!(vt100::char_width(AMBIGUOUS), Some(2));
    guard.set("");
    assert!(!vt100::has_overrides(), "an empty value clears the table");
    assert_eq!(vt100::char_width(AMBIGUOUS), Some(1));
}

#[test]
fn appending_keeps_the_existing_entries() {
    // `set -sa codepoint-widths ...` appends an ITEM to the array, and the
    // width table is then built from every element in index order. Modelled
    // here as those elements written as one value.
    let guard = WidthGuard::new();
    guard.set("U+2502=2");
    guard.set("U+2502=2,U+4E2D=1");
    assert_eq!(vt100::char_width(AMBIGUOUS), Some(2), "original entry kept");
    assert_eq!(vt100::char_width(WIDE), Some(1), "appended entry applied");
}

// ---------------------------------------------------------------------------
// Layer 3: the emulator grid -- the decisive proof
// ---------------------------------------------------------------------------

fn parser(rows: u16, cols: u16) -> vt100::Parser {
    vt100::Parser::new(rows, cols, 0)
}

/// Render a row with continuation cells shown as the empty string, so a
/// stranded half appears as a doubled or misplaced glyph rather than vanishing.
fn row_text(screen: &vt100::Screen, row: u16, cols: u16) -> String {
    let mut s = String::new();
    for c in 0..cols {
        if let Some(cell) = screen.cell(row, c) {
            s.push_str(cell.contents());
        }
    }
    s.trim_end().to_string()
}

/// The invariant tmux maintains in `screen_write_overwrite`: every wide cell is
/// followed by a continuation cell, and every continuation cell is preceded by
/// a wide cell. A violation IS the stranded cell users report.
fn assert_wide_pairs_intact(screen: &vt100::Screen, rows: u16, cols: u16, ctx: &str) {
    for r in 0..rows {
        for c in 0..cols {
            let Some(cell) = screen.cell(r, c) else { continue };
            if cell.is_wide() {
                assert!(
                    c + 1 < cols,
                    "{ctx}: row {r} col {c}: wide glyph {:?} has no room for its continuation",
                    cell.contents()
                );
                assert!(
                    screen.cell(r, c + 1).is_some_and(vt100::Cell::is_wide_continuation),
                    "{ctx}: row {r} col {c}: wide glyph {:?} lost its continuation cell \
                     -- this is a stranded half",
                    cell.contents()
                );
            }
            if cell.is_wide_continuation() {
                assert!(
                    c > 0 && screen.cell(r, c - 1).is_some_and(vt100::Cell::is_wide),
                    "{ctx}: row {r} col {c}: continuation cell with no wide glyph before it \
                     -- this is a stranded half"
                );
            }
        }
    }
}

#[test]
fn without_the_override_an_ambiguous_char_takes_one_column() {
    // The before picture. Nothing about the default may change.
    let _guard = WidthGuard::new();
    let mut p = parser(1, 10);
    p.process("\u{2502}ab".as_bytes());
    let screen = p.screen();

    assert!(!screen.cell(0, 0).unwrap().is_wide(), "ambiguous stays narrow");
    assert_eq!(screen.cell(0, 1).unwrap().contents(), "a", "'a' lands in column 1");
    assert_eq!(screen.cell(0, 2).unwrap().contents(), "b");
    assert_eq!(screen.cursor_position(), (0, 3), "cursor advanced 3 columns");
    assert_wide_pairs_intact(screen, 1, 10, "default width");
}

#[test]
fn with_the_override_an_ambiguous_char_reserves_two_columns() {
    // THE decisive assertion for this feature: an override of 2 must make the
    // emulator reserve a real second cell, not merely report a bigger number.
    let guard = WidthGuard::new();
    guard.set("U+2502=2");

    let mut p = parser(1, 10);
    p.process("\u{2502}ab".as_bytes());
    let screen = p.screen();

    let lead = screen.cell(0, 0).unwrap();
    assert_eq!(lead.contents(), "\u{2502}");
    assert!(lead.is_wide(), "the lead cell is marked wide");
    assert!(
        screen.cell(0, 1).unwrap().is_wide_continuation(),
        "column 1 is reserved as the continuation half"
    );
    assert_eq!(
        screen.cell(0, 2).unwrap().contents(),
        "a",
        "'a' is pushed to column 2, not column 1"
    );
    assert_eq!(screen.cell(0, 3).unwrap().contents(), "b");
    assert_eq!(screen.cursor_position(), (0, 4), "cursor advanced 4 columns");
    assert_wide_pairs_intact(screen, 1, 10, "override width 2");
}

#[test]
fn overwriting_the_lead_half_clears_the_continuation() {
    // Reserving two columns is only half the job. If an overwrite cleared the
    // lead but left the continuation, we would have INVENTED the very bug the
    // option is meant to cure.
    let guard = WidthGuard::new();
    guard.set("U+2502=2");

    let mut p = parser(1, 10);
    p.process("\u{2502}ab".as_bytes());
    // Home, then write a narrow character over the lead half.
    p.process(b"\x1b[HX");
    let screen = p.screen();

    assert_eq!(screen.cell(0, 0).unwrap().contents(), "X");
    assert!(!screen.cell(0, 0).unwrap().is_wide());
    assert!(
        !screen.cell(0, 1).unwrap().is_wide_continuation(),
        "the orphaned continuation half must be cleared, not left behind"
    );
    assert_eq!(row_text(screen, 0, 10), "X ab", "no ghost glyph survives");
    assert_wide_pairs_intact(screen, 1, 10, "overwrite lead half");
}

#[test]
fn overwriting_the_continuation_half_clears_the_lead() {
    // The mirror case: tmux walks BACKWARDS from a padding cell to erase the
    // character that owns it.
    let guard = WidthGuard::new();
    guard.set("U+2502=2");

    let mut p = parser(1, 10);
    p.process("\u{2502}ab".as_bytes());
    // Column 1 (1-based col 2) is the continuation half.
    p.process(b"\x1b[1;2HX");
    let screen = p.screen();

    assert!(
        !screen.cell(0, 0).unwrap().is_wide(),
        "the lead half must not survive as a stranded wide cell"
    );
    assert_eq!(
        screen.cell(0, 0).unwrap().contents(),
        "",
        "the lead half is cleared, not left painting a ghost glyph"
    );
    assert_eq!(screen.cell(0, 1).unwrap().contents(), "X");
    assert_wide_pairs_intact(screen, 1, 10, "overwrite continuation half");
}

#[test]
fn erasing_the_line_clears_both_halves() {
    // The #639 shape: a full screen program exits and the screen is erased.
    let guard = WidthGuard::new();
    guard.set("U+2500-U+257F=2");

    let mut p = parser(2, 12);
    p.process("\u{2502}\u{2500}\u{2502}x".as_bytes());
    let screen = p.screen();
    assert_eq!(screen.cursor_position(), (0, 7), "three wide + one narrow");

    // ED (erase in display), as issued on exit from an alt-screen program.
    p.process(b"\x1b[2J");
    let screen = p.screen();
    assert_eq!(row_text(screen, 0, 12), "", "the whole row is blank");
    for c in 0..12 {
        let cell = screen.cell(0, c).unwrap();
        assert!(!cell.is_wide(), "col {c} kept a wide flag after ED");
        assert!(
            !cell.is_wide_continuation(),
            "col {c} kept a continuation flag after ED"
        );
    }
    assert_wide_pairs_intact(screen, 2, 12, "after ED");
}

#[test]
fn a_narrowing_override_stops_reserving_the_second_column() {
    // The other direction end to end: force a genuinely wide character to one
    // column and the grid must stop reserving a continuation for it.
    let guard = WidthGuard::new();
    guard.set("U+4E2D=1");

    let mut p = parser(1, 10);
    p.process("\u{4E2D}ab".as_bytes());
    let screen = p.screen();

    assert!(!screen.cell(0, 0).unwrap().is_wide(), "forced narrow");
    assert_eq!(screen.cell(0, 1).unwrap().contents(), "a", "'a' in column 1");
    assert_eq!(screen.cursor_position(), (0, 3));
    assert_wide_pairs_intact(screen, 1, 10, "override width 1");
}

#[test]
fn a_live_change_applies_to_text_drawn_afterwards() {
    // tmux rebuilds the cache from the option-changed hook
    // (`utf8_update_width_cache`), so a `set -s` mid session takes effect on
    // the next character drawn rather than at the next server start.
    let guard = WidthGuard::new();

    let mut p = parser(2, 10);
    p.process("\u{2502}a".as_bytes());
    assert!(
        !p.screen().cell(0, 0).unwrap().is_wide(),
        "drawn before the option was set: narrow"
    );

    guard.set("U+2502=2");
    p.process(b"\r\n");
    p.process("\u{2502}a".as_bytes());
    let screen = p.screen();
    assert!(
        screen.cell(1, 0).unwrap().is_wide(),
        "drawn after the option was set: wide"
    );
    assert!(screen.cell(1, 1).unwrap().is_wide_continuation());
    assert_eq!(screen.cell(1, 2).unwrap().contents(), "a");
}

#[test]
fn capture_pane_sees_the_overridden_glyph_once() {
    // `capture-pane` renders a wide glyph from its lead cell and skips the
    // trailing half (src/copy_mode.rs::push_capture_cell). With the override
    // active the character must still appear exactly once, not twice and not
    // followed by a stray blank that shifts the rest of the line.
    let guard = WidthGuard::new();
    guard.set("U+2502=2");

    let mut p = parser(1, 12);
    p.process("\u{2502}ab".as_bytes());
    let screen = p.screen();

    // Mirror of push_capture_cell.
    let mut captured = String::new();
    for c in 0..12 {
        match screen.cell(0, c) {
            Some(cell) if cell.is_wide_continuation() => {}
            Some(cell) if cell.has_contents() => captured.push_str(cell.contents()),
            _ => captured.push(' '),
        }
    }
    assert_eq!(captured.trim_end(), "\u{2502}ab", "captured exactly once");
}
