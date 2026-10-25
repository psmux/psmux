//! The CLI side of shared mode (#798 step 2), entered from the step 1 gates
//! (`gate_server_start`) wherever the CLI is about to start a server: `new-session`,
//! `start-server`, a bare `psmux`, `-C`/`-CC`. It never returns.
//!
//! `new-session` finds the namespace's live shared server through its marker
//! (`registry`), starts one if there is none, and sends the request to the
//! server's control listener. Without `-d` it then attaches exactly as
//! `psmux attach -t <session>` does, by running that command: the session's
//! registry files have today's shape, so the ordinary client attaches to it.

use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

use super::registry::{self, Marker};
use super::router::{self, NewSessionArgs};

const GLOBAL_VALUE_FLAGS: &[&str] = &["-t", "-L", "-f", "-S"];

fn fail(msg: &str) -> ! {
    let _ = io::stdout().flush();
    eprintln!("{}", msg);
    std::process::exit(1);
}

/// The command word and its arguments, after the global flags.
fn split_command(args: &[String]) -> (Option<String>, Vec<String>, bool) {
    let mut i = 1;
    let mut control = false;
    while i < args.len() {
        let a = args[i].as_str();
        if GLOBAL_VALUE_FLAGS.contains(&a) {
            i += 2;
            continue;
        }
        if a == "-C" || a == "-CC" {
            control = true;
            i += 1;
            continue;
        }
        if a.starts_with('-') && a.len() > 1 {
            i += 1;
            continue;
        }
        return (Some(a.to_string()), args[i + 1..].to_vec(), control);
    }
    (None, Vec::new(), control)
}

/// Parsed `new-session` flags (tmux: `Ac:dDe:EF:f:n:Ps:t:x:Xy:`).
#[derive(Debug, Default)]
struct NewSessionCli {
    args: NewSessionArgs,
    detached: bool,
    attach_if_exists: bool,
    group: Option<String>,
}

fn parse_new_session(words: &[String]) -> Result<NewSessionCli, String> {
    let mut out = NewSessionCli::default();
    let mut i = 0;
    let mut positional: Vec<String> = Vec::new();
    let mut raw = false;
    while i < words.len() {
        let w = &words[i];
        if w == "--" {
            raw = true;
            positional.extend(words[i + 1..].iter().cloned());
            break;
        }
        if !w.starts_with('-') || w.len() < 2 {
            positional.extend(words[i..].iter().cloned());
            break;
        }
        let flags: Vec<char> = w[1..].chars().collect();
        let mut j = 0;
        while j < flags.len() {
            let f = flags[j];
            let takes_value = matches!(f, 'c' | 'e' | 'F' | 'f' | 'n' | 's' | 't' | 'x' | 'y');
            if takes_value {
                let attached: String = flags[j + 1..].iter().collect();
                let value = if !attached.is_empty() {
                    attached
                } else {
                    i += 1;
                    words
                        .get(i)
                        .cloned()
                        .ok_or_else(|| format!("new-session: option requires an argument -- {}", f))?
                };
                match f {
                    'c' => out.args.start_dir = Some(value),
                    'e' => {
                        let (k, v) = value
                            .split_once('=')
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .unwrap_or((value.clone(), String::new()));
                        out.args.env.push((k, v));
                    }
                    'F' => out.args.format = Some(value),
                    'n' => out.args.window_name = Some(value),
                    's' => out.args.name = Some(value),
                    't' => out.group = Some(value),
                    'x' => out.args.width = value.parse().ok(),
                    'y' => out.args.height = value.parse().ok(),
                    _ => {} // -f client flags: nothing in step 2
                }
                break;
            }
            match f {
                'd' => out.detached = true,
                'A' => out.attach_if_exists = true,
                'P' => out.args.print = true,
                'D' | 'E' | 'X' => {}
                other => return Err(format!("new-session: unknown option -- {}", other)),
            }
            j += 1;
        }
        i += 1;
    }
    if !positional.is_empty() {
        if raw {
            out.args.raw = Some(positional);
        } else {
            out.args.command = Some(positional.join(" "));
        }
    }
    Ok(out)
}

fn ping(m: &Marker) -> bool {
    control_request(m, &["ping".to_string()]).is_ok()
}

/// One request on the server's control listener. `Ok` is the reply text
/// after the server's `OK` line; an `ERROR: ` reply is `Err(message)`.
fn control_request(m: &Marker, lines: &[String]) -> Result<String, String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], m.port));
    let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(1000))
        .map_err(|e| format!("cannot reach the shared server: {e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(70)));
    let mut msg = format!("AUTH {}\n", m.key);
    for l in lines {
        msg.push_str(l);
        msg.push('\n');
    }
    s.write_all(msg.as_bytes()).map_err(|e| e.to_string())?;
    let _ = s.flush();
    let mut r = io::BufReader::new(s);
    let mut first = String::new();
    r.read_line(&mut first).map_err(|e| e.to_string())?;
    if let Some(e) = first.trim_end().strip_prefix("ERROR: ") {
        return Err(e.to_string());
    }
    if first.trim_end() != "OK" {
        return Err("no response from the shared server".to_string());
    }
    let mut rest = String::new();
    let _ = io::Read::read_to_string(&mut r, &mut rest);
    if let Some(e) = rest.strip_prefix("ERROR: ") {
        return Err(e.trim_end().to_string());
    }
    Ok(rest)
}

/// Mark this process's stdout/stderr non-inheritable before spawning the
/// server, as the per session cold spawn does (main.rs), so a caller waiting
/// on our pipes is not held open by the server.
#[cfg(windows)]
fn stdio_not_inheritable() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n: u32) -> *mut std::ffi::c_void;
        fn SetHandleInformation(h: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    unsafe {
        SetHandleInformation(GetStdHandle(0xFFFF_FFF5), 1, 0);
        SetHandleInformation(GetStdHandle(0xFFFF_FFF4), 1, 0);
    }
}
#[cfg(not(windows))]
fn stdio_not_inheritable() {}

/// The namespace's live shared server, started if there is none.
fn ensure_server(ns: Option<&str>) -> Result<Marker, String> {
    if let Some(m) = registry::live_marker(ns) {
        if ping(&m) {
            return Ok(m);
        }
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
    let mut args: Vec<String> = vec!["server".into(), super::SHARED_SERVER_ARG.into()];
    if let Some(n) = ns {
        args.push("-L".into());
        args.push(n.to_string());
    }
    stdio_not_inheritable();
    #[cfg(windows)]
    {
        crate::platform::spawn_server_hidden(&exe, &args).map_err(|e| format!("failed to start the shared server: {e}"))?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new(&exe)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to start the shared server: {e}"))?;
    }
    // A concurrent start may win the race; either way the marker that
    // answers a ping is the namespace's server.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(m) = registry::live_marker(ns) {
            if ping(&m) {
                return Ok(m);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("the shared server did not start (timed out)".to_string())
}

fn session_base(ns: Option<&str>, name: &str) -> String {
    match ns {
        Some(n) => format!("{}__{}", n, name),
        None => name.to_string(),
    }
}

fn session_exists(ns: Option<&str>, name: &str) -> bool {
    let base = session_base(ns, name);
    let Ok(p) = std::fs::read_to_string(crate::paths::port_file(&base)) else { return false };
    let Ok(port) = p.trim().parse::<u16>() else { return false };
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(300),
    )
    .is_ok()
}

/// Attach to a session the ordinary way and leave with the client's code.
fn attach(ns: Option<&str>, name: &str) -> ! {
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
    let mut cmd = std::process::Command::new(exe);
    if let Some(n) = ns {
        cmd.arg("-L").arg(n);
    }
    cmd.arg("attach-session").arg("-t").arg(name);
    match cmd.status() {
        Ok(st) => std::process::exit(st.code().unwrap_or(1)),
        Err(e) => fail(&format!("psmux: {e}")),
    }
}

fn new_session(ns: Option<&str>, words: &[String]) -> ! {
    let parsed = match parse_new_session(words) {
        Ok(p) => p,
        Err(e) => fail(&format!("psmux: {e}")),
    };
    if let Some(g) = parsed.group {
        fail(&format!(
            "psmux: new-session -t {}: session groups are not supported by the shared server yet (step 2 of #798)",
            g
        ));
    }
    // Refused before the shared server is started for it; the server checks
    // again for a new-session that reaches it another way.
    if let Some(name) = parsed.args.name.as_deref().filter(|n| !n.is_empty()) {
        if let Some(msg) = crate::session::reserved_session_name_error(ns, name) {
            fail(&msg);
        }
    }
    if parsed.attach_if_exists {
        if let Some(ref name) = parsed.args.name {
            if session_exists(ns, name) {
                if parsed.detached {
                    std::process::exit(0);
                }
                attach(ns, name);
            }
        }
    }
    let mut args = parsed.args;
    args.cwd = std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned());
    let marker = match ensure_server(ns) {
        Ok(m) => m,
        Err(e) => fail(&format!("psmux: {e}")),
    };
    let mut lines = vec!["new-session".to_string()];
    lines.extend(router::new_session_fields(&args));
    lines.push("end".to_string());
    let reply = match control_request(&marker, &lines) {
        Ok(r) => r,
        Err(e) => fail(&e),
    };
    let mut name = String::new();
    let mut printed = String::new();
    for (i, l) in reply.lines().enumerate() {
        if i == 0 {
            if let Some(n) = l.strip_prefix("session=") {
                name = n.to_string();
                continue;
            }
        }
        printed.push_str(l);
        printed.push('\n');
    }
    if !printed.is_empty() {
        print!("{}", printed);
        let _ = io::stdout().flush();
    }
    if parsed.detached {
        std::process::exit(0);
    }
    attach(ns, &name);
}

/// The shared mode CLI. Never returns.
pub(super) fn run(ns: Option<&str>) -> ! {
    let args: Vec<String> = std::env::args().collect();
    let (cmd, rest, control) = split_command(&args);
    if control {
        fail("psmux: control mode (-C/-CC) is not supported by the shared server yet (step 2 of #798)");
    }
    match cmd.as_deref() {
        None => new_session(ns, &[]),
        Some("new-session") | Some("new") => new_session(ns, &rest),
        Some("start-server") | Some("start") | Some("warmup") => match ensure_server(ns) {
            Ok(_) => std::process::exit(0),
            Err(e) => fail(&format!("psmux: {e}")),
        },
        Some(other) => fail(&format!(
            "psmux: {}: not supported by the shared server yet (step 2 of #798)",
            other
        )),
    }
}
