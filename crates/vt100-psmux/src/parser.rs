/// A parser for terminal output which produces an in-memory representation of
/// the terminal contents.
pub struct Parser<CB: crate::callbacks::Callbacks = ()> {
    parser: vte::Parser,
    screen: crate::perform::WrappedScreen<CB>,
    passthrough: crate::passthrough::Passthrough,
    /// The last call's bytes ended inside a DCS introducer that has not
    /// reached its final byte (`ESC`, `ESC P`, `ESC P 1;2`), which may still
    /// turn out to be `ESC P tmux;`.
    dcs_may_continue: bool,
    /// The last call's bytes ended in ESC; see [`Parser::process`].
    conhost_prev_esc: bool,
}

impl Parser {
    /// Creates a new terminal parser of the given size and with the given
    /// amount of scrollback.
    #[must_use]
    pub fn new(rows: u16, cols: u16, scrollback_len: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            screen: crate::perform::WrappedScreen::new(
                rows,
                cols,
                scrollback_len,
            ),
            passthrough: crate::passthrough::Passthrough::default(),
            dcs_may_continue: false,
            conhost_prev_esc: false,
        }
    }
}

impl<CB: crate::callbacks::Callbacks> Parser<CB> {
    /// Creates a new terminal parser of the given size and with the given
    /// amount of scrollback. Terminal events will be reported via method
    /// calls on the provided [`Callbacks`](crate::callbacks::Callbacks)
    /// implementation.
    pub fn new_with_callbacks(
        rows: u16,
        cols: u16,
        scrollback_len: usize,
        callbacks: CB,
    ) -> Self {
        Self {
            parser: vte::Parser::new(),
            screen: crate::perform::WrappedScreen::new_with_callbacks(
                rows,
                cols,
                scrollback_len,
                callbacks,
            ),
            passthrough: crate::passthrough::Passthrough::default(),
            dcs_may_continue: false,
            conhost_prev_esc: false,
        }
    }

    /// A copy of this parser's screen: same grid, same scrollback, same
    /// visible rows and cursor, with a fresh escape-sequence parser.
    ///
    /// psmux's copy mode reads one of these instead of the live screen, which
    /// is how tmux's copy mode works (`window_copy_init` copies the pane's
    /// grid).  The pane keeps running underneath the frozen view, so nothing
    /// the application prints can shift, evict or re-anchor what the user is
    /// reading, and leaving copy mode shows everything that arrived meanwhile.
    #[must_use]
    pub fn snapshot(&self) -> Self
    where
        CB: Default,
    {
        let mut screen = self.screen.screen.clone();
        screen.capture_queries(false);
        Self {
            parser: vte::Parser::new(),
            screen: crate::perform::WrappedScreen::from_screen(screen, CB::default()),
            passthrough: crate::passthrough::Passthrough::default(),
            dcs_may_continue: false,
            conhost_prev_esc: false,
        }
    }

    /// A [`snapshot`](Self::snapshot) of the main screen that the alternate
    /// screen covers, or `None` when the alternate screen is not in use.
    /// This is what tmux's `capture-pane -a` reads (`wp->base.saved_grid`).
    #[must_use]
    pub fn saved_normal_snapshot(&self) -> Option<Self>
    where
        CB: Default,
    {
        if !self.screen().alternate_screen() {
            return None;
        }
        let mut snap = self.snapshot();
        snap.screen.screen.show_saved_normal_grid();
        Some(snap)
    }

    /// Processes the contents of the given byte string, and updates the
    /// in-memory terminal state.
    pub fn process(&mut self, bytes: &[u8]) {
        if !self.screen.conhost_c0_cells {
            self.process_vt(bytes);
            return;
        }
        // The inbox conhost of Windows 10 has no DCS parser: for an
        // application's `ESC P tmux; ESC ESC ] ... ESC \` it prints `tmux;`
        // into its buffer and forwards the inner OSC as it cached it, both
        // escapes included. Two escapes in a row arrive from no other source,
        // so at the second one the screen learns that the next text conhost
        // paints may be that body (issue #972, see `arm_conhost_dcs_body`).
        let mut rest = bytes;
        let mut prev_esc = self.conhost_prev_esc;
        loop {
            let second = if prev_esc && rest.first() == Some(&0x1b) {
                Some(0)
            } else {
                memchr::memmem::find(rest, b"\x1b\x1b").map(|i| i + 1)
            };
            let Some(i) = second else { break };
            if i > 0 {
                self.process_vt(&rest[..i]);
            }
            if !self.passthrough.active() {
                self.screen.arm_conhost_dcs_body();
            }
            rest = &rest[i..];
            prev_esc = false;
        }
        self.process_vt(rest);
        self.conhost_prev_esc = bytes.last() == Some(&0x1b);
    }

    fn process_vt(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        let mut dcs_may_continue =
            std::mem::replace(&mut self.dcs_may_continue, dcs_introducer_open(bytes));
        if self.passthrough.expired() {
            self.passthrough.end();
            self.parser.advance(&mut self.screen, b"\x1b\\");
        }
        loop {
            if self.passthrough.active() {
                match self.passthrough.skip(rest) {
                    crate::passthrough::Skip::More => return,
                    crate::passthrough::Skip::End { resume, close } => {
                        self.parser.advance(&mut self.screen, close);
                        rest = &rest[resume..];
                    }
                }
            }
            // Nothing before the first `ESC P` can begin a passthrough, so it
            // goes through vte's plain `advance`: the per byte `terminated`
            // check below cost about 4% on SGR heavy output.
            let start = if dcs_may_continue || self.screen.tmux_prefix_pending() {
                0
            } else {
                dcs_start(rest).unwrap_or(rest.len())
            };
            dcs_may_continue = false;
            if start > 0 {
                self.parser.advance(&mut self.screen, &rest[..start]);
                rest = &rest[start..];
            }
            if rest.is_empty() {
                return;
            }
            // Stops right after an `ESC P tmux;`, so its body is skipped
            // above instead of parsed (see `passthrough`).
            let n = self.parser.advance_until_terminated(&mut self.screen, rest);
            rest = &rest[n..];
            if !self.screen.take_tmux_passthrough() {
                return;
            }
            self.passthrough.begin();
        }
    }

    /// Read C0 controls the way the inbox Windows console host writes them:
    /// as cells of one column each (issue #972). New parsers take
    /// [`set_conhost_c0_cells_default`](crate::set_conhost_c0_cells_default).
    pub fn set_conhost_c0_cells(&mut self, on: bool) {
        self.screen.conhost_c0_cells = on;
    }

    /// Whether this parser reads C0 controls as conhost cells.
    #[must_use]
    pub fn conhost_c0_cells(&self) -> bool {
        self.screen.conhost_c0_cells
    }

    /// Returns a reference to a [`Screen`](crate::Screen) object containing
    /// the terminal state.
    #[must_use]
    pub fn screen(&self) -> &crate::Screen {
        &self.screen.screen
    }

    /// Returns a mutable reference to a [`Screen`](crate::Screen) object
    /// containing the terminal state.
    #[must_use]
    pub fn screen_mut(&mut self) -> &mut crate::Screen {
        &mut self.screen.screen
    }

    /// Returns a reference to the [`Callbacks`](crate::callbacks::Callbacks)
    /// state object passed into the constructor.
    pub fn callbacks(&self) -> &CB {
        &self.screen.callbacks
    }

    /// Returns a mutable reference to the
    /// [`Callbacks`](crate::callbacks::Callbacks) state object passed into
    /// the constructor.
    pub fn callbacks_mut(&mut self) -> &mut CB {
        &mut self.screen.callbacks
    }
}

impl Default for Parser {
    /// Returns a parser with dimensions 80x24 and no scrollback.
    fn default() -> Self {
        Self::new(24, 80, 0)
    }
}

impl std::io::Write for Parser {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.process(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Where the first `ESC P` in `bytes` starts.
fn dcs_start(bytes: &[u8]) -> Option<usize> {
    memchr::memmem::find(bytes, b"\x1bP")
}

/// Whether `bytes` ends inside an escape that may still become the start of
/// a DCS (a lone ESC) or a DCS introducer without its final byte yet (`ESC P`
/// and parameter or intermediate bytes).
fn dcs_introducer_open(bytes: &[u8]) -> bool {
    let Some(e) = memchr::memrchr(0x1b, bytes) else {
        return false;
    };
    match &bytes[e + 1..] {
        [] => true,
        [b'P', tail @ ..] => tail.iter().all(|b| (0x20..=0x3f).contains(b)),
        _ => false,
    }
}
