//! The `clock-mode-style` option: the time format of the clock in clock mode,
//! a window option in tmux (options-table.c:1330-1336 at tag 3.8) with the
//! values of `options_table_clock_mode_style_list` (options-table.c:38-40).
//!
//! tmux's default is `24`. psmux keeps the format its clock always had,
//! `24-with-seconds`, as its default.

use crate::types::AppState;

pub(crate) const OPTION: &str = "clock-mode-style";

/// The values the option takes, in tmux's order.
pub const CHOICES: &[&str] = &["12", "24", "12-with-seconds", "24-with-seconds"];

/// psmux's default (tmux's is `24`).
pub const DEFAULT_NAME: &str = "24-with-seconds";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClockStyle {
    H12,
    H24,
    H12Seconds,
    /// psmux's default, see [`DEFAULT_NAME`].
    #[default]
    H24Seconds,
}

impl ClockStyle {
    /// One of [`CHOICES`] as it is: tmux compares with `strcmp`
    /// (`options_find_choice`, options.c:1239-1255 at tag 3.8).
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "12" => Self::H12,
            "24" => Self::H24,
            "12-with-seconds" => Self::H12Seconds,
            "24-with-seconds" => Self::H24Seconds,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::H12 => "12",
            Self::H24 => "24",
            Self::H12Seconds => "12-with-seconds",
            Self::H24Seconds => "24-with-seconds",
        }
    }
}

/// The text tmux's clock shows for `hour` (0 to 23), `minute` and `second`
/// in `style` (`window_clock_draw_screen`, window-clock.c:249-263 at tag
/// 3.8): `%l:%M ` or `%l:%M:%S ` and then `AM` or `PM`, so an hour under 10
/// starts with a space, or `%H:%M` or `%H:%M:%S`.
pub fn time_text(style: ClockStyle, hour: u32, minute: u32, second: u32) -> String {
    // %l: the hour on the 12 hour clock, 1 to 12, padded with a space.
    let h12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    let ampm = if hour >= 12 { "PM" } else { "AM" };
    match style {
        ClockStyle::H12 => format!("{h12:>2}:{minute:02} {ampm}"),
        ClockStyle::H12Seconds => format!("{h12:>2}:{minute:02}:{second:02} {ampm}"),
        ClockStyle::H24 => format!("{hour:02}:{minute:02}"),
        ClockStyle::H24Seconds => format!("{hour:02}:{minute:02}:{second:02}"),
    }
}

/// The style the server puts in the frame: the active window's own value,
/// else the global one, as tmux reads `w->options` (window-clock.c:243).
/// `None` when neither is set or the value is not one of [`CHOICES`], so an
/// ordinary frame carries nothing and the client draws its default.
pub(crate) fn frame_value(app: &AppState) -> Option<ClockStyle> {
    app.windows
        .get(app.active_idx)
        .and_then(|w| w.window_options.get(OPTION))
        .or_else(|| app.user_options.get(OPTION))
        .and_then(|v| ClockStyle::parse(v))
}

/// Add `"clock_style":"<name>"` to a frame `buf` that ends with `}`, when
/// [`frame_value`] gives one. Both servers call it.
pub(crate) fn append_frame_json(app: &AppState, buf: &mut String) {
    let Some(style) = frame_value(app) else { return };
    if !buf.ends_with('}') {
        return;
    }
    buf.pop();
    buf.push_str(",\"clock_style\":\"");
    buf.push_str(style.name());
    buf.push_str("\"}");
}

#[cfg(test)]
#[path = "../tests-rs/test_clock_mode_style.rs"]
mod tests;
