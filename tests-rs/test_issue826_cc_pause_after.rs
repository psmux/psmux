// Issue #826: control mode pause-after never paused a pane under backlog and
// output queued without bound. The ControlOutputGate follows tmux control.c:
// the age that counts is that of the OLDEST output the client was not sent
// yet (control_check_age); past pause-after the pane is paused and its
// backlog dropped until `refresh-client -A '%N:continue'`; without
// pause-after a client CONTROL_MAXIMUM_AGE behind is dropped.

use super::*;
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn parse_pause_after_flag_follows_tmux() {
    assert_eq!(parse_pause_after_flag("pause-after=1"), Some(Some(1)));
    assert_eq!(parse_pause_after_flag("pause-after=30"), Some(Some(30)));
    // Bare pause-after is an age of 0 (server-client.c).
    assert_eq!(parse_pause_after_flag("pause-after"), Some(Some(0)));
    assert_eq!(parse_pause_after_flag("!pause-after"), Some(None));
    assert_eq!(parse_pause_after_flag("no-pause"), Some(None));
    // sscanf("%u") reads the leading digits.
    assert_eq!(parse_pause_after_flag("pause-after=5s"), Some(Some(5)));
    assert_eq!(parse_pause_after_flag("pause-after=x"), None);
    assert_eq!(parse_pause_after_flag("no-output"), None);
    assert_eq!(parse_pause_after_flag("read-only"), None);
}

#[test]
fn fast_client_gets_every_chunk_with_its_real_age() {
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(1000));
    let t0 = Instant::now();
    for i in 0..100u64 {
        let now = t0 + ms(i * 5);
        let seq = match g.enqueue(1, now) {
            GateEnqueue::Send(s) => s,
            other => panic!("chunk {} not sent: {:?}", i, other),
        };
        // Written 2 ms later: %extended-output with age 2.
        assert_eq!(
            g.dequeue(1, seq, now, now + ms(2)),
            GateWrite::Write { age_ms: 2, extended: true }
        );
    }
    assert!(!g.is_paused(1));
    assert!(g.take_pending_pauses().is_empty());
}

#[test]
fn without_pause_after_output_is_plain() {
    let g = ControlOutputGate::new(None);
    let t0 = Instant::now();
    let GateEnqueue::Send(seq) = g.enqueue(3, t0) else { panic!() };
    assert_eq!(g.dequeue(3, seq, t0, t0), GateWrite::Write { age_ms: 0, extended: false });
}

#[test]
fn stalled_client_pauses_pane_when_backlog_older_than_pause_after() {
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(1000));
    let t0 = Instant::now();
    // The client stops reading: chunks queue, none is written.
    let mut seqs = Vec::new();
    for i in 0..10u64 {
        match g.enqueue(1, t0 + ms(i * 100)) {
            GateEnqueue::Send(s) => seqs.push(s),
            other => panic!("chunk {} at {} ms: {:?}", i, i * 100, other),
        }
    }
    // Oldest backlog is now 1000 ms old: the pane pauses (not before).
    assert_eq!(g.enqueue(1, t0 + ms(1000)), GateEnqueue::Paused);
    assert!(g.is_paused(1));
    assert_eq!(g.take_pending_pauses(), vec![1]);
    // Nothing more is queued for the paused pane.
    assert_eq!(g.enqueue(1, t0 + ms(1100)), GateEnqueue::Skip);
    assert_eq!(g.enqueue(1, t0 + ms(9000)), GateEnqueue::Skip);
    // The backlog that was queued before the pause is discarded.
    for s in &seqs {
        assert_eq!(g.dequeue(1, *s, t0, t0 + ms(9000)), GateWrite::Drop);
    }
}

#[test]
fn other_panes_are_not_paused() {
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(1000));
    let t0 = Instant::now();
    let GateEnqueue::Send(_) = g.enqueue(1, t0) else { panic!() };
    assert_eq!(g.enqueue(1, t0 + ms(1500)), GateEnqueue::Paused);
    // Pane 2 has no old backlog of its own.
    let GateEnqueue::Send(s2) = g.enqueue(2, t0 + ms(1500)) else { panic!() };
    assert_eq!(
        g.dequeue(2, s2, t0 + ms(1500), t0 + ms(1501)),
        GateWrite::Write { age_ms: 1, extended: true }
    );
}

#[test]
fn writer_pauses_on_old_chunk_when_output_stopped() {
    // The pane went quiet during the stall: no new output reaches the
    // server check, so the writer finds the old chunk (control_write_pending).
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(1000));
    let t0 = Instant::now();
    let GateEnqueue::Send(a) = g.enqueue(1, t0) else { panic!() };
    let GateEnqueue::Send(b) = g.enqueue(1, t0 + ms(10)) else { panic!() };
    assert_eq!(g.dequeue(1, a, t0, t0 + ms(5000)), GateWrite::Pause);
    assert_eq!(g.dequeue(1, b, t0 + ms(10), t0 + ms(5000)), GateWrite::Drop);
    assert!(g.is_paused(1));
    // The writer printed the %pause itself; nothing pending.
    assert!(g.take_pending_pauses().is_empty());
}

#[test]
fn continue_resumes_with_new_output_only() {
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(1000));
    let t0 = Instant::now();
    let GateEnqueue::Send(old) = g.enqueue(1, t0) else { panic!() };
    assert_eq!(g.enqueue(1, t0 + ms(2000)), GateEnqueue::Paused);
    assert!(!g.continue_pane(2), "pane 2 was never paused");
    assert!(g.continue_pane(1));
    assert!(!g.continue_pane(1), "second continue is a no-op");
    // Stale output queued before the pause is still dropped.
    assert_eq!(g.dequeue(1, old, t0, t0 + ms(2100)), GateWrite::Drop);
    let GateEnqueue::Send(new) = g.enqueue(1, t0 + ms(2100)) else { panic!() };
    assert_eq!(
        g.dequeue(1, new, t0 + ms(2100), t0 + ms(2103)),
        GateWrite::Write { age_ms: 3, extended: true }
    );
}

#[test]
fn manual_pause_and_continue() {
    let g = ControlOutputGate::new(None);
    let t0 = Instant::now();
    let GateEnqueue::Send(s) = g.enqueue(4, t0) else { panic!() };
    assert!(g.pause_pane(4));
    assert!(!g.pause_pane(4));
    // The command's reply carries this %pause, not the writer.
    assert!(g.take_pending_pauses().is_empty());
    assert_eq!(g.dequeue(4, s, t0, t0), GateWrite::Drop);
    assert_eq!(g.enqueue(4, t0), GateEnqueue::Skip);
    assert!(g.continue_pane(4));
    assert!(matches!(g.enqueue(4, t0), GateEnqueue::Send(_)));
}

#[test]
fn clearing_pause_after_keeps_paused_panes_paused() {
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(1000));
    let t0 = Instant::now();
    let GateEnqueue::Send(_) = g.enqueue(1, t0) else { panic!() };
    assert_eq!(g.enqueue(1, t0 + ms(1000)), GateEnqueue::Paused);
    g.set_pause_after(None);
    assert_eq!(g.pause_after_ms(), None);
    assert!(g.is_paused(1));
    assert_eq!(g.enqueue(1, t0 + ms(1100)), GateEnqueue::Skip);
}

#[test]
fn unqueue_forgets_a_chunk_the_channel_refused() {
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(1000));
    let t0 = Instant::now();
    let GateEnqueue::Send(s) = g.enqueue(1, t0) else { panic!() };
    g.unqueue(1, s);
    // No backlog left, so a later chunk does not count the refused one's age.
    assert!(matches!(g.enqueue(1, t0 + ms(5000)), GateEnqueue::Send(_)));
}

#[test]
fn no_pause_after_client_too_far_behind_is_dropped() {
    let g = ControlOutputGate::new(None);
    let t0 = Instant::now();
    let GateEnqueue::Send(_) = g.enqueue(1, t0) else { panic!() };
    // Behind, but under CONTROL_MAXIMUM_AGE: keep going.
    assert!(matches!(g.enqueue(1, t0 + ms(CONTROL_MAXIMUM_AGE_MS - 1)), GateEnqueue::Send(_)));
    assert_eq!(g.enqueue(1, t0 + ms(CONTROL_MAXIMUM_AGE_MS)), GateEnqueue::TooFarBehind);
    assert!(g.is_too_far_behind());
    assert_eq!(g.enqueue(2, t0 + ms(CONTROL_MAXIMUM_AGE_MS)), GateEnqueue::Skip);
}

#[test]
fn writer_reports_too_far_behind_on_ancient_chunk() {
    let g = ControlOutputGate::new(None);
    let t0 = Instant::now();
    let GateEnqueue::Send(s) = g.enqueue(1, t0) else { panic!() };
    assert_eq!(
        g.dequeue(1, s, t0, t0 + ms(CONTROL_MAXIMUM_AGE_MS + 1)),
        GateWrite::TooFarBehind
    );
    assert!(g.is_too_far_behind());
}

#[test]
fn bare_pause_after_pauses_on_any_age() {
    // tmux: pause_age 0 pauses as soon as the backlog is 1 ms old.
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(0));
    let t0 = Instant::now();
    let GateEnqueue::Send(_) = g.enqueue(1, t0) else { panic!() };
    assert!(matches!(g.enqueue(1, t0), GateEnqueue::Send(_)), "same ms is not behind");
    assert_eq!(g.enqueue(1, t0 + ms(1)), GateEnqueue::Paused);
}
