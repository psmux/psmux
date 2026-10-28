// #925: in shared server mode the paste buffers are one set for the whole
// server, as tmux keeps them (paste.c:32 to :36, file statics). The shared
// server lends its set to a session for one request and takes it back after,
// so a buffer made in one session is there in every other. No server, socket
// or pane is started: the sessions are bare AppStates.

use super::*;
use crate::types::AppState;

fn with<R>(bufs: &mut ServerBuffers, app: &mut AppState, f: impl FnOnce(&mut AppState) -> R) -> R {
    bufs.lend(app);
    let r = f(app);
    bufs.take_back(app);
    r
}

#[test]
fn a_buffer_set_in_one_session_is_seen_in_another() {
    let mut bufs = ServerBuffers::new();
    let mut a = AppState::new("a".to_string());
    let mut b = AppState::new("b".to_string());
    with(&mut bufs, &mut a, |app| app.paste_set("g", "hola\n".to_string()));
    assert_eq!(a.paste_get("g"), None, "outside a request the session holds nothing of its own");
    let seen = with(&mut bufs, &mut b, |app| app.paste_get("g").map(str::to_string));
    assert_eq!(seen.as_deref(), Some("hola\n"),
        "BUG #925: set-buffer -b g in session a, paste-buffer -b g in session b found no buffer g");
    with(&mut bufs, &mut b, |app| assert!(app.paste_delete("g")));
    let seen = with(&mut bufs, &mut a, |app| app.paste_get("g").map(str::to_string));
    assert_eq!(seen, None, "a delete in b is seen in a");
}

#[test]
fn automatic_names_are_one_sequence_for_the_server() {
    let mut bufs = ServerBuffers::new();
    let mut a = AppState::new("a".to_string());
    let mut b = AppState::new("b".to_string());
    with(&mut bufs, &mut a, |app| app.paste_add("first".to_string()));
    with(&mut bufs, &mut b, |app| app.paste_add("second".to_string()));
    let listing = with(&mut bufs, &mut a, |app| app.paste_listing(false));
    assert_eq!(listing, "buffer1: 6 bytes: \"second\"\nbuffer0: 5 bytes: \"first\"\n",
        "tmux names automatic buffers from one server counter, newest first");
    assert_eq!(bufs.top(), Some("second"));
    assert_eq!(bufs.len(), 2);
}

#[test]
fn buffers_a_session_made_on_its_own_join_the_server_set() {
    let mut bufs = ServerBuffers::new();
    let mut a = AppState::new("a".to_string());
    let mut b = AppState::new("b".to_string());
    with(&mut bufs, &mut a, |app| app.paste_set("x", "server".to_string()));
    b.paste_set("x", "stray".to_string());
    b.paste_set("own", "kept".to_string());
    let (x, own) = with(&mut bufs, &mut b, |app| {
        (app.paste_get("x").map(str::to_string), app.paste_get("own").map(str::to_string))
    });
    assert_eq!(x.as_deref(), Some("server"), "the server's buffer of a name wins");
    assert_eq!(own.as_deref(), Some("kept"), "nothing a session held is dropped");
    assert_eq!(bufs.get("own"), Some("kept"));
    assert_eq!(b.paste_get("own"), None);
}

#[test]
fn the_name_counter_never_goes_back() {
    let mut bufs = ServerBuffers::new();
    let mut a = AppState::new("a".to_string());
    with(&mut bufs, &mut a, |app| {
        app.paste_add("one".to_string());
        app.paste_add("two".to_string());
    });
    // A fresh session starts its own counter at 0.
    let mut c = AppState::new("c".to_string());
    with(&mut bufs, &mut c, |app| app.paste_add("three".to_string()));
    assert_eq!(bufs.get("buffer2"), Some("three"));
    assert_eq!(bufs.get("buffer0"), Some("one"));
}
