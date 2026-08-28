const BASE64: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
const CLIPBOARD_SELECTOR: &[u8] = b"cpqs01234567";

pub struct WrappedScreen<CB: crate::callbacks::Callbacks = ()> {
    pub screen: crate::screen::Screen,
    pub callbacks: CB,
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
