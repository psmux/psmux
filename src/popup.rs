//! Popup overlay module.
//!
//! A popup is a **Pane rendered as a floating overlay**, not part of the
//! window tree.  By storing an actual `Pane` inside `PopupMode`, the popup
//! inherits all pane infrastructure: vt100 parsing, PTY I/O, run-length
//! encoded screen serialization, color rendering, and (in the future)
//! copy-mode, scrollback, etc.
//!
//! This module centralises popup-specific logic:
//!  - PTY-backed pane creation  (`create_popup_pane`)
//!  - Server-side JSON serialization (`serialize_popup_overlay`)
//!  - In-process TUI rendering   (`render_popup_overlay`)

use std::sync::{Arc, Mutex};

use crate::layout::serialize_screen_rows;
use crate::types::{Pane, AppState, Mode};

/// Appends the extended underline style and SGR 58 colour of one run to the
/// hand-written popup JSON, in the same shape `dump_layout_json_fast` uses
/// (#589).  Both fields are omitted for ordinary runs.
fn push_run_underline(run: &crate::layout::CellRunJson, out: &mut String) {
    if run.ul > 1 {
        let _ = std::fmt::Write::write_fmt(out, format_args!(",\"ul\":{}", run.ul));
    }
    if let Some(ulc) = &run.ulc {
        out.push_str(",\"ulc\":\"");
        out.push_str(ulc);
        out.push('"');
    }
}

/// Diagnostic-only popup logging, gated by PSMUX_POPUP_DEBUG=1 (no-op otherwise).
/// Writes to %TEMP%\psmux_popup_debug.log (never inside the repo).
fn popup_debug(msg: &str) {
    if std::env::var("PSMUX_POPUP_DEBUG").map(|v| v == "1").unwrap_or(false) {
        let tmp = std::env::var("TEMP").or_else(|_| std::env::var("TMP")).unwrap_or_else(|_| ".".to_string());
        let path = format!("{}\\psmux_popup_debug.log", tmp);
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = std::io::Write::write_all(&mut f, format!("[{} pid={}] {}\n", ts, std::process::id(), msg).as_bytes());
        }
    }
}

// ── display-popup arguments and the popup frame (#792) ─────────────
//
// tmux keeps a popup's look in `struct popup_data` (popup.c): the title from
// `-T`, the border line set from `-B`/`-b` or `popup-border-lines`, and the
// `-S`/`-s` style strings that win over `popup-border-style`/`popup-style`.
// Everything below is the one place psmux decides those things, so the server
// (which sizes the popup's PTY and serialises it), the attached client and the
// in-process renderer can never disagree about the frame again.

/// The values tmux accepts for `popup-border-lines` and `display-popup -b`
/// (options-table.c `options_table_popup_border_lines_list`).
pub const POPUP_BORDER_LINES: [&str; 7] =
    ["single", "double", "heavy", "simple", "rounded", "padded", "none"];

/// Frame colour when `popup-border-style` is unset (psmux's documented default).
pub const DEFAULT_POPUP_BORDER_STYLE: &str = "fg=yellow";

/// What `display-popup` asked the frame to look like.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupLook {
    /// Frame title. Empty draws no title, which is what tmux does without `-T`.
    pub title: String,
    /// Border line set fixed when the popup opened (tmux stores
    /// `pd->border_lines` once). `None` reads `popup-border-lines` live.
    pub border_lines: Option<String>,
    /// `-S border-style`, applied over `popup-border-style`.
    pub border_style: Option<String>,
    /// `-s style`, applied over `popup-style`.
    pub style: Option<String>,
}

impl PopupLook {
    /// A frame titled `title` with every other choice left to the options.
    /// Used by psmux's own text popups (list output, run-shell output).
    pub fn titled(title: &str) -> Self {
        PopupLook { title: title.to_string(), ..Default::default() }
    }
}

/// `display-popup` as tmux parses it: `[-BCEkN] [-b border-lines]
/// [-c target-client] [-d start-directory] [-e environment] [-h height]
/// [-s style] [-S border-style] [-t target-pane] [-T title] [-w width]
/// [-x position] [-y position] [shell-command]`, plus psmux's `-K` (keep the
/// popup open after the command exits). psmux reads `-c` as a start directory,
/// as it always has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayPopupArgs {
    pub width: Option<String>,
    pub height: Option<String>,
    pub start_dir: Option<String>,
    /// Number of `-E` flags (tmux: one closes on exit, two on exit status 0).
    pub close_on_exit: u8,
    /// psmux `-K`.
    pub keep_open: bool,
    /// `-C`: close the popup that is open.
    pub close_existing: bool,
    /// `-B`: no border at all.
    pub no_border: bool,
    /// `-b`, already checked against [`POPUP_BORDER_LINES`].
    pub border_lines: Option<String>,
    pub title: Option<String>,
    pub border_style: Option<String>,
    pub style: Option<String>,
    pub env: Vec<(String, String)>,
    /// The shell command, its words joined with single spaces.
    pub command: String,
}

/// Parse `display-popup` arguments (the words after the command name).
///
/// Like tmux's getopt, flags end at the first word that is not a flag (or at
/// `--`), flags cluster (`-BE`), and a value may be attached (`-Ttitle`). Every
/// word from there on is the command, so a `-T` or `-B` can no longer leak
/// into the command string.
pub fn parse_display_popup_args<S: AsRef<str>>(args: &[S]) -> Result<DisplayPopupArgs, String> {
    let mut out = DisplayPopupArgs::default();
    let mut i = 0;
    let mut after_marker = false;
    while i < args.len() {
        let tok = args[i].as_ref();
        if tok == "--" {
            i += 1;
            after_marker = true;
            break;
        }
        if !tok.starts_with('-') || tok.len() < 2 {
            break;
        }
        i += 1;
        let letters: Vec<(usize, char)> = tok.char_indices().skip(1).collect();
        for &(pos, flag) in letters.iter() {
            let takes_value = matches!(flag, 'b' | 'c' | 'd' | 'e' | 'h' | 's' | 'S' | 't' | 'T' | 'w' | 'x' | 'y');
            if !takes_value {
                match flag {
                    'B' => out.no_border = true,
                    'C' => out.close_existing = true,
                    'E' => out.close_on_exit = out.close_on_exit.saturating_add(1),
                    'K' => out.keep_open = true,
                    'k' | 'N' => {}
                    _ => return Err(format!("unknown flag -{}", flag)),
                }
                continue;
            }
            let attached = &tok[pos + flag.len_utf8()..];
            let value = if !attached.is_empty() {
                attached.to_string()
            } else if let Some(v) = args.get(i) {
                i += 1;
                v.as_ref().to_string()
            } else {
                return Err(format!("-{} expects an argument", flag));
            };
            match flag {
                'b' => {
                    if !POPUP_BORDER_LINES.contains(&value.as_str()) {
                        return Err(format!("popup-border-lines unknown value: {}", value));
                    }
                    out.border_lines = Some(value);
                }
                'c' | 'd' => out.start_dir = Some(value),
                'e' => {
                    if let Some((k, v)) = value.split_once('=') {
                        out.env.push((k.to_string(), v.to_string()));
                    }
                }
                'h' => out.height = Some(value),
                's' => out.style = Some(value),
                'S' => out.border_style = Some(value),
                'T' => out.title = Some(value),
                'w' => out.width = Some(value),
                // -t target pane and -x/-y position: consumed so they never
                // become part of the command.
                _ => {}
            }
            break; // a value-taking flag ends the cluster
        }
    }
    let words: Vec<&str> = args[i.min(args.len())..].iter().map(|a| a.as_ref()).collect();
    // `-- prog args...` is an argument vector, run as it stands (tmux
    // job_run execvp's an argv of more than one word), in the argv form
    // create_popup_pane hands to build_command (#794). Without `--` the words
    // stay one command string, as they always have here.
    out.command = if after_marker && words.len() > 1 {
        crate::pane::encode_argv_form(&words)
    } else {
        words.join(" ")
    };
    Ok(out)
}

/// The border line set a new popup gets: `-B` beats `-b` beats the option,
/// and an unknown option value falls back to `single` (tmux's default).
pub fn resolve_popup_border_lines(no_border: bool, flag: Option<&str>, option: Option<&str>) -> String {
    if no_border {
        return "none".to_string();
    }
    let pick = flag.or(option).unwrap_or("single");
    if POPUP_BORDER_LINES.contains(&pick) { pick.to_string() } else { "single".to_string() }
}

/// Columns and rows the popup's content (and so its PTY child) gets inside a
/// `width` x `height` popup: everything with no border, two fewer each way
/// with one (tmux popup_display: `jx = sx - 2; jy = sy - 2`).
pub fn popup_content_size(width: u16, height: u16, border_lines: &str) -> (u16, u16) {
    if border_lines == "none" {
        (width, height)
    } else {
        (width.saturating_sub(2), height.saturating_sub(2))
    }
}

/// The style strings a popup is drawn with: the option first, then the flag,
/// so the flag's colours win the way tmux's `style_parse` over the option cell
/// does. An unset `popup-border-style` stands in as [`DEFAULT_POPUP_BORDER_STYLE`].
pub fn popup_effective_styles(
    look: &PopupLook,
    opt_border_style: Option<&str>,
    opt_style: Option<&str>,
) -> (String, String) {
    fn join(a: Option<&str>, b: Option<&str>) -> String {
        [a, b].iter().flatten().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(",")
    }
    let border_opt = match opt_border_style {
        Some(s) if !s.trim().is_empty() => s,
        _ => DEFAULT_POPUP_BORDER_STYLE,
    };
    (
        join(Some(border_opt), look.border_style.as_deref()),
        join(opt_style, look.style.as_deref()),
    )
}

/// Everything needed to draw one popup frame, shared by every renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupFrame {
    pub title: String,
    pub border_lines: String,
    pub border_style: ratatui::style::Style,
    /// Default colours of the content area (`popup-style`).
    pub style: ratatui::style::Style,
}

impl PopupFrame {
    /// Build a frame from resolved strings. Like tmux (popup.c sets
    /// `border_cell.attr = 0` and `defaults.attr = 0`) only the colours of the
    /// styles are kept.
    pub fn new(title: &str, border_lines: &str, border_style: &str, style: &str) -> Self {
        fn colours(s: &str) -> ratatui::style::Style {
            let parsed = crate::style::parse_tmux_style(s);
            ratatui::style::Style { fg: parsed.fg, bg: parsed.bg, ..Default::default() }
        }
        let lines = if POPUP_BORDER_LINES.contains(&border_lines) { border_lines } else { "single" };
        PopupFrame {
            title: title.to_string(),
            border_lines: lines.to_string(),
            border_style: colours(border_style),
            style: colours(style),
        }
    }

    pub fn has_border(&self) -> bool { self.border_lines != "none" }
}

/// The glyphs of each border line set (tmux tty-acs.c and tmux.h
/// SIMPLE_BORDERS / PADDED_BORDERS).
pub fn popup_border_set(lines: &str) -> ratatui::symbols::border::Set<'static> {
    use ratatui::symbols::border;
    match lines {
        "double" => border::DOUBLE,
        "heavy" => border::THICK,
        "rounded" => border::ROUNDED,
        "simple" => border::Set {
            top_left: "+", top_right: "+", bottom_left: "+", bottom_right: "+",
            vertical_left: "|", vertical_right: "|",
            horizontal_top: "-", horizontal_bottom: "-",
        },
        "padded" => border::Set {
            top_left: " ", top_right: " ", bottom_left: " ", bottom_right: " ",
            vertical_left: " ", vertical_right: " ",
            horizontal_top: " ", horizontal_bottom: " ",
        },
        _ => border::PLAIN,
    }
}

/// Where a `want_w` x `want_h` popup sits inside `area`: centred, and kept at
/// least one cell clear of each edge.
pub fn popup_rect(area: ratatui::layout::Rect, want_w: u16, want_h: u16) -> ratatui::layout::Rect {
    let w = want_w.min(area.width.saturating_sub(2));
    let h = want_h.min(area.height.saturating_sub(2));
    ratatui::layout::Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// The content area of a popup drawn at `popup_area`.
pub fn popup_inner_rect(popup_area: ratatui::layout::Rect, has_border: bool) -> ratatui::layout::Rect {
    if !has_border {
        return popup_area;
    }
    ratatui::layout::Rect {
        x: popup_area.x.saturating_add(1),
        y: popup_area.y.saturating_add(1),
        width: popup_area.width.saturating_sub(2),
        height: popup_area.height.saturating_sub(2),
    }
}

/// Draw a popup: clear its area, draw the frame and title (tmux
/// screen_write_box: the title starts two cells in and is cut to width - 4),
/// then the content, with cells left at the terminal default colours taking
/// `popup-style`'s colours.
pub fn draw_popup(
    buf: &mut ratatui::buffer::Buffer,
    popup_area: ratatui::layout::Rect,
    frame: &PopupFrame,
    content: ratatui::text::Text<'static>,
    scroll: u16,
) {
    use ratatui::style::Color;
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = popup_area.intersection(buf.area);
    if popup_area.width == 0 || popup_area.height == 0 {
        return;
    }
    Clear.render(popup_area, buf);
    if frame.has_border() {
        Block::default()
            .borders(Borders::ALL)
            .border_set(popup_border_set(&frame.border_lines))
            .border_style(frame.border_style)
            .render(popup_area, buf);
        if !frame.title.is_empty() && popup_area.width > 4 {
            let max = (popup_area.width - 4) as usize;
            buf.set_stringn(popup_area.x + 2, popup_area.y, &frame.title, max, frame.border_style);
        }
    }
    let inner = popup_inner_rect(popup_area, frame.has_border());
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    Paragraph::new(content).scroll((scroll, 0)).render(inner, buf);
    let (fg, bg) = (frame.style.fg, frame.style.bg);
    if fg.is_none() && bg.is_none() {
        return;
    }
    for y in inner.top()..inner.bottom() {
        for x in inner.left()..inner.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if let Some(c) = fg { if cell.fg == Color::Reset { cell.fg = c; } }
                if let Some(c) = bg { if cell.bg == Color::Reset { cell.bg = c; } }
            }
        }
    }
}

/// Open a `display-popup` request on `app`: size the PTY child to the frame
/// it will be drawn in and store the look every renderer reads.
///
/// `area` is the size percentages resolve against. `shell_when_empty` starts
/// the default shell when no command is given (tmux parity, #351).
pub fn open_display_popup(
    app: &mut AppState,
    req: DisplayPopupArgs,
    area: (u16, u16),
    close_on_exit: bool,
    shell_when_empty: bool,
    session_name: &str,
) {
    if req.close_existing {
        if matches!(app.mode, Mode::PopupMode { .. }) {
            app.mode = Mode::Passthrough;
        }
        return;
    }
    let width = crate::commands::parse_popup_dim_local(req.width.as_deref().unwrap_or("80"), area.0, 80);
    let height = crate::commands::parse_popup_dim_local(req.height.as_deref().unwrap_or("24"), area.1, 24);
    let border_lines = resolve_popup_border_lines(
        req.no_border,
        req.border_lines.as_deref(),
        app.user_options.get("popup-border-lines").map(|s| s.as_str()),
    );
    // tmux expands -T and -d as formats against the target.
    let title = req.title.as_deref().map(|t| crate::format::expand_format(t, app)).unwrap_or_default();
    let start_dir = req.start_dir.as_deref().map(|d| crate::format::expand_format(d, app)).filter(|d| !d.is_empty());
    let (cols, rows) = popup_content_size(width, height, &border_lines);
    let pane = if shell_when_empty || !req.command.is_empty() {
        let env_owned;
        let env = if req.env.is_empty() {
            &app.environment
        } else {
            let mut e = app.environment.clone();
            for (k, v) in &req.env {
                e.insert(k.clone(), v.clone());
            }
            env_owned = e;
            &env_owned
        };
        create_popup_pane(
            &req.command,
            start_dir.as_deref(),
            rows,
            cols,
            app.next_pane_id,
            session_name,
            env,
            &app.env_scopes.child_removals(&app.environment),
            app.host_colors.as_ref(),
        )
    } else {
        None
    };
    let look = PopupLook {
        title,
        border_lines: Some(border_lines),
        border_style: req.border_style,
        style: req.style,
    };
    let failed = shell_when_empty && pane.is_none();
    app.mode = Mode::PopupMode {
        command: req.command,
        // PTY spawn failed: a static, closable popup so the user is never
        // stuck with a blank, unresponsive box.
        output: if failed { "Failed to start popup shell. Press 'q' or Escape to close\n".to_string() } else { String::new() },
        process: None,
        width,
        height,
        close_on_exit: close_on_exit || failed,
        popup_pane: pane,
        scroll_offset: 0,
        look,
    };
}

/// The frame of the popup open on `app`, with the options read now (tmux
/// reapplies popup styles on every redraw, popup_reapply_styles).
pub fn popup_frame_for(app: &AppState, look: &PopupLook) -> PopupFrame {
    let (title, lines, bs, s) = popup_frame_wire(app, look);
    PopupFrame::new(&title, &lines, &bs, &s)
}

/// The frame as the strings the server sends the client: title, border line
/// set, border style and content style. The client rebuilds it with
/// [`PopupFrame::new`], the same constructor [`popup_frame_for`] uses.
pub fn popup_frame_wire(app: &AppState, look: &PopupLook) -> (String, String, String, String) {
    let opt = |k: &str| app.user_options.get(k).map(|s| s.as_str());
    let lines = look.border_lines.clone().unwrap_or_else(|| resolve_popup_border_lines(false, None, opt("popup-border-lines")));
    let (bs, s) = popup_effective_styles(look, opt("popup-border-style"), opt("popup-style"));
    (look.title.clone(), lines, bs, s)
}

// ── Popup pane creation ─────────────────────────────────────────────

/// Spawn a PTY-backed `Pane` for use inside a popup overlay.
///
/// This reuses the same PTY infrastructure as regular panes (ConPTY,
/// vt100 parser, reader thread) but does NOT add the pane to any window
/// tree.  The returned `Pane` is stored inside `Mode::PopupMode`.
pub fn create_popup_pane(
    command: &str,
    start_dir: Option<&str>,
    rows: u16,
    cols: u16,
    pane_id: usize,
    session_name: &str,
    environment: &std::collections::HashMap<String, String>,
    env_removals: &[String],
    host_colors: Option<&crate::types::HostColors>,
) -> Option<Pane> {
    let pty_sys = portable_pty::native_pty_system();
    let pty_size = portable_pty::PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = pty_sys.openpty(pty_size).ok()?;

    let mut cmd_builder = if command.trim().is_empty() {
        crate::pane::build_command(None, true, false)
    } else if command.trim_start().starts_with("-- ") {
        // The `-- prog args...` argv form (#582) that new-pane's parser
        // emits for a multi-token command after `--`. Handed to
        // `pwsh -Command` it was the string "-- cmd.exe /q /k", which pwsh
        // rejects at once, so the floating pane died as soon as it was made
        // (issue #793 follow up). The tiled panes' builder decodes it.
        crate::pane::build_command(Some(command), true, false)
    } else {
        let mut builder = portable_pty::CommandBuilder::new(
            if cfg!(windows) { "pwsh" } else { "sh" },
        );
        if cfg!(windows) {
            builder.args(["-NoProfile", "-Command", command]);
        } else {
            builder.args(["-c", command]);
        }
        builder
    };
    if let Some(dir) = start_dir {
        cmd_builder.cwd(dir);
    } else if let Ok(dir) = std::env::current_dir() {
        cmd_builder.cwd(dir);
    }
    // Color support env vars (#154)
    cmd_builder.env("TERM", "xterm-256color");
    cmd_builder.env("COLORTERM", "truecolor");
    cmd_builder.env("PSMUX_SESSION", session_name);
    // A popup command is a job in tmux (popup.c:648 job_run), so it has $TMUX
    // like a run-shell child, and no TMUX_PANE (#804).
    let job_env = crate::commands::job_env();
    if !job_env.is_empty() {
        for (k, v) in &job_env {
            cmd_builder.env(k, v);
        }
        cmd_builder.env_remove(crate::socket_path::SOCKET_PATH_ENV);
    }
    // A popup pty is not a window pane, so a client started in here is not a
    // nested session — tmux allows `attach` inside a popup because the popup
    // pty never appears in all_window_panes.  Mark the child so the psmux
    // nested-session guard can make the same distinction (#537).
    cmd_builder.env(crate::util::POPUP_CHILD_ENV, "1");
    // A psmux client can now legitimately run in here (#537), and it must not
    // query psmux for the terminal colors, so hand it the ones we already know.
    crate::pane::set_host_colors_env(&mut cmd_builder, host_colors);
    crate::pane::apply_user_environment(&mut cmd_builder, environment);
    // Same environment as a new pane (#775): names removed with
    // `set-environment -r` or hidden with `-h` must not come back from the
    // registry fill (#773) in a popup either.
    crate::pane::apply_env_removals(&mut cmd_builder, env_removals);
    // NOTE: the interactive-shell-for-empty-command behavior (tmux parity, #351)
    // is handled above where cmd_builder is constructed: an empty command uses
    // build_command(None, true, false) to launch the default shell as a REPL.
    // The remaining half of the #351 fix (answering ESC[6n so PSReadLine renders)
    // is the shared spawn_reader_thread + cpr_pending wiring below.

    let child = pair.slave.spawn_command(cmd_builder).ok()?;
    popup_debug(&format!("create_popup_pane: spawned child for command=[{}] rows={} cols={}", command, rows, cols));
    drop(pair.slave); // required for ConPTY

    let term: Arc<Mutex<vt100::Parser>> =
        Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let term_reader = term.clone();

    // Shared signal Arcs. Using the SAME reader thread as regular panes is what
    // makes interactive popups work: spawn_reader_thread scans for ESC[6n (CPR)
    // queries and raises cpr_pending / CPR_DATA_PENDING so the server can answer
    // them. Without that, PSReadLine (and fzf, etc.) block forever on startup
    // waiting for the cursor-position report and the popup renders blank (#351).
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let cursor_shape = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
        crate::pane::CURSOR_SHAPE_UNSET,
    ));
    let bell_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cpr_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let color_query_pending = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let output_ring = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));

    match pair.master.try_clone_reader() {
        Ok(reader) => {
            crate::pane::spawn_reader_thread(
                reader,
                term_reader,
                data_version.clone(),
                cursor_shape.clone(),
                bell_pending.clone(),
                cpr_pending.clone(),
                color_query_pending.clone(),
                output_ring.clone(),
                pane_id,
                crate::platform::mouse_inject::get_child_pid(&*child),
            );
        }
        Err(e) => {
            popup_debug(&format!("popup reader: try_clone_reader FAILED: {}", e));
        }
    }

    let pty_writer = crate::pane::spawn_pane_write_queue(pair.master.take_writer().ok()?);

    // Brief delay so the reader thread processes initial output before the
    // first frame is serialized to clients.
    std::thread::sleep(std::time::Duration::from_millis(50));
    if std::env::var("PSMUX_POPUP_DEBUG").map(|v| v == "1").unwrap_or(false) {
        if let Ok(p) = term.lock() {
            let contents = p.screen().contents();
            let preview: String = contents.chars().take(120).collect();
            popup_debug(&format!("post-50ms screen contents len={} preview=[{}]", contents.len(), preview.replace('\n', "\\n")));
        }
    }

    let child_pid = crate::platform::mouse_inject::get_child_pid(&*child);
    let epoch = std::time::Instant::now() - std::time::Duration::from_secs(2);

    Some(Pane {
        master: pair.master,
        writer: pty_writer,
        child,
        term,
        last_rows: rows,
        last_cols: cols,
        id: pane_id,
        title: String::new(),
        title_locked: false,
        child_pid,
        data_version,
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None, win32_input_latched: false, input_off: false,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape,
        bell_pending,
        // cpr_pending is now driven by the shared spawn_reader_thread above, so
        // the server can answer ESC[6n queries for interactive popup shells (#351).
        cpr_pending,
        color_query_pending,
        copy_state: None, live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring,
        // Popup shells are overlays, never tiled panes; never auto-heal them.
        spawned_at: None,
        // A popup is not a window pane, so it has no `#{pane_start_command}`
        // of its own (#580).
        start_command: String::new(),
        cwd_hint: None,
    })
}

// ── Empty (childless) panes ─────────────────────────────────────────

/// A stand-in `Child` for an empty pane (tmux `-E`), which has no process.
/// `try_wait` always reports "still running" so the pane is never reaped, and
/// `kill` is a no-op. This lets a pane exist with a live PTY but no command,
/// exactly like a tmux empty pane, until `respawn-pane` gives it a command.
#[derive(Debug, Clone)]
pub struct NullChild;

impl portable_pty::ChildKiller for NullChild {
    fn kill(&mut self) -> std::io::Result<()> { Ok(()) }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> { Box::new(NullChild) }
}

impl portable_pty::Child for NullChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> { Ok(None) }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> { Ok(portable_pty::ExitStatus::with_exit_code(0)) }
    fn process_id(&self) -> Option<u32> { None }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> { None }
}

/// Create an EMPTY pane (tmux `-E`): a PTY-backed `Pane` with NO child process.
/// It renders blank and ignores input until `respawn-pane` gives it a command.
/// No reader thread is spawned since there is never any output.
pub fn create_empty_pane(rows: u16, cols: u16, pane_id: usize) -> Option<Pane> {
    let pty_sys = portable_pty::native_pty_system();
    let pty_size = portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_sys.openpty(pty_size).ok()?;
    let pty_writer = crate::pane::spawn_pane_write_queue(pair.master.take_writer().ok()?);
    // Drop the slave: with no child attached the pty stays inert; we never read.
    drop(pair.slave);
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = std::time::Instant::now() - std::time::Duration::from_secs(2);
    Some(Pane {
        master: pair.master,
        writer: pty_writer,
        child: Box::new(NullChild),
        term,
        last_rows: rows,
        last_cols: cols,
        id: pane_id,
        title: String::new(),
        title_locked: false,
        child_pid: None,
        data_version: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None, win32_input_latched: false, input_off: false,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(crate::pane::CURSOR_SHAPE_UNSET)),
        bell_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        cpr_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        color_query_pending: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        // Empty (`-E`) panes have a NullChild that never "exits"; not healable.
        spawned_at: None,
        // Nothing was ever started here, so `#{pane_start_command}` is empty
        // until a respawn gives the pane a command (#580).
        start_command: String::new(),
        cwd_hint: None,
    })
}

// ── Server-side popup serialization ─────────────────────────────────

/// Build a JSON fragment with overlay state for the current popup.
///
/// Returns a string like `,"popup_active":true,"popup_rows":[...]`
/// that the server injects into the dump-state JSON.  Reuses the same
/// `rows_v2` serialization format as regular panes via
/// `serialize_screen_rows()`.
pub fn serialize_popup_overlay(app: &AppState) -> String {
    use crate::server::helpers::json_escape_string;

    let mut out = String::new();
    match &app.mode {
        Mode::PopupMode {
            command,
            output,
            width,
            height,
            popup_pane,
            look,
            ..
        } => {
            out.push_str(",\"popup_active\":true");
            out.push_str(",\"popup_command\":\"");
            out.push_str(&json_escape_string(command));
            out.push('"');
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(",\"popup_width\":{},\"popup_height\":{}", width, height),
            );
            // The frame, resolved here once so the client draws exactly what
            // the server sized the PTY for (#792).
            let frame = popup_frame_wire(app, look);
            for (key, val) in [
                ("popup_title", &frame.0),
                ("popup_border_lines", &frame.1),
                ("popup_border_style", &frame.2),
                ("popup_style", &frame.3),
            ] {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!(",\"{}\":\"{}\"", key, json_escape_string(val)));
            }
            let (inner_w, inner_h) = popup_content_size(*width, *height, &frame.1);

            if let Some(pane) = popup_pane {
                // PTY popup: serialize using the shared pane screen serializer
                // The popup child owns the cursor while the popup is up (tmux
                // popup_mode_cb), so publish its position and visibility too.
                // Without them the client has nothing to place the cursor on
                // and leaves it parked on the pane underneath (#507).
                let mut cur_row: u16 = 0;
                let mut cur_col: u16 = 0;
                let mut cur_hidden = false;
                out.push_str(",\"popup_rows\":[");
                if let Ok(parser) = pane.term.lock() {
                    let screen = parser.screen();
                    let (cr, cc) = screen.cursor_position();
                    cur_row = cr;
                    cur_col = cc;
                    cur_hidden = screen.hide_cursor();
                    let rows_data = serialize_screen_rows(screen, inner_h, inner_w);
                    for (i, row) in rows_data.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        // Serialize RowRunsJson inline (avoids serde for perf)
                        out.push_str("{\"runs\":[");
                        for (j, run) in row.runs.iter().enumerate() {
                            if j > 0 {
                                out.push(',');
                            }
                            out.push_str("{\"text\":\"");
                            json_esc_inline(&run.text, &mut out);
                            out.push_str("\",\"fg\":\"");
                            out.push_str(&run.fg);
                            out.push_str("\",\"bg\":\"");
                            out.push_str(&run.bg);
                            let _ = std::fmt::Write::write_fmt(
                                &mut out,
                                format_args!("\",\"flags\":{},\"width\":{}", run.flags, run.width),
                            );
                            push_run_underline(run, &mut out);
                            out.push('}');
                        }
                        out.push_str("]}");
                    }
                }
                out.push(']');
                out.push_str(",\"popup_lines\":[]");
                out.push_str(",\"popup_has_pty\":true");
                let _ = std::fmt::Write::write_fmt(
                    &mut out,
                    format_args!(
                        ",\"popup_cursor_row\":{},\"popup_cursor_col\":{},\"popup_hide_cursor\":{}",
                        cur_row.min(inner_h.saturating_sub(1)),
                        cur_col.min(inner_w.saturating_sub(1)),
                        cur_hidden
                    ),
                );
            } else {
                // Static (non-PTY) popup: plain text lines
                out.push_str(",\"popup_rows\":[]");
                out.push_str(",\"popup_lines\":[");
                for (i, line) in output.lines().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    out.push_str(&json_escape_string(line));
                    out.push('"');
                }
                out.push(']');
                out.push_str(",\"popup_has_pty\":false");
            }
        }
        Mode::MenuMode { menu } => {
            out.push_str(",\"popup_active\":false,\"popup_rows\":[],\"popup_lines\":[],\"popup_has_pty\":false");
            out.push_str(",\"menu_active\":true");
            out.push_str(",\"menu_title\":\"");
            out.push_str(&json_escape_string(&menu.title));
            out.push('"');
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(",\"menu_selected\":{}", menu.selected),
            );
            out.push_str(",\"menu_items\":[");
            for (i, item) in menu.items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str("{\"name\":\"");
                out.push_str(&json_escape_string(&item.name));
                out.push_str("\",\"key\":");
                if let Some(k) = item.key {
                    out.push('"');
                    out.push(k);
                    out.push('"');
                } else {
                    out.push_str("null");
                }
                out.push_str(",\"command\":\"");
                out.push_str(&json_escape_string(&item.command));
                out.push_str("\",\"is_separator\":");
                out.push_str(if item.is_separator { "true" } else { "false" });
                out.push('}');
            }
            out.push(']');
        }
        Mode::ConfirmMode { prompt, .. } => {
            out.push_str(",\"popup_active\":false,\"popup_rows\":[],\"popup_lines\":[],\"popup_has_pty\":false");
            out.push_str(",\"menu_active\":false,\"menu_title\":\"\",\"menu_selected\":0,\"menu_items\":[]");
            out.push_str(",\"confirm_active\":true,\"confirm_prompt\":\"");
            out.push_str(&json_escape_string(prompt));
            out.push('"');
        }
        Mode::PaneChooser { .. } => {
            out.push_str(",\"popup_active\":false,\"popup_rows\":[],\"popup_lines\":[],\"popup_has_pty\":false");
            out.push_str(",\"menu_active\":false,\"menu_title\":\"\",\"menu_selected\":0,\"menu_items\":[]");
            out.push_str(",\"confirm_active\":false,\"confirm_prompt\":\"\"");
            out.push_str(",\"display_panes\":true");
        }
        Mode::CustomizeMode { ref options, selected, scroll_offset, editing, ref edit_buffer, edit_cursor, ref filter } => {
            out.push_str(",\"popup_active\":false,\"popup_rows\":[],\"popup_lines\":[],\"popup_has_pty\":false");
            out.push_str(",\"menu_active\":false,\"menu_title\":\"\",\"menu_selected\":0,\"menu_items\":[]");
            out.push_str(",\"confirm_active\":false,\"confirm_prompt\":\"\"");
            out.push_str(",\"display_panes\":false");
            out.push_str(",\"customize_active\":true");
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(",\"customize_selected\":{},\"customize_scroll\":{},\"customize_editing\":{},\"customize_cursor\":{}",
                    selected, scroll_offset, editing, edit_cursor),
            );
            out.push_str(",\"customize_edit_buf\":\"");
            out.push_str(&json_escape_string(edit_buffer));
            out.push('"');
            out.push_str(",\"customize_filter\":\"");
            out.push_str(&json_escape_string(filter));
            out.push('"');
            // Serialize visible option rows
            out.push_str(",\"customize_options\":[");
            let filter_lower = filter.to_lowercase();
            let mut first = true;
            for (i, (name, value, scope)) in options.iter().enumerate() {
                if !filter.is_empty() && !name.to_lowercase().contains(&filter_lower) {
                    continue;
                }
                if !first { out.push(','); }
                first = false;
                out.push_str("{\"i\":");
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{}", i));
                out.push_str(",\"n\":\"");
                out.push_str(&json_escape_string(name));
                out.push_str("\",\"v\":\"");
                out.push_str(&json_escape_string(value));
                out.push_str("\",\"s\":\"");
                out.push_str(&json_escape_string(scope));
                out.push_str("\"}");
            }
            out.push(']');
        }
        _ => {
            out.push_str(",\"popup_active\":false,\"popup_rows\":[],\"popup_lines\":[],\"popup_has_pty\":false");
            out.push_str(",\"menu_active\":false,\"menu_title\":\"\",\"menu_selected\":0,\"menu_items\":[]");
            out.push_str(",\"confirm_active\":false,\"confirm_prompt\":\"\"");
            out.push_str(",\"display_panes\":false");
        }
    }
    out
}

/// Serialize the active window's floating panes into a JSON fragment
/// (`,"floats":[{x,y,w,h,border,focused,title,rows}]`) for the client to draw
/// as positioned overlays. Returns an empty string when there are no floats.
/// Only the active window's floats are emitted, so non-active windows' floats
/// are correctly hidden.
pub fn serialize_floats_json(app: &AppState) -> String {
    use crate::server::helpers::json_escape_string;
    let win = match app.windows.get(app.active_idx) {
        Some(w) => w,
        None => return String::new(),
    };
    if win.floating.is_empty() {
        return String::new();
    }
    let mut out = String::from(",\"floats\":[");
    for (i, fp) in win.floating.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let inner_h = fp.h.saturating_sub(2);
        let inner_w = fp.w.saturating_sub(2);
        let focused = win.floating_focus == Some(i);
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "{{\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"border\":\"{}\",\"focused\":{},\"title\":\"",
                fp.x, fp.y, fp.w, fp.h, json_escape_string(&fp.border), focused
            ),
        );
        out.push_str(&json_escape_string(&fp.title));
        // new-pane -s/-S/-R: the float's own styles, sent only when set.
        for (key, value) in [
            ("style", &fp.styles.style),
            ("astyle", &fp.styles.active_border),
            ("bstyle", &fp.styles.border),
        ] {
            if !value.is_empty() {
                out.push_str("\",\"");
                out.push_str(key);
                out.push_str("\":\"");
                out.push_str(&json_escape_string(value));
            }
        }
        out.push_str("\",\"rows\":[");
        // The float's own cursor, so the client can put the terminal cursor
        // inside the focused float the way tmux does for its active pane
        // (#767). Without it the client only knew the tiled active pane's
        // cursor and left the cursor there while typing went into the float.
        let mut cur_row: u16 = 0;
        let mut cur_col: u16 = 0;
        let mut cur_hidden = false;
        if let Ok(parser) = fp.pane.term.lock() {
            let screen = parser.screen();
            let (cr, cc) = screen.cursor_position();
            cur_row = cr;
            cur_col = cc;
            cur_hidden = screen.hide_cursor();
            let rows_data = serialize_screen_rows(screen, inner_h, inner_w);
            for (j, row) in rows_data.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                out.push_str("{\"runs\":[");
                for (k, run) in row.runs.iter().enumerate() {
                    if k > 0 {
                        out.push(',');
                    }
                    out.push_str("{\"text\":\"");
                    json_esc_inline(&run.text, &mut out);
                    out.push_str("\",\"fg\":\"");
                    out.push_str(&run.fg);
                    out.push_str("\",\"bg\":\"");
                    out.push_str(&run.bg);
                    let _ = std::fmt::Write::write_fmt(
                        &mut out,
                        format_args!("\",\"flags\":{},\"width\":{}", run.flags, run.width),
                    );
                    push_run_underline(run, &mut out);
                    out.push('}');
                }
                out.push_str("]}");
            }
        }
        out.push(']');
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                ",\"cursor_row\":{},\"cursor_col\":{},\"hide_cursor\":{}}}",
                cur_row, cur_col, cur_hidden
            ),
        );
    }
    out.push(']');
    out
}

/// JSON-escape a string inline (for popup run serialization).
fn json_esc_inline(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = std::fmt::Write::write_fmt(out, format_args!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

// ── In-process popup rendering (app.rs TUI) ─────────────────────────

/// Render a popup overlay inside the TUI frame.
///
/// Used by the in-process (non-server) rendering path in `app.rs`.
/// Reads the popup pane's vt100 screen directly and renders with full
/// color/style support.
pub fn render_popup_overlay(
    f: &mut ratatui::Frame,
    area: ratatui::prelude::Rect,
    app: &AppState,
) {
    use ratatui::prelude::*;

    if let Mode::PopupMode {
        output,
        width,
        height,
        ref popup_pane,
        scroll_offset,
        look,
        ..
    } = &app.mode
    {
        // Same geometry, frame and drawing as the attached client (#792).
        let frame = popup_frame_for(app, look);
        let popup_area = popup_rect(area, *width, *height);
        let inner = popup_inner_rect(popup_area, frame.has_border());

        let content = if let Some(pane) = popup_pane {
            if let Ok(parser) = pane.term.lock() {
                let screen = parser.screen();
                let inner_h = inner.height;
                let inner_w = inner.width;
                let mut lines: Vec<Line<'static>> = Vec::new();
                for row in 0..inner_h {
                    let mut spans: Vec<Span<'static>> = Vec::new();
                    let mut current_text = String::new();
                    let mut current_style = Style::default();
                    for col in 0..inner_w {
                        if let Some(cell) = screen.cell(row, col) {
                            let mut style = Style::default();
                            // Issue #685: a popup pane owns a palette like any
                            // other pane (tmux gives one to `popup_data` too,
                            // popup.c:624), so its indexed colours resolve
                            // through it before they reach the screen.
                            style = style.fg(crate::rendering::vt_to_color(screen.resolve_colour(cell.fgcolor())));
                            style = style.bg(crate::rendering::vt_to_color(screen.resolve_colour(cell.bgcolor())));
                            if cell.dim() {
                                style = style.add_modifier(Modifier::DIM);
                            }
                            if cell.bold() {
                                style = style.add_modifier(Modifier::BOLD);
                            }
                            if cell.italic() {
                                style = style.add_modifier(Modifier::ITALIC);
                            }
                            style = crate::rendering::with_underline(
                                style,
                                cell.underline_style().sgr_subparam(),
                                match screen.resolve_colour(cell.underline_color()) {
                                    vt100::Color::Default => None,
                                    c => Some(crate::rendering::vt_to_color(c)),
                                },
                            );
                            if cell.inverse() {
                                style = style.add_modifier(Modifier::REVERSED);
                            }
                            if cell.blink() {
                                style = style.add_modifier(Modifier::SLOW_BLINK);
                            }
                            if cell.strikethrough() {
                                style = style.add_modifier(Modifier::CROSSED_OUT);
                            }
                            // ratatui-crossterm 0.1.0 omits SGR 8, so
                            // Modifier::HIDDEN won't reach the terminal.
                            // Render hidden cells as spaces instead.
                            let ch = if cell.hidden() {
                                " ".to_string()
                            } else {
                                cell.contents().to_string()
                            };
                            if style != current_style {
                                if !current_text.is_empty() {
                                    spans.push(Span::styled(
                                        std::mem::take(&mut current_text),
                                        current_style,
                                    ));
                                }
                                current_style = style;
                            }
                            if ch.is_empty() {
                                current_text.push(' ');
                            } else {
                                current_text.push_str(&ch);
                            }
                        } else {
                            current_text.push(' ');
                        }
                    }
                    if !current_text.is_empty() {
                        spans.push(Span::styled(current_text, current_style));
                    }
                    lines.push(Line::from(spans));
                }
                Text::from(lines)
            } else {
                Text::from(output.clone())
            }
        } else {
            Text::from(output.clone())
        };

        draw_popup(f.buffer_mut(), popup_area, &frame, content, *scroll_offset);
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue792_popup_style.rs"]
mod test_issue792_popup_style;
