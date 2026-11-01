// run-shell flag parsing, opaque CLI command lines, and start-directory
// selection with tmux's HOME fallback (job.c:143 to :147).
// End to end checks are in tests/test_run_shell_b_cli.ps1 and
// tests/test_run_shell_server_jobs.ps1.

use super::*;

fn words(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn c_takes_the_next_word_as_the_directory() {
    let p = parse_run_shell_args(&["-c", "C:\\work dir", "echo", "x"]).unwrap();
    assert_eq!(p.cwd.as_deref(), Some("C:\\work dir"));
    assert_eq!(p.command, words(&["echo", "x"]));
}

#[test]
fn c_takes_the_rest_of_its_word() {
    let p = parse_run_shell_args(&["-cC:\\w", "echo"]).unwrap();
    assert_eq!(p.cwd.as_deref(), Some("C:\\w"));
    assert_eq!(p.command, words(&["echo"]));
}

#[test]
fn c_combines_with_b_and_d() {
    let p = parse_run_shell_args(&["-bc", "D:\\x", "-d", "1", "go"]).unwrap();
    assert!(p.background);
    assert_eq!(p.cwd.as_deref(), Some("D:\\x"));
    assert_eq!(p.delay, Some(std::time::Duration::from_secs(1)));
    assert_eq!(p.command, words(&["go"]));
}

#[test]
fn c_without_a_value_is_an_error() {
    assert_eq!(
        parse_run_shell_args(&["-c"]).unwrap_err(),
        "-c expects an argument"
    );
    // -d still says -d.
    assert_eq!(
        parse_run_shell_args(&["-d"]).unwrap_err(),
        "-d expects an argument"
    );
}

#[test]
fn c_after_the_command_is_the_commands_own_flag() {
    // `psmux new-window -c dir` as the command keeps its -c.
    let p = parse_run_shell_args(&["psmux", "new-window", "-c", "dir"]).unwrap();
    assert_eq!(p.cwd, None);
    assert_eq!(p.command, words(&["psmux", "new-window", "-c", "dir"]));
}

#[test]
fn server_line_carries_the_directory_and_one_quoted_command() {
    let line = cli_run_shell_server_line(
        true,
        None,
        Some("C:\\Users\\me\\my dir"),
        "helper.exe 'a b'",
    );
    assert!(line.starts_with("run-shell -b -c "), "{line}");
    assert!(line.ends_with('\n'));
    let args = parse_command_line(line.trim_end());
    let parsed = parse_run_shell_args(&args[1..]).unwrap();
    assert!(parsed.background);
    assert_eq!(parsed.cwd.as_deref(), Some("C:\\Users\\me\\my dir"));
    assert_eq!(parsed.command.join(" ").trim(), "helper.exe 'a b'");
}

#[test]
fn server_line_without_a_directory_sends_no_c() {
    let line = cli_run_shell_server_line(false, Some(std::time::Duration::from_millis(1500)), None, "x");
    assert_eq!(line, "run-shell -d 1.5 -- \"x\"\n");
}

fn cwd_of(cwd: Option<&str>) -> String {
    let mut c = std::process::Command::new("cmd.exe");
    c.args(["/d", "/c", "cd"]);
    if let Some(cwd) = resolve_run_shell_cwd(cwd) {
        c.current_dir(cwd);
    }
    let out = c.output().expect("cmd.exe runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn job_runs_in_the_given_directory() {
    let dir = std::env::temp_dir().join(format!("psmux_rs_cwd_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let got = cwd_of(Some(dir.to_str().unwrap()));
    let _ = std::fs::remove_dir(&dir);
    assert_eq!(got.to_lowercase(), dir.to_str().unwrap().to_lowercase());
}

#[test]
fn a_directory_that_is_not_there_falls_back_to_home() {
    let home = crate::paths::home_dir();
    let got = cwd_of(Some("Z:\\psmux\\no\\such\\dir\\here"));
    assert_eq!(got.to_lowercase(), home.to_lowercase());
}

#[test]
fn tilde_is_home() {
    let home = crate::paths::home_dir();
    assert_eq!(cwd_of(Some("~")).to_lowercase(), home.to_lowercase());
}

#[test]
fn no_directory_leaves_the_inherited_one() {
    let here = std::env::current_dir().unwrap();
    assert_eq!(cwd_of(None).to_lowercase(), here.to_str().unwrap().to_lowercase());
}
