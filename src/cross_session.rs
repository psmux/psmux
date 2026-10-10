//! Cross-session pane transfer orchestration.
//!
//! Coordinates moving a pane between two session servers via a TCP I/O
//! tunnel.  The real ConPTY stays in the source process; the target gets
//! a proxy pane whose reads/writes are forwarded over the tunnel.
//!
//! Protocol flow (driven by the CLI process in main.rs):
//!
//!   1. CLI sends `pane-forward-extract <window>.<pane>` to **source** session
//!      Source replies: `FORWARD <forward_id> <listen_port> <pid> <title> <rows> <cols> <screen_b64_len>\n<screen_b64>`
//!
//!   2. CLI sends `pane-forward-inject <source_session> <source_addr> <source_key>
//!      <forward_id> <pid> <title> <rows> <cols> <screen_b64_len>\n<screen_b64>`
//!      to **target** session
//!      Target creates a ProxyMasterPty, connects the I/O tunnel, inserts pane.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Resolve a session name to (port, key).
pub fn resolve_session(session_name: &str) -> io::Result<(u16, String)> {
    // The registry lookup folds case; a session stored under another spelling
    // is a different session (issue #774, tmux strcmp).
    if crate::session::registry_case_variant(session_name).is_some() {
        return Err(io::Error::new(io::ErrorKind::NotFound,
            format!("no server for session '{}'", session_name)));
    }
    let port_path = crate::paths::port_file(session_name);
    let port: u16 = std::fs::read_to_string(&port_path)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound,
            format!("no server for session '{}'", session_name)))?
        .trim()
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad port file"))?;
    let key = crate::session::read_session_key(session_name).unwrap_or_default();
    Ok((port, key))
}

/// Send a command to a specific session and return the full response.
pub(crate) fn send_to_session(port: u16, key: &str, cmd: &str) -> io::Result<String> {
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = TcpStream::connect_timeout(
        &addr.parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{}", e)))?,
        Duration::from_millis(2000),
    )?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(5000)));
    write!(stream, "AUTH {}\n{}\n", key, cmd)?;
    stream.flush()?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 65536];
    const MAX_RESPONSE: usize = 4 * 1024 * 1024; // 4 MB cap
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.len() > MAX_RESPONSE {
                    return Err(io::Error::new(io::ErrorKind::InvalidData,
                        "response exceeded 4 MB limit"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock
                   || e.kind() == io::ErrorKind::TimedOut => break,
            Err(_) => break,
        }
    }
    let r = String::from_utf8_lossy(&buf).to_string();
    Ok(if r.starts_with("OK\n") { r[3..].to_string() } else { r })
}

/// Issue #555: resolve a cross-session switch-client target's window/pane
/// components on the DESTINATION server before the switch is signalled.
/// Returns Err("can't find window/pane: X") when a component definitively
/// does not resolve there. Conservative on query trouble: an unreachable or
/// unparseable destination returns Ok so a transient failure never blocks a
/// switch the old path would have performed (the destination session's own
/// existence is the caller's check). One `list-panes -a` round trip carries
/// every fact needed: window identity per row for index/@id/name matching,
/// pane indexes scoped to their window, global pane ids, and the active
/// window for pane-only targets.
pub fn validate_switch_target(
    port: u16,
    key: &str,
    pt: &crate::types::ParsedTarget,
) -> Result<(), String> {
    if pt.pane.is_none() && pt.window.is_none() && pt.window_name.is_none() {
        return Ok(());
    }
    let resp = match send_to_session(
        port,
        key,
        "list-panes -a -F #{window_active}|#{window_id}|#{window_index}|#{window_name}|#{pane_index}|#{pane_id}",
    ) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    struct Row {
        active: bool,
        wid: String,
        widx: String,
        wname: String,
        pidx: String,
        pid: String,
    }
    let mut rows: Vec<Row> = Vec::new();
    for line in resp.lines() {
        let parts: Vec<&str> = line.trim().splitn(6, '|').collect();
        if parts.len() != 6 || !parts[1].starts_with('@') {
            continue;
        }
        rows.push(Row {
            active: parts[0] == "1",
            wid: parts[1].to_string(),
            widx: parts[2].to_string(),
            wname: parts[3].to_string(),
            pidx: parts[4].to_string(),
            pid: parts[5].to_string(),
        });
    }
    if rows.is_empty() {
        return Ok(()); // nothing parseable; do not block
    }
    let win_match = |r: &Row| -> bool {
        if let Some(w) = pt.window {
            if pt.window_is_id {
                r.wid == format!("@{}", w)
            } else {
                r.widx == w.to_string()
            }
        } else if let Some(ref n) = pt.window_name {
            r.wname == *n
        } else {
            // Pane-only target: the pane index resolves against the
            // destination's active window, matching select-pane semantics.
            r.active
        }
    };
    if (pt.window.is_some() || pt.window_name.is_some()) && !rows.iter().any(&win_match) {
        let spec = if let Some(w) = pt.window {
            if pt.window_is_id {
                format!("@{}", w)
            } else {
                w.to_string()
            }
        } else {
            pt.window_name.clone().unwrap_or_default()
        };
        return Err(format!("can't find window: {}", spec));
    }
    if let Some(p) = pt.pane {
        if pt.pane_is_id {
            if !rows.iter().any(|r| r.pid == format!("%{}", p)) {
                return Err(format!("can't find pane: %{}", p));
            }
        } else if !rows.iter().any(|r| win_match(r) && r.pidx == p.to_string()) {
            return Err(format!("can't find pane: {}", p));
        }
    }
    Ok(())
}

/// The window and pane half of a join-pane `-s` / `-t` spec (everything after
/// the session's colon, or the whole spec when it has no session half) in the
/// form the pane-forward commands read: `%<id>` and `@<id>` as they are, and
/// anything else behind a colon, so `1.0` is window 1 pane 0 and not session
/// "1" pane 0, and an empty half is `:` (the active window and pane).
pub fn window_pane_half(spec: &str) -> String {
    let rest = match spec.split_once(':') {
        Some((_, r)) => r,
        None => spec,
    };
    if rest.starts_with(['%', '@']) {
        rest.to_string()
    } else {
        format!(":{}", rest)
    }
}

/// Orchestrate a cross-session pane transfer.
///
/// Called from main.rs when join-pane's `-s` session differs from `-t` session.
/// `src_spec` and `tgt_spec` are window/pane halves from [`window_pane_half`].
/// The target is checked on its server BEFORE the source pane is extracted,
/// so a target that does not resolve leaves both sessions untouched.
/// Returns Ok(()) on success or an error description.
pub fn orchestrate_cross_session_join(
    src_session: &str,
    src_spec: &str,
    tgt_session: &str,
    tgt_spec: &str,
    horizontal: bool,
) -> io::Result<()> {
    // 1. Resolve both sessions
    let (src_port, src_key) = resolve_session(src_session)?;
    let (tgt_port, tgt_key) = resolve_session(tgt_session)?;
    let src_addr = format!("127.0.0.1:{}", src_port);

    // The target window and pane must exist before anything moves.
    validate_switch_target(tgt_port, &tgt_key, &crate::cli::parse_target(tgt_spec))
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e))?;

    // 2. Tell source to extract the pane and start forwarding
    let extract_cmd = format!("pane-forward-extract {}", src_spec);
    let extract_resp = send_to_session(src_port, &src_key, &extract_cmd)?;

    // Parse: FORWARD <forward_id> <listen_port> <pid> <title> <rows> <cols> <screen_b64_len>
    // followed by optional base64 screen data
    let extract_resp = extract_resp.trim();
    if !extract_resp.starts_with("FORWARD ") {
        return Err(io::Error::new(io::ErrorKind::Other,
            format!("extract failed: {}", extract_resp)));
    }

    // The screen base64 payload follows the FORWARD line after a newline.
    // Split it off FIRST: tokenizing the whole response would glue
    // "<screen_b64_len>\n<payload>" into one token whose parse() fails and
    // silently discards the screen snapshot (the pre-fix behavior).
    let (forward_line, payload_after_nl) = match extract_resp.find('\n') {
        Some(nl_pos) => (&extract_resp[..nl_pos], Some(&extract_resp[nl_pos + 1..])),
        None => (extract_resp, None),
    };
    let parts: Vec<&str> = forward_line.trim_end().splitn(8, ' ').collect();
    if parts.len() < 8 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad FORWARD response"));
    }
    let forward_id: u64 = parts[1].parse().unwrap_or(0);
    let fwd_port: u16 = parts[2].parse().unwrap_or(0);
    let pid: u32 = parts[3].parse().unwrap_or(0);
    let title = parts[4].replace('\x01', " "); // spaces encoded as \x01
    let rows: u16 = parts[5].parse().unwrap_or(24);
    let cols: u16 = parts[6].parse().unwrap_or(80);
    let screen_b64_len: usize = parts[7].parse().unwrap_or(0);

    let screen_b64 = match (screen_b64_len, payload_after_nl) {
        (0, _) | (_, None) => None,
        (len, Some(data)) => {
            let data = data.trim_end();
            if data.len() >= len {
                // base64 is ASCII; a corrupt payload must not panic here.
                Some(data.get(..len).unwrap_or(data).to_string())
            } else {
                Some(data.to_string())
            }
        }
    };

    // 3. Build inject command for target. `-tgt=` carries the target window
    // and pane; it used to be computed here and never sent, so the pane
    // landed beside the target session's active pane.
    let h_flag = if horizontal { " -h" } else { "" };
    let tgt_flag = format!(" -tgt={}", tgt_spec);
    let screen_payload = screen_b64.as_deref().unwrap_or("");
    // The server reads commands line by line, and its inject handler collects
    // the payload from the remaining same-line tokens (connection.rs). Base64
    // contains no spaces, so ship it as one trailing token on the command
    // line; a "\n<payload>" continuation would be consumed as a bogus
    // follow-up command and the snapshot silently dropped.
    let payload_token = if screen_payload.is_empty() {
        String::new()
    } else {
        format!(" {}", screen_payload)
    };
    let inject_cmd = format!(
        "pane-forward-inject {} {} {} {} {} {} {} {} {} {}{}{}{}",
        src_session,
        src_addr,
        src_key,
        forward_id,
        fwd_port,
        pid,
        title.replace(' ', "\x01"),
        rows,
        cols,
        screen_payload.len(),
        h_flag,
        tgt_flag,
        payload_token,
    );

    // 4. Tell target to create proxy pane
    let inject_resp = send_to_session(tgt_port, &tgt_key, &inject_cmd)?;
    if inject_resp.trim().starts_with("ERR") {
        return Err(io::Error::new(io::ErrorKind::Other,
            format!("inject failed: {}", inject_resp.trim())));
    }

    Ok(())
}
