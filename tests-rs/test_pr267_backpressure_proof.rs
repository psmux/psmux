use super::*;

/// Serialize backpressure tests: push_frame broadcasts to ALL registered
/// slots globally, so concurrent tests cross-contaminate each other.
pub(super) static BP_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Burst large enough that any naive "first wins" or unbounded-buffer
/// implementation would be observable. The swap-chain slot has effective
/// capacity 1, so any value > 1 is sufficient; 20 makes intent obvious.
const BURST_SIZE: usize = 20;

/// Proof that push_frame delivers the newest frame after a burst of pushes
/// without an intervening read. The swap-chain slot (Arc<Mutex<Option<String>>>)
/// holds at most one pending frame; later pushes overwrite earlier ones.
///
/// History: PR #267 introduced a bounded sync_channel(16) with drain-on-Full
/// to preserve the "newest wins" property. That fix held a Mutex<Receiver>
/// across blocking TCP writes, causing the freeze this branch replaces with
/// a single-slot swap chain.
#[test]
fn push_frame_slot_holds_only_newest_after_burst() {
    let _guard = BP_TEST_LOCK.lock().unwrap();
    let client_id = u64::MAX - 9990;
    // Clean up any prior registration
    shutdown_client_stream(client_id);

    // The writer thread's wake channel. `register_frame_channel` pokes it on
    // every empty->full transition so the writer does not have to wait out its
    // own response-channel timeout to notice a frame. The receiver is held for
    // the life of the test so the sender stays live.
    let (poke_tx, poke_rx) = std::sync::mpsc::channel::<WriterWake>();
    let slot = register_frame_channel(client_id, poke_tx);

    // Push a burst of stale frames, then the one we care about.
    assert!(BURST_SIZE > 1, "precondition: burst must exceed slot capacity (1)");
    for idx in 0..BURST_SIZE {
        push_frame(&format!("stale-{idx}"));
    }
    push_frame("NEWEST_CRITICAL_FRAME");

    // Take the slot's content, then verify the slot is now empty.
    let first = slot.lock().expect("frame slot lock").take();
    let second = slot.lock().expect("frame slot lock").take();
    shutdown_client_stream(client_id);

    assert_eq!(
        first.as_deref(),
        Some("NEWEST_CRITICAL_FRAME"),
        "expected only the newest frame in the swap-chain slot",
    );
    assert!(
        second.is_none(),
        "slot should hold at most one frame; second take must be empty",
    );

    // A burst must not queue one wake per push. The writer reads the slot, not
    // the messages, so a poke per frame would be pure wakeups: 21 pushes here
    // deliver one frame and must cost one wake.
    let pokes = poke_rx.try_iter().count();
    assert_eq!(
        pokes, 1,
        "{} pushes into an unread slot queued {} wakes; only the empty->full \
         transition may poke, or a flooding pane wakes the writer per batch",
        BURST_SIZE + 1,
        pokes,
    );
}

/// PR #267's original intent: saturated frame queue delivers latest snapshot.
/// With the swap-chain replacement, "saturation" is implicit (capacity = 1)
/// and the assertion collapses to "only the latest push survives a burst".
#[test]
fn push_frame_replaces_stale_backlog() {
    let _guard = BP_TEST_LOCK.lock().unwrap();
    let client_id = u64::MAX - 246;
    shutdown_client_stream(client_id);

    let (poke_tx, _poke_rx) = std::sync::mpsc::channel::<WriterWake>();
    let slot = register_frame_channel(client_id, poke_tx);
    assert!(BURST_SIZE > 1, "precondition: burst must exceed slot capacity (1)");
    for idx in 0..BURST_SIZE {
        push_frame(&format!("stale-{idx}"));
    }
    push_frame("newest");

    let first = slot.lock().expect("frame slot lock").take();
    let second = slot.lock().expect("frame slot lock").take();
    shutdown_client_stream(client_id);

    assert_eq!(first.as_deref(), Some("newest"));
    assert!(second.is_none(), "slot should hold at most one frame");
}
