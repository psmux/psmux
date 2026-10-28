// Issue #431: the shared server's frame carries the sixel fields too.
//
// The per-session server builds its frame in two places in server/mod.rs and
// both carry `sixel` and `image_blobs`; the shared server builds its own in
// shared::frame::build_frame, which had neither, so a client of a shared
// server never received an image.

use super::{build_frame, FrameCache};

fn build(app: &mut crate::types::AppState, cache: &mut FrameCache) -> String {
    let mut meta_dirty = true;
    build_frame(app, cache, &mut meta_dirty).expect("build_frame");
    cache.combined_buf.clone()
}

fn frame_json(app: &mut crate::types::AppState, cache: &mut FrameCache) -> serde_json::Value {
    serde_json::from_str(&build(app, cache)).expect("the frame is JSON")
}

fn blob_count(frame: &serde_json::Value) -> usize {
    frame.get("image_blobs").and_then(|b| b.as_object()).map_or(0, |b| b.len())
}

#[test]
fn shared_frame_ships_the_blob_once_and_the_option() {
    let mut app = crate::server::test_issue431_m4_blobs::app_with_sixel_pane();
    let mut cache = FrameCache::new();

    let first = frame_json(&mut app, &mut cache);
    assert_eq!(first["sixel"], serde_json::Value::Bool(true), "the sixel option rides on the frame");
    assert_eq!(first["sixel_mode"], "auto", "and so does its value");
    assert_eq!(blob_count(&first), 1, "the visible image's blob is shipped: {first}");

    let second = frame_json(&mut app, &mut cache);
    assert_eq!(blob_count(&second), 0, "an already shipped blob is not sent again");

    app.sixel = crate::types::SixelMode::Off;
    let third = frame_json(&mut app, &mut cache);
    assert_eq!(third["sixel"], serde_json::Value::Bool(false));
    assert_eq!(third["sixel_mode"], "off");

    app.sixel = crate::types::SixelMode::On;
    let fourth = frame_json(&mut app, &mut cache);
    assert_eq!(fourth["sixel"], serde_json::Value::Bool(true));
    assert_eq!(fourth["sixel_mode"], "on");
}

// The server sends a blob in one frame only. The builder's text is followed
// by the client render options and the overlay fields, so a blob written
// inside it was not where frame_oneshot::take looks: a frame superseded
// unread (server slot, client queue, client drain) took its blob with it and
// the client never drew the image. Measured end to end before the fix: a
// sixel written next to a pane printing as fast as it can reached the outer
// terminal in 1 of 10 runs.
#[test]
fn a_superseded_frame_hands_its_blob_to_the_newer_one() {
    let mut app = crate::server::test_issue431_m4_blobs::app_with_sixel_pane();
    let mut cache = FrameCache::new();
    let mut older = build(&mut app, &mut cache);
    let mut newer = build(&mut app, &mut cache);
    assert!(older.contains("\"image_blobs\""), "the first frame ships the blob");
    assert!(!newer.contains("\"image_blobs\""), "the second does not ship it again");

    crate::frame_oneshot::supersede(&mut older, &mut newer);
    let shots = crate::frame_oneshot::take(&mut newer);
    assert_eq!(shots.image_blob_map().len(), 1, "the blob survives the supersede: {newer}");
    let v: serde_json::Value = serde_json::from_str(&newer).expect("still JSON");
    assert!(v.get("image_blobs").is_none(), "taken off the frame kept for drawing");
}
