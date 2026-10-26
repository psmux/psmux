// Issue #813: control mode answered `unknown command` for set-buffer,
// paste-buffer and delete-buffer, ran the rest of a line after a failing
// command, and treated an unknown command as a runtime error instead of
// tmux's parse error. Registered from src/server/connection.rs.
//
// No server is started: the dispatcher is driven with a stand in server
// thread that runs the buffer requests on its own AppState with the same
// executors the server uses.

use super::*;

/// Run one control command against a stand in server whose AppState starts
/// with the named buffers `have`; return (answered, reply, the AppState after,
/// requests the stand in did not run).
fn run(cmd: &str, args: &[&str], have: &[(&str, &str)]) -> (bool, String, crate::types::AppState, Vec<CtrlReq>) {
    let (ctx, crx) = mpsc::channel::<CtrlReq>();
    let have: Vec<(String, String)> = have.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect();
    let server = std::thread::spawn(move || {
        let mut app = crate::types::AppState::new("t".to_string());
        for (n, t) in have {
            app.paste_set(&n, t);
        }
        let mut other = Vec::new();
        for req in crx {
            match req {
                CtrlReq::SetBufferCmd(set, resp) => {
                    let _ = resp.send(crate::commands::run_set_buffer_args(&mut app, &set));
                }
                CtrlReq::DeleteBufferCmd(name, resp) => {
                    let _ = resp.send(crate::commands::run_delete_buffer(&mut app, name.as_deref()));
                }
                CtrlReq::PasteBuffer(pb, resp) => {
                    let missing = pb.buffer.as_ref().filter(|b| app.paste_get(b).is_none());
                    let _ = resp.send(missing.map(|b| format!("no buffer {}", b)));
                    other.push(CtrlReq::PasteBuffer(pb, mpsc::channel().0));
                }
                o => other.push(o),
            }
        }
        (app, other)
    });
    let (rs, rr) = mpsc::channel::<String>();
    let answered = {
        let tx = TargetedSender::new(&ctx, None);
        dispatch_control_command(cmd, args, &tx, rs, None, false, None, 1, &Default::default(), false, &Default::default())
    };
    drop(ctx);
    let reply = rr.try_recv().unwrap_or_else(|_| "<no reply>".to_string());
    let (app, other) = server.join().unwrap();
    (answered, reply, app, other)
}

#[test]
fn set_buffer_is_a_control_command() {
    let (answered, reply, app, _) = run("set-buffer", &["-b", "mtpaste", "echo from-buffer"], &[]);
    assert!(answered);
    assert_eq!(reply, "");
    assert_eq!(app.paste_get("mtpaste"), Some("echo from-buffer"));
    // Without -b it is a new automatic buffer, named as tmux names it.
    let (_, reply, app, _) = run("set-buffer", &["abc"], &[]);
    assert_eq!(reply, "");
    assert_eq!(app.paste_top(), Some("abc"));
    assert_eq!(app.paste_buffers[0].name, "buffer0");
}

#[test]
fn set_buffer_without_data_is_tmux_error() {
    let (_, reply, app, _) = run("set-buffer", &["-b", "x"], &[]);
    assert_eq!(reply, "\u{0001}ERR\u{0001}no data specified");
    assert_eq!(app.paste_get("x"), None);
}

#[test]
fn set_buffer_append_and_rename() {
    let (_, reply, app, _) = run("set-buffer", &["-a", "-b", "k", "def"], &[("k", "abc")]);
    assert_eq!(reply, "");
    assert_eq!(app.paste_get("k"), Some("abcdef"));

    let (_, reply, app, _) = run("set-buffer", &["-b", "k", "-n", "renamed"], &[("k", "abc")]);
    assert_eq!(reply, "");
    assert_eq!(app.paste_get("k"), None);
    assert_eq!(app.paste_get("renamed"), Some("abc"));

    let (_, reply, _, _) = run("set-buffer", &["-b", "nosuch", "-n", "renamed"], &[]);
    assert_eq!(reply, "\u{0001}ERR\u{0001}unknown buffer: nosuch");
    let (_, reply, _, _) = run("set-buffer", &["-n", "renamed"], &[]);
    assert_eq!(reply, "\u{0001}ERR\u{0001}no buffer");
}

#[test]
fn paste_buffer_is_a_control_command() {
    let (answered, reply, _, seen) = run("paste-buffer", &["-p", "-d", "-b", "mtpaste"], &[("mtpaste", "x")]);
    assert!(answered);
    assert_eq!(reply, "");
    assert!(matches!(seen.as_slice(), [CtrlReq::PasteBuffer(pb, _)] if pb.bracket && pb.delete && pb.buffer.as_deref() == Some("mtpaste")));

    let (_, reply, _, _) = run("paste-buffer", &["-b", "nosuch"], &[]);
    assert_eq!(reply, "\u{0001}ERR\u{0001}no buffer nosuch");
}

#[test]
fn delete_buffer_errors_like_tmux() {
    let (_, reply, app, _) = run("delete-buffer", &["-b", "k"], &[("k", "abc")]);
    assert_eq!(reply, "");
    assert_eq!(app.paste_get("k"), None);

    let (_, reply, _, _) = run("delete-buffer", &["-b", "nosuch"], &[]);
    assert_eq!(reply, "\u{0001}ERR\u{0001}unknown buffer: nosuch");

    // No -b: the newest automatic buffer, and none at all is "no buffer".
    let (_, reply, _, _) = run("delete-buffer", &[], &[("named", "x")]);
    assert_eq!(reply, "\u{0001}ERR\u{0001}no buffer");
}

#[test]
fn paste_api_announces_buffer_changes() {
    let mut app = crate::types::AppState::new("t".to_string());
    let (ntx, nrx) = std::sync::mpsc::channel();
    app.control_clients.insert(1, crate::types::ControlClient {
        client_id: 1,
        cmd_counter: 0,
        echo_enabled: false,
        notification_tx: ntx,
        paused_panes: Default::default(),
        subscriptions: Default::default(),
        subscription_values: Default::default(),
        subscription_last_check: Default::default(),
        output_gate: Default::default(),
        size: None,
        window_sizes: Default::default(),
    });
    let drain = || -> Vec<String> { nrx.try_iter().map(|n| crate::control::format_notification(&n)).collect() };
    app.paste_set("mtpaste", "echo".to_string());
    assert_eq!(drain(), vec!["%paste-buffer-changed mtpaste"]);
    // paste_set frees a buffer of the same name first (paste.c).
    app.paste_set("mtpaste", "again".to_string());
    assert_eq!(drain(), vec!["%paste-buffer-deleted mtpaste", "%paste-buffer-changed mtpaste"]);
    app.paste_add("auto".to_string());
    assert_eq!(drain(), vec!["%paste-buffer-changed buffer0"]);
    assert!(app.paste_delete("mtpaste"));
    assert_eq!(drain(), vec!["%paste-buffer-deleted mtpaste"]);
    assert!(!app.paste_delete("mtpaste"));
    assert!(drain().is_empty());
    crate::commands::run_set_buffer(&mut app, Some("buffer0"), "", false, Some("named")).unwrap();
    assert_eq!(drain(), vec!["%paste-buffer-deleted buffer0", "%paste-buffer-changed named"]);
}

#[test]
fn parse_check_knows_catalog_extension_and_prefix_names() {
    assert!(is_catalog_command("set-buffer"));
    assert!(is_catalog_command("pasteb"));
    assert!(is_control_extension_command("phony-command"));
    assert!(is_control_extension_command("dump-state"));
    // tmux resolves an unambiguous prefix (cmd.c cmd_find_entry).
    assert!(is_control_extension_command("show-option"));
    assert!(!is_catalog_command("bogus-cmd"));
    assert!(!is_control_extension_command("bogus-cmd"));
    assert!(!is_control_extension_command(""));
}
