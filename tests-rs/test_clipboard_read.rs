// A pane's OSC 52 query, answered as tmux 3.8's input_osc_52_reply answers it
// (input.c:3341). Every expectation is what tmux 3.8 did, measured, except
// where a comment says psmux differs.

use super::*;

fn set(app: &mut AppState, name: &str, value: &str) {
    crate::server::options::apply_set_option(app, name, value, false).expect(name);
}

/// A server with `set-clipboard on`, the only value tmux reads OSC 52 on.
fn app() -> AppState {
    let mut a = AppState::new("clipboard_read".to_string());
    set(&mut a, "set-clipboard", "on");
    a
}

fn no_clipboard() -> Result<Option<String>, ClipboardBusy> {
    panic!("the system clipboard is read only for request and both")
}

/// The data of an answer that did not wait.
fn now(answered: Answer) -> Option<String> {
    match answered {
        Answer::Now(data) => data,
        Answer::Busy => panic!("busy"),
    }
}

#[test]
fn the_reply_is_tmux_form_ended_as_asked() {
    // Measured: buffer `hello`, OSC 52 ; c ; ? answered ESC ] 52 ; c ;
    // aGVsbG8= with the query's end; with no selection the reply has none.
    assert_eq!(reply(b'c', "hello", false), "\x1b]52;c;aGVsbG8=\x1b\\");
    assert_eq!(reply(b'c', "hello", true), "\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(reply(0, "hi", false), "\x1b]52;;aGk=\x1b\\");
    assert_eq!(reply(b's', "hi", false), "\x1b]52;s;aGk=\x1b\\");
}

#[test]
fn buffer_answers_with_the_newest_automatic_buffer() {
    let mut a = app();
    assert_eq!(now(answer(&mut a, no_clipboard)), None, "no buffer, no reply");
    a.paste_set_stack(&["newest", "older"]);
    assert_eq!(now(answer(&mut a, no_clipboard)).as_deref(), Some("newest"));
    // Measured: a named buffer is not read.
    let mut a = app();
    a.paste_set("foo", "named".to_string());
    assert_eq!(now(answer(&mut a, no_clipboard)), None);
}

#[test]
fn set_clipboard_off_or_get_clipboard_off_has_no_reply() {
    let mut a = app();
    a.paste_set_stack(&["hello"]);
    // psmux: tmux answers only on `on` (input.c:3379); psmux takes a
    // program's clipboard write on `external` too, so its query as well.
    assert_eq!(now(answer(&mut a, no_clipboard)).as_deref(), Some("hello"), "on");
    for value in ["off", "external"] {
        set(&mut a, "set-clipboard", value);
        assert_eq!(now(answer(&mut a, no_clipboard)), None, "{value}");
    }
    set(&mut a, "set-clipboard", "on");
    set(&mut a, "get-clipboard", "off");
    assert_eq!(now(answer(&mut a, no_clipboard)), None);
}

#[test]
fn request_and_both_read_the_system_clipboard_with_a_client_attached() {
    let mut a = app();
    a.paste_set_stack(&["buffered"]);
    set(&mut a, "get-clipboard", "request");
    // Measured: with no client tmux has no terminal to ask, and no reply.
    assert_eq!(now(answer(&mut a, || Ok(Some("copied".into())))), None);
    a.attached_clients = 1;
    assert_eq!(now(answer(&mut a, || Ok(Some("copied".into())))).as_deref(), Some("copied"));
    assert_eq!(a.paste_top(), Some("buffered"), "request keeps no buffer");
    assert_eq!(now(answer(&mut a, || Ok(None))), None, "no text, no reply");
    assert_eq!(now(answer(&mut a, || Ok(Some(String::new())))), None);
    set(&mut a, "get-clipboard", "both");
    assert_eq!(now(answer(&mut a, || Ok(Some("copied".into())))).as_deref(), Some("copied"));
    assert_eq!(a.paste_top(), Some("copied"), "both keeps it as a buffer");
}

#[test]
fn the_option_takes_tmux_values_only() {
    for value in GET_CLIPBOARD_CHOICES {
        assert_eq!(crate::server::option_catalog::validate_option_value("get-clipboard", value), Ok(()));
    }
    // Measured: `set -s get-clipboard bad` says `unknown value: bad`.
    assert_eq!(
        crate::server::option_catalog::validate_option_value("get-clipboard", "bad"),
        Err("unknown value: bad".to_string())
    );
    assert_eq!(crate::server::options::get_option_value(&app(), "get-clipboard"), "buffer");
}

#[test]
fn a_batch_with_a_clipboard_query_waits_for_the_server() {
    let st = crate::pane::DecrqmState::default();
    let colours = crate::types::HostColors::campbell();
    let mut da1_open = false;
    let q = vt100::TerminalQuery::Clipboard { selection: b'c', bel: false };
    let replies = crate::pane::pane_replies(&[q], &st, &colours, &mut da1_open);
    assert!(wanted(&replies));
    let q = vt100::TerminalQuery::ForegroundColour { colour: None, bel: false };
    assert!(!wanted(&crate::pane::pane_replies(&[q], &st, &colours, &mut da1_open)));
}

fn batch() -> Batch {
    Batch::new(1, None, Vec::new(), Arc::new(AtomicU32::new(0)))
}

fn not_again() -> Result<Option<String>, ClipboardBusy> {
    panic!("a batch that waits does not read again")
}

/// Poll `batch` until its read ends, for up to two seconds.
fn finish(a: &mut AppState, b: &mut Batch) -> Option<String> {
    for _ in 0..200 {
        if let Some(data) = data_for(a, b, not_again, || panic!("one thread per wait")) {
            return data;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the read did not end")
}

#[test]
fn a_busy_clipboard_is_read_on_a_thread_and_the_batch_waits() {
    // A window holding the clipboard open (#428) leaves the batch waiting
    // rather than the server loop, and its answer comes when the read does.
    let mut a = app();
    set(&mut a, "get-clipboard", "both");
    a.attached_clients = 1;
    let mut b = batch();
    let first = data_for(&mut a, &mut b, || Err(ClipboardBusy), || {
        std::thread::sleep(Duration::from_millis(50));
        Some("late".into())
    });
    assert_eq!(first, None, "it waits");
    assert_eq!(finish(&mut a, &mut b).as_deref(), Some("late"));
    assert_eq!(a.paste_top(), Some("late"), "both keeps it as a buffer");
}

#[test]
fn a_read_that_gives_up_has_no_reply() {
    let mut a = app();
    set(&mut a, "get-clipboard", "request");
    a.attached_clients = 1;
    let mut b = batch();
    assert_eq!(data_for(&mut a, &mut b, || Err(ClipboardBusy), || None), None);
    assert_eq!(finish(&mut a, &mut b), None);
}

#[test]
fn the_options_are_read_again_when_the_wait_ends() {
    let mut a = app();
    set(&mut a, "get-clipboard", "request");
    a.attached_clients = 1;
    let mut b = batch();
    assert_eq!(data_for(&mut a, &mut b, || Err(ClipboardBusy), || Some("late".into())), None);
    set(&mut a, "get-clipboard", "off");
    assert_eq!(finish(&mut a, &mut b), None);
}

#[test]
fn buffer_and_no_client_never_wait() {
    let mut a = app();
    a.paste_set_stack(&["buffered"]);
    let mut b = batch();
    let busy = || -> Result<Option<String>, ClipboardBusy> { Err(ClipboardBusy) };
    assert_eq!(data_for(&mut a, &mut b, busy, || panic!("no wait")), Some(Some("buffered".into())));
    set(&mut a, "get-clipboard", "request");
    assert_eq!(data_for(&mut a, &mut b, busy, || panic!("no wait")), Some(None), "no client");
}

#[test]
fn the_defaults_give_no_reply_and_never_read_the_clipboard() {
    // tmux 3.8 with its defaults (set-clipboard external, get-clipboard
    // buffer) gives no reply: input_osc_52_parse returns unless set-clipboard
    // is on (input.c:3379). A program printing to a pane must not get the
    // user's copies back without the user opting in.
    let mut a = AppState::new("clipboard_read".to_string());
    a.paste_set_stack(&["secret"]);
    a.attached_clients = 1;
    assert_eq!(a.set_clipboard, "external");
    assert_eq!(crate::server::options::get_option_value(&a, "get-clipboard"), "buffer");
    assert_eq!(now(answer(&mut a, no_clipboard)), None, "buffer");
    for mode in ["request", "both"] {
        set(&mut a, "get-clipboard", mode);
        assert_eq!(now(answer(&mut a, no_clipboard)), None, "{mode} on external");
    }
    assert_eq!(a.paste_top(), Some("secret"), "both made no buffer");
}
