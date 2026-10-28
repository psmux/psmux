// shared: confirm-before on the server route (CLI, TCP, hooks) arrives as
// CtrlReq::ConfirmBefore and is answered by CtrlReq::ConfirmRespond. The
// shared dispatch dropped both, so `psmux confirm-before kill-pane` followed
// by `y` killed nothing in shared mode while a per session server killed the
// pane. No server, socket or pane is started: the sessions are bare AppStates.

use super::*;

fn session(name: &str) -> Session {
    let mut app = AppState::new(name.to_string());
    app.socket_name = Some("tconfirm".to_string());
    // A confirm prompt needs a client to show it on (no current client
    // otherwise, as in tmux).
    app.register_client(1, false);
    let aliases = app.command_aliases.clone();
    Session {
        app,
        lp: SessionLoop::new(aliases),
        cwd: None,
        port: 0,
        stop: Arc::new(AtomicBool::new(false)),
        _guard: None,
    }
}

struct EnvBack(Option<String>);
impl Drop for EnvBack {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => std::env::set_var("PSMUX_TARGET_SESSION", v),
            None => std::env::remove_var("PSMUX_TARGET_SESSION"),
        }
    }
}

fn send(s: &mut Session, ids: &mut ServerIds, ns: &Option<String>, req: CtrlReq) {
    let pty_system = portable_pty::native_pty_system();
    let flow = run_request(s, ids, &mut crate::server::shared::buffers::ServerBuffers::new(), ns, |app, lp| {
        crate::server::shared::dispatch::dispatch(app, lp, req, &*pty_system)
    });
    assert_eq!(flow, Flow::Continue);
}

fn confirm_prompt(app: &AppState) -> Option<(String, String, char, bool)> {
    match &app.mode {
        Mode::ConfirmMode { prompt, command, key, default_yes, .. } => {
            Some((prompt.clone(), command.clone(), *key, *default_yes))
        }
        _ => None,
    }
}

#[test]
fn confirm_before_opens_the_prompt_in_the_requesting_session_only() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tconfirm".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let mut b = session("B");
    let spec = crate::confirm::ConfirmSpec { key: 'н', default_yes: true, ..Default::default() };
    send(&mut a, &mut ids, &ns, CtrlReq::ConfirmBefore(spec, "kill-pane".to_string(), None));
    let (prompt, command, key, default_yes) = confirm_prompt(&a.app)
        .expect("BUG: shared dispatch dropped ConfirmBefore, no prompt opened");
    assert_eq!(command, "kill-pane");
    // tmux names the confirm key in the prompt (cmd-confirm-before.c:104-108).
    assert_eq!(prompt, "Confirm 'kill-pane'? (н/n)");
    assert_eq!((key, default_yes), ('н', true), "the parsed -c and -y reach the prompt");
    assert!(confirm_prompt(&b.app).is_none(), "B's mode is untouched");
}

#[test]
fn a_custom_prompt_is_shown_as_given() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tconfirm".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let spec = crate::confirm::ConfirmSpec { prompt: Some("really?".to_string()), ..Default::default() };
    send(&mut a, &mut ids, &ns, CtrlReq::ConfirmBefore(spec, "kill-pane".to_string(), None));
    assert_eq!(confirm_prompt(&a.app).map(|p| p.0).as_deref(), Some("really?"));
}

#[test]
fn no_closes_the_prompt_and_runs_nothing() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tconfirm".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    send(&mut a, &mut ids, &ns, CtrlReq::ConfirmBefore(Default::default(), "rename-window NO_RAN".to_string(), None));
    assert!(confirm_prompt(&a.app).is_some(), "BUG: no prompt opened");
    send(&mut a, &mut ids, &ns, CtrlReq::ConfirmRespond(false));
    assert!(confirm_prompt(&a.app).is_none(), "n closes the prompt");
    assert!(matches!(a.app.mode, Mode::Passthrough));
}

#[test]
fn yes_closes_the_prompt_and_runs_the_command() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tconfirm".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    send(&mut a, &mut ids, &ns, CtrlReq::ConfirmBefore(Default::default(), "set-option -g @confirmed ran".to_string(), None));
    assert!(confirm_prompt(&a.app).is_some(), "BUG: no prompt opened");
    send(&mut a, &mut ids, &ns, CtrlReq::ConfirmRespond(true));
    assert!(matches!(a.app.mode, Mode::Passthrough), "y closes the prompt");
    assert_eq!(a.app.user_options.get("@confirmed").map(String::as_str), Some("ran"),
        "y runs the guarded command in the session");
}

#[test]
fn no_attached_client_opens_the_prompt_for_scripts_and_the_first_attach_drops_it() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tconfirm".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    assert!(a.app.reap_client(1));
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    send(&mut a, &mut ids, &ns, CtrlReq::ConfirmBefore(Default::default(), "kill-pane".to_string(), Some(tx)));
    assert_eq!(rx.recv().unwrap(), "", "accepted, as on a per session server");
    assert!(confirm_prompt(&a.app).is_some(), "a script can answer it with confirm-respond");
    assert!(a.app.register_client(2, false));
    assert!(confirm_prompt(&a.app).is_none(), "BUG: it greeted the client that attached");
}

#[test]
fn an_answer_without_a_prompt_does_nothing() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tconfirm".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    send(&mut a, &mut ids, &ns, CtrlReq::ConfirmRespond(true));
    assert!(matches!(a.app.mode, Mode::Passthrough));
    assert!(a.app.user_options.get("@confirmed").is_none());
}
