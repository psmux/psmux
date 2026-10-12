//! The choose-tree (prefix+w) list model, ported from tmux so that the rows a
//! user sees, the keys those rows answer to and the default collapse state all
//! match tmux instead of merely resembling it (issue #625).
//!
//! Three tmux functions are mirrored here:
//!
//! * `window_tree_build_window` (window-tree.c) decides the default expansion.
//!   `prefix+w` runs `choose-tree -Zw`, which sets `data->type` to
//!   `WINDOW_TREE_WINDOW`, and the builder reads:
//!
//!   ```text
//!   if (data->type == WINDOW_TREE_SESSION ||
//!       data->type == WINDOW_TREE_WINDOW)
//!           expanded = 0;
//!   else
//!           expanded = 1;
//!   ```
//!
//!   so every window row starts COLLAPSED and its panes are hidden.
//!   `window_tree_build_session` only collapses for `WINDOW_TREE_SESSION`, so
//!   under `-w` the session rows stay expanded and the windows are listed.
//!
//! * `mode_tree_build_lines` (mode-tree.c) walks the tree and emits a line for
//!   each item, recursing into children ONLY while the parent is expanded:
//!
//!   ```text
//!   mti->line = (mtd->line_size - 1);
//!   if (mti->expanded)
//!           mode_tree_build_lines(mtd, &mti->children, depth + 1);
//!   ```
//!
//!   then stamps the jump key on each visible line:
//!
//!   ```text
//!   } else if (mti->line < 10)
//!           mti->key = '0' + mti->line;
//!   else if (mti->line < 36)
//!           mti->key = KEYC_META|('a' + mti->line - 10);
//!   else
//!           mti->key = KEYC_NONE;
//!   ```
//!
//!   The key is therefore a ZERO based index over the VISIBLE lines, not the
//!   window number and not a one based row number.
//!
//! * `mode_tree_key` (mode-tree.c) consumes that key with no confirmation
//!   step at all:
//!
//!   ```text
//!   choice = -1;
//!   for (i = 0; i < mtd->line_size; i++) {
//!           if (*key == mtd->line_list[i].item->key) {
//!                   choice = i;
//!                   break;
//!           }
//!   }
//!   if (choice != -1) {
//!           ...
//!           mtd->current = choice;
//!           *key = '\r';
//!           return (0);
//!   }
//!   ```
//!
//!   The digit moves the cursor AND is rewritten to Enter, so
//!   `window_tree_key` runs its `'\r'` arm on the very same keystroke: the
//!   target is activated and the chooser closes. No Enter is typed by the
//!   user, and there is no multi digit accumulator to get stuck in.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyModifiers};

/// What a row in the flat chooser list represents. psmux stores rows as
/// `(is_win, win_id, pane_id, label, session)`, where a session header is
/// `is_win == true` with `win_id == usize::MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Session,
    Window,
    Pane,
}

/// Classify one psmux chooser row.
pub fn row_kind(is_win: bool, win_id: usize) -> RowKind {
    if !is_win {
        RowKind::Pane
    } else if win_id == usize::MAX {
        RowKind::Session
    } else {
        RowKind::Window
    }
}

/// The depth of a row, matching the `depth` argument `mode_tree_build_lines`
/// recurses with: sessions 0, windows 1, panes 2.
pub fn depth(kind: RowKind) -> usize {
    match kind {
        RowKind::Session => 0,
        RowKind::Window => 1,
        RowKind::Pane => 2,
    }
}

/// The index of a row's parent. Rows are emitted in tree order (a session,
/// then each of its windows, then each window's panes), so the parent is the
/// nearest preceding row one level shallower.
pub fn parent_of(kinds: &[RowKind], idx: usize) -> Option<usize> {
    let want = depth(*kinds.get(idx)?).checked_sub(1)?;
    (0..idx).rev().find(|&i| depth(kinds[i]) == want)
}

/// tmux `window_tree_build_window` under `choose-tree -w`: every window row
/// starts collapsed, so panes are hidden until the user expands the window.
/// Session rows stay expanded, which is what makes the visible line numbers
/// line up with the window numbers in the status bar.
pub fn default_collapsed(kinds: &[RowKind]) -> HashSet<usize> {
    kinds
        .iter()
        .enumerate()
        .filter(|(_, k)| **k == RowKind::Window)
        .map(|(i, _)| i)
        .collect()
}

/// tmux `mode_tree_build_lines`: the visible lines, in order, as indices into
/// the full row list. A row is visible only while every ancestor is expanded.
pub fn visible_lines(kinds: &[RowKind], collapsed: &HashSet<usize>) -> Vec<usize> {
    let mut out = Vec::with_capacity(kinds.len());
    // The deepest level currently being skipped; rows at or below it are
    // children of a collapsed ancestor.
    let mut hide_below: Option<usize> = None;
    for (i, kind) in kinds.iter().enumerate() {
        let d = depth(*kind);
        if let Some(limit) = hide_below {
            if d > limit {
                continue;
            }
            hide_below = None;
        }
        out.push(i);
        if collapsed.contains(&i) {
            hide_below = Some(d);
        }
    }
    out
}

/// Whether the tree has no expandable rows at all. tmux tracks this per line
/// as `line->flat` and uses it to make Left/Right fall back to plain cursor
/// movement.
pub fn is_flat(kinds: &[RowKind]) -> bool {
    !kinds.iter().any(|k| depth(*k) > 0)
}

/// The jump key tmux stamps on a visible line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKey {
    /// `'0' + line` for lines 0..9.
    Digit(char),
    /// `KEYC_META|('a' + line - 10)` for lines 10..35.
    Meta(char),
    /// `KEYC_NONE` for line 36 and beyond.
    None,
}

/// tmux `mode_tree_build_lines` key assignment for a zero based visible line.
pub fn line_key(line: usize) -> LineKey {
    if line < 10 {
        LineKey::Digit((b'0' + line as u8) as char)
    } else if line < 36 {
        LineKey::Meta((b'a' + (line - 10) as u8) as char)
    } else {
        LineKey::None
    }
}

/// How tmux prints that key: `key_string_lookup_key` gives "0".."9" and
/// "M-a".."M-z", and an unkeyed line prints nothing.
pub fn line_key_label(line: usize) -> String {
    match line_key(line) {
        LineKey::Digit(c) => c.to_string(),
        LineKey::Meta(c) => format!("M-{}", c),
        LineKey::None => String::new(),
    }
}

/// tmux `mode_tree_key`: the visible line a bare digit selects.
pub fn digit_line(c: char) -> Option<usize> {
    if c.is_ascii_digit() {
        Some((c as u8 - b'0') as usize)
    } else {
        None
    }
}

/// tmux `mode_tree_key`: the visible line an `M-<letter>` selects.
pub fn meta_line(c: char) -> Option<usize> {
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        Some(10 + (lower as u8 - b'a') as usize)
    } else {
        None
    }
}

/// What tmux's `KEYC_LEFT` / `h` / `-` arm does, expressed as a decision:
///
/// ```text
/// if (line->flat || !current->expanded)
///         current = current->parent;
/// if (current == NULL)
///         mode_tree_up(mtd, 0);
/// else {
///         current->expanded = 0;
///         mtd->current = current->line;
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollapseAction {
    /// Collapse this row (an index into the full row list) and select it.
    Collapse(usize),
    /// No parent to climb to: move the cursor up one visible line.
    MoveUp,
}

/// Resolve the Left/collapse key for the row at `idx` in the full row list.
pub fn collapse_action(
    kinds: &[RowKind],
    collapsed: &HashSet<usize>,
    idx: usize,
) -> CollapseAction {
    let flat = is_flat(kinds);
    let target = if flat || collapsed.contains(&idx) || !has_children(kinds, idx) {
        parent_of(kinds, idx)
    } else {
        Some(idx)
    };
    match target {
        Some(t) => CollapseAction::Collapse(t),
        None => CollapseAction::MoveUp,
    }
}

/// What tmux's `KEYC_RIGHT` / `l` / `+` arm does:
///
/// ```text
/// if (line->flat || current->expanded)
///         mode_tree_down(mtd, 0);
/// else if (!line->flat) {
///         current->expanded = 1;
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandAction {
    /// Expand this row.
    Expand(usize),
    /// Already expanded, or nothing to expand: move the cursor down one line.
    MoveDown,
}

/// Resolve the Right/expand key for the row at `idx` in the full row list.
pub fn expand_action(
    kinds: &[RowKind],
    collapsed: &HashSet<usize>,
    idx: usize,
) -> ExpandAction {
    if is_flat(kinds) || !collapsed.contains(&idx) || !has_children(kinds, idx) {
        ExpandAction::MoveDown
    } else {
        ExpandAction::Expand(idx)
    }
}

/// Whether a row has any child rows at all. A window with no panes listed and
/// a pane row are both leaves, and tmux never gives a leaf an expand marker.
pub fn has_children(kinds: &[RowKind], idx: usize) -> bool {
    let Some(k) = kinds.get(idx) else { return false };
    let d = depth(*k);
    matches!(kinds.get(idx + 1), Some(next) if depth(*next) > d)
}

/// tmux `mode_tree_key`, `KEYC_PPAGE` / `C-b` (mode-tree.c:1318-1325 in 3.7b):
///
/// ```text
/// for (i = 0; i < mtd->height; i++) {
///         if (mtd->current == 0)
///                 break;
///         mode_tree_up(mtd, 1);
/// }
/// ```
///
/// A page is the height of the list as drawn, not a fixed count, so a tall
/// chooser pages further than a short one (discussion #776).
pub fn page_up(current: usize, height: usize) -> usize {
    current.saturating_sub(height)
}

/// tmux `mode_tree_key`, `KEYC_NPAGE` / `C-f` (mode-tree.c:1326-1333): down
/// by the list height, stopping on the last line.
pub fn page_down(current: usize, len: usize, height: usize) -> usize {
    current.saturating_add(height).min(len.saturating_sub(1))
}

/// A key `mode_tree_key` gives a meaning that psmux's choosers spell with a
/// different key. Each one is turned into the key the chooser already
/// handles, so the alias and the original can never drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeKey {
    /// `q`, `C-g` (and Escape, `C-[`): `return (1)`, the mode exits.
    Exit,
    /// `C-p`, as Up.
    Up,
    /// `C-n`, as Down.
    Down,
    /// `C-b`, as Page Up.
    PageUp,
    /// `C-f`, as Page Down.
    PageDown,
    /// `h` and `-` in a tree, as Left (collapse).
    Collapse,
    /// `l` and `+` in a tree, as Right (expand).
    Expand,
}

/// Resolve a key to its `mode_tree_key` meaning when psmux spells that
/// meaning differently (mode-tree.c:1288-1412 in 3.7b). `tree` is true for
/// the tree chooser, whose rows have depth; the session and buffer choosers
/// are flat lists, where tmux's `line->flat` makes `h`/`l` plain movement,
/// which those choosers already do, so `h`, `l`, `+` and `-` are left alone
/// there.
///
/// Returns `None` for every key the chooser already handles itself.
pub fn mode_tree_alias(code: KeyCode, mods: KeyModifiers, tree: bool) -> Option<ModeKey> {
    let ctrl = mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT);
    let plain = !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT);
    match code {
        KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
            'g' => Some(ModeKey::Exit),
            'p' => Some(ModeKey::Up),
            'n' => Some(ModeKey::Down),
            'b' => Some(ModeKey::PageUp),
            'f' => Some(ModeKey::PageDown),
            _ => None,
        },
        KeyCode::Char('q') if plain => Some(ModeKey::Exit),
        KeyCode::Char('h') | KeyCode::Char('-') if plain && tree => Some(ModeKey::Collapse),
        KeyCode::Char('l') | KeyCode::Char('+') if plain && tree => Some(ModeKey::Expand),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue625_choose_tree_lines.rs"]
mod tests_issue625_choose_tree_lines;

#[cfg(test)]
#[path = "../tests-rs/test_disc776_chooser_parity.rs"]
mod tests_disc776_chooser_parity;
