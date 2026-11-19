// AttachConsole retry table (see `attach_retry_delays_ms`).
//
// An attach right after this process attached to and left the same console
// came back now and then with ERROR_INVALID_FUNCTION (1), and the wheel
// record that attach was for was dropped. Only that error waits and tries
// again; the others either have their own retry (5) or cannot change by
// waiting (6: no console, 87: no process), so they must stay immediate.

use super::*;

#[test]
fn invalid_function_is_retried_with_short_waits() {
    let d = attach_retry_delays_ms(1);
    assert!(!d.is_empty(), "error 1 must be retried");
    assert!(d.len() <= 3, "a few tries at most: {:?}", d);
    assert_eq!(d[0], 1, "the first retry comes 1 ms later (10 of 10 took there)");
    let total: u64 = d.iter().sum();
    assert!(total <= 10, "the console lock is held across the waits: {} ms", total);
}

#[test]
fn other_attach_errors_are_not_waited_on() {
    for err in [0u32, 5, 6, 87, 31, 1450] {
        assert!(attach_retry_delays_ms(err).is_empty(), "error {} must not wait", err);
    }
}

#[test]
fn invalid_function_has_a_meaning_in_the_failure_report() {
    assert!(attach_error_meaning(1).starts_with("ERROR_INVALID_FUNCTION"));
}
