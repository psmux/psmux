const BASE64: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
const CLIPBOARD_SELECTOR: &[u8] = b"cpqs01234567";
/// tmux's `input-buffer-size` default (`INPUT_BUF_DEFAULT_SIZE`, tmux.h), which
/// is also the option's minimum (options-table.c).
pub const DEFAULT_INPUT_BUFFER_SIZE: usize = 1 << 20;

/// The most a sixel DCS may hold before it is thrown away: tmux's
/// `input-buffer-size`, past which `input_input` sets `INPUT_DISCARD` and the
/// image is never drawn.  Without a limit a pane that writes `ESC P q` and no
/// ST grew the server by twice what it wrote.  One per process because tmux
/// keeps it in a static (`input_buffer_size`, input.c) that every pane's
/// parser reads; psmux stores it here so the parser can do the same.
static INPUT_BUFFER_SIZE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(DEFAULT_INPUT_BUFFER_SIZE);

/// Record the `input-buffer-size` server option for every pane parser in this
/// process, as tmux's `input_set_buffer_size` (input.c) does.
///
/// Only a sixel DCS is held to it here; tmux also holds OSC and APC strings
/// to it.
pub fn set_input_buffer_size(bytes: usize) {
    INPUT_BUFFER_SIZE.store(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// The `input-buffer-size` last given to [`set_input_buffer_size`].
#[must_use]
pub fn input_buffer_size() -> usize {
    INPUT_BUFFER_SIZE.load(std::sync::atomic::Ordering::Relaxed)
}

pub struct WrappedScreen<CB: crate::callbacks::Callbacks = ()> {
    pub screen: crate::screen::Screen,
    pub callbacks: CB,
    /// Set by `hook` when a sixel DCS (final `q`, no intermediates) begins, so
    /// `put`/`unhook` know to accumulate and materialise the image.  Any other
    /// DCS leaves this `false`, preserving the pre-existing no-op behaviour.
    dcs_active: bool,
    /// Accumulates the exact re-emit bytes of an in-progress sixel DCS:
    /// `ESC P` + params + intermediates + `q` (from `hook`), the payload (from
    /// `put`), and a trailing `ST` (appended in `unhook`).
    dcs_buf: Vec<u8>,
}

impl WrappedScreen<()> {
    pub fn new(rows: u16, cols: u16, scrollback_len: usize) -> Self {
        Self::new_with_callbacks(rows, cols, scrollback_len, ())
    }
}

impl<CB: crate::callbacks::Callbacks> WrappedScreen<CB> {
    pub fn new_with_callbacks(
        rows: u16,
        cols: u16,
        scrollback_len: usize,
        callbacks: CB,
    ) -> Self {
        Self {
            screen: crate::screen::Screen::new(
                crate::grid::Size { rows, cols },
                scrollback_len,
            ),
            callbacks,
            dcs_active: false,
            dcs_buf: Vec::new(),
        }
    }

    /// Wrap an existing screen, with no DCS in progress.  Used by
    /// `Parser::snapshot`, which clones the screen but starts a fresh parser.
    pub(crate) fn from_screen(screen: crate::screen::Screen, callbacks: CB) -> Self {
        Self {
            screen,
            callbacks,
            dcs_active: false,
            dcs_buf: Vec::new(),
        }
    }
}

impl<CB: crate::callbacks::Callbacks> vte::Perform for WrappedScreen<CB> {
    fn print(&mut self, c: char) {
        if c == '\u{fffd}' || ('\u{80}'..'\u{a0}').contains(&c) {
            self.callbacks.unhandled_char(&mut self.screen, c);
        } else {
            self.screen.text(c);
        }
    }

    fn execute(&mut self, b: u8) {
        match b {
            7 => {
                self.screen.audible_bell_count = self.screen.audible_bell_count.wrapping_add(1);
                self.callbacks.audible_bell(&mut self.screen);
            }
            8 => self.screen.bs(),
            9 => self.screen.tab(),
            10 => self.screen.lf(),
            11 => self.screen.vt(),
            12 => self.screen.ff(),
            13 => self.screen.cr(),
            // we don't implement shift in/out alternate character sets, but
            // it shouldn't count as an "error"
            14 | 15 => {}
            _ => self.callbacks.unhandled_control(&mut self.screen, b),
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, b: u8) {
        if let Some(i) = intermediates.first() {
            self.callbacks.unhandled_escape(
                &mut self.screen,
                Some(*i),
                intermediates.get(1).copied(),
                b,
            );
        } else {
            match b {
                b'7' => self.screen.decsc(),
                b'8' => self.screen.decrc(),
                b'=' => self.screen.deckpam(),
                b'>' => self.screen.deckpnm(),
                b'M' => self.screen.ri(),
                b'c' => self.screen.ris(),
                b'g' => self.callbacks.visual_bell(&mut self.screen),
                _ => {
                    self.callbacks.unhandled_escape(
                        &mut self.screen,
                        None,
                        None,
                        b,
                    );
                }
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        c: char,
    ) {
        if let Some(query) = crate::queries::csi_query(params, intermediates, _ignore, c) {
            self.screen.query(query);
        }
        let unhandled = |screen: &mut crate::screen::Screen| {
            self.callbacks.unhandled_csi(
                screen,
                intermediates.first().copied(),
                intermediates.get(1).copied(),
                &params.iter().collect::<Vec<_>>(),
                c,
            );
        };
        match intermediates.first() {
            None => match c {
                '@' => self.screen.ich(canonicalize_params_1(params, 1)),
                'A' => self.screen.cuu(canonicalize_params_1(params, 1)),
                'B' => self.screen.cud(canonicalize_params_1(params, 1)),
                'C' => self.screen.cuf(canonicalize_params_1(params, 1)),
                'D' => self.screen.cub(canonicalize_params_1(params, 1)),
                'E' => self.screen.cnl(canonicalize_params_1(params, 1)),
                'F' => self.screen.cpl(canonicalize_params_1(params, 1)),
                'G' => self.screen.cha(canonicalize_params_1(params, 1)),
                'H' | 'f' => self.screen.cup(canonicalize_params_2(params, 1, 1)),
                'J' => self
                    .screen
                    .ed(canonicalize_params_1(params, 0), unhandled),
                'K' => self
                    .screen
                    .el(canonicalize_params_1(params, 0), unhandled),
                'L' => self.screen.il(canonicalize_params_1(params, 1)),
                'M' => self.screen.dl(canonicalize_params_1(params, 1)),
                'P' => self.screen.dch(canonicalize_params_1(params, 1)),
                'S' => self.screen.su(canonicalize_params_1(params, 1)),
                'T' => self.screen.sd(canonicalize_params_1(params, 1)),
                'X' => self.screen.ech(canonicalize_params_1(params, 1)),
                'd' => self.screen.vpa(canonicalize_params_1(params, 1)),
                'm' => self.screen.sgr(params, unhandled),
                'n' => {
                    // DSR (Device Status Report) — in passthrough mode the
                    // child sends this and expects a response.  We ignore it
                    // at the parser level (the host must respond via the PTY
                    // writer if needed), but we must not call unhandled.
                }
                'r' => self.screen.decstbm(canonicalize_params_decstbm(
                    params,
                    self.screen.grid().size(),
                )),
                's' => self.screen.decsc(),
                'u' => self.screen.decrc(),
                't' => {
                    let mut params_iter = params.iter();
                    let op =
                        params_iter.next().and_then(|x| x.first().copied());
                    if op == Some(8) {
                        let (screen_rows, screen_cols) = self.screen.size();
                        let rows =
                            params_iter.next().map_or(screen_rows, |x| {
                                *x.first().unwrap_or(&screen_rows)
                            });
                        let cols =
                            params_iter.next().map_or(screen_cols, |x| {
                                *x.first().unwrap_or(&screen_cols)
                            });
                        self.callbacks.resize(&mut self.screen, (rows, cols));
                    } else {
                        self.callbacks.unhandled_csi(
                            &mut self.screen,
                            None,
                            None,
                            &params.iter().collect::<Vec<_>>(),
                            c,
                        );
                    }
                }
                _ => {
                    self.callbacks.unhandled_csi(
                        &mut self.screen,
                        None,
                        None,
                        &params.iter().collect::<Vec<_>>(),
                        c,
                    );
                }
            },
            Some(b'?') => match c {
                'J' => self
                    .screen
                    .decsed(canonicalize_params_1(params, 0), unhandled),
                'K' => self
                    .screen
                    .decsel(canonicalize_params_1(params, 0), unhandled),
                'h' => self.screen.decset(params, unhandled),
                'l' => self.screen.decrst(params, unhandled),
                _ => {
                    self.callbacks.unhandled_csi(
                        &mut self.screen,
                        Some(b'?'),
                        intermediates.get(1).copied(),
                        &params.iter().collect::<Vec<_>>(),
                        c,
                    );
                }
            },
            // XTMODKEYS (`CSI > 4 ; n m`) and its reset (`CSI > 4 n`): the
            // extended key mode tmux keeps per pane (issue #860).  The
            // callback still runs, as it did before for every `>` sequence.
            Some(b'>') if c == 'm' || c == 'n' => {
                let mut it = params.iter();
                // tmux input_get(ictx, 0, 0, 0) / (ictx, 1, 0, 0): a missing
                // parameter reads as 0.
                let resource = it.next().and_then(|x| x.first().copied()).unwrap_or(0);
                if c == 'm' {
                    let level = it.next().and_then(|x| x.first().copied()).unwrap_or(0);
                    self.screen.xtmodkeys_set(resource, level);
                } else {
                    self.screen.xtmodkeys_off(resource);
                }
                self.callbacks.unhandled_csi(
                    &mut self.screen,
                    Some(b'>'),
                    intermediates.get(1).copied(),
                    &params.iter().collect::<Vec<_>>(),
                    c,
                );
            }
            Some(i) => {
                self.callbacks.unhandled_csi(
                    &mut self.screen,
                    Some(*i),
                    intermediates.get(1).copied(),
                    &params.iter().collect::<Vec<_>>(),
                    c,
                );
            }
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bel_terminated: bool) {
        if let Some(query) = crate::queries::osc_query(params, self.screen.program_status_enabled()) {
            self.screen.query(query);
            return;
        }
        match params {
            [b"0", s] => {
                self.callbacks.set_window_icon_name(&mut self.screen, s);
                self.callbacks.set_window_title(&mut self.screen, s);
                self.screen.set_title(s);
            }
            [b"1", s] => {
                self.callbacks.set_window_icon_name(&mut self.screen, s);
            }
            [b"2", s] => {
                self.callbacks.set_window_title(&mut self.screen, s);
                self.screen.set_title(s);
            }
            // ---- OSC 4 : set one or more palette entries (issue #685) ----
            // `\e]4;<i>;<colour>[;<i>;<colour>]*\e\\`.  vte splits on ';', so
            // the pairs arrive as flat params.  tmux parses exactly the same
            // shape in `input_osc_4` (input.c:2927).  A `?` spec is a QUERY,
            // which psmux answers on the raw output scan in the pane reader
            // thread (pane.rs `scan_color_queries`, issue #473), preferring
            // this pane's own entry when it has one; nothing to do here.
            [b"4", rest @ ..] if !rest.is_empty() => {
                let mut i = 0usize;
                while i + 1 < rest.len() {
                    let Some(idx) = crate::palette::parse_palette_index(rest[i])
                    else {
                        // tmux stops the whole sequence on a bad index
                        // (input.c:2941 `bad = 1; break;`).
                        break;
                    };
                    let spec = rest[i + 1];
                    i += 2;
                    if spec == b"?" {
                        continue;
                    }
                    if let Some(rgb) = crate::palette::parse_x11_colour(spec) {
                        self.screen.set_palette_entry(idx, Some(rgb));
                    }
                    // An unparseable colour is skipped, not fatal
                    // (input.c:2959 `s = next; continue;`).
                }
            }
            // ---- OSC 104 : reset palette entries (issue #685) ----
            // Bare `\e]104\e\\` (and `\e]104;\e\\`, which vte hands us as an
            // empty trailing param) clears everything; `\e]104;<i>[;<i>]*`
            // clears those indexes.  tmux: `input_osc_104` (input.c:3446).
            [b"104"] | [b"104", b""] => {
                self.screen.clear_palette();
            }
            [b"104", rest @ ..] => {
                for raw in rest {
                    let Some(idx) = crate::palette::parse_palette_index(raw)
                    else {
                        break;
                    };
                    self.screen.set_palette_entry(idx, None);
                }
            }
            [b"7", uri] => {
                self.screen.set_path(uri);
            }
            [b"9", b"4", state, progress] => {
                // OSC 9;4 — Windows Terminal progress indicator.
                //   state: 0=hide, 1=default, 2=error, 3=indeterminate, 4=warning
                //   progress: 0..=100
                let s = std::str::from_utf8(state)
                    .ok()
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(0);
                let v = std::str::from_utf8(progress)
                    .ok()
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(0);
                self.screen.set_progress(s, v);
                self.callbacks.set_progress(&mut self.screen, s, v);
            }
            [b"9", b"9", cwd_rest @ ..] if !cwd_rest.is_empty() => {
                // OSC 9;9;<cwd> — ConEmu's "set working directory", the form
                // Windows Terminal documents for shell integration and the one
                // a WSL shell can emit to tell psmux where it really is
                // (issue #615).  A Windows path may contain ';' (rare but
                // legal), which the OSC splitter turned into extra params, so
                // rejoin them.
                let mut cwd = Vec::new();
                for (i, part) in cwd_rest.iter().enumerate() {
                    if i > 0 {
                        cwd.push(b';');
                    }
                    cwd.extend_from_slice(part);
                }
                self.screen.set_path_literal(&cwd);
            }
            [b"7501", body @ ..] if self.screen.program_status_enabled() => {
                // vte excludes OSC/ST and semicolon separators from params.
                let sequence_len = 2 + params.iter().map(|p| p.len()).sum::<usize>()
                    + params.len().saturating_sub(1) + if _bel_terminated { 1 } else { 2 };
                if sequence_len <= 4096 {
                    self.screen.program_status_mut().report(&body.join(&b';'));
                }
            }
            [b"9999", ..] => {
                self.screen.squelch_cleared = true;
            }
            [b"8", id_params, uri_rest @ ..] => {
                // OSC 8 ; params ; URI  — hyperlink. The URI may itself contain
                // ';' (which the OSC parser splits into extra params), so rejoin
                // the trailing parts. An empty URI closes the current link.
                let mut uri = Vec::new();
                for (i, part) in uri_rest.iter().enumerate() {
                    if i > 0 {
                        uri.push(b';');
                    }
                    uri.extend_from_slice(part);
                }
                self.screen.set_hyperlink(id_params, &uri);
            }
            [b"52", ty, data] => {
                match (
                    ty.iter().all(|c| CLIPBOARD_SELECTOR.contains(c)),
                    *data,
                ) {
                    (true, b"?") => {
                        self.callbacks
                            .paste_from_clipboard(&mut self.screen, ty);
                    }
                    // tmux parity: b64_pton skips ASCII whitespace anywhere
                    // in an OSC 52 payload, so producers that wrap long
                    // base64 must still get through this gate. The server
                    // side decoder applies the same tolerance.
                    (true, data)
                        if data.iter().all(|c| {
                            BASE64.contains(c) || c.is_ascii_whitespace()
                        }) =>
                    {
                        // Stage the payload on Screen so the psmux server
                        // can drain it and forward an OSC 52 to the host
                        // terminal.  Unblocks tools like Claude Code's
                        // `/copy` running inside a pane (OSC 52 was being
                        // swallowed by the default no-op callbacks).
                        self.screen.set_clipboard(ty, data);
                        self.callbacks.copy_to_clipboard(
                            &mut self.screen,
                            ty,
                            data,
                        );
                    }
                    _ => {
                        self.callbacks
                            .unhandled_osc(&mut self.screen, params);
                    }
                }
            }
            // ---- OSC 133 FinalTerm semantic prompts (issue #299) ----
            // 133;A (prompt start) — clear any pending command, shell is idle.
            // Extras (k=i, cl=line, aid=...) are optional kitty/ghostty
            // parameters; we ignore them via `..`.
            [b"133", b"A", ..] => {
                self.screen.program_status_mut().expire();
                self.screen.set_shell_command(None);
            }
            // 133;C with cmdline_url= parameter (kitty's fish integration).
            // vte splits on ';' so OSC 133;C;cmdline_url=foo arrives as
            // [b"133", b"C", b"cmdline_url=foo"].
            [b"133", b"C", param, ..] => {
                if let Some(cmd) = parse_cmdline_param(param) {
                    self.screen.set_shell_command(Some(cmd));
                }
                // else: bare C with unknown param — leave shell_command alone.
            }
            // Bare 133;C — leave whatever's there (latches prior SetUserVar/633E).
            [b"133", b"C"] => {}
            // 133;D[;<exit>] — command done. Clear.
            [b"133", b"D", ..] => {
                self.screen.set_shell_command(None);
            }
            // 133;B (end of prompt / start of input) — no-op for our purposes.
            [b"133", b"B", ..] => {}
            // ---- OSC 1337 SetUserVar (iTerm2 user vars; WezTerm precedent) ----
            // Form: 1337;SetUserVar=<NAME>=<base64-value>
            //   * vte presents this as a SINGLE param slot because there's no
            //     ';' separator between SetUserVar=... and the next field.
            //   * Only WEZTERM_PROG is recognized as a command-identity source
            //     for now; other vars (WEZTERM_HOST, WEZTERM_USER, custom)
            //     are not relevant here.
            [b"1337", payload] if payload.starts_with(b"SetUserVar=") => {
                let after = &payload[b"SetUserVar=".len()..];
                if let Some(eq) = after.iter().position(|&b| b == b'=') {
                    let name = &after[..eq];
                    let value_b64 = &after[eq + 1..];
                    if name == b"WEZTERM_PROG" {
                        if let Some(decoded) = decode_base64(value_b64) {
                            if let Ok(s) = String::from_utf8(decoded) {
                                self.screen.set_shell_command(Some(s));
                            }
                        }
                    }
                }
            }
            // ---- OSC 633 VS Code shell-integration lifecycle (issue #299) ----
            // VS Code emits 633 rather than 133; mirror the 133 lifecycle so a
            // finished command doesn't latch.
            // 633;A (prompt start) — shell idle, clear.
            [b"633", b"A", ..] => {
                self.screen.set_shell_command(None);
            }
            // 633;D[;<exit>] — command done, clear.
            [b"633", b"D", ..] => {
                self.screen.set_shell_command(None);
            }
            // 633;B (prompt end) / 633;C (pre-exec) — no-op; C keeps the E command.
            [b"633", b"B", ..] => {}
            [b"633", b"C", ..] => {}
            // ---- OSC 633;E (VS Code shellIntegration.ps1) ----
            // Form: 633;E;<escaped-command>[;<nonce>]
            // Take the command segment (everything before the next ';' or end).
            [b"633", b"E", cmd, ..] => {
                if let Ok(s) = std::str::from_utf8(cmd) {
                    // VS Code's __VSCode-Escape-Value replaces control chars,
                    // backslashes, newlines, and ';' with `\x<hex>` sequences.
                    // Full decode is non-trivial; for the common case (printable
                    // command text), the value passes through unchanged.
                    self.screen.set_shell_command(Some(s.to_string()));
                }
            }
            _ => {
                self.callbacks.unhandled_osc(&mut self.screen, params);
            }
        }
    }

    // --- DCS (Device Control String) handling ------------------------------
    //
    // vte drives the DCS state machine and, for any DCS, calls hook (once, with
    // the parsed params/intermediates/final byte), then put per payload byte,
    // then unhook at the terminator.  Historically WrappedScreen left all three
    // as trait defaults, so sixel graphics (DCS `q`) were parsed and silently
    // discarded (issue #431).  We now capture ONLY sixel and leave every other
    // DCS (e.g. DECRQSS `ESC P $ q ... ST`) a no-op exactly as before.

    fn hook(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // Sixel is DCS with final byte 'q' and NO intermediates.  DECRQSS also
        // ends in 'q' but carries the '$' (0x24) intermediate, so gating on an
        // empty intermediates slice keeps DECRQSS (and all other DCS) untouched.
        if action == 'q' && intermediates.is_empty() {
            self.dcs_buf.clear();
            // Reconstruct the exact introducer: ESC P + params + intermediates
            // + 'q'.  (intermediates is empty here but appended for fidelity.)
            self.dcs_buf.push(0x1b); // ESC
            self.dcs_buf.push(b'P');
            for (i, param) in params.iter().enumerate() {
                if i > 0 {
                    self.dcs_buf.push(b';');
                }
                // A single top-level param may carry colon-separated
                // subparams; rejoin them with ':' so the blob round-trips.
                for (j, sub) in param.iter().enumerate() {
                    if j > 0 {
                        self.dcs_buf.push(b':');
                    }
                    self.dcs_buf.extend_from_slice(sub.to_string().as_bytes());
                }
            }
            self.dcs_buf.extend_from_slice(intermediates);
            self.dcs_buf.push(b'q');
            self.dcs_active = true;
        }
        // Any other DCS: no-op (do NOT set active, do NOT route anywhere).
    }

    fn put(&mut self, byte: u8) {
        if self.dcs_active {
            if self.dcs_buf.len() >= input_buffer_size() {
                self.dcs_active = false;
                self.dcs_buf = Vec::new();
                return;
            }
            self.dcs_buf.push(byte);
        }
    }

    fn unhook(&mut self) {
        if !self.dcs_active {
            return;
        }
        // Terminate the reconstructed blob with ST (ESC \).
        self.dcs_buf.push(0x1b);
        self.dcs_buf.push(b'\\');

        let (row, top, bottom) = self.screen.sixel_text_area();
        let (_, col) = self.screen.cursor_position();
        let (layout, cell_masks) =
            sixel_cell_masks(&self.dcs_buf, bottom + 1 - row, bottom + 1 - top);
        let (px_width, px_height) = (layout.px_width, layout.px_height);
        let (cell_width, cell_height) =
            crate::image::cell_dims_from_px(px_width, px_height);

        let img = crate::image::SixelImage {
            id: 0,             // assigned by add_image
            raw: bound_background(&self.dcs_buf, &layout),
            px_width,
            px_height,
            cell_width,
            cell_height,
            anchor_line: 0,    // filled by add_image
            anchor_col: 0,     // filled by add_image
            on_alt: false,     // filled by add_image
            erased: Vec::new(),
            cell_masks,
        };
        self.screen.add_image(img);

        // The pane's real terminal is the console host, and the text it sends
        // after the image (a CR LF, a prompt, cursor moves) assumes the cursor
        // is where that host left it.  tmux moves to the line below the image
        // (screen-write.c:2947), but OpenConsole, the only host that passes
        // sixel through, follows Windows Terminal: same column, on the row
        // holding the top of the last sixel band.  Following tmux here put
        // every later line one row lower than the host had it.
        let final_row = row
            .saturating_add(layout.cursor_rows)
            .saturating_sub(layout.scroll_rows)
            .max(top)
            .min(bottom);
        self.screen
            .finish_sixel(layout.scroll_rows, final_row, col);

        self.dcs_active = false;
        // Let go of the image's bytes rather than keep the capacity.
        self.dcs_buf = Vec::new();
    }
}

/// Where a sixel lands and where it leaves the text cursor, in the units the
/// pane's console host uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SixelLayout {
    /// Image width in pixels: the widest band, or the raster width if larger.
    pub px_width: u32,
    /// Image height in pixels: down to the bottom of the last band that drew
    /// something, or the raster height if larger.
    pub px_height: u32,
    /// Rows from the image's top row down to the row the cursor ends on.
    pub cursor_rows: u16,
    /// Lines the text scrolls up first so the image fits above the bottom
    /// margin.
    pub scroll_rows: u16,
    /// Pixel aspect ratio the introducer selects, before any raster command.
    pub aspect: u32,
}

/// The blob with a raster command `"aspect;1;width;height` put right after the
/// introducer, so a terminal that fills the background stops at the image.
///
/// With background select 0 (the default) and no raster size, Windows
/// Terminal fills from the image to the bottom right of the page with colour
/// 0 (`_fillImageBackground`).  Text written afterwards erases that fill,
/// which hides it when an application prints the image once.  The client
/// draws the image again whenever the set of images changes, though, and
/// ratatui does not rewrite unchanged cells, so the fill blanked every row
/// below the image, other panes to its right and the status line.
///
/// The aspect ratio is the one the introducer already selects, so the image
/// keeps its shape, and a raster command of the image's own comes after this
/// one and still wins.
fn bound_background(buf: &[u8], layout: &SixelLayout) -> Vec<u8> {
    let Some(q) = buf.iter().position(|&b| b == b'q') else {
        return buf.to_vec();
    };
    let raster = format!(
        "\"{};1;{};{}",
        layout.aspect, layout.px_width, layout.px_height
    );
    let mut out = Vec::with_capacity(buf.len() + raster.len());
    out.extend_from_slice(&buf[..=q]);
    out.extend_from_slice(raster.as_bytes());
    out.extend_from_slice(&buf[q + 1..]);
    out
}

/// Lays out a reconstructed sixel DCS the way Windows Terminal's
/// `SixelParser` does (microsoft/terminal, src/terminal/adapter/SixelParser.cpp
/// at c829d4ca), since OpenConsole runs that parser for the pane and its
/// later output assumes the result.  `avail_rows` is the room from the cursor
/// row to the bottom margin and `margin_rows` the height of the margins.
///
/// - Cells are 10 by 20 pixels (`CellSizeForLevel`).
/// - A sixel band is 6 pixels times the pixel aspect ratio.  The ratio comes
///   from the first introducer parameter, 2:1 when it is 0 or omitted
///   (`_initRasterAttributes`), and a raster command `"Pan;Pad;...` replaces
///   it with `ceil(Pan / Pad)` (`_updateRasterAttributes`).
/// - Each graphics new line (`-`) first scrolls the text while the band does
///   not fit with a pixel to spare, then moves down one band
///   (`_executeNextLine`, `_maybeFlushImageBuffer`).  The end of the sequence
///   runs the same scroll check.
/// - The cursor ends on the row holding the top of the last band
///   (`_updateTextCursor`).
pub(crate) fn sixel_layout(buf: &[u8], avail_rows: u16, margin_rows: u16) -> SixelLayout {
    sixel_layout_painting(buf, avail_rows, margin_rows, &mut |_, _, _, _, _| {})
}

/// [`sixel_layout`], calling `paint(x, y, aspect, bits, n)` for each sixel
/// that draws: the six `bits` at pixel column `x`, `n` columns wide, the top
/// bit on pixel row `y` and each bit `aspect` rows tall.
fn sixel_layout_painting(
    buf: &[u8],
    avail_rows: u16,
    margin_rows: u16,
    paint: &mut dyn FnMut(u32, i64, i64, u8, u32),
) -> SixelLayout {
    let cell_h = i64::from(crate::image::DEFAULT_CELL_PX.1);

    // The introducer sits between ESC P and the first 'q'.
    let q = buf.iter().position(|&b| b == b'q').unwrap_or(buf.len());
    let intro = buf.get(2..q).unwrap_or(&[]);
    let macro_param = intro
        .split(|&b| b == b';')
        .next()
        .and_then(|p| std::str::from_utf8(p).ok())
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    let intro_aspect: u32 = match macro_param {
        0 | 1 | 5 | 6 => 2,
        2 => 5,
        3 | 4 => 3,
        _ => 1,
    };
    let mut aspect = i64::from(intro_aspect);
    let margin_px = i64::from(margin_rows) * cell_h;
    let max_aspect = (margin_px / 6).max(1);
    let mut sixel_h = 6 * aspect;
    let mut segment_h = sixel_h;
    let mut avail = i64::from(avail_rows) * cell_h;
    let mut y: i64 = 0;
    let mut scroll: i64 = 0;
    let mut cur_x: u32 = 0;
    let mut max_x: u32 = 0;
    let mut drawn_bottom: i64 = 0;
    let mut raster = (0u32, 0u32);

    let fit = |segment_h: i64, avail: &mut i64, scroll: &mut i64| {
        if segment_h > *avail {
            while *avail < margin_px && segment_h >= *avail {
                *scroll += 1;
                *avail += cell_h;
            }
        }
    };

    let payload = buf.get(q + 1..).unwrap_or(&[]);
    let mut i = 0usize;
    // Reads `;` separated decimal fields starting at `payload[i]`.
    let read_fields = |i: &mut usize| -> Vec<u32> {
        let mut fields = vec![0u32];
        while *i < payload.len() {
            match payload[*i] {
                b @ b'0'..=b'9' => {
                    let last = fields.last_mut().expect("never empty");
                    *last = last.saturating_mul(10).saturating_add(u32::from(b - b'0'));
                }
                b';' => fields.push(0),
                _ => break,
            }
            *i += 1;
        }
        fields
    };
    while i < payload.len() {
        match payload[i] {
            b'"' => {
                i += 1;
                let f = read_fields(&mut i);
                let (pan, pad) = (f.first().copied().unwrap_or(0), f.get(1).copied().unwrap_or(0));
                if pad > 0 {
                    aspect = (i64::from(pan.div_ceil(pad))).clamp(1, max_aspect);
                    sixel_h = 6 * aspect;
                    segment_h = segment_h.max(sixel_h);
                }
                raster = (
                    f.get(2).copied().filter(|&w| w > 0).unwrap_or(raster.0),
                    f.get(3).copied().filter(|&h| h > 0).unwrap_or(raster.1),
                );
                max_x = max_x.max(cur_x);
                cur_x = 0;
                continue;
            }
            b'#' => {
                i += 1;
                let _ = read_fields(&mut i);
                continue;
            }
            b'!' => {
                i += 1;
                let n = read_fields(&mut i).first().copied().unwrap_or(0).max(1);
                if i < payload.len() && (0x3f..=0x7e).contains(&payload[i]) {
                    paint(cur_x, y, aspect, payload[i] - 0x3f, n);
                    cur_x = cur_x.saturating_add(n);
                    drawn_bottom = drawn_bottom.max(y + sixel_h);
                    i += 1;
                }
                continue;
            }
            b'$' => {
                max_x = max_x.max(cur_x);
                cur_x = 0;
            }
            b'-' => {
                max_x = max_x.max(cur_x);
                cur_x = 0;
                fit(segment_h, &mut avail, &mut scroll);
                segment_h = sixel_h;
                if avail > 0 {
                    y += sixel_h;
                    avail -= sixel_h;
                }
            }
            c @ 0x3f..=0x7e => {
                paint(cur_x, y, aspect, c - 0x3f, 1);
                cur_x = cur_x.saturating_add(1);
                drawn_bottom = drawn_bottom.max(y + sixel_h);
            }
            0x1b => break, // ESC of the ST terminator
            _ => {}
        }
        i += 1;
    }
    fit(segment_h, &mut avail, &mut scroll);
    max_x = max_x.max(cur_x);

    let to_u32 = |v: i64| u32::try_from(v.max(0)).unwrap_or(u32::MAX);
    let to_u16 = |v: i64| u16::try_from(v.max(0)).unwrap_or(u16::MAX);
    SixelLayout {
        px_width: max_x.max(raster.0).max(1),
        px_height: to_u32(drawn_bottom).max(raster.1).max(1),
        cursor_rows: to_u16(y / cell_h),
        scroll_rows: to_u16(scroll),
        aspect: intro_aspect,
    }
}

/// The most work [`sixel_cell_masks`] does for one sixel, in pixel rows
/// visited plus bitset words written.  A raster command can make every sixel
/// thousands of pixels tall and `$` draws the same columns again, so a few
/// bytes could otherwise cost millions of writes.  Past this the sixel's
/// pixels are taken as unknown, which only keeps the images under it.
const PAINT_MAX_WORK: u64 = 1 << 24;

/// The widest and tallest sixel [`sixel_cell_masks`] traces, in pixels.
const PAINT_MAX_SIDE: u32 = 4096;

/// The pixels a sixel sets, one bitset per pixel row.
#[derive(Default)]
struct PaintedPixels {
    rows: Vec<Vec<u64>>,
    work: u64,
}

impl PaintedPixels {
    /// Sets the pixels of one sixel, as `sixel_layout_painting` reports it.
    fn paint(&mut self, x: u32, y: i64, aspect: i64, bits: u8, n: u32) {
        let x1 = x.saturating_add(n).min(PAINT_MAX_SIDE);
        if bits == 0 || x >= x1 {
            return;
        }
        let to_usize = |v: u32| usize::try_from(v).unwrap_or(usize::MAX);
        let (x0, x1) = (to_usize(x), to_usize(x1));
        let words = (x1 + 63) / 64;
        for k in 0..6 {
            if bits & (1 << k) == 0 {
                continue;
            }
            let top = y + i64::from(k) * aspect;
            for r in top.max(0)..(top + aspect).min(i64::from(PAINT_MAX_SIDE)) {
                if self.work > PAINT_MAX_WORK {
                    return;
                }
                let r = usize::try_from(r).unwrap_or(usize::MAX);
                if self.rows.len() <= r {
                    self.rows.resize_with(r + 1, Vec::new);
                }
                let row = &mut self.rows[r];
                if row.len() < words {
                    row.resize(words, 0);
                }
                for (w, word) in row.iter_mut().enumerate().take(words).skip(x0 / 64) {
                    let lo = (w * 64).max(x0) - w * 64;
                    let hi = ((w + 1) * 64).min(x1) - w * 64;
                    *word |= if hi - lo == 64 { u64::MAX } else { ((1u64 << (hi - lo)) - 1) << lo };
                }
                self.work = self
                    .work
                    .saturating_add(u64::try_from(words - x0 / 64).unwrap_or(u64::MAX))
                    .saturating_add(1);
            }
        }
    }

    /// Whether pixel `x` of pixel row `y` is set.
    fn is_set(&self, x: u32, y: u32) -> bool {
        let to_usize = |v: u32| usize::try_from(v).unwrap_or(usize::MAX);
        let (x, y) = (to_usize(x), to_usize(y));
        self.rows
            .get(y)
            .and_then(|row| row.get(x / 64))
            .is_some_and(|w| (w >> (x % 64)) & 1 == 1)
    }
}

/// The introducer's background select, its second parameter, 0 when absent.
fn background_select(buf: &[u8]) -> u32 {
    let q = buf.iter().position(|&b| b == b'q').unwrap_or(buf.len());
    buf.get(2..q)
        .unwrap_or(&[])
        .split(|&b| b == b';')
        .nth(1)
        .and_then(|p| std::str::from_utf8(p).ok())
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

/// Lays a sixel out as [`sixel_layout`] does, and also tells which pixels of
/// each of its cells it sets (see [`crate::image::SixelImage::cell_masks`]),
/// or nothing when that is not known.
///
/// Windows Terminal draws a sixel into each row's `ImageSlice` pixel by pixel
/// and leaves a pixel the sixel does not set as it was
/// (`_maybeFlushImageBuffer`, src/terminal/adapter/SixelParser.cpp), so an
/// older image under a newer one stays visible only where the newer one
/// leaves a pixel unset.  With background select 0 or 2 the background fill
/// sets every pixel of the image (`_initRasterAttributes`,
/// `_fillImageBackground`; the raster is the one [`bound_background`] puts
/// in), and with 1 only the sixels drawn do.
pub(crate) fn sixel_cell_masks(
    buf: &[u8],
    avail_rows: u16,
    margin_rows: u16,
) -> (SixelLayout, Vec<crate::image::CellMask>) {
    let transparent = background_select(buf) == 1;
    let mut pixels = PaintedPixels::default();
    let layout = if transparent {
        sixel_layout_painting(buf, avail_rows, margin_rows, &mut |x, y, aspect, bits, n| {
            pixels.paint(x, y, aspect, bits, n);
        })
    } else {
        sixel_layout(buf, avail_rows, margin_rows)
    };
    if layout.px_width > PAINT_MAX_SIDE
        || layout.px_height > PAINT_MAX_SIDE
        || (transparent && pixels.work > PAINT_MAX_WORK)
    {
        return (layout, Vec::new());
    }
    let (cell_px_w, cell_px_h) = crate::image::DEFAULT_CELL_PX;
    let (cells_w, cells_h) = crate::image::cell_dims_from_px(layout.px_width, layout.px_height);
    let mut masks = vec![[0u64; 4]; usize::from(cells_w) * usize::from(cells_h)];
    for (i, mask) in masks.iter_mut().enumerate() {
        let to_u32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
        let (r, c) = (to_u32(i / usize::from(cells_w)), to_u32(i % usize::from(cells_w)));
        let (x0, y0) = (c * cell_px_w, r * cell_px_h);
        let reach_w = layout.px_width.saturating_sub(x0).min(cell_px_w);
        let reach_h = layout.px_height.saturating_sub(y0).min(cell_px_h);
        if !transparent {
            *mask = crate::image::cell_mask_reaching(reach_w, reach_h);
            continue;
        }
        for y in 0..reach_h {
            for x in 0..reach_w {
                if pixels.is_set(x0 + x, y0 + y) {
                    crate::image::set_cell_mask_bit(mask, y * cell_px_w + x);
                }
            }
        }
    }
    (layout, masks)
}

// ---------------------------------------------------------------------------
// Helpers for issue #299 OSC parsing.
// ---------------------------------------------------------------------------

/// Parse a single OSC 133;C parameter slot. Returns the decoded command if the
/// slot is `cmdline_url=<url-escaped>` (kitty's fish integration). Other
/// parameter names (`cmdline=`, `aid=`, `redraw=`, etc.) return None.
fn parse_cmdline_param(param: &[u8]) -> Option<String> {
    if let Some(value) = param.strip_prefix(b"cmdline_url=") {
        return percent_decode(value);
    }
    None
}

/// Decode `%xx` percent-encoded sequences. Returns None if the input is not
/// valid UTF-8 after decode (drops the value rather than mangling it).
fn percent_decode(input: &[u8]) -> Option<String> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            let hi = hex_digit(input[i + 1])?;
            let lo = hex_digit(input[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode standard base64. Returns None on malformed input. Inlined to avoid
/// pulling a base64 dependency into vt100-psmux.
fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    // Strip trailing '=' padding, count it.
    let pad = input.iter().rev().take_while(|&&b| b == b'=').count();
    let body = &input[..input.len().saturating_sub(pad)];
    if body.iter().any(|&b| val(b).is_none()) {
        return None;
    }
    if body.is_empty() {
        return Some(Vec::new());
    }

    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let mut chunk = [0u8; 4];
    let mut idx = 0;
    for &b in body {
        chunk[idx] = val(b)?;
        idx += 1;
        if idx == 4 {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
            out.push((chunk[2] << 6) | chunk[3]);
            idx = 0;
        }
    }
    // Handle remaining 2 or 3 chars (1 or 2 output bytes).
    match idx {
        2 => out.push((chunk[0] << 2) | (chunk[1] >> 4)),
        3 => {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
        }
        _ => {}
    }
    Some(out)
}

fn canonicalize_params_1(params: &vte::Params, default: u16) -> u16 {
    let first = params.iter().next().map_or(0, |x| *x.first().unwrap_or(&0));
    if first == 0 {
        default
    } else {
        first
    }
}

fn canonicalize_params_2(
    params: &vte::Params,
    default1: u16,
    default2: u16,
) -> (u16, u16) {
    let mut iter = params.iter();
    let first = iter.next().map_or(0, |x| *x.first().unwrap_or(&0));
    let first = if first == 0 { default1 } else { first };

    let second = iter.next().map_or(0, |x| *x.first().unwrap_or(&0));
    let second = if second == 0 { default2 } else { second };

    (first, second)
}

fn canonicalize_params_decstbm(
    params: &vte::Params,
    size: crate::grid::Size,
) -> (u16, u16) {
    let mut iter = params.iter();
    let top = iter.next().map_or(0, |x| *x.first().unwrap_or(&0));
    let top = if top == 0 { 1 } else { top };

    let bottom = iter.next().map_or(0, |x| *x.first().unwrap_or(&0));
    let bottom = if bottom == 0 { size.rows } else { bottom };

    (top, bottom)
}

#[cfg(test)]
#[path = "../../../tests-rs/test_issue431_sixel_parse.rs"]
mod test_issue431_sixel_parse;
