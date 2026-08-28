//! Proxy PTY for cross-session pane transfer.
//!
//! When a pane is moved between sessions, the real ConPTY stays in the
//! source server process.  The target server gets a `ProxyMasterPty` that
//! tunnels all I/O over a TCP connection back to the source.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{MasterPty, PtySize};

// ── ProxyMasterPty ──────────────────────────────────────────────────────

/// A MasterPty implementation that forwards reads/writes over a TCP stream
/// to the real ConPTY in another session's server process.
///
/// Wire protocol on the I/O stream (binary, length-prefixed):
///   - Bytes from child flow directly on the stream (source -> target)
///   - Input to child flows directly on the stream (target -> source)
///
/// Resize and child status use a separate control TCP connection.
pub struct ProxyMasterPty {
    /// TCP stream for reading PTY output (source -> target).
    reader_stream: Arc<Mutex<TcpStream>>,
    /// TCP stream for writing PTY input (target -> source).
    writer_stream: Arc<Mutex<Option<TcpStream>>>,
    /// Control connection for resize/status commands.
    control_addr: String,
    control_key: String,
    /// Forwarding session and pane identifiers for control commands.
    source_session: String,
    forward_id: u64,
    size: Arc<Mutex<PtySize>>,
}

impl ProxyMasterPty {
    pub fn new(
        reader: TcpStream,
        writer: TcpStream,
        control_addr: String,
        control_key: String,
        source_session: String,
        forward_id: u64,
        rows: u16,
        cols: u16,
    ) -> Self {
        Self {
            reader_stream: Arc::new(Mutex::new(reader)),
            writer_stream: Arc::new(Mutex::new(Some(writer))),
            control_addr,
            control_key,
            source_session,
            forward_id,
            size: Arc::new(Mutex::new(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })),
        }
    }
}

impl MasterPty for ProxyMasterPty {
    fn resize(&self, size: PtySize) -> Result<(), anyhow::Error> {
        // Send resize command via the control connection to the source server
        let cmd = format!(
            "AUTH {}\npane-forward-resize {} {} {}\n",
            self.control_key, self.forward_id, size.rows, size.cols,
        );
        let addr: std::net::SocketAddr = self.control_addr.parse()
            .map_err(|e| anyhow::anyhow!("bad control addr: {}", e))?;
        // Fire-and-forget resize: short timeout since resize is non-critical
        // (local screen updates immediately, source PTY catches up)
        if let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(50)) {
            let _ = s.set_nodelay(true);
            let _ = s.write_all(cmd.as_bytes());
            let _ = s.flush();
        }
        if let Ok(mut sz) = self.size.lock() {
            *sz = size;
        }
        Ok(())
    }

    fn get_size(&self) -> Result<PtySize, anyhow::Error> {
        Ok(self.size.lock().map_err(|e| anyhow::anyhow!("{}", e))?.clone())
    }

    fn try_clone_reader(&self) -> Result<Box<dyn Read + Send>, anyhow::Error> {
        let stream = self.reader_stream.lock()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let cloned = stream.try_clone()
            .map_err(|e| anyhow::anyhow!("clone reader: {}", e))?;
        Ok(Box::new(cloned))
    }

    fn take_writer(&self) -> Result<Box<dyn Write + Send>, anyhow::Error> {
        let mut guard = self.writer_stream.lock()
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        guard.take()
            .map(|s| -> Box<dyn Write + Send> { Box::new(s) })
            .ok_or_else(|| anyhow::anyhow!("writer already taken"))
    }

    // The proxied PTY lives in another process; there is no local fd or
    // termios to expose. `pid_t` is `i32` on every unix target, so the
    // plain alias is written out to avoid a `libc` dependency here.
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<i32> { None }

    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> { None }

    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> { None }
}

// ── ProxyChild ──────────────────────────────────────────────────────────

/// A Child implementation that proxies wait/kill to the source session.
#[derive(Debug)]
pub struct ProxyChild {
    control_addr: String,
    control_key: String,
    forward_id: u64,
    pid: Option<u32>,
    exited: bool,
}

impl ProxyChild {
    pub fn new(
        control_addr: String,
        control_key: String,
        forward_id: u64,
        pid: Option<u32>,
    ) -> Self {
        Self { control_addr, control_key, forward_id, pid, exited: false }
    }

    fn send_control(&self, cmd: &str) -> io::Result<String> {
        let msg = format!("AUTH {}\n{}\n", self.control_key, cmd);
        let addr: std::net::SocketAddr = self.control_addr.parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{}", e)))?;
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(200))?;
        let _ = s.set_nodelay(true);
        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
        s.write_all(msg.as_bytes())?;
        s.flush()?;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            match s.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock
                       || e.kind() == io::ErrorKind::TimedOut => break,
                Err(_) => break,
            }
        }
        let r = String::from_utf8_lossy(&buf).to_string();
        Ok(if r.starts_with("OK\n") { r[3..].to_string() } else { r })
    }
}

impl portable_pty::Child for ProxyChild {
    fn try_wait(&mut self) -> io::Result<Option<portable_pty::ExitStatus>> {
        if self.exited { return Ok(Some(portable_pty::ExitStatus::with_exit_code(0))); }
        let resp = self.send_control(&format!("pane-forward-status {}", self.forward_id))?;
        if resp.trim() == "exited" {
            self.exited = true;
            Ok(Some(portable_pty::ExitStatus::with_exit_code(0)))
        } else {
            Ok(None)
        }
    }

    fn wait(&mut self) -> io::Result<portable_pty::ExitStatus> {
        loop {
            if let Some(st) = self.try_wait()? { return Ok(st); }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn process_id(&self) -> Option<u32> { self.pid }

    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> { None }
}

impl portable_pty::ChildKiller for ProxyChild {
    fn kill(&mut self) -> io::Result<()> {
        let _ = self.send_control(&format!("pane-forward-kill {}", self.forward_id));
        self.exited = true;
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(ProxyChildKiller {
            control_addr: self.control_addr.clone(),
            control_key: self.control_key.clone(),
            forward_id: self.forward_id,
        })
    }
}

#[derive(Debug)]
struct ProxyChildKiller {
    control_addr: String,
    control_key: String,
    forward_id: u64,
}

impl portable_pty::ChildKiller for ProxyChildKiller {
    fn kill(&mut self) -> io::Result<()> {
        let msg = format!("AUTH {}\npane-forward-kill {}\n", self.control_key, self.forward_id);
        if let Ok(addr) = self.control_addr.parse::<std::net::SocketAddr>() {
            if let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
                let _ = s.write_all(msg.as_bytes());
                let _ = s.flush();
            }
        }
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(ProxyChildKiller {
            control_addr: self.control_addr.clone(),
            control_key: self.control_key.clone(),
            forward_id: self.forward_id,
        })
    }
}

// ── Pane assembly from proxy ────────────────────────────────────────────

/// Create a Pane backed by proxy I/O streams instead of a real ConPTY.
/// The reader thread will be started by the caller (server/mod.rs) after
/// inserting the pane into the window tree, same as warm pane transplants.
pub fn create_proxy_pane(
    reader: TcpStream,
    writer: TcpStream,
    control_addr: String,
    control_key: String,
    source_session: String,
    forward_id: u64,
    pid: Option<u32>,
    title: String,
    rows: u16,
    cols: u16,
    pane_id: usize,
    screen_snapshot: Option<Vec<u8>>,
) -> io::Result<crate::types::Pane> {
    let proxy_master = ProxyMasterPty::new(
        reader, writer.try_clone()?, control_addr.clone(),
        control_key.clone(), source_session, forward_id, rows, cols,
    );
    let proxy_child = ProxyChild::new(control_addr, control_key, forward_id, pid);
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10000)));
    // Replay screen snapshot if provided (captures terminal state from source)
    if let Some(snap) = screen_snapshot {
        if let Ok(mut p) = term.lock() {
            p.process(&snap);
        }
    }
    let epoch = Instant::now() - Duration::from_secs(2);
    Ok(crate::types::Pane {
        master: Box::new(proxy_master),
        writer: crate::pane::spawn_pane_write_queue(Box::new(writer)),
        child: Box::new(proxy_child),
        term,
        last_rows: rows,
        last_cols: cols,
        id: pane_id,
        title,
        title_locked: false,
        child_pid: pid,
        data_version: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        bell_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        // CPR responses written via this field are TCP-forwarded to the source
        // ConPTY via the ProxyMasterPty writer.
        cpr_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        // Proxy panes mirror a remote pane's ConPTY; respawning a local shell
        // here would be wrong, so they are never auto-healed.
        spawned_at: None,
    })
}
