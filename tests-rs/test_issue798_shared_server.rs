// Issue #798 step 2: the shared server's pure parts. Nothing here opens a
// socket, starts a server or reads the process environment.

use super::ids::ServerIds;
use super::registry::{self, Marker};
use super::reply::{reply_text, Outcome};
use super::router::{self, NewSessionArgs, Opening};

// the id allocator

#[test]
fn a_fresh_server_starts_where_a_per_session_server_does() {
    let ids = ServerIds::new();
    assert_eq!(ids.next_window(), 1);
    assert_eq!(ids.next_pane(), 1);
}

#[test]
fn lending_to_sessions_in_turn_never_repeats_an_id() {
    // Three sessions, each with its own AppState style counters, served one
    // request at a time as the loop does: lend, the request allocates, take
    // back. Every id handed out across all of them must be unique and dense.
    let mut ids = ServerIds::new();
    let mut sessions = [(0usize, 0usize); 3];
    let mut windows = Vec::new();
    let mut panes = Vec::new();
    for round in 0..300 {
        let s = &mut sessions[round % 3];
        ids.lend(&mut s.0, &mut s.1);
        // A request that makes a window with a pane, then splits it.
        for _ in 0..(round % 2 + 1) {
            windows.push(s.0);
            s.0 += 1;
            panes.push(s.1);
            s.1 += 1;
        }
        panes.push(s.1);
        s.1 += 1;
        ids.take_back(s.0, s.1);
    }
    let mut w = windows.clone();
    w.sort_unstable();
    w.dedup();
    assert_eq!(w.len(), windows.len(), "window ids repeat");
    let mut p = panes.clone();
    p.sort_unstable();
    p.dedup();
    assert_eq!(p.len(), panes.len(), "pane ids repeat");
    assert_eq!(w, (1..=windows.len()).collect::<Vec<_>>(), "window ids are dense from 1");
    assert_eq!(p, (1..=panes.len()).collect::<Vec<_>>(), "pane ids are dense from 1");
}

#[test]
fn take_back_never_moves_a_counter_backwards() {
    let mut ids = ServerIds::new();
    let (mut w, mut p) = (0, 0);
    ids.lend(&mut w, &mut p);
    ids.take_back(10, 20);
    // A session whose own field was lowered meanwhile.
    ids.take_back(3, 4);
    assert_eq!((ids.next_window(), ids.next_pane()), (10, 20));
}

#[test]
fn direct_allocation_interleaves_with_lending() {
    let mut ids = ServerIds::new();
    assert_eq!(ids.alloc_window(), 1);
    assert_eq!(ids.alloc_pane(), 1);
    let (mut w, mut p) = (0, 0);
    ids.lend(&mut w, &mut p);
    assert_eq!((w, p), (2, 2));
    ids.take_back(w + 1, p + 2);
    assert_eq!(ids.alloc_window(), 3);
    assert_eq!(ids.alloc_pane(), 4);
}

// the router

#[test]
fn opening_needs_whole_lines() {
    assert_eq!(router::parse_opening(b""), Opening::Undecided);
    assert_eq!(router::parse_opening(b"AUTH k"), Opening::Undecided);
    assert_eq!(router::parse_opening(b"AUTH k\n"), Opening::Undecided);
    assert_eq!(router::parse_opening(b"AUTH k\nlist-win"), Opening::Undecided);
    assert!(!router::opening_complete(b"AUTH k\nlist-win"));
    assert!(router::opening_complete(b"AUTH k\nlist-windows\n"));
}

#[test]
fn opening_kinds() {
    assert_eq!(
        router::parse_opening(b"AUTH abc\nPERSISTENT\nclient-attach\n"),
        Opening::Persistent { key: "abc".into() }
    );
    for c in ["CONTROL", "CONTROL_QUIET", "CONTROL_NOECHO"] {
        let bytes = format!("AUTH abc\r\n{}\r\n", c);
        assert_eq!(router::parse_opening(bytes.as_bytes()), Opening::Control { key: "abc".into() });
    }
    assert_eq!(
        router::parse_opening(b"AUTH abc\nlist-windows -F x\n"),
        Opening::Command { key: "abc".into(), line: "list-windows -F x".into(), lines: 2 }
    );
    assert!(matches!(router::parse_opening(b"hello\nworld\n"), Opening::Undecided));
}

#[test]
fn a_target_line_puts_the_command_on_the_third_line() {
    assert_eq!(router::parse_opening(b"AUTH k\nTARGET A:1\n"), Opening::Undecided);
    assert_eq!(
        router::parse_opening(b"AUTH k\nTARGET A:1\nnew-window\n"),
        Opening::Command { key: "k".into(), line: "new-window".into(), lines: 3 }
    );
}

#[test]
fn supported_and_unsupported_commands() {
    for ok in ["new-window", "neww", "split-window", "send-keys", "capture-pane", "list-sessions",
        "has-session", "kill-session", "kill-server", "dump-state", "client-attach", "set", "bind"] {
        assert!(router::is_supported(ok), "{ok}");
    }
    for no in ["rotate-window", "move-window", "link-window", "join-pane", "swap-pane",
        "choose-tree", "new-session", "rename-session", "run-shell", "if-shell", "source-file"] {
        assert!(!router::is_supported(no), "{no}");
    }
    assert_eq!(router::first_unsupported("list-windows -F '#{window_id}'"), None);
    assert_eq!(router::first_unsupported("rotate-window -t A"), Some("rotate-window".into()));
    // Anywhere in a list: tmux parses the whole list before it runs any of it.
    assert_eq!(router::first_unsupported("list-windows ; bogus ; neww"), Some("bogus".into()));
    assert_eq!(router::first_unsupported("neww \\; join-pane -s 1"), Some("join-pane".into()));
}

// #896: the CLI's save-buffer sends show-buffer and writes the reply into the
// user's file, so the shared server must run show-buffer, not refuse it.
#[test]
fn show_buffer_is_supported() {
    assert!(router::is_supported("show-buffer"));
    assert!(router::is_supported("showb"));
    assert_eq!(router::first_unsupported("show-buffer"), None);
    assert_eq!(router::first_unsupported("show-buffer -b buffer0"), None);
}

#[test]
fn server_wide_writes() {
    for g in ["set -g status-left x", "set-option -gq @a 1", "set -s escape-time 0", "setw -g mode-keys vi",
        "set -wg mode-keys vi", "set-hook -g after-new-window 'display x'", "setenv -g FOO bar",
        "set-environment -gu FOO", "bind x kill-pane", "unbind -a", "set -ug status-left"] {
        assert!(router::is_global_write(g), "{g}");
    }
    for s in ["set status-left x", "set -t A status-left x", "set -p @x 1", "setw mode-keys vi",
        "set-hook after-new-window x", "setenv FOO bar", "show -g status-left", "list-keys",
        "set -t gg status-left x"] {
        assert!(!router::is_global_write(s), "{s}");
    }
    assert_eq!(
        router::global_writes("set -g @a 1 ; new-window ; bind q kill-pane"),
        vec!["set -g \"@a\" \"1\"".to_string(), "bind q kill-pane".to_string()]
    );
}

#[test]
fn a_global_write_loses_its_target_for_the_other_sessions() {
    assert_eq!(router::without_target("set -g -t A @x 1"), "set -g \"@x\" \"1\"");
    assert_eq!(router::without_target("set -gt A @x 1"), "set -g \"@x\" \"1\"");
    assert_eq!(router::without_target("set -tA -g @x 1"), "set -g \"@x\" \"1\"");
    assert_eq!(router::without_target("setenv -g -t B FOO 'a b'"), "setenv -g \"FOO\" \"a b\"");
    assert_eq!(router::without_target("set-hook -g after-new-window 'display x'"), "set-hook -g \"after-new-window\" \"display x\"");
    // Words after the options are kept, even one that looks like -t.
    assert_eq!(router::without_target("set -g @x -t"), "set -g \"@x\" \"-t\"");
    // Not an option, hook or environment command: unchanged.
    assert_eq!(router::without_target("bind -T root x kill-pane"), "bind -T root x kill-pane");
}

#[test]
fn control_fields_round_trip() {
    for v in ["plain", "with space", "100%", "%0A literal", "two\nlines\r\n", ""] {
        assert_eq!(router::decode_field(&router::encode_field(v)), v, "{v:?}");
        assert!(!router::encode_field(v).contains('\n'));
    }
    let a = NewSessionArgs {
        name: Some("A b".into()),
        window_name: Some("w".into()),
        start_dir: Some("C:\\x y".into()),
        cwd: Some("C:\\".into()),
        width: Some(120),
        height: Some(30),
        command: None,
        raw: Some(vec!["cmd.exe".into(), "/c".into(), "echo a=b".into()]),
        env: vec![("K".into(), "v=1".into())],
        print: true,
        format: Some("#{pane_id}".into()),
    };
    let fields: Vec<(String, String)> = router::new_session_fields(&a)
        .iter()
        .map(|l| {
            let (k, v) = l.split_once('=').unwrap();
            (k.to_string(), router::decode_field(v))
        })
        .collect();
    assert_eq!(router::new_session_args_from_fields(&fields), a);
}

// the reply helper

#[test]
fn the_one_reply_helper() {
    // The per session server's unknown command line, byte for byte.
    assert_eq!(reply_text(&Outcome::Unsupported("bogus".into())), "ERROR: unknown command: bogus\n");
    assert_eq!(reply_text(&Outcome::Failed("duplicate session: A".into())), "ERROR: duplicate session: A\n");
    assert_eq!(reply_text(&Outcome::Done(String::new())), "");
    assert_eq!(reply_text(&Outcome::Done("x".into())), "x\n");
    assert_eq!(reply_text(&Outcome::Done("x\n".into())), "x\n");
}

// the registry layout

#[test]
fn marker_names_never_collide() {
    assert_eq!(registry::marker_file_name(None), "default.server");
    assert_eq!(registry::marker_file_name(Some("work")), "L-work.server");
    // `-L default` is not the default namespace (#844).
    assert_ne!(registry::marker_file_name(Some("default")), registry::marker_file_name(None));
    let p = registry::marker_path_in(std::path::Path::new("D:\\data"), Some("ns"));
    assert_eq!(p, std::path::Path::new("D:\\data").join("shared").join("L-ns.server"));
    // A marker is never a `.port` file, so nothing that lists sessions sees it.
    assert_ne!(p.extension().and_then(|e| e.to_str()), Some("port"));
}

#[test]
fn marker_text_round_trips_and_rejects_partial_files() {
    let m = Marker { pid: 1234, creation: 133_000_000_000_000_000, port: 50123, key: "00ff".into() };
    assert_eq!(registry::parse_marker(&registry::format_marker(&m)), Some(m.clone()));
    assert_eq!(registry::parse_marker(""), None);
    assert_eq!(registry::parse_marker("pid=1\ncreation=2\nport=3\nkey=k\n"), None, "no mode line");
    assert_eq!(registry::parse_marker("mode=shared\npid=1\ncreation=2\nport=0\nkey=k\n"), None, "port 0");
    assert_eq!(registry::parse_marker("mode=shared\npid=1\ncreation=2\nport=3\n"), None, "no key");
}
