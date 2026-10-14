// Issue #804: run-shell, if-shell, #() and popup children (tmux "jobs") had no
// $TMUX. tmux gives every job environ_for_session (job.c:92, environ.c:253):
// the global and session environment plus TMUX (environ.c:282), and sets
// TMUX_PANE for panes only (spawn.c:394).
//
// These tests build commands and read their environment back; nothing is
// spawned and no server is started.

use super::*;
use std::ffi::OsStr;

// JOB_ENV is process wide; the tests that publish into it take this lock and
// leave it empty again.
static JOB_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn env_of<'a>(c: &'a std::process::Command, name: &str) -> Option<Option<&'a OsStr>> {
    c.get_envs()
        .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

struct Published;
impl Published {
    fn new(port: u16, ns: Option<&str>) -> Self {
        publish_job_env(Some(port), ns);
        Published
    }
}
impl Drop for Published {
    fn drop(&mut self) {
        if let Ok(mut g) = JOB_ENV.write() {
            g.clear();
        }
    }
}

#[test]
fn job_env_tmux_matches_the_pane_value() {
    let vars = job_env_vars(Some(51234), Some("ns804"));
    let tmux = vars.iter().find(|(k, _)| k == "TMUX").map(|(_, v)| v.clone()).expect("TMUX");
    assert_eq!(tmux, crate::pane::tmux_env_value(Some(51234), Some("ns804")));
    assert!(tmux.ends_with(",51234,0"), "TMUX = {tmux}");
    assert!(tmux.contains("ns804"), "TMUX names the -L namespace: {tmux}");
    assert!(vars.iter().any(|(k, v)| k == "MSYS2_ENV_CONV_EXCL" && v == "TMUX"));
}

#[test]
fn job_env_has_no_tmux_pane_and_no_pane_marker() {
    // tmux: TMUX_PANE only in spawn.c:394 (panes). PSMUX_SESSION is the psmux
    // pane marker the nested session guard reads.
    let vars = job_env_vars(Some(1), None);
    assert!(!vars.iter().any(|(k, _)| k == "TMUX_PANE"));
    assert!(!vars.iter().any(|(k, _)| k == "PSMUX_SESSION"));
}

#[test]
fn client_process_builds_run_shell_without_tmux() {
    let _l = JOB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let c = build_run_shell_command("echo hi");
    assert!(env_of(&c, "TMUX").is_none());
}

#[test]
fn server_run_shell_child_gets_tmux() {
    let _l = JOB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _p = Published::new(40804, Some("ns804"));
    let want = crate::pane::tmux_env_value(Some(40804), Some("ns804"));
    for cmd in ["echo hi", "pwsh -NoProfile -Command Write-Output hi", "cmd /c echo hi"] {
        let c = build_run_shell_command(cmd);
        assert_eq!(env_of(&c, "TMUX"), Some(Some(OsStr::new(&want))), "{cmd}");
        assert!(env_of(&c, "TMUX_PANE").is_none(), "{cmd}");
        // The -S path env is replaced by TMUX, as for panes.
        assert_eq!(env_of(&c, crate::socket_path::SOCKET_PATH_ENV), Some(None), "{cmd}");
    }
}

#[test]
fn apply_job_env_on_a_plain_command() {
    let _l = JOB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _p = Published::new(40805, None);
    let mut c = std::process::Command::new("cmd");
    apply_job_env(&mut c);
    let want = crate::pane::tmux_env_value(Some(40805), None);
    assert_eq!(env_of(&c, "TMUX"), Some(Some(OsStr::new(&want))));
}

#[test]
fn job_environment_is_server_env_plus_tmux() {
    let _l = JOB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _p = Published::new(40806, Some("ns804"));
    let env = job_environment();
    let tmux: Vec<_> = env.iter().filter(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("TMUX")).collect();
    assert_eq!(tmux.len(), 1, "exactly one TMUX");
    assert_eq!(tmux[0].1, std::ffi::OsString::from(crate::pane::tmux_env_value(Some(40806), Some("ns804"))));
    assert!(!env.iter().any(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(crate::socket_path::SOCKET_PATH_ENV)));
    assert!(env.iter().any(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH")), "server PATH carried");
    // Round trips through the block the server sends for `job-environ`.
    let block = crate::client_env::encode_env_block(env.clone());
    let back = crate::client_env::decode_env_block(&block).expect("decodes");
    // The block drops cmd.exe's "=C:" style entries, which no child needs.
    let usable = env.iter().filter(|(k, _)| !k.to_string_lossy().starts_with('=')).count();
    assert_eq!(back.len(), usable);
    assert!(back.iter().any(|(k, _)| k == "TMUX"));
}

#[test]
fn cli_job_takes_the_server_environment() {
    let server_env: Vec<(std::ffi::OsString, std::ffi::OsString)> = vec![
        ("TMUX".into(), "/tmp/psmux-1/ns,2,0".into()),
        ("I804_G".into(), "gval".into()),
        ("PATH".into(), r"C:\server\bin".into()),
    ];
    let mut c = build_run_shell_command("echo hi");
    apply_server_job_environment(&mut c, &server_env);
    assert_eq!(env_of(&c, "TMUX"), Some(Some(OsStr::new("/tmp/psmux-1/ns,2,0"))));
    assert_eq!(env_of(&c, "I804_G"), Some(Some(OsStr::new("gval"))));
    #[cfg(windows)]
    {
        // The server's PATH, with psmux still reachable on it (#660).
        let path = env_of(&c, "PATH").flatten().unwrap().to_string_lossy().into_owned();
        assert!(path.ends_with(r"C:\server\bin"), "PATH = {path}");
        let own = std::env::current_exe().unwrap().parent().unwrap().to_string_lossy().into_owned();
        assert!(path.split(';').any(|p| p.eq_ignore_ascii_case(&own)), "PATH = {path}");
    }
}
