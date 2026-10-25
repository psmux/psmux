//! The single place that decides how many columns a codepoint occupies.
//!
//! Every width decision in psmux -- the emulator's wide/continuation cell
//! logic, `capture-pane`, the renderer, the status line and the layout
//! measurements -- routes through [`char_width`] / [`str_width`] here, so a
//! `codepoint-widths` override cannot be honoured in one place and ignored in
//! another. A disagreement between two width call sites is exactly how cells
//! get stranded on screen (issue #639), so there is deliberately no second
//! opinion available.
//!
//! # `codepoint-widths`
//!
//! tmux exposes the same escape hatch as a server option (`options-table.c`,
//! `OPTIONS_TABLE_IS_ARRAY` with a `,` separator) parsed by
//! `utf8_add_to_width_cache` in `utf8.c` and applied by `utf8_width`. This
//! module mirrors that parser entry for entry; see
//! [`parse_entry`] for the accepted syntax.
//!
//! The override table is process global, matching tmux's own global
//! `utf8_width_cache`, and is rebuilt wholesale whenever the option changes
//! (tmux's `utf8_update_width_cache`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;

/// Number of entries currently in the override table.
///
/// This is the hot path gate. `codepoint-widths` is empty for essentially
/// every user, so the common case must not pay for a lock: a single relaxed
/// atomic load of zero short circuits straight to `unicode-width`.
static OVERRIDE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Codepoint -> column count. Only consulted when `OVERRIDE_COUNT` is nonzero.
static OVERRIDES: RwLock<Option<HashMap<u32, u8>>> = RwLock::new(None);

/// The largest width tmux will accept for an override (`strtonum(cp, 0, 2)`).
pub const MAX_OVERRIDE_WIDTH: u8 = 2;

/// A parsed `codepoint-widths` entry: an inclusive codepoint range and the
/// width every codepoint in it should report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WidthOverride {
    /// First codepoint of the range (inclusive).
    pub start: u32,
    /// Last codepoint of the range (inclusive); equal to `start` for a single
    /// codepoint entry.
    pub end: u32,
    /// Columns the range should occupy: 0, 1 or 2.
    pub width: u8,
}

/// Parse one `codepoint-widths` array entry, matching tmux's
/// `utf8_add_to_width_cache` (`utf8.c`) exactly.
///
/// Accepted forms, all requiring a literal `=` before the width:
///
/// - `U+XXXX=N` -- a single codepoint written in hex. The `U+` prefix is
///   mandatory for the hex form and the hex digits must be the whole of the
///   rest of the token.
/// - `U+XXXX-U+YYYY=N` -- an inclusive range. tmux requires the `U+` prefix on
///   BOTH ends (`strncmp(endptr, "U+", 2) != 0` rejects `U+3000-3002`) and
///   rejects a range whose end is below its start.
/// - `<char>=N` -- a single literal character, e.g. `字=2`. tmux takes this
///   branch whenever the token does not start with `U+`, and rejects it unless
///   it decodes to exactly one codepoint.
///
/// `N` is validated with `strtonum(cp, 0, 2)`, so the only legal widths are
/// **0, 1 and 2**; anything else (including a negative number or trailing
/// junk) makes tmux drop the entry silently.
///
/// A codepoint of zero is rejected (tmux tests `n == 0`), as is one above the
/// Unicode maximum. Returns `None` for every malformed entry rather than
/// erroring, because tmux discards bad entries without complaint.
#[must_use]
pub fn parse_entry(entry: &str) -> Option<WidthOverride> {
    // tmux splits on the FIRST '=' (strchr), so a literal '=' character can be
    // given a width via "==1".
    let split = entry.find('=')?;
    let (spec, width_str) = (&entry[..split], &entry[split + 1..]);

    // strtonum(cp, 0, 2, &errstr): the whole token must be an integer in
    // 0..=2. tmux's strtonum is `strtoll` plus a `*ep != '\0'` check
    // (compat/strtonum.c:52), so it accepts what strtoll accepts at the FRONT
    // -- leading whitespace and a leading '+' -- but rejects any trailing
    // text. A negative value parses and is then refused by the `< minval`
    // bound. Rust's `u8::from_str` already accepts a leading '+' and rejects
    // trailing text and negatives, so only the leading whitespace needs
    // matching explicitly.
    let width: u8 = width_str.trim_start().parse().ok()?;
    if width > MAX_OVERRIDE_WIDTH {
        return None;
    }

    if let Some(hex) = spec.strip_prefix("U+") {
        // Range form: split on the '-' that separates the two U+ tokens.
        let (start_hex, end_hex) = match hex.find('-') {
            Some(dash) => {
                // tmux requires the second half to carry its own "U+".
                let rest = hex[dash + 1..].strip_prefix("U+")?;
                (&hex[..dash], Some(rest))
            }
            None => (hex, None),
        };
        let start = parse_codepoint(start_hex)?;
        let end = match end_hex {
            Some(e) => parse_codepoint(e)?,
            None => start,
        };
        // tmux: `(wchar_t)n < wc_start` rejects a descending range.
        if end < start {
            return None;
        }
        Some(WidthOverride { start, end, width })
    } else {
        // Literal character form: must be exactly one codepoint.
        let mut chars = spec.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let cp = c as u32;
        // tmux rejects a zero codepoint on the hex path; a literal NUL can
        // never appear in an option string, so this only guards the parser.
        if cp == 0 {
            return None;
        }
        Some(WidthOverride {
            start: cp,
            end: cp,
            width,
        })
    }
}

/// Parse the hex digits of a `U+XXXX` token.
fn parse_codepoint(hex: &str) -> Option<u32> {
    if hex.is_empty() {
        return None;
    }
    // tmux uses strtoull, which accepts a leading '+'/'-' and whitespace; the
    // surrounding checks then reject the result. Rust's from_str_radix accepts
    // a leading '+' too, so exclude sign characters explicitly to keep
    // "U+-1=2" from parsing.
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    // tmux: `n == 0 || n > WCHAR_MAX`. Rust chars top out at U+10FFFF, and a
    // surrogate is not a valid char, but the table is keyed by u32 so a
    // surrogate simply never matches anything.
    if n == 0 || n > 0x0010_FFFF {
        return None;
    }
    Some(n)
}

/// Rebuild the process global override table from the option's array entries.
///
/// This is tmux's `utf8_update_width_cache`, which `options.c` calls from the
/// option-changed hook so a live `set -s codepoint-widths ...` takes effect on
/// the next character drawn rather than at the next server start. Malformed
/// entries are dropped individually; a later entry wins over an earlier one
/// for the same codepoint, matching tmux's `utf8_insert_width_cache` replacing
/// the existing tree node.
pub fn set_codepoint_widths<S: AsRef<str>>(entries: &[S]) {
    let mut table: HashMap<u32, u8> = HashMap::new();
    for entry in entries {
        let entry = entry.as_ref().trim();
        if entry.is_empty() {
            continue;
        }
        let Some(WidthOverride { start, end, width }) = parse_entry(entry) else {
            continue;
        };
        for cp in start..=end {
            table.insert(cp, width);
        }
    }

    let count = table.len();
    if let Ok(mut guard) = OVERRIDES.write() {
        *guard = if count == 0 { None } else { Some(table) };
        // Publish the count only once the table is in place, and while still
        // holding the write lock, so a reader that sees a nonzero count is
        // guaranteed to find the table behind it.
        OVERRIDE_COUNT.store(count, Ordering::Release);
    }
}

/// Drop every override, restoring pure `unicode-width` behaviour.
pub fn clear_codepoint_widths() {
    set_codepoint_widths::<&str>(&[]);
}

/// True when at least one override is active. Cheap enough for a debug path.
#[must_use]
pub fn has_overrides() -> bool {
    OVERRIDE_COUNT.load(Ordering::Acquire) != 0
}

/// Look up an active override for `c`, if any.
#[must_use]
fn override_for(c: char) -> Option<u8> {
    // Fast path: no overrides configured, so never touch the lock.
    if OVERRIDE_COUNT.load(Ordering::Acquire) == 0 {
        return None;
    }
    let guard = OVERRIDES.read().ok()?;
    guard.as_ref()?.get(&(c as u32)).copied()
}

/// Columns `c` occupies, honouring `codepoint-widths`.
///
/// This is the shared replacement for `UnicodeWidthChar::width`. `None` keeps
/// unicode-width's meaning: `c` is a control character with no width at all,
/// which callers handle differently from a zero width combining mark. An
/// override always produces `Some`, since an explicit `=0` is a deliberate
/// zero width rather than "not printable".
///
/// tmux's default ambiguous-width resolution is 1 and psmux matches it; this
/// function does NOT change that. The override is opt in and empty by default.
#[must_use]
pub fn char_width(c: char) -> Option<usize> {
    if let Some(w) = override_for(c) {
        return Some(usize::from(w));
    }
    if c == SOFT_HYPHEN {
        return Some(1);
    }
    unicode_width::UnicodeWidthChar::width(c)
}

/// U+00AD SOFT HYPHEN. `unicode-width` calls it zero columns (it is a format
/// character), but Neovim, tmux and Markus Kuhn's wcwidth all draw it as one.
/// Counting it as zero put the emulator one column behind the program that
/// wrote it, so every following cell on the row landed in the wrong place.
const SOFT_HYPHEN: char = '\u{AD}';

/// Columns `s` occupies, honouring `codepoint-widths`.
///
/// The shared replacement for `UnicodeWidthStr::width`. With no overrides
/// active this delegates to `unicode-width` so grapheme handling is
/// byte-for-byte what it was before; only once an override exists does it fall
/// back to summing per character, which is the granularity the option works at.
#[must_use]
pub fn str_width(s: &str) -> usize {
    if OVERRIDE_COUNT.load(Ordering::Acquire) == 0 && !s.contains(SOFT_HYPHEN) {
        return unicode_width::UnicodeWidthStr::width(s);
    }
    s.chars().map(|c| char_width(c).unwrap_or(0)).sum()
}

// ---------------------------------------------------------------------------
// Combining characters into cells (#749)
// ---------------------------------------------------------------------------

/// U+FE0F VARIATION SELECTOR-16, which requests emoji presentation.
const VS16: char = '\u{FE0F}';

/// U+200D ZERO WIDTH JOINER, which glues the emoji either side of it into
/// one glyph (tmux `utf8_is_zwj`).
const ZWJ: char = '\u{200D}';

/// U+3164 HANGUL FILLER, which tmux ignores entirely (`screen_write_combine`
/// returns before looking at the grid). It is invisible and zero width here
/// anyway, so dropping it changes no column, only what a copy picks up.
const HANGUL_FILLER: char = '\u{3164}';

/// The most UTF-8 bytes one cell holds, combining characters included: tmux's
/// `UTF8_SIZE` (tmux.h).
pub const MAX_CELL_BYTES: usize = 32;

/// U+1F1E6 to U+1F1FF, the regional indicator letters a flag is spelled with.
fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// U+1F3FB to U+1F3FF, the five Fitzpatrick skin tone modifiers.
fn is_skin_tone_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// The emoji tmux lets a skin tone modifier attach to: the `switch (a)` table
/// in `utf8_should_combine` (utf8-combined.c), copied entry for entry.
fn takes_skin_tone(c: char) -> bool {
    matches!(
        u32::from(c),
        0x1F44B..=0x1F450
            | 0x1F466..=0x1F469
            | 0x1F46E
            | 0x1F470..=0x1F478
            | 0x1F47C
            | 0x1F481..=0x1F483
            | 0x1F485..=0x1F487
            | 0x1F4AA
            | 0x1F575
            | 0x1F57A
            | 0x1F590
            | 0x1F595
            | 0x1F596
            | 0x1F645..=0x1F647
            | 0x1F64B..=0x1F64F
            | 0x1F6B4..=0x1F6B6
            | 0x1F926
            | 0x1F937..=0x1F939
            | 0x1F93D
            | 0x1F93E
            | 0x1F9B5
            | 0x1F9B6
            | 0x1F9B8
            | 0x1F9B9
            | 0x1F9CD..=0x1F9CF
            | 0x1F9D1..=0x1F9DF
    )
}

/// tmux `utf8_should_combine(with, add)`. tmux decodes only the first code
/// point of each side (`mbtowc` stops after one character) but counts the
/// regional indicators over the whole of each, so a cell that already holds a
/// flag never takes a third indicator. The skin tone test reads the way tmux
/// wrote it, with the modifier on the `with` side; `join` tries both orders
/// exactly as `screen_write_combine` does, so a modifier combines after its
/// emoji and before it.
fn should_combine(with: &str, add: &str) -> bool {
    let (Some(w), Some(a)) = (with.chars().next(), add.chars().next()) else {
        return false;
    };
    if is_regional_indicator(a) && is_regional_indicator(w) {
        let count = |s: &str| s.chars().filter(|&c| is_regional_indicator(c)).count();
        return count(with) == 1 && count(add) == 1;
    }
    takes_skin_tone(a) && is_skin_tone_modifier(w)
}

/// What a character does to the cells it is written after.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Join {
    /// It is thrown away and no column moves.
    Discard,
    /// It starts a cell of its own, `char_width` columns wide.
    NewCell,
    /// It folds into the previous cell. `widen` says the cell goes from one
    /// column to two.
    Combine {
        /// The previous cell was one column and becomes two.
        widen: bool,
    },
}

/// Decide what `c` does after the cell `prev`: discard, new cell, or combine.
///
/// `width` is `c`'s own width; `prev` is the cell's text and its width in
/// columns, or `None` after nothing (the first column, or a cursor that is not
/// right after the start of a cell).
///
/// This is tmux's `screen_write_combine` (screen-write.c), and the one place
/// psmux makes that decision: the pane's grid (`Screen::text`) and the
/// prompts that must count columns the way the grid does (`str_cells`) both
/// ask here, so they cannot disagree.
///
/// Emoji presentation is a property of a *sequence*: a ZWJ family, a skin
/// tone pair, a flag of two regional indicators and a base followed by VS16
/// each draw as one glyph two columns wide in tmux and in nearly every
/// terminal (Windows Terminal, VS Code, `JetBrains`, `WezTerm`, Alacritty,
/// `ConEmu` and conhost; mintty is the one that draws the parts apart). The rules, all
/// tmux's:
/// - U+3164 HANGUL FILLER is discarded outright.
/// - ZWJ, VS16 and any other zero width character make no sense alone: they
///   fold into the previous cell and are discarded when there is none.
/// - VS16 widens a one column cell to two (#533; tmux's
///   `variation-selector-always-wide`, on by default). A cell already two
///   wide stays two.
/// - An ASCII character never combines.
/// - Any other character folds in only when `should_combine` pairs it with
///   the cell (a second regional indicator, or a skin tone modifier and its
///   emoji in either order), which also widens a one column cell to two, or
///   when the cell ends in a ZWJ, which keeps the cell's width. So a letter,
///   ZWJ and an emoji stay one column, as in tmux.
/// - Nothing folds past `MAX_CELL_BYTES` (tmux `UTF8_SIZE`): a zero width
///   character is then discarded and any other starts its own cell.
#[must_use]
pub fn join(prev: Option<(&str, usize)>, c: char, width: usize) -> Join {
    if c == HANGUL_FILLER {
        return Join::Discard;
    }
    let zero_width = c == ZWJ || c == VS16 || width == 0;
    let alone = if zero_width {
        Join::Discard
    } else {
        Join::NewCell
    };

    // tmux: "Cannot combine empty character or at left." `ud->size < 2` is a
    // single byte, which is ASCII.
    let Some((text, prev_width)) = prev else {
        return alone;
    };
    if c.is_ascii() {
        return alone;
    }

    let mut force_wide = c == VS16;
    if !zero_width {
        let mut buf = [0u8; 4];
        let add: &str = c.encode_utf8(&mut buf);
        if should_combine(text, add) || should_combine(add, text) {
            force_wide = true;
        } else if !text.ends_with(ZWJ) {
            return Join::NewCell;
        }
    }

    // A blank cell is stored empty but holds a space, as in tmux.
    if text.len().max(1) + c.len_utf8() > MAX_CELL_BYTES {
        return alone;
    }
    Join::Combine {
        widen: force_wide && prev_width == 1,
    }
}

/// Where each cell of `s` starts, as a byte offset, and its columns.
///
/// That is for `s` written into a row wide enough to hold all of it. This
/// walks `join` exactly as the grid does, so a prompt drawn in the same
/// terminal as a pane counts the columns the pane counts (#749, #750).
/// Control characters, which the grid does not draw, take no cell.
#[must_use]
pub fn str_cells(s: &str) -> Vec<(usize, usize)> {
    let mut cells: Vec<(usize, usize)> = Vec::new();
    // The text of the last cell. It is a run of `s` except where a discarded
    // character sat inside it, so it is kept apart.
    let mut last = String::new();
    for (i, c) in s.char_indices() {
        let Some(width) = char_width(c) else {
            continue;
        };
        let prev = cells.last().map(|&(_, w)| (last.as_str(), w));
        match join(prev, c, width) {
            Join::Discard => {}
            Join::NewCell => {
                cells.push((i, width));
                last.clear();
                last.push(c);
            }
            Join::Combine { widen } => {
                last.push(c);
                if widen {
                    if let Some(cell) = cells.last_mut() {
                        cell.1 = 2;
                    }
                }
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_hyphen_takes_one_column() {
        assert_eq!(char_width(SOFT_HYPHEN), Some(1));
        assert_eq!(str_width("a\u{AD}b"), 3);
    }

    #[test]
    fn parses_single_hex_codepoint() {
        assert_eq!(
            parse_entry("U+3000=2"),
            Some(WidthOverride {
                start: 0x3000,
                end: 0x3000,
                width: 2
            })
        );
    }

    #[test]
    fn parses_inclusive_range() {
        assert_eq!(
            parse_entry("U+2500-U+2502=2"),
            Some(WidthOverride {
                start: 0x2500,
                end: 0x2502,
                width: 2
            })
        );
    }

    #[test]
    fn parses_literal_character() {
        assert_eq!(
            parse_entry("\u{2502}=2"),
            Some(WidthOverride {
                start: 0x2502,
                end: 0x2502,
                width: 2
            })
        );
    }

    #[test]
    fn rejects_malformed_entries() {
        // tmux drops each of these silently.
        assert_eq!(parse_entry("U+3000"), None, "no '=' separator");
        assert_eq!(parse_entry("U+3000=3"), None, "width above 2");
        assert_eq!(parse_entry("U+3000=-1"), None, "negative width");
        assert_eq!(parse_entry("U+0=1"), None, "zero codepoint");
        assert_eq!(parse_entry("U+ZZZZ=1"), None, "non hex digits");
        assert_eq!(parse_entry("U+=1"), None, "empty hex");
        assert_eq!(parse_entry("U+2502-2504=2"), None, "range end lacks U+");
        assert_eq!(parse_entry("U+2504-U+2500=2"), None, "descending range");
        assert_eq!(parse_entry("ab=2"), None, "more than one literal char");
        assert_eq!(parse_entry("U+110000=1"), None, "above Unicode max");
    }

    #[test]
    fn accepts_every_legal_width() {
        for w in 0..=MAX_OVERRIDE_WIDTH {
            assert_eq!(
                parse_entry(&format!("U+3000={w}")).map(|o| o.width),
                Some(w)
            );
        }
    }
}
