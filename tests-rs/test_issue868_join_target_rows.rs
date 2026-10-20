// #868: the destination check of a cross-session join-pane runs before the
// source pane is extracted, so whatever the destination's inject would refuse
// must be refused here, or the pane ends up in neither session.

use super::check_target_rows;
use crate::cli::parse_target;

// Window 0 (@1, active) has tiled panes 0 and 1 (%1, %2) and a floating pane
// 2 (%3); window 1 (@2, named "a|b") has pane 0 (%4).
const ROWS: &str = "1|@1|0|main|0|%1|0\n\
                    1|@1|0|main|1|%2|0\n\
                    1|@1|0|main|2|%3|1\n\
                    0|@2|1|a|b|0|%4|0\n";

fn join(spec: &str) -> Option<Result<(), String>> {
    check_target_rows(ROWS, &parse_target(spec), true)
}

fn switch(spec: &str) -> Option<Result<(), String>> {
    check_target_rows(ROWS, &parse_target(spec), false)
}

const FLOAT: &str = "size or position can't split a floating pane";

#[test]
fn join_refuses_a_floating_pane_by_index() {
    assert_eq!(join("X:0.2"), Some(Err(FLOAT.to_string())));
    assert_eq!(join("X:.2"), Some(Err(FLOAT.to_string())));
}

#[test]
fn join_refuses_a_floating_pane_by_id() {
    assert_eq!(join("X:%3"), Some(Err(FLOAT.to_string())));
    assert_eq!(join("%3"), Some(Err(FLOAT.to_string())));
}

#[test]
fn join_accepts_tiled_panes_and_windows() {
    assert_eq!(join("X:0.0"), Some(Ok(())));
    assert_eq!(join("X:0.1"), Some(Ok(())));
    assert_eq!(join("X:%2"), Some(Ok(())));
    assert_eq!(join("X:0"), Some(Ok(())));
    assert_eq!(join("X:1.0"), Some(Ok(())));
}

#[test]
fn switch_still_accepts_a_floating_pane() {
    // switch-client may make a floating pane current; only join refuses it.
    assert_eq!(switch("X:0.2"), Some(Ok(())));
    assert_eq!(switch("X:%3"), Some(Ok(())));
}

#[test]
fn missing_window_and_pane_are_refused() {
    assert_eq!(join("X:5"), Some(Err("can't find window: 5".to_string())));
    assert_eq!(join("X:0.7"), Some(Err("can't find pane: 7".to_string())));
    assert_eq!(join("X:%9"), Some(Err("can't find pane: %9".to_string())));
}

#[test]
fn a_window_name_with_a_bar_still_matches() {
    let rows = "0|@2|1|a|b|0|%4|0\n";
    let pt = crate::types::ParsedTarget { window_name: Some("a|b".to_string()), pane: Some(0), ..Default::default() };
    assert_eq!(check_target_rows(rows, &pt, true), Some(Ok(())));
    let pt = crate::types::ParsedTarget { window_name: Some("a|b".to_string()), pane: Some(1), ..Default::default() };
    assert_eq!(check_target_rows(rows, &pt, true), Some(Err("can't find pane: 1".to_string())));
}

#[test]
fn a_server_without_the_floating_column_reads_as_tiled() {
    // An older server prints the six columns it knows (an unknown format
    // variable expands to nothing, so the line may also end in '|').
    let old = "1|@1|0|main|0|%1\n1|@1|0|main|1|%2|\n";
    assert_eq!(check_target_rows(old, &parse_target("X:0.1"), true), Some(Ok(())));
    assert_eq!(
        check_target_rows(old, &parse_target("X:0.5"), true),
        Some(Err("can't find pane: 5".to_string()))
    );
}

#[test]
fn nothing_parseable_is_none() {
    assert_eq!(check_target_rows("", &parse_target("X:0.1"), true), None);
    assert_eq!(check_target_rows("ERROR: nope\n", &parse_target("X:0.1"), true), None);
}
