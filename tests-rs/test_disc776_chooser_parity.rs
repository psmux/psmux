//! Discussion #776: chooser keys against tmux's `mode_tree_key`
//! (mode-tree.c, tag 3.7b).
//!
//! * Page Up and Page Down move by the list height, `mtd->height`
//!   (mode-tree.c:1318-1333), where psmux moved a fixed 10 rows.
//! * `q` and `C-g` exit every mode (:1289-1293); the tree and session
//!   choosers ignored `q`.
//! * `C-p`, `C-n`, `C-b`, `C-f` alias Up, Down, Page Up, Page Down (:1301,
//!   :1307, :1319, :1327).
//! * In the tree, `h`/`-` share the Left arm and `l`/`+` the Right arm
//!   (:1390-1412); psmux wired only the arrows, so `+` and `-` did nothing
//!   and `h`/`l` merely moved.
//! * Another session's windows carry pane rows, so they can be expanded
//!   like the current session's (window_tree_build_window builds pane items
//!   for every window).

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const ALT: KeyModifiers = KeyModifiers::ALT;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

// ---- paging -------------------------------------------------------------

#[test]
fn page_down_moves_by_the_list_height_not_ten() {
    // 40 lines drawn 25 high: from line 1 a page lands on line 26.
    assert_eq!(page_down(1, 40, 25), 26);
    // A short list of 6 rows pages by 6, not 10.
    assert_eq!(page_down(0, 40, 6), 6);
}

#[test]
fn page_down_stops_on_the_last_line() {
    // tmux breaks out of the loop at line_size - 1.
    assert_eq!(page_down(30, 34, 25), 33);
    assert_eq!(page_down(33, 34, 25), 33);
}

#[test]
fn page_down_on_an_empty_list_stays_at_zero() {
    assert_eq!(page_down(0, 0, 25), 0);
}

#[test]
fn page_up_moves_by_the_list_height_and_stops_at_zero() {
    assert_eq!(page_up(30, 25), 5);
    assert_eq!(page_up(4, 25), 0);
    assert_eq!(page_up(0, 25), 0);
}

#[test]
fn a_zero_height_page_does_not_move() {
    // tmux's loop runs `mtd->height` times, so a height of 0 moves nothing.
    assert_eq!(page_up(7, 0), 7);
    assert_eq!(page_down(7, 20, 0), 7);
}

// ---- exit keys ----------------------------------------------------------

#[test]
fn q_exits_every_chooser() {
    assert_eq!(mode_tree_alias(KeyCode::Char('q'), NONE, true), Some(ModeKey::Exit));
    assert_eq!(mode_tree_alias(KeyCode::Char('q'), NONE, false), Some(ModeKey::Exit));
}

#[test]
fn ctrl_g_exits_every_chooser() {
    assert_eq!(mode_tree_alias(KeyCode::Char('g'), CTRL, true), Some(ModeKey::Exit));
    assert_eq!(mode_tree_alias(KeyCode::Char('g'), CTRL, false), Some(ModeKey::Exit));
}

#[test]
fn plain_g_is_not_an_exit() {
    // `g` is Home in mode_tree_key; the chooser handles it itself.
    assert_eq!(mode_tree_alias(KeyCode::Char('g'), NONE, true), None);
}

#[test]
fn alt_q_and_ctrl_q_are_not_exit() {
    assert_eq!(mode_tree_alias(KeyCode::Char('q'), ALT, true), None);
    assert_eq!(mode_tree_alias(KeyCode::Char('q'), CTRL, true), None);
}

// ---- emacs style movement -----------------------------------------------

#[test]
fn ctrl_p_n_b_f_move_like_the_arrows_and_pages() {
    for tree in [true, false] {
        assert_eq!(mode_tree_alias(KeyCode::Char('p'), CTRL, tree), Some(ModeKey::Up));
        assert_eq!(mode_tree_alias(KeyCode::Char('n'), CTRL, tree), Some(ModeKey::Down));
        assert_eq!(mode_tree_alias(KeyCode::Char('b'), CTRL, tree), Some(ModeKey::PageUp));
        assert_eq!(mode_tree_alias(KeyCode::Char('f'), CTRL, tree), Some(ModeKey::PageDown));
    }
}

#[test]
fn ctrl_with_alt_is_not_an_alias() {
    assert_eq!(mode_tree_alias(KeyCode::Char('n'), CTRL | ALT, true), None);
}

// ---- expand and collapse ------------------------------------------------

#[test]
fn plus_and_l_expand_in_the_tree() {
    assert_eq!(mode_tree_alias(KeyCode::Char('+'), NONE, true), Some(ModeKey::Expand));
    // A US keyboard reports `+` with SHIFT held; that is still `+`.
    assert_eq!(mode_tree_alias(KeyCode::Char('+'), SHIFT, true), Some(ModeKey::Expand));
    assert_eq!(mode_tree_alias(KeyCode::Char('l'), NONE, true), Some(ModeKey::Expand));
}

#[test]
fn minus_and_h_collapse_in_the_tree() {
    assert_eq!(mode_tree_alias(KeyCode::Char('-'), NONE, true), Some(ModeKey::Collapse));
    assert_eq!(mode_tree_alias(KeyCode::Char('h'), NONE, true), Some(ModeKey::Collapse));
}

#[test]
fn flat_choosers_keep_their_own_h_l_plus_minus() {
    // tmux's `line->flat` makes h/l plain movement, which the session and
    // buffer choosers already do; + and - have nothing to expand there.
    for c in ['h', 'l', '+', '-'] {
        assert_eq!(mode_tree_alias(KeyCode::Char(c), NONE, false), None, "key {c}");
    }
}

#[test]
fn alt_letters_stay_jump_keys() {
    // M-a.. are the tree's jump keys for lines 10 and up.
    assert_eq!(mode_tree_alias(KeyCode::Char('h'), ALT, true), None);
    assert_eq!(mode_tree_alias(KeyCode::Char('l'), ALT, true), None);
}

#[test]
fn keys_the_chooser_owns_are_not_aliased() {
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right, KeyCode::Enter,
                 KeyCode::Esc, KeyCode::PageUp, KeyCode::PageDown, KeyCode::Char('j'),
                 KeyCode::Char('k'), KeyCode::Char('x'), KeyCode::Char('p'), KeyCode::Char('3')] {
        assert_eq!(mode_tree_alias(code, NONE, true), None, "{code:?}");
    }
}

// ---- the aliases drive the existing expand and collapse model ----------

/// Two sessions, each with a window of two panes and a window of one, as the
/// tree is now built: every session's windows carry their pane rows.
fn two_session_tree() -> Vec<RowKind> {
    vec![
        RowKind::Session, // 0 current
        RowKind::Window,  // 1
        RowKind::Pane,    // 2
        RowKind::Pane,    // 3
        RowKind::Window,  // 4
        RowKind::Pane,    // 5
        RowKind::Session, // 6 other session
        RowKind::Window,  // 7
        RowKind::Pane,    // 8
        RowKind::Pane,    // 9
        RowKind::Window,  // 10
        RowKind::Pane,    // 11
    ]
}

#[test]
fn another_sessions_window_can_be_expanded() {
    let kinds = two_session_tree();
    let collapsed = default_collapsed(&kinds);
    assert!(has_children(&kinds, 7), "other session's window has pane rows");
    assert_eq!(expand_action(&kinds, &collapsed, 7), ExpandAction::Expand(7));
}

#[test]
fn another_sessions_window_without_panes_was_a_leaf() {
    // The old builder dropped the other session's pane rows, which made its
    // window rows leaves: Right (and now +) could only move down.
    let kinds = vec![
        RowKind::Session, RowKind::Window, RowKind::Pane,
        RowKind::Session, RowKind::Window, RowKind::Window,
    ];
    let collapsed = default_collapsed(&kinds);
    assert_eq!(expand_action(&kinds, &collapsed, 4), ExpandAction::MoveDown);
}

#[test]
fn expanding_then_collapsing_restores_the_visible_lines() {
    let kinds = two_session_tree();
    let mut collapsed = default_collapsed(&kinds);
    let before = visible_lines(&kinds, &collapsed);
    if let ExpandAction::Expand(t) = expand_action(&kinds, &collapsed, 7) {
        collapsed.remove(&t);
    }
    let expanded = visible_lines(&kinds, &collapsed);
    assert_eq!(expanded.len(), before.len() + 2);
    assert!(expanded.contains(&8) && expanded.contains(&9));
    if let CollapseAction::Collapse(t) = collapse_action(&kinds, &collapsed, 7) {
        collapsed.insert(t);
    }
    assert_eq!(visible_lines(&kinds, &collapsed), before);
}
