// #759 follow up: the session's name has to ride on EVERY frame an attached
// client receives. The dump-state reply carried it since issue #7 batch D, but
// the server's push path builds the frame separately and left it out, and an
// attached client takes most of its frames from the push. Its `[#S]` status,
// `command-prompt -I '#S'` and the rename overlay's fallback then kept the port
// file base (`ns__name`) for the life of the client, and a rename-session made
// anywhere else never reached it.

use super::append_session_name_json;

#[test]
fn session_name_is_appended_as_a_top_level_field() {
    let mut buf = String::from("{\"layout\":{},\"windows\":[]}");
    append_session_name_json("work", &mut buf);
    let v: serde_json::Value = serde_json::from_str(&buf).expect("still valid JSON");
    assert_eq!(v["session_name"], "work");
}

#[test]
fn session_name_is_escaped() {
    // A frame always has fields before this one; the helper appends with a
    // leading comma, as every sibling appender does.
    let mut buf = String::from("{\"windows\":[]}");
    append_session_name_json("a\"b\\c", &mut buf);
    let v: serde_json::Value = serde_json::from_str(&buf).expect("still valid JSON");
    assert_eq!(v["session_name"], "a\"b\\c");
}

#[test]
fn a_buffer_that_is_not_an_object_is_left_alone() {
    let mut buf = String::from("NC");
    append_session_name_json("work", &mut buf);
    assert_eq!(buf, "NC");
}

/// Both frame builders in server/mod.rs, the dump-state reply and the push,
/// must append the name. One call site is exactly the defect this pins.
#[test]
fn both_frame_builders_append_the_session_name() {
    let src = include_str!("../src/server/mod.rs");
    let calls = src.matches("helpers::append_session_name_json(&app.session_name, &mut combined_buf)").count();
    assert_eq!(calls, 2, "the dump-state reply and the push frame each append session_name");
    let pushes = src.matches("crate::types::push_frame(&combined_buf)").count()
        + src.matches("crate::types::push_frame_answered(&combined_buf,").count();
    assert_eq!(pushes, 2, "a new frame builder must append session_name too; update this test with it");
}
