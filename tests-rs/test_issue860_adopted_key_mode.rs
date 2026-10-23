// Issue #860 regression: a pane created before the config (or a later `set`)
// chose `extended-keys` takes the key mode tmux would give a pane created at
// the moment it is adopted (spare transplant, warm claim, early boot pane).
//
// Only the pure rule is tested here: `reconcile_consumed_parser` reads the
// process wide option that other tests in this binary may be using.

use crate::warm_pane_sync::consumed_extended_keys_mode as adopt;

#[test]
fn always_forces_mode_1_on_a_standard_pane() {
    assert_eq!(adopt(0, 2), 1);
}

#[test]
fn always_keeps_a_mode_the_application_asked_for() {
    assert_eq!(adopt(1, 2), 1);
    assert_eq!(adopt(2, 2), 2);
}

#[test]
fn off_drops_a_mode_left_from_an_older_value() {
    assert_eq!(adopt(1, 0), 0);
    assert_eq!(adopt(2, 0), 0);
    assert_eq!(adopt(0, 0), 0);
}

#[test]
fn on_keeps_whatever_the_application_asked_for() {
    assert_eq!(adopt(0, 1), 0);
    assert_eq!(adopt(1, 1), 1);
    assert_eq!(adopt(2, 1), 2);
}
