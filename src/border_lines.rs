//! Pane border line styles for the tmux `pane-border-lines` option.
//!
//! psmux draws the separators *between* panes (not full boxes), then a
//! post-pass (`rendering::fix_border_intersections`) upgrades straight runs to
//! junction glyphs where a vertical and horizontal separator meet. This module
//! maps a `pane-border-lines` value to the box-drawing glyph set used for both
//! passes.
//!
//! Glyphs match tmux exactly (see `screen-write.c` `SIMPLE_BORDERS`, and
//! `tty-acs.c` `tty_acs_double_borders_list` / `tty_acs_heavy_borders_list`):
//!
//! | value    | line    | cross | tees                     |
//! |----------|---------|-------|--------------------------|
//! | single   | │ ─     | ┼     | ├ ┤ ┬ ┴                  |
//! | double   | ║ ═     | ╬     | ╠ ╣ ╦ ╩                  |
//! | heavy    | ┃ ━     | ╋     | ┣ ┫ ┳ ┻                  |
//! | simple   | \| -    | +     | + + + +                  |
//! | spaces   | (space) | space | (all spaces)             |
//! | none     | (no borders drawn at all)                        |
//!
//! `number` is accepted (tmux draws single-line borders plus pane-number
//! indicators); psmux renders it with the single-line glyph set. Unknown
//! values fall back to single, matching tmux's choice-option default.

/// The seven glyphs psmux needs to draw and join pane separators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BorderChars {
    /// Vertical separator (between horizontally-adjacent panes).
    pub vertical: char,
    /// Horizontal separator (between vertically-adjacent panes).
    pub horizontal: char,
    /// Four-way junction (left+right+up+down), e.g. `┼`.
    pub cross: char,
    /// Tee opening to the right (right+up+down), e.g. `├`.
    pub left_tee: char,
    /// Tee opening to the left (left+up+down), e.g. `┤`.
    pub right_tee: char,
    /// Tee opening downward (left+right+down), e.g. `┬`.
    pub top_tee: char,
    /// Tee opening upward (left+right+up), e.g. `┴`.
    pub bottom_tee: char,
    /// Whether junction glyphs differ from straight-line glyphs.
    pub has_distinct_junction_glyphs: bool,
}

/// The default `pane-border-lines` value (tmux `PANE_LINES_SINGLE`).
pub const DEFAULT: &str = "single";

/// The valid choice values, for validation / customize-mode.
pub const CHOICES: &[&str] = &["single", "double", "heavy", "simple", "number", "spaces", "none"];

const SINGLE: BorderChars = BorderChars {
    vertical: '│', horizontal: '─', cross: '┼',
    left_tee: '├', right_tee: '┤', top_tee: '┬', bottom_tee: '┴',
    has_distinct_junction_glyphs: true,
};

/// Resolve a `pane-border-lines` value to its glyph set.
///
/// Returns `None` for `none` (no separators are drawn). Unknown values fall
/// back to the single-line default, mirroring tmux's choice-option handling.
pub fn border_chars(name: &str) -> Option<BorderChars> {
    match name.trim() {
        "none" => None,
        "double" => Some(BorderChars {
            vertical: '║', horizontal: '═', cross: '╬',
            left_tee: '╠', right_tee: '╣', top_tee: '╦', bottom_tee: '╩',
            has_distinct_junction_glyphs: true,
        }),
        "heavy" => Some(BorderChars {
            vertical: '┃', horizontal: '━', cross: '╋',
            left_tee: '┣', right_tee: '┫', top_tee: '┳', bottom_tee: '┻',
            has_distinct_junction_glyphs: true,
        }),
        "simple" => Some(BorderChars {
            vertical: '|', horizontal: '-', cross: '+',
            left_tee: '+', right_tee: '+', top_tee: '+', bottom_tee: '+',
            has_distinct_junction_glyphs: true,
        }),
        "spaces" => Some(BorderChars {
            vertical: ' ', horizontal: ' ', cross: ' ',
            left_tee: ' ', right_tee: ' ', top_tee: ' ', bottom_tee: ' ',
            has_distinct_junction_glyphs: false,
        }),
        // "single", "default", "number", and any unknown value.
        _ => Some(SINGLE),
    }
}

/// Whether `name` is a recognised `pane-border-lines` value.
pub fn is_valid(name: &str) -> bool {
    CHOICES.contains(&name.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_suppresses_borders() {
        assert_eq!(border_chars("none"), None);
    }

    #[test]
    fn single_is_default_and_uses_light_glyphs() {
        let bc = border_chars("single").unwrap();
        assert_eq!((bc.vertical, bc.horizontal, bc.cross), ('│', '─', '┼'));
        assert_eq!((bc.left_tee, bc.right_tee, bc.top_tee, bc.bottom_tee), ('├', '┤', '┬', '┴'));
        assert!(bc.has_distinct_junction_glyphs);
        // The catalog default resolves to the single glyph set.
        assert_eq!(border_chars(DEFAULT), Some(bc));
    }

    #[test]
    fn double_uses_double_glyphs() {
        let bc = border_chars("double").unwrap();
        assert_eq!((bc.vertical, bc.horizontal, bc.cross), ('║', '═', '╬'));
        assert_eq!((bc.left_tee, bc.right_tee, bc.top_tee, bc.bottom_tee), ('╠', '╣', '╦', '╩'));
    }

    #[test]
    fn heavy_uses_heavy_glyphs() {
        let bc = border_chars("heavy").unwrap();
        assert_eq!((bc.vertical, bc.horizontal, bc.cross), ('┃', '━', '╋'));
        assert_eq!((bc.left_tee, bc.right_tee, bc.top_tee, bc.bottom_tee), ('┣', '┫', '┳', '┻'));
    }

    #[test]
    fn simple_is_ascii() {
        let bc = border_chars("simple").unwrap();
        assert_eq!((bc.vertical, bc.horizontal, bc.cross), ('|', '-', '+'));
        // All junctions collapse to '+', matching tmux SIMPLE_BORDERS.
        assert_eq!((bc.left_tee, bc.right_tee, bc.top_tee, bc.bottom_tee), ('+', '+', '+', '+'));
    }

    #[test]
    fn spaces_use_the_same_glyph_for_lines_and_junctions() {
        let bc = border_chars("spaces").unwrap();
        assert_eq!(bc.vertical, ' ');
        assert_eq!(bc.horizontal, ' ');
        assert!(!bc.has_distinct_junction_glyphs);
    }

    #[test]
    fn number_and_unknown_fall_back_to_single() {
        assert_eq!(border_chars("number"), border_chars("single"));
        assert_eq!(border_chars("bogus"), border_chars("single"));
    }

    #[test]
    fn validity_matches_choices() {
        for c in CHOICES { assert!(is_valid(c), "{c} should be valid"); }
        assert!(!is_valid("bogus"));
        // Whitespace is tolerated by the parser.
        assert_eq!(border_chars("  double  "), border_chars("double"));
    }
}
