// #712 audit: copy mode search resumed one BYTE after each hit, which is
// inside the hit when the query starts with a multi byte character, and the
// slice panicked the server (`send-keys -X search-forward 日本`).

use super::search_line_matches;

// Columns are display cells: each CJK character is two of them.
#[test]
fn cjk_query_finds_every_hit_at_its_display_column() {
    let m = search_line_matches("x\u{65e5}\u{672c}y\u{65e5}\u{672c}z\u{65e5}\u{672c}w", "\u{65e5}\u{672c}");
    assert_eq!(m, vec![(1, 5), (6, 10), (11, 15)]);
}

#[test]
fn two_byte_query_back_to_back() {
    let line = "\u{e9}cole\u{e9}cole\u{e9}cole";
    let m = search_line_matches(line, "\u{e9}cole");
    assert_eq!(m, vec![(0, 5), (5, 10), (10, 15)]);
}

#[test]
fn four_byte_query_overlapping() {
    let line = "\u{1f600}\u{1f600}\u{1f600}";
    let m = search_line_matches(line, "\u{1f600}\u{1f600}");
    assert_eq!(m, vec![(0, 4), (2, 6)]);
}

#[test]
fn ascii_overlap_unchanged() {
    assert_eq!(search_line_matches("aaaa", "aa"), vec![(0, 2), (1, 3), (2, 4)]);
    assert_eq!(search_line_matches("abc", "x"), vec![]);
    assert_eq!(search_line_matches("", "x"), vec![]);
    assert_eq!(search_line_matches("abc", ""), vec![]);
}

#[test]
fn hit_at_the_very_end() {
    assert_eq!(search_line_matches("ab\u{65e5}", "\u{65e5}"), vec![(2, 4)]);
}

#[test]
fn narrow_non_ascii_is_one_column_per_char() {
    // Box drawing and Latin accents are 1 column despite 2 or 3 bytes.
    let m = search_line_matches("\u{2500}\u{2500}x\u{2500}x", "x");
    assert_eq!(m, vec![(2, 3), (4, 5)]);
}

#[test]
fn soft_hyphen_takes_its_grid_column() {
    // The grid gives U+00AD one cell (#916), so a hit after one starts a
    // column later than unicode-width's zero would put it.
    assert_eq!(search_line_matches("a\u{ad}bc", "b"), vec![(2, 3)]);
    assert_eq!(search_line_matches("a\u{ad}bc", "\u{ad}b"), vec![(1, 3)]);
}
