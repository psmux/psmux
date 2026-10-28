// The cells of an image text has been written over reach the client in the
// image's descriptor, as `[row, col, len]` runs, and an older frame without
// them still reads.
use super::visible_pane_images;

const IMAGE: &[u8] = b"\x1bPq#1;2;100;0;0#1!40~-!40~\x1b\\";

#[test]
fn the_descriptor_carries_the_cells_written_over() {
    let mut parser = vt100::Parser::new(24, 80, 100);
    parser.process(IMAGE);
    let imgs = visible_pane_images(parser.screen(), 24, 0);
    assert!(imgs[0].erased.is_empty(), "nothing written over yet");

    parser.process(b"\r\n\x1b[1Cab");
    let imgs = visible_pane_images(parser.screen(), 24, 0);
    assert_eq!(imgs[0].erased, vec![[1, 1, 2]]);
}

#[test]
fn runs_are_split_where_cells_are_left() {
    let mut parser = vt100::Parser::new(24, 80, 100);
    parser.process(IMAGE);
    parser.process(b"\x1b[1;1Ha\x1b[1;4Hb\r\nabcd");
    let imgs = visible_pane_images(parser.screen(), 24, 0);
    assert_eq!(imgs[0].erased, vec![[0, 0, 1], [0, 3, 1], [1, 0, 4]]);
}

#[test]
fn the_client_reads_descriptors_with_and_without_erased_cells() {
    let old: crate::layout::ImageDescJson =
        serde_json::from_str(r#"{"id":1,"row":0,"col":0,"pw":40,"ph":24,"cw":4,"ch":2}"#).unwrap();
    assert!(old.erased.is_empty());
    let new: crate::layout::ImageDescJson =
        serde_json::from_str(r#"{"id":1,"row":0,"col":0,"pw":40,"ph":24,"cw":4,"ch":2,"erased":[[1,0,4]]}"#).unwrap();
    assert_eq!(new.erased, vec![[1, 0, 4]]);
}
