use super::*;
use std::sync::mpsc::{self, Receiver, TryRecvError};

fn request(
    channels: &mut WaitChannels,
    name: &str,
    op: WaitForOp,
    client: u64,
) -> Receiver<Result<(), String>> {
    let (tx, rx) = mpsc::channel();
    channels.execute(name.to_string(), op, client, tx);
    rx
}

#[test]
fn parser_accepts_tmux_flags_and_channel_operands() {
    for (args, name, op) in [
        (vec!["ready"], "ready", WaitForOp::Wait),
        (vec!["-L", "mutex"], "mutex", WaitForOp::Lock),
        (vec!["-U", "mutex"], "mutex", WaitForOp::Unlock),
        (vec!["-SLU", "ready"], "ready", WaitForOp::Signal),
        (vec!["--", "-ready"], "-ready", WaitForOp::Wait),
        (vec!["-S", "two words"], "two words", WaitForOp::Signal),
        (vec!["-S", ""], "", WaitForOp::Signal),
    ] {
        assert_eq!(parse(&args), Ok((name.to_string(), op)));
    }
    for args in [vec![], vec!["one", "two"], vec!["-Z", "ready"], vec!["-L"]] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
}

fn pending(rx: &Receiver<Result<(), String>>) {
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

fn completed(rx: &Receiver<Result<(), String>>) {
    assert_eq!(rx.try_recv(), Ok(Ok(())));
}

#[test]
fn signal_before_wait_is_consumed_once() {
    let mut channels = WaitChannels::default();
    assert!(channels.is_empty());
    completed(&request(&mut channels, "ready", WaitForOp::Signal, 1));
    completed(&request(&mut channels, "ready", WaitForOp::Wait, 2));
    assert!(channels.is_empty());
    let next = request(&mut channels, "ready", WaitForOp::Wait, 3);
    pending(&next);
    completed(&request(&mut channels, "ready", WaitForOp::Signal, 1));
    completed(&next);
}

#[test]
fn signal_wakes_all_waiters_but_not_lockers() {
    let mut channels = WaitChannels::default();
    completed(&request(&mut channels, "ready", WaitForOp::Lock, 1));
    let lock = request(&mut channels, "ready", WaitForOp::Lock, 2);
    let first = request(&mut channels, "ready", WaitForOp::Wait, 3);
    let second = request(&mut channels, "ready", WaitForOp::Wait, 4);
    pending(&lock);
    pending(&first);
    pending(&second);
    completed(&request(&mut channels, "ready", WaitForOp::Signal, 5));
    completed(&first);
    completed(&second);
    pending(&lock);
    let next = request(&mut channels, "ready", WaitForOp::Wait, 6);
    pending(&next);
}

#[test]
fn unlock_transfers_to_one_locker_in_order_and_does_not_signal() {
    let mut channels = WaitChannels::default();
    completed(&request(&mut channels, "mutex", WaitForOp::Lock, 1));
    let first = request(&mut channels, "mutex", WaitForOp::Lock, 2);
    let second = request(&mut channels, "mutex", WaitForOp::Lock, 3);
    let wait = request(&mut channels, "mutex", WaitForOp::Wait, 4);
    pending(&first);
    pending(&second);
    pending(&wait);
    completed(&request(&mut channels, "mutex", WaitForOp::Unlock, 5));
    completed(&first);
    pending(&second);
    pending(&wait);
    completed(&request(&mut channels, "mutex", WaitForOp::Unlock, 5));
    completed(&second);
    pending(&wait);
    completed(&request(&mut channels, "mutex", WaitForOp::Unlock, 5));
    pending(&wait);
    assert_eq!(
        request(&mut channels, "mutex", WaitForOp::Unlock, 5).try_recv(),
        Ok(Err("channel mutex not locked".to_string()))
    );
}

#[test]
fn unlock_missing_channel_is_an_error() {
    let mut channels = WaitChannels::default();
    assert!(channels.is_empty());
    assert_eq!(
        request(&mut channels, "missing", WaitForOp::Unlock, 1).try_recv(),
        Ok(Err("channel missing not locked".to_string()))
    );
    assert!(channels.is_empty());
}

#[test]
fn disconnect_removes_waiters_and_lockers_without_releasing_acquired_lock() {
    let mut channels = WaitChannels::default();
    completed(&request(&mut channels, "mutex", WaitForOp::Lock, 1));
    let dead_lock = request(&mut channels, "mutex", WaitForOp::Lock, 2);
    let live_lock = request(&mut channels, "mutex", WaitForOp::Lock, 3);
    let dead_wait = request(&mut channels, "ready", WaitForOp::Wait, 2);
    pending(&dead_lock);
    pending(&live_lock);
    pending(&dead_wait);
    channels.client_lost(2);
    assert_eq!(dead_lock.try_recv(), Err(TryRecvError::Disconnected));
    assert_eq!(dead_wait.try_recv(), Err(TryRecvError::Disconnected));
    assert!(!channels.0.contains_key("ready"));
    channels.client_lost(1);
    pending(&live_lock);
    completed(&request(&mut channels, "mutex", WaitForOp::Unlock, 4));
    completed(&live_lock);
}

#[test]
fn a_woken_locked_channel_stays_woken_until_it_can_be_removed() {
    let mut channels = WaitChannels::default();
    completed(&request(&mut channels, "both", WaitForOp::Lock, 1));
    completed(&request(&mut channels, "both", WaitForOp::Signal, 2));
    // tmux's remove refuses a locked channel; waiting does not clear woken.
    completed(&request(&mut channels, "both", WaitForOp::Wait, 3));
    completed(&request(&mut channels, "both", WaitForOp::Wait, 4));
    completed(&request(&mut channels, "both", WaitForOp::Unlock, 1));
    assert!(channels.is_empty());
}

#[test]
fn unlock_skips_cancelled_receivers() {
    let mut channels = WaitChannels::default();
    completed(&request(&mut channels, "mutex", WaitForOp::Lock, 1));
    let dead = request(&mut channels, "mutex", WaitForOp::Lock, 2);
    let live = request(&mut channels, "mutex", WaitForOp::Lock, 3);
    pending(&dead);
    pending(&live);
    drop(dead);
    completed(&request(&mut channels, "mutex", WaitForOp::Unlock, 1));
    completed(&live);
}

#[test]
fn channels_are_independent() {
    let mut channels = WaitChannels::default();
    let a = request(&mut channels, "a", WaitForOp::Wait, 1);
    let b = request(&mut channels, "b", WaitForOp::Wait, 2);
    pending(&a);
    pending(&b);
    completed(&request(&mut channels, "a", WaitForOp::Signal, 3));
    completed(&a);
    pending(&b);
}

#[test]
fn repeated_signal_without_waiters_removes_the_latch_like_tmux() {
    let mut channels = WaitChannels::default();
    completed(&request(&mut channels, "ready", WaitForOp::Signal, 1));
    assert!(!channels.is_empty());
    completed(&request(&mut channels, "ready", WaitForOp::Signal, 1));
    assert!(channels.is_empty());
    pending(&request(&mut channels, "ready", WaitForOp::Wait, 2));
}
