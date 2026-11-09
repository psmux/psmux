/// A parser for terminal output which produces an in-memory representation of
/// the terminal contents.
pub struct Parser<CB: crate::callbacks::Callbacks = ()> {
    parser: vte::Parser,
    screen: crate::perform::WrappedScreen<CB>,
    passthrough: crate::passthrough::Passthrough,
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
        let mut rest = bytes;
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
