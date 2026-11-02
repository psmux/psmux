// `refresh-client -B name:target:format` subscriptions ignored their target:
// every one was expanded against the active pane and reported with the active
// pane's id. iTerm2 subscribes once per pane (`<name>:%<id>:#{pane_title}`),
// so every tab of a psmux session showed the title of whichever pane was
// active at the time.
//
// tmux 3.8 (monitor.c: monitor_parse, monitor_check_*) expands a
// subscription once per object its target names: `%N` that pane, `%*` every
// pane, `@N` that window, `@*` every window, empty the session; a pane or
// window that does not exist is skipped, and any other target is not a
// subscription at all (cmd-refresh-client.c removes the subscription named
// by the whole value instead).
//
// The tests call the server's target resolution and per-check change
// detection over a pane tree with no psmux server and no session. Registered
// from src/server/mod.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::Node;

#[derive(Default)]
struct NullWriter;

impl crate::pane::PaneInputSink for NullWriter {}

impl std::io::Write for NullWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { Ok(buf.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn make_pane(id: usize, title: &str) -> crate::types::Pane {
    let (master, _sink) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: 10, cols: 40, pixel_width: 0, pixel_height: 0 });
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer: Box::new(NullWriter),
        child: crate::util::StubChild::exited(),
        term: Arc::new(Mutex::new(vt100::Parser::new(10, 40, 100))),
        last_rows: 10,
        last_cols: 40,
        id,
        title: title.to_string(),
        title_locked: true,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        dead_info: None,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        win32_input_latched: false,
        input_off: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(id: usize, name: &str, panes: Vec<crate::types::Pane>, active: usize) -> crate::types::Window {
    let n = panes.len();
    crate::types::Window {
        root: Node::Split {
            kind: LayoutKind::Vertical,
            sizes: vec![100 / n as u16; n],
            children: panes.into_iter().map(Node::Leaf).collect(),
        },
        active_path: vec![active],
        name: name.to_string(),
        id,
        area: Rect::new(0, 0, 40, 10),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

/// Window 0 (@10, "editor") holds %1; window 1 (@11, "shell") holds %2 %3
/// with %3 active. The session's active window is window 1, so the active
/// pane is %3.
fn app() -> AppState {
    let mut app = AppState::new("subs".to_string());
    app.session_id = 0;
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows.push(make_window(10, "editor", vec![make_pane(1, "one")], 0));
    app.windows.push(make_window(11, "shell", vec![make_pane(2, "two"), make_pane(3, "three")], 1));
    app.active_idx = 1;
    app
}

fn make_float(id: usize, title: &str) -> crate::types::FloatingPane {
    crate::types::FloatingPane {
        pane: make_pane(id, title),
        x: 2,
        y: 2,
        w: 20,
        h: 6,
        border: String::new(),
        id,
        title: title.to_string(),
        position: None,
        styles: crate::types::FloatStyles { style: String::new(), active_border: String::new(), border: String::new() },
    }
}

fn set_title(app: &mut AppState, id: usize, title: &str) {
    crate::tree::find_pane_mut_by_id_global(app, id).expect("pane").title = title.to_string();
}

/// The notification lines one check of a subscription writes.
fn check(app: &AppState, last: &mut std::collections::HashMap<String, String>, name: &str, target: &str, format: &str) -> Vec<String> {
    check_subscription(app, last, name, target, format)
        .iter()
        .map(crate::control::format_notification)
        .collect()
}

fn values(app: &AppState, target: &str, format: &str) -> Vec<(String, Option<usize>, Option<usize>, String)> {
    subscription_instances(app, target, format)
}

#[test]
fn a_pane_target_reports_that_pane_not_the_active_one() {
    let app = app();
    assert_eq!(values(&app, "%1", "#{pane_title}"), vec![("%1:0".to_string(), Some(0), Some(1), "one".to_string())]);
    assert_eq!(values(&app, "%2", "#{pane_title}"), vec![("%2:1".to_string(), Some(1), Some(2), "two".to_string())]);
    assert_eq!(values(&app, "%3", "#{pane_title}"), vec![("%3:1".to_string(), Some(1), Some(3), "three".to_string())]);
}

#[test]
fn a_pane_target_expands_window_formats_for_the_panes_window() {
    let app = app();
    assert_eq!(values(&app, "%1", "#{window_name}:#{pane_id}")[0].3, "editor:%1");
    assert_eq!(values(&app, "%2", "#{window_name}:#{pane_id}")[0].3, "shell:%2");
}

#[test]
fn every_pane_target_reports_each_pane_under_its_own_key() {
    let app = app();
    assert_eq!(
        values(&app, "%*", "#{pane_title}"),
        vec![
            ("%1:0".to_string(), Some(0), Some(1), "one".to_string()),
            ("%2:1".to_string(), Some(1), Some(2), "two".to_string()),
            ("%3:1".to_string(), Some(1), Some(3), "three".to_string()),
        ]
    );
}

#[test]
fn window_targets_report_the_window_and_expand_its_active_pane() {
    let app = app();
    assert_eq!(values(&app, "@10", "#{window_name}/#{pane_title}"), vec![("@10:0".to_string(), Some(0), None, "editor/one".to_string())]);
    assert_eq!(
        values(&app, "@*", "#{window_name}/#{pane_title}"),
        vec![
            ("@10:0".to_string(), Some(0), None, "editor/one".to_string()),
            ("@11:1".to_string(), Some(1), None, "shell/three".to_string()),
        ]
    );
}

#[test]
fn a_session_target_expands_the_active_pane_and_names_no_window_or_pane() {
    let app = app();
    assert_eq!(values(&app, "", "#{pane_title}"), vec![(String::new(), None, None, "three".to_string())]);
}

#[test]
fn a_missing_pane_or_window_reports_nothing() {
    let app = app();
    assert!(values(&app, "%99", "#{pane_title}").is_empty());
    assert!(values(&app, "@99", "#{pane_title}").is_empty());
    assert!(values(&app, "%x", "#{pane_title}").is_empty());
}

#[test]
fn clearing_a_subscription_forgets_its_per_object_values_only() {
    let mut v = std::collections::HashMap::new();
    v.insert("t".to_string(), "x".to_string());
    v.insert("t\u{1f}%1:0".to_string(), "x".to_string());
    v.insert("t\u{1f}@10:0".to_string(), "x".to_string());
    v.insert("tt".to_string(), "x".to_string());
    v.insert("tt\u{1f}%1:0".to_string(), "x".to_string());
    clear_subscription_values(&mut v, "t");
    let mut left: Vec<_> = v.keys().cloned().collect();
    left.sort();
    assert_eq!(left, vec!["tt".to_string(), "tt\u{1f}%1:0".to_string()]);
}

#[test]
fn a_target_is_read_as_tmux_3_8_reads_it() {
    use crate::control::{parse_subscription_target as parse, SubscriptionTarget as T};
    assert_eq!(parse("%*"), Some(T::AllPanes));
    assert_eq!(parse("%2"), Some(T::Pane(2)));
    // sscanf("%%%d"): leading digits, white space and a sign first.
    assert_eq!(parse("%2x"), Some(T::Pane(2)));
    assert_eq!(parse("% 2"), Some(T::Pane(2)));
    assert_eq!(parse("%+2"), Some(T::Pane(2)));
    assert_eq!(parse("%-0"), Some(T::Pane(0)));
    assert_eq!(parse("@*"), Some(T::AllWindows));
    assert_eq!(parse("@10"), Some(T::Window(10)));
    assert_eq!(parse(""), Some(T::Session));
    for bad in ["%", "%x", "%-1", "@", "@x", "$0", "x", "*", "%99999999999"] {
        assert_eq!(parse(bad), None, "{bad:?}");
    }
}

#[test]
fn a_value_that_does_not_parse_removes_the_subscription_it_names() {
    use crate::control::{parse_subscription_request as req, SubscriptionRequest as R};
    let add = |n: &str, t: &str, f: &str| R::Add { name: n.into(), target: t.into(), format: f.into() };
    let remove = |n: &str| R::Remove { name: n.into() };
    assert_eq!(req("t:%1:#{pane_title}"), add("t", "%1", "#{pane_title}"));
    assert_eq!(req("t::#{session_name}"), add("t", "", "#{session_name}"));
    // The format keeps its own colons.
    assert_eq!(req("t:@1:#{?pane_active,a:b,c}"), add("t", "@1", "#{?pane_active,a:b,c}"));
    // tmux: a bare name removes that subscription...
    assert_eq!(req("t"), remove("t"));
    // ...and so does any value that is not name:what:format, by its whole text.
    assert_eq!(req("t:%x:#{pane_title}"), remove("t:%x:#{pane_title}"));
    assert_eq!(req("t:%1"), remove("t:%1"));
    // psmux's own `name:` removes `name`.
    assert_eq!(req("t:"), remove("t"));
}

#[test]
fn a_pane_target_skips_a_dead_pane() {
    let mut app = app();
    crate::tree::find_pane_mut_by_id_global(&mut app, 2).expect("pane").dead = true;
    let mut float = make_float(4, "float");
    float.pane.dead = true;
    app.windows[0].floating.push(float);
    assert!(values(&app, "%2", "#{pane_title}").is_empty());
    assert!(values(&app, "%4", "#{pane_title}").is_empty());
    // `%*` still reports them, as tmux's all-panes check does.
    assert_eq!(values(&app, "%*", "#{pane_title}").len(), 4);
}

#[test]
fn a_dead_pane_that_comes_back_is_reported_only_if_its_value_changed() {
    let mut app = app();
    let mut last = std::collections::HashMap::new();
    assert_eq!(check(&app, &mut last, "p", "%2", "#{pane_title}"), vec!["%subscription-changed p $0 @11 1 %2 : two"]);
    crate::tree::find_pane_mut_by_id_global(&mut app, 2).expect("pane").dead = true;
    assert!(check(&app, &mut last, "p", "%2", "#{pane_title}").is_empty());
    // tmux keeps the last value of a pane it skips.
    crate::tree::find_pane_mut_by_id_global(&mut app, 2).expect("pane").dead = false;
    assert!(check(&app, &mut last, "p", "%2", "#{pane_title}").is_empty());
    set_title(&mut app, 2, "TWO");
    assert_eq!(check(&app, &mut last, "p", "%2", "#{pane_title}"), vec!["%subscription-changed p $0 @11 1 %2 : TWO"]);
}

#[test]
fn floating_panes_are_panes_for_pane_targets() {
    let mut app = app();
    app.windows[0].floating.push(make_float(4, "float"));
    assert_eq!(values(&app, "%4", "#{pane_id}"), vec![("%4:0".to_string(), Some(0), Some(4), "%4".to_string())]);
    let all: Vec<_> = values(&app, "%*", "#{pane_id}").into_iter().map(|(k, _, p, _)| (k, p)).collect();
    assert_eq!(
        all,
        vec![
            ("%1:0".to_string(), Some(1)),
            ("%4:0".to_string(), Some(4)),
            ("%2:1".to_string(), Some(2)),
            ("%3:1".to_string(), Some(3)),
        ]
    );
}

#[test]
fn a_check_writes_only_the_objects_whose_value_changed() {
    let mut app = app();
    let mut last = std::collections::HashMap::new();
    assert_eq!(
        check(&app, &mut last, "t", "%*", "#{pane_title}"),
        vec![
            "%subscription-changed t $0 @10 0 %1 : one",
            "%subscription-changed t $0 @11 1 %2 : two",
            "%subscription-changed t $0 @11 1 %3 : three",
        ]
    );
    assert!(check(&app, &mut last, "t", "%*", "#{pane_title}").is_empty());
    set_title(&mut app, 2, "TWO");
    assert_eq!(check(&app, &mut last, "t", "%*", "#{pane_title}"), vec!["%subscription-changed t $0 @11 1 %2 : TWO"]);
}

#[test]
fn notifications_carry_the_displayed_window_index_and_dashes() {
    let mut app = app();
    app.window_base_index = 1;
    let mut last = std::collections::HashMap::new();
    assert_eq!(check(&app, &mut last, "p", "%1", "#{pane_title}"), vec!["%subscription-changed p $0 @10 1 %1 : one"]);
    assert_eq!(check(&app, &mut last, "w", "@11", "#{window_index}"), vec!["%subscription-changed w $0 @11 2 - : 2"]);
    assert_eq!(check(&app, &mut last, "s", "", "#{pane_title}"), vec!["%subscription-changed s $0 - - - : three"]);
}

#[test]
fn a_closed_pane_leaves_no_value_behind() {
    let mut app = app();
    let mut last = std::collections::HashMap::new();
    check(&app, &mut last, "t", "%*", "#{pane_title}");
    check(&app, &mut last, "u", "%2", "#{pane_title}");
    assert!(last.contains_key("t\u{1f}%2:1"));
    // Window 1 and its panes %2 and %3 go away.
    app.windows.remove(1);
    app.active_idx = 0;
    assert!(check(&app, &mut last, "t", "%*", "#{pane_title}").is_empty());
    assert!(check(&app, &mut last, "u", "%2", "#{pane_title}").is_empty());
    let mut keys: Vec<_> = last.keys().cloned().collect();
    keys.sort();
    // `%*` drops them, as monitor_sweep_all_panes does; `u` keeps its value,
    // as tmux keeps the entries of a `%N` subscription.
    assert_eq!(keys, vec!["t\u{1f}%1:0".to_string(), "u\u{1f}%2:1".to_string()]);
}

#[test]
fn a_pane_or_window_under_a_new_index_is_reported_again() {
    let mut app = app();
    let mut last = std::collections::HashMap::new();
    check(&app, &mut last, "t", "%*", "#{pane_title}");
    check(&app, &mut last, "p", "%2", "#{pane_title}");
    check(&app, &mut last, "w", "@11", "#{window_name}");
    // Window 0 goes away: window @11 is now index 0, as tmux keys its last
    // values by pane or window and index (`wl->idx`).
    app.windows.remove(0);
    app.active_idx = 0;
    assert_eq!(
        check(&app, &mut last, "t", "%*", "#{pane_title}"),
        vec![
            "%subscription-changed t $0 @11 0 %2 : two",
            "%subscription-changed t $0 @11 0 %3 : three",
        ]
    );
    assert_eq!(check(&app, &mut last, "p", "%2", "#{pane_title}"), vec!["%subscription-changed p $0 @11 0 %2 : two"]);
    assert_eq!(check(&app, &mut last, "w", "@11", "#{window_name}"), vec!["%subscription-changed w $0 @11 0 - : shell"]);
    assert!(check(&app, &mut last, "t", "%*", "#{pane_title}").is_empty());
}

#[test]
fn an_empty_session_checks_nothing() {
    let mut app = app();
    app.windows.clear();
    let mut last = std::collections::HashMap::new();
    assert!(check(&app, &mut last, "s", "", "#{pane_title}").is_empty());
    assert!(check(&app, &mut last, "t", "%*", "#{pane_title}").is_empty());
}

// A `#()` in a subscription format starts no command and expands to nothing,
// as tmux's FORMAT_NOJOBS does for monitors (monitor.c
// monitor_create_formats, format.c "#() is disabled"). psmux checks
// subscriptions in the server loop, so a `#()` there ran synchronously once
// per object every second: `#(ping -n 2 127.0.0.1)` under `%*` held every
// client 2 to 3 s per check. The guard must not leak into later expansions.
#[test]
fn a_job_in_a_subscription_format_does_not_run() {
    let app = app();
    let mut last = std::collections::HashMap::new();
    assert_eq!(
        check(&app, &mut last, "j", "%1", "<#(echo RAN969)>#{pane_title}"),
        vec!["%subscription-changed j $0 @10 0 %1 : <>one"]
    );
    let all = values(&app, "%*", "#(echo RAN969)");
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|v| v.3.is_empty()), "{:?}", all);
    assert_eq!(
        crate::format::expand_format("#(echo RAN969)", &app),
        "RAN969",
        "a one shot expansion after a subscription check still runs its job"
    );
}
