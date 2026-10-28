// Issue #431 (M5): client-side sixel MODEL layer.
//
// Proves the parse/model contract the client relies on WITHOUT any rendering:
//  1. A dump-state leaf's per-pane `images` array deserializes into
//     `LayoutJson::Leaf.images` with the correct id / SIGNED row (incl. a
//     negative anchor above the viewport) / pixel + cell dims.
//  2. The top-level `image_blobs` map (stringified id -> base64) deserializes
//     and base64-decodes back to the exact raw ESC P..ST bytes that go into the
//     session blob cache.
//
// These two assertions mirror exactly what the client frame loop does: parse
// the leaf descriptors (via serde on LayoutJson) and accumulate decoded blobs
// keyed by u64 id into a HashMap cache.

use crate::layout::LayoutJson;
use base64::Engine;
use std::collections::HashMap;

#[test]
fn leaf_images_deserialize_with_signed_rows_and_dims() {
    // A minimal `leaf` node carrying two images: one fully in view (row 3) and
    // one anchored ABOVE the viewport (row -2, the negative/signed case).
    let json = r#"{
        "type":"leaf",
        "id":1,"rows":24,"cols":80,"cursor_row":0,"cursor_col":0,
        "active":true,"copy_mode":false,"scroll_offset":0,
        "sel_start_row":null,"sel_start_col":null,
        "sel_end_row":null,"sel_end_col":null,
        "images":[
            {"id":7,"row":3,"col":0,"pw":320,"ph":240,"cw":32,"ch":12},
            {"id":8,"row":-2,"col":5,"pw":100,"ph":40,"cw":10,"ch":2}
        ]
    }"#;

    let node: LayoutJson = serde_json::from_str(json).expect("leaf deserializes");
    let LayoutJson::Leaf { images, .. } = node else {
        panic!("expected a leaf node");
    };
    assert_eq!(images.len(), 2, "both descriptors parsed");

    let a = &images[0];
    assert_eq!(a.id, 7);
    assert_eq!(a.row, 3); // in-view, positive
    assert_eq!(a.col, 0);
    assert_eq!(a.pw, 320);
    assert_eq!(a.ph, 240);
    assert_eq!(a.cw, 32);
    assert_eq!(a.ch, 12);

    let b = &images[1];
    assert_eq!(b.id, 8);
    assert_eq!(b.row, -2, "negative row (anchored above viewport) survives i32");
    assert_eq!(b.col, 5);
    assert_eq!(b.pw, 100);
    assert_eq!(b.ph, 40);
    assert_eq!(b.cw, 10);
    assert_eq!(b.ch, 2);
}

#[test]
fn leaf_without_images_defaults_to_empty() {
    // Older/empty frames omit the field entirely: serde(default) => empty Vec.
    let json = r#"{
        "type":"leaf",
        "id":1,"rows":24,"cols":80,"cursor_row":0,"cursor_col":0,
        "active":true,"copy_mode":false,"scroll_offset":0,
        "sel_start_row":null,"sel_start_col":null,
        "sel_end_row":null,"sel_end_col":null
    }"#;
    let node: LayoutJson = serde_json::from_str(json).expect("leaf without images deserializes");
    let LayoutJson::Leaf { images, .. } = node else { panic!("expected leaf") };
    assert!(images.is_empty(), "missing images array => empty Vec");
}

#[test]
fn image_blobs_deserialize_and_base64_decode_into_cache() {
    // Raw sixel-ish bytes as would be re-emitted verbatim by the client.
    let raw: &[u8] = b"\x1bPq#0;2;0;0;0#0~~-~~\x1b\\";
    let b64 = base64::engine::general_purpose::STANDARD.encode(raw);

    // Top-level dump-state ships `image_blobs` as stringified-id -> base64.
    let json = format!(r#"{{"7":"{b64}"}}"#);
    let blobs: HashMap<String, String> =
        serde_json::from_str(&json).expect("image_blobs map deserializes");

    // Mirror the client's per-frame accumulation into the u64-keyed cache.
    let mut cache: HashMap<u64, Vec<u8>> = HashMap::new();
    for (id_str, val) in &blobs {
        let id = id_str.parse::<u64>().expect("numeric id key");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(val.as_bytes())
            .expect("valid base64");
        cache.insert(id, bytes);
    }

    assert_eq!(cache.len(), 1);
    assert_eq!(
        cache.get(&7).map(|v| v.as_slice()),
        Some(raw),
        "decoded blob matches the exact raw ESC P..ST bytes"
    );
}

#[test]
fn malformed_blob_id_or_base64_is_skipped_not_panicked() {
    // Non-numeric key and invalid base64 must be skipped gracefully.
    let json = r#"{"notanumber":"AAAA","9":"!!!!not-base64!!!!"}"#;
    let blobs: HashMap<String, String> = serde_json::from_str(json).unwrap();
    let mut cache: HashMap<u64, Vec<u8>> = HashMap::new();
    for (id_str, val) in &blobs {
        let Ok(id) = id_str.parse::<u64>() else { continue };
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(val.as_bytes()) {
            cache.insert(id, bytes);
        }
    }
    // Neither entry lands in the cache; no panic occurred.
    assert!(cache.is_empty(), "malformed entries skipped, cache stays empty");
}

#[test]
fn a_reshipped_id_replaces_the_cached_blob() {
    // After switch-client the client talks to another server, whose image ids
    // start at 1 as well.  Its blob for id 1 must replace the one cached from
    // the first server, or the client draws the old image.
    let old: &[u8] = b"\x1bPq#0~~\x1b\\";
    let new: &[u8] = b"\x1bPq#1@@\x1b\\";
    let mut cache: HashMap<u64, Vec<u8>> = HashMap::new();
    let ship = |raw: &[u8]| -> HashMap<String, String> {
        HashMap::from([("1".to_string(), base64::engine::general_purpose::STANDARD.encode(raw))])
    };
    crate::sixel::draw::merge_image_blobs(&mut cache, &ship(old));
    crate::sixel::draw::merge_image_blobs(&mut cache, &ship(new));
    assert_eq!(cache.get(&1).map(|v| v.as_slice()), Some(new));

    // Malformed entries are still skipped.
    let bad: HashMap<String, String> = serde_json::from_str(r#"{"x":"AAAA","2":"!!"}"#).unwrap();
    crate::sixel::draw::merge_image_blobs(&mut cache, &bad);
    assert_eq!(cache.len(), 1);
}
