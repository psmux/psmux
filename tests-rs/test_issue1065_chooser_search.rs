//! Issue #1065: search in the choosers, tmux's `/` in tree mode.
//!
//! tmux `mode_tree_key` opens a `(search) ` prompt on `?`, `/` or `C-s`
//! and repeats the last search with `n` (forward) and `N` (backward)
//! (mode-tree.c:1769-1784). The walk (`mode_tree_search_forward` and
//! `mode_tree_search_backward`) visits items in tree pre order starting
//! after the current one, wraps, never tests the current item, and the hit's
//! parents are expanded (`mode_tree_search_set`). A search written all in
//! lower case ignores case (`mode_tree_is_lowercase`).

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};
use std::collections::HashSet;

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const ALT: KeyModifiers = KeyModifiers::ALT;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

// ---- keys ---------------------------------------------------------------

#[test]
fn slash_question_mark_and_ctrl_s_open_the_prompt() {
    assert!(opens_search(KeyCode::Char('/'), NONE));
    assert!(opens_search(KeyCode::Char('?'), NONE));
    assert!(opens_search(KeyCode::Char('?'), SHIFT));
    assert!(opens_search(KeyCode::Char('s'), CTRL));
    assert!(opens_search(KeyCode::Char('S'), CTRL));
}

#[test]
fn other_keys_do_not_open_the_prompt() {
    assert!(!opens_search(KeyCode::Char('s'), NONE));
    assert!(!opens_search(KeyCode::Char('/'), ALT));
    assert!(!opens_search(KeyCode::Char('/'), CTRL));
    assert!(!opens_search(KeyCode::Char('s'), ALT));
    assert!(!opens_search(KeyCode::Enter, NONE));
}

#[test]
fn n_repeats_forward_and_shift_n_backward() {
    assert_eq!(repeat_search(KeyCode::Char('n'), NONE), Some(SearchDir::Forward));
    assert_eq!(repeat_search(KeyCode::Char('N'), SHIFT), Some(SearchDir::Backward));
    assert_eq!(repeat_search(KeyCode::Char('N'), NONE), Some(SearchDir::Backward));
    assert_eq!(repeat_search(KeyCode::Char('n'), CTRL), None);
    assert_eq!(repeat_search(KeyCode::Char('n'), ALT), None);
    assert_eq!(repeat_search(KeyCode::Char('m'), NONE), None);
}

// ---- case ---------------------------------------------------------------

#[test]
fn a_lower_case_search_ignores_case() {
    assert!(search_ignores_case("logs"));
    assert!(search_ignores_case("my-app 2"));
    assert!(search_hit("Build-Logs", "logs", true));
    assert!(search_hit("BUILD", "build", true));
}

#[test]
fn any_upper_case_letter_makes_the_search_exact() {
    assert!(!search_ignores_case("Logs"));
    assert!(search_hit("Logs", "Logs", false));
    assert!(!search_hit("logs", "Logs", false));
}

#[test]
fn non_ascii_case_folds_too() {
    assert!(search_ignores_case("журнал"));
    assert!(search_hit("Журнал", "журнал", true));
    assert!(!search_ignores_case("Ж"));
}

// ---- the walk -----------------------------------------------------------

fn names() -> Vec<&'static str> {
    // tree order: session, its windows, their panes
    vec!["alpha", "first", "beta", "editor", "logs", "gamma", "logs2"]
}

fn find(current: usize, dir: SearchDir, search: &str) -> Option<usize> {
    let n = names();
    let icase = search_ignores_case(search);
    search_step(n.len(), current, dir, |i| search_hit(n[i], search, icase))
}

#[test]
fn forward_finds_the_next_hit_after_the_cursor() {
    assert_eq!(find(0, SearchDir::Forward, "logs"), Some(4));
    assert_eq!(find(4, SearchDir::Forward, "logs"), Some(6));
}

#[test]
fn forward_wraps_to_the_top() {
    assert_eq!(find(6, SearchDir::Forward, "logs"), Some(4));
    assert_eq!(find(5, SearchDir::Forward, "alpha"), Some(0));
}

#[test]
fn backward_walks_up_and_wraps_to_the_bottom() {
    assert_eq!(find(6, SearchDir::Backward, "logs"), Some(4));
    assert_eq!(find(4, SearchDir::Backward, "logs"), Some(6));
    assert_eq!(find(0, SearchDir::Backward, "gamma"), Some(5));
}

#[test]
fn the_current_row_is_never_a_hit() {
    // tmux breaks out of the loop when it comes back to `last` before
    // testing it, so the selected row alone finds nothing.
    assert_eq!(find(1, SearchDir::Forward, "first"), None);
    assert_eq!(find(1, SearchDir::Backward, "first"), None);
}

#[test]
fn no_hit_is_none() {
    assert_eq!(find(0, SearchDir::Forward, "nothing"), None);
}

#[test]
fn an_empty_list_or_a_cursor_past_it_finds_nothing() {
    assert_eq!(search_step(0, 0, SearchDir::Forward, |_| true), None);
    assert_eq!(search_step(3, 3, SearchDir::Forward, |_| true), None);
}

#[test]
fn the_walk_tests_each_other_row_once() {
    let mut seen = Vec::new();
    let r = search_step(5, 2, SearchDir::Forward, |i| { seen.push(i); false });
    assert_eq!(r, None);
    assert_eq!(seen, vec![3, 4, 0, 1]);
    seen.clear();
    search_step(5, 2, SearchDir::Backward, |i| { seen.push(i); false });
    assert_eq!(seen, vec![1, 0, 4, 3]);
}

#[test]
fn the_walk_stops_at_the_first_hit() {
    // Pane rows ask a server for their command, so the walk must not test
    // rows past the hit.
    let mut seen = Vec::new();
    let r = search_step(6, 0, SearchDir::Forward, |i| { seen.push(i); i == 2 });
    assert_eq!(r, Some(2));
    assert_eq!(seen, vec![1, 2]);
}

// ---- expanding the hit's parents ----------------------------------------

#[test]
fn a_hit_inside_collapsed_rows_expands_its_parents() {
    use RowKind::*;
    // s0, w1 (p2, p3), s4, w5 (p6)
    let kinds = vec![Session, Window, Pane, Pane, Session, Window, Pane];
    let mut collapsed: HashSet<usize> = default_collapsed(&kinds);
    collapsed.insert(4);
    // The pane row 6 is hidden under the collapsed session 4 and window 5.
    assert!(!visible_lines(&kinds, &collapsed).contains(&6));
    expand_ancestors(&kinds, &mut collapsed, 6);
    assert!(visible_lines(&kinds, &collapsed).contains(&6));
    // Only its own parents opened: window 1 stays collapsed.
    assert!(collapsed.contains(&1));
    assert!(!collapsed.contains(&4));
    assert!(!collapsed.contains(&5));
}

#[test]
fn a_session_hit_expands_nothing() {
    use RowKind::*;
    let kinds = vec![Session, Window, Pane, Session, Window];
    let mut collapsed: HashSet<usize> = default_collapsed(&kinds);
    let before = collapsed.clone();
    expand_ancestors(&kinds, &mut collapsed, 3);
    assert_eq!(collapsed, before);
}
