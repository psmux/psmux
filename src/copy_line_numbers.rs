//! Copy-mode line numbers (tmux `copy-mode-line-numbers`).
//!
//! Pure logic for the left gutter shown in copy mode, and for the position
//! indicator in the top right, which reads the same numbers. The rendering side
//! (client.rs) asks this module for the gutter width, the number to print on
//! each visible row, and the indicator's position/limit pair; all the mode
//! arithmetic lives here so it can be unit tested against tmux's formulas
//! (window-copy.c).
//!
//! Row/offset conventions match tmux `window_copy_write_line`:
//! - `py`    visible row index, 0 = top of the pane.
//! - `oy`    scroll offset, 0 = scrolled to the live bottom.
//! - `cy`    the copy cursor's visible row.
//! - `hsize` number of scrollback (history) rows above the visible area.
//!
//! Modes:
//! - `off`       no gutter.
//! - `default`   distance of the row from the scroll offset: `|py - oy|`.
//! - `absolute`  1-based absolute line in the full grid: `hsize - oy + py + 1`.
//! - `relative`  distance from the cursor row: `|py - cy|` (cursor line = 0).
//! - `hybrid`    cursor line shows `absolute`; every other row shows `relative`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyLnMode {
    Off,
    Default,
    Absolute,
    Relative,
    Hybrid,
}

/// The valid choice values (tmux order), for validation / customize-mode.
pub const CHOICES: &[&str] = &["off", "default", "absolute", "relative", "hybrid"];

/// The default value (tmux `default_num = 0` -> first choice `off`).
pub const DEFAULT: &str = "off";

impl CopyLnMode {
    /// Parse an option value. Unknown values are treated as `off`.
    pub fn parse(name: &str) -> CopyLnMode {
        match name.trim() {
            "default" => CopyLnMode::Default,
            "absolute" => CopyLnMode::Absolute,
            "relative" => CopyLnMode::Relative,
            "hybrid" => CopyLnMode::Hybrid,
            _ => CopyLnMode::Off,
        }
    }

    pub fn is_active(self) -> bool {
        self != CopyLnMode::Off
    }
}

/// Total gutter width in columns, including the trailing space separator.
/// Returns 0 when the gutter is inactive. Mirrors
/// `window_copy_line_number_width`: digits of `hsize + height + 1`, minimum 3,
/// plus 1 for the separating space.
pub fn gutter_width(mode: CopyLnMode, hsize: usize, height: usize) -> usize {
    if !mode.is_active() {
        return 0;
    }
    let mut lines = hsize + height + 1;
    let mut digits = 1;
    while lines >= 10 {
        lines /= 10;
        digits += 1;
    }
    if digits < 3 {
        digits = 3;
    }
    digits + 1
}

/// The number to print on visible row `py`.
pub fn line_number(mode: CopyLnMode, py: usize, oy: usize, cy: usize, hsize: usize) -> usize {
    match mode {
        CopyLnMode::Off => 0,
        CopyLnMode::Default => py.abs_diff(oy),
        CopyLnMode::Absolute => (hsize + py + 1).saturating_sub(oy),
        CopyLnMode::Relative => py.abs_diff(cy),
        CopyLnMode::Hybrid => {
            if py == cy {
                (hsize + py + 1).saturating_sub(oy)
            } else {
                py.abs_diff(cy)
            }
        }
    }
}

/// Format the gutter cell for row `py`: the number right-aligned in
/// `width - 1` columns followed by a single space, exactly `width` columns.
pub fn gutter_text(mode: CopyLnMode, width: usize, py: usize, oy: usize, cy: usize, hsize: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let n = line_number(mode, py, oy, cy, hsize);
    format!("{:>w$} ", n, w = width - 1)
}

/// Whether row `py` is the current (cursor) line, which is styled distinctly.
pub fn is_current_row(py: usize, cy: usize) -> bool {
    py == cy
}

/// tmux `window_copy_line_number_is_absolute`: the modes that count lines from
/// the top of the history rather than from the scroll offset.
fn is_absolute(mode: CopyLnMode) -> bool {
    matches!(mode, CopyLnMode::Absolute | CopyLnMode::Relative | CopyLnMode::Hybrid)
}

/// The `(position, limit)` pair the copy-mode position indicator prints.
///
/// It lives here because the gutter mode decides which of the two readings
/// tmux uses (`window_copy_formats` in `window-copy.c`). With the gutter off or
/// in `default` mode the pair is the scroll offset over the scrollback size.
/// In `absolute`, `relative` and `hybrid` mode it is the 1-based absolute line
/// under the top of the view over the total number of lines, so the indicator
/// agrees with the numbers in the gutter beside it.
pub fn position_pair(mode: CopyLnMode, oy: usize, hsize: usize, height: usize) -> (usize, usize) {
    if is_absolute(mode) {
        ((hsize + 1).saturating_sub(oy), hsize + height)
    } else {
        (oy, hsize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measured against tmux 3.7c (`-L postest`, 30-row pane, `seq 1 200`):
    /// at the live bottom `#{copy_position}/#{copy_position_limit}` read
    /// `0/173`, after scrolling up `68/173`, and with
    /// `copy-mode-line-numbers absolute` the same view read `106/203`.
    #[test]
    fn position_pair_matches_tmux() {
        assert_eq!(position_pair(CopyLnMode::Off, 0, 173, 30), (0, 173));
        assert_eq!(position_pair(CopyLnMode::Off, 68, 173, 30), (68, 173));
        assert_eq!(position_pair(CopyLnMode::Default, 68, 173, 30), (68, 173));
        assert_eq!(position_pair(CopyLnMode::Absolute, 68, 173, 30), (106, 203));
        assert_eq!(position_pair(CopyLnMode::Relative, 68, 173, 30), (106, 203));
        assert_eq!(position_pair(CopyLnMode::Hybrid, 68, 173, 30), (106, 203));
    }

    #[test]
    fn position_pair_survives_an_offset_past_the_history() {
        // A frame caught mid resize can carry an offset larger than the
        // history it was measured against. Saturating keeps the position at 0
        // instead of wrapping to a huge number.
        assert_eq!(position_pair(CopyLnMode::Absolute, 500, 100, 30), (0, 130));
    }

    #[test]
    fn parse_and_off() {
        assert_eq!(CopyLnMode::parse("off"), CopyLnMode::Off);
        assert_eq!(CopyLnMode::parse("bogus"), CopyLnMode::Off);
        assert_eq!(CopyLnMode::parse(DEFAULT), CopyLnMode::Off);
        assert!(!CopyLnMode::Off.is_active());
        assert!(CopyLnMode::Relative.is_active());
        assert_eq!(gutter_width(CopyLnMode::Off, 1000, 40), 0);
    }

    #[test]
    fn width_min_three_plus_space() {
        // hsize+height+1 = 41 -> 2 digits -> min 3 -> +1 = 4
        assert_eq!(gutter_width(CopyLnMode::Absolute, 0, 40), 4);
        // 10000 -> 5 digits -> +1 = 6
        assert_eq!(gutter_width(CopyLnMode::Absolute, 9959, 40), 6);
    }

    #[test]
    fn absolute_counts_from_history_top() {
        // At the live bottom (oy = 0): absolute = hsize + py + 1.
        assert_eq!(line_number(CopyLnMode::Absolute, 0, 0, 5, 100), 101);
        assert_eq!(line_number(CopyLnMode::Absolute, 3, 0, 5, 100), 104);
        // Scrolled up 10: absolute shifts down by 10.
        assert_eq!(line_number(CopyLnMode::Absolute, 0, 10, 5, 100), 91);
    }

    #[test]
    fn relative_is_distance_from_cursor() {
        assert_eq!(line_number(CopyLnMode::Relative, 5, 0, 5, 100), 0); // cursor line
        assert_eq!(line_number(CopyLnMode::Relative, 2, 0, 5, 100), 3);
        assert_eq!(line_number(CopyLnMode::Relative, 9, 0, 5, 100), 4);
    }

    #[test]
    fn default_is_distance_from_offset() {
        // oy = 0: number equals the row index.
        assert_eq!(line_number(CopyLnMode::Default, 0, 0, 5, 100), 0);
        assert_eq!(line_number(CopyLnMode::Default, 7, 0, 5, 100), 7);
        // oy = 3: |py - 3|
        assert_eq!(line_number(CopyLnMode::Default, 1, 3, 5, 100), 2);
    }

    #[test]
    fn hybrid_cursor_absolute_others_relative() {
        // Cursor row shows absolute...
        assert_eq!(line_number(CopyLnMode::Hybrid, 5, 0, 5, 100), 106);
        // ...others show relative distance.
        assert_eq!(line_number(CopyLnMode::Hybrid, 2, 0, 5, 100), 3);
        assert_eq!(line_number(CopyLnMode::Hybrid, 8, 0, 5, 100), 3);
    }

    #[test]
    fn gutter_text_is_right_aligned_with_trailing_space() {
        // width 4 -> 3-wide number + space.
        assert_eq!(gutter_text(CopyLnMode::Relative, 4, 2, 0, 5, 100), "  3 ");
        assert_eq!(gutter_text(CopyLnMode::Absolute, 6, 0, 0, 5, 9959), " 9960 ");
    }
}
