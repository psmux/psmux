use crate::term::BufWrite as _;
use unicode_width::UnicodeWidthChar as _;

/// Parse an OSC 7 URI into a filesystem path.
/// Accepts `file://hostname/path`, `file:///path`, or a bare `/path`.
/// Percent-decodes the path component.
fn parse_osc7_uri(raw: &str) -> String {
    let stripped = if let Some(rest) = raw.strip_prefix("file://") {
        // Skip hostname: everything up to the next '/'
        if let Some(slash) = rest.find('/') {
            &rest[slash..]
        } else {
            rest
        }
    } else {
        raw
    };
    percent_decode(stripped)
}

/// Minimal percent-decoding for OSC 7 paths (e.g. `%20` → ` `).
fn percent_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                hex_val(bytes[i + 1]),
                hex_val(bytes[i + 2]),
            ) {
                out.push(char::from(hi << 4 | lo));
                i += 3;
                continue;
            }
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

const MODE_APPLICATION_KEYPAD: u8 = 0b0000_0001;
const MODE_APPLICATION_CURSOR: u8 = 0b0000_0010;
const MODE_HIDE_CURSOR: u8 = 0b0000_0100;
const MODE_ALTERNATE_SCREEN: u8 = 0b0000_1000;
const MODE_BRACKETED_PASTE: u8 = 0b0001_0000;

/// The xterm mouse handling mode currently in use.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub enum MouseProtocolMode {
    /// Mouse handling is disabled.
    #[default]
    None,

    /// Mouse button events should be reported on button press. Also known as
    /// X10 mouse mode.
    Press,

    /// Mouse button events should be reported on button press and release.
    /// Also known as VT200 mouse mode.
    PressRelease,

    // Highlight,
    /// Mouse button events should be reported on button press and release, as
    /// well as when the mouse moves between cells while a button is held
    /// down.
    ButtonMotion,

    /// Mouse button events should be reported on button press and release,
    /// and mouse motion events should be reported when the mouse moves
    /// between cells regardless of whether a button is held down or not.
    AnyMotion,
    // DecLocator,
}

/// The encoding to use for the enabled [`MouseProtocolMode`].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub enum MouseProtocolEncoding {
    /// Default single-printable-byte encoding.
    #[default]
    Default,

    /// UTF-8-based encoding.
    Utf8,

    /// SGR-like encoding.
    Sgr,
    // Urxvt,
}

/// Represents the overall terminal state.
#[derive(Clone, Debug)]
pub struct Screen {
    grid: crate::grid::Grid,
    alternate_grid: crate::grid::Grid,

    attrs: crate::attrs::Attrs,
    /// DECSC slot for the MAIN screen. Also the slot DECSET 1049 saves into on
    /// the way into the alternate screen, and restores from on the way out.
    saved_attrs: crate::attrs::Attrs,
    /// DECSC slot for the ALTERNATE screen (issue #502). Each screen owns its
    /// own saved cell, matching tmux (DECSC/DECRC use `ictx->old_cell` in
    /// input.c, entirely separate from the alternate screen's
    /// `s->saved_cell`). Sharing one slot let a full screen app that saved the
    /// cursor while its colorscheme was active poison the main screen: on exit
    /// DECSET 1049 restored the app's colors instead of the shell's, and every
    /// line printed afterwards inherited them.
    alternate_saved_attrs: crate::attrs::Attrs,

    modes: u8,
    mouse_protocol_mode: MouseProtocolMode,
    mouse_protocol_encoding: MouseProtocolEncoding,

    /// Window title set by the application via OSC 0 or OSC 2.
    osc_title: String,

    /// Path announced by the shell via OSC 7 (`\e]7;file://host/path\a`).
    /// Used as a fallback for CWD when PEB walking fails (SSH, WSL).
    osc7_path: Option<String>,

    /// Progress indicator state set via OSC 9;4 (Windows Terminal progress).
    /// Format: `Some((state, value))` where state ∈ {0=hide,1=default,2=error,
    /// 3=indeterminate,4=warning} and value ∈ 0..=100. `None` until first set;
    /// stays `Some` after that so a clear (state=0) is also forwarded.
    osc94_progress: Option<(u8, u8)>,

    /// Pending OSC 52 clipboard payload emitted by the child process inside
    /// this pane.  Format: `Some((selector_bytes, base64_payload))`.  The
    /// selector is the raw selector field from the OSC (e.g. `b"c"`,
    /// `b"p"`, etc., or empty) and `base64_payload` is the still-encoded
    /// data string.  Consumed once via [`Screen::take_clipboard`]; the
    /// psmux server drains this and stages it onto `App.clipboard_osc52`
    /// so the client re-emits OSC 52 on its own stdout to reach the host
    /// terminal (Windows Terminal, etc.).
    osc52_clipboard: Option<(Vec<u8>, Vec<u8>)>,

    /// OSC 8 hyperlink store.  URIs are interned here; a cell's `Attrs.link`
    /// is the index into this Vec plus 1 (0 = no link), mirroring tmux's
    /// per-screen hyperlink table.  Grows for the life of the screen, which is
    /// fine: real workloads use a bounded set of distinct URIs.
    hyperlinks: Vec<String>,

    /// Currently-running command as announced by the shell via shell-integration
    /// OSC sequences (issue #299). `None` when the shell is idle (at the prompt)
    /// or hasn't emitted any command-identity sequence. `Some(<cmd>)` when:
    ///
    ///   * OSC 133;C;cmdline_url=<...> (kitty fish)     — URL-decoded
    ///   * OSC 1337;SetUserVar=WEZTERM_PROG=<b64> (WezTerm) — base64-decoded
    ///   * OSC 633;E;<cmd>[;<nonce>]   (VS Code script) — command segment only
    ///
    /// Cleared on OSC 133;A / 633;A (prompt start) and OSC 133;D / 633;D
    /// (command done). Bare OSC 133;C (no recognized param) leaves the value
    /// alone so a prior SetUserVar/633;E can "latch" via the subsequent C marker.
    osc_shell_command: Option<String>,

    /// Set to `true` when the screen is cleared (CSI 2J) while
    /// `squelch_clear_pending` is active.  The layout serialiser
    /// checks this flag to know that `cls` has finished.
    pub(crate) squelch_cleared: bool,

    /// Set by the server before injecting `cd; cls`.  When true,
    /// the next CSI 2J (erase display mode 2) sets `squelch_cleared`.
    pub(crate) squelch_clear_pending: bool,

    /// Incremented each time the parser encounters a standalone BEL (0x07)
    /// that is NOT an OSC/DCS/APC string terminator.  Use `take_audible_bell()`
    /// to consume the counter.
    pub(crate) audible_bell_count: u32,

    /// Controls how alternate-screen exits interact with main-grid
    /// scrollback.  Default `true` = legacy behaviour: alt-screen
    /// content is ephemeral, vanishes on exit (matches xterm/tmux
    /// default).  When set `false`, the visible rows of the alt grid
    /// are copied into main-grid scrollback at the moment of exit,
    /// so a user running `capture-pane -S` or copy-mode page-up
    /// after a TUI session sees what was on screen when the app left.
    ///
    /// We keep the option name `alternate-screen` to match tmux, but
    /// the Windows implementation differs: ConPTY emits its own
    /// "clear + restore" sequences around 1049 toggles regardless of
    /// whether we honour the toggle, so simply dropping 1049 (the
    /// tmux approach) does not preserve content on this platform.
    /// Copy-on-exit is the equivalent end-user behaviour.
    pub(crate) allow_alternate_screen: bool,
}

impl Screen {
    pub(crate) fn new(
        size: crate::grid::Size,
        scrollback_len: usize,
    ) -> Self {
        let mut grid = crate::grid::Grid::new(size, scrollback_len);
        grid.allocate_rows();
        Self {
            grid,
            alternate_grid: crate::grid::Grid::new(size, 0),

            attrs: crate::attrs::Attrs::default(),
            saved_attrs: crate::attrs::Attrs::default(),
            alternate_saved_attrs: crate::attrs::Attrs::default(),

            modes: 0,
            mouse_protocol_mode: MouseProtocolMode::default(),
            mouse_protocol_encoding: MouseProtocolEncoding::default(),
            osc_title: String::new(),
            osc7_path: None,
            osc94_progress: None,
            osc52_clipboard: None,
            hyperlinks: Vec::new(),
            osc_shell_command: None,
            squelch_cleared: false,
            squelch_clear_pending: false,
            audible_bell_count: 0,
            allow_alternate_screen: true,
        }
    }

    /// Resizes the terminal.
    pub fn set_size(&mut self, rows: u16, cols: u16) {
        self.grid.set_size(crate::grid::Size { rows, cols });
        self.alternate_grid
            .set_size(crate::grid::Size { rows, cols });
    }

    /// Returns the current size of the terminal.
    ///
    /// The return value will be (rows, cols).
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        let size = self.grid().size();
        (size.rows, size.cols)
    }

    /// Scrolls to the given position in the scrollback.
    ///
    /// This position indicates the offset from the top of the screen, and
    /// should be `0` to put the normal screen in view.
    ///
    /// This affects the return values of methods called on the screen: for
    /// instance, `screen.cell(0, 0)` will return the top left corner of the
    /// screen after taking the scrollback offset into account.
    ///
    /// The value given will be clamped to the actual size of the scrollback.
    pub fn set_scrollback(&mut self, rows: usize) {
        self.grid_mut().set_scrollback(rows);
    }

    /// Returns the number of rows currently held in main-grid
    /// scrollback (the actual retained count, not the configured
    /// maximum).  Always reads the main grid, even while alt-screen
    /// is active — `#{history_size}` and capture-pane-S are about
    /// "what can the user scroll back to", and that lives on the
    /// main grid regardless of which grid is currently rendering.
    #[must_use]
    pub fn scrollback_filled(&self) -> usize {
        self.grid.scrollback_filled()
    }

    /// Updates the maximum scrollback buffer size for the main grid.  Rows
    /// in excess of the new limit are trimmed from the oldest end.  The
    /// alternate grid is intentionally left at zero scrollback (apps like
    /// vim use the alternate screen and do not retain history).
    pub fn set_scrollback_len(&mut self, new_len: usize) {
        self.grid_mut().set_scrollback_len(new_len);
    }

    /// Copy-mode freeze (psmux issue #494): while set, the main grid's
    /// visible region stays anchored to its current content — new rows
    /// entering scrollback bump the scrollback offset instead of shifting
    /// the view, matching tmux's frozen copy-mode screen.  Always targets
    /// the main grid (the alternate grid has no scrollback to anchor to).
    pub fn set_frozen(&mut self, frozen: bool) {
        self.grid.set_frozen(frozen);
    }

    /// Whether the copy-mode freeze anchor is currently set on the main grid.
    #[must_use]
    pub fn frozen(&self) -> bool {
        self.grid.frozen()
    }

    /// Returns the configured maximum size of the scrollback buffer.
    #[must_use]
    pub fn scrollback_len(&self) -> usize {
        self.grid().scrollback_len()
    }

    /// Whether DEC private modes 47/1049 (alternate screen) are honoured.
    #[must_use]
    pub fn allow_alternate_screen(&self) -> bool {
        self.allow_alternate_screen
    }

    /// Toggle whether alt-screen content is preserved into main-grid
    /// scrollback when the alt screen exits.  See the field comment.
    /// If we are currently inside alt mode at the moment of toggling
    /// off, also flush what's visible into scrollback right now —
    /// otherwise a user who flipped the option on while a TUI is
    /// already running would lose the current frame.
    pub fn set_allow_alternate_screen(&mut self, allowed: bool) {
        let was = self.allow_alternate_screen;
        self.allow_alternate_screen = allowed;
        if !allowed && was && self.mode(MODE_ALTERNATE_SCREEN) {
            self.copy_alt_visible_to_main_scrollback();
        }
    }

    /// Returns the current position in the scrollback.
    ///
    /// This position indicates the offset from the top of the screen, and is
    /// `0` when the normal screen is in view.
    #[must_use]
    pub fn scrollback(&self) -> usize {
        self.grid().scrollback()
    }

    /// Returns the text contents of the terminal.
    ///
    /// This will not include any formatting information, and will be in plain
    /// text format.
    #[must_use]
    pub fn contents(&self) -> String {
        let mut contents = String::new();
        self.write_contents(&mut contents);
        contents
    }

    fn write_contents(&self, contents: &mut String) {
        self.grid().write_contents(contents);
    }

    /// Returns the text contents of the terminal by row, restricted to the
    /// given subset of columns.
    ///
    /// This will not include any formatting information, and will be in plain
    /// text format.
    ///
    /// Newlines will not be included.
    pub fn rows(
        &self,
        start: u16,
        width: u16,
    ) -> impl Iterator<Item = String> + '_ {
        self.grid().visible_rows().map(move |row| {
            let mut contents = String::new();
            row.write_contents(&mut contents, start, width, false);
            contents
        })
    }

    /// Returns the text contents of the terminal logically between two cells.
    /// This will include the remainder of the starting row after `start_col`,
    /// followed by the entire contents of the rows between `start_row` and
    /// `end_row`, followed by the beginning of the `end_row` up until
    /// `end_col`. This is useful for things like determining the contents of
    /// a clipboard selection.
    #[must_use]
    pub fn contents_between(
        &self,
        start_row: u16,
        start_col: u16,
        end_row: u16,
        end_col: u16,
    ) -> String {
        match start_row.cmp(&end_row) {
            std::cmp::Ordering::Less => {
                let (_, cols) = self.size();
                let mut contents = String::new();
                for (i, row) in self
                    .grid()
                    .visible_rows()
                    .enumerate()
                    .skip(usize::from(start_row))
                    .take(usize::from(end_row) - usize::from(start_row) + 1)
                {
                    if i == usize::from(start_row) {
                        row.write_contents(
                            &mut contents,
                            start_col,
                            cols - start_col,
                            false,
                        );
                        if !row.wrapped() {
                            contents.push('\n');
                        }
                    } else if i == usize::from(end_row) {
                        row.write_contents(&mut contents, 0, end_col, false);
                    } else {
                        row.write_contents(&mut contents, 0, cols, false);
                        if !row.wrapped() {
                            contents.push('\n');
                        }
                    }
                }
                contents
            }
            std::cmp::Ordering::Equal => {
                if start_col < end_col {
                    self.rows(start_col, end_col - start_col)
                        .nth(usize::from(start_row))
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            }
            std::cmp::Ordering::Greater => String::new(),
        }
    }

    /// Return escape codes sufficient to reproduce the entire contents of the
    /// current terminal state. This is a convenience wrapper around
    /// [`contents_formatted`](Self::contents_formatted) and
    /// [`input_mode_formatted`](Self::input_mode_formatted).
    #[must_use]
    pub fn state_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_contents_formatted(&mut contents);
        self.write_input_mode_formatted(&mut contents);
        contents
    }

    /// Return escape codes sufficient to turn the terminal state of the
    /// screen `prev` into the current terminal state. This is a convenience
    /// wrapper around [`contents_diff`](Self::contents_diff) and
    /// [`input_mode_diff`](Self::input_mode_diff).
    #[must_use]
    pub fn state_diff(&self, prev: &Self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_contents_diff(&mut contents, prev);
        self.write_input_mode_diff(&mut contents, prev);
        contents
    }

    /// Returns the formatted visible contents of the terminal.
    ///
    /// Formatting information will be included inline as terminal escape
    /// codes. The result will be suitable for feeding directly to a raw
    /// terminal parser, and will result in the same visual output.
    #[must_use]
    pub fn contents_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_contents_formatted(&mut contents);
        contents
    }

    fn write_contents_formatted(&self, contents: &mut Vec<u8>) {
        crate::term::HideCursor::new(self.hide_cursor()).write_buf(contents);
        let prev_attrs = self.grid().write_contents_formatted(contents);
        self.attrs.write_escape_code_diff(contents, &prev_attrs);
    }

    /// Returns the formatted visible contents of the terminal by row,
    /// restricted to the given subset of columns.
    ///
    /// Formatting information will be included inline as terminal escape
    /// codes. The result will be suitable for feeding directly to a raw
    /// terminal parser, and will result in the same visual output.
    ///
    /// You are responsible for positioning the cursor before printing each
    /// row, and the final cursor position after displaying each row is
    /// unspecified.
    // the unwraps in this method shouldn't be reachable
    #[allow(clippy::missing_panics_doc)]
    pub fn rows_formatted(
        &self,
        start: u16,
        width: u16,
    ) -> impl Iterator<Item = Vec<u8>> + '_ {
        let mut wrapping = false;
        self.grid().visible_rows().enumerate().map(move |(i, row)| {
            // number of rows in a grid is stored in a u16 (see Size), so
            // visible_rows can never return enough rows to overflow here
            let i = i.try_into().unwrap();
            let mut contents = vec![];
            row.write_contents_formatted(
                &mut contents,
                start,
                width,
                i,
                wrapping,
                None,
                None,
            );
            if start == 0 && width == self.grid.size().cols {
                wrapping = row.wrapped();
            }
            contents
        })
    }

    /// Returns a terminal byte stream sufficient to turn the visible contents
    /// of the screen described by `prev` into the visible contents of the
    /// screen described by `self`.
    ///
    /// The result of rendering `prev.contents_formatted()` followed by
    /// `self.contents_diff(prev)` should be equivalent to the result of
    /// rendering `self.contents_formatted()`. This is primarily useful when
    /// you already have a terminal parser whose state is described by `prev`,
    /// since the diff will likely require less memory and cause less
    /// flickering than redrawing the entire screen contents.
    #[must_use]
    pub fn contents_diff(&self, prev: &Self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_contents_diff(&mut contents, prev);
        contents
    }

    fn write_contents_diff(&self, contents: &mut Vec<u8>, prev: &Self) {
        if self.hide_cursor() != prev.hide_cursor() {
            crate::term::HideCursor::new(self.hide_cursor())
                .write_buf(contents);
        }
        let prev_attrs = self.grid().write_contents_diff(
            contents,
            prev.grid(),
            prev.attrs,
        );
        self.attrs.write_escape_code_diff(contents, &prev_attrs);
    }

    /// Returns a sequence of terminal byte streams sufficient to turn the
    /// visible contents of the subset of each row from `prev` (as described
    /// by `start` and `width`) into the visible contents of the corresponding
    /// row subset in `self`.
    ///
    /// You are responsible for positioning the cursor before printing each
    /// row, and the final cursor position after displaying each row is
    /// unspecified.
    // the unwraps in this method shouldn't be reachable
    #[allow(clippy::missing_panics_doc)]
    pub fn rows_diff<'a>(
        &'a self,
        prev: &'a Self,
        start: u16,
        width: u16,
    ) -> impl Iterator<Item = Vec<u8>> + 'a {
        self.grid()
            .visible_rows()
            .zip(prev.grid().visible_rows())
            .enumerate()
            .map(move |(i, (row, prev_row))| {
                // number of rows in a grid is stored in a u16 (see Size), so
                // visible_rows can never return enough rows to overflow here
                let i = i.try_into().unwrap();
                let mut contents = vec![];
                row.write_contents_diff(
                    &mut contents,
                    prev_row,
                    start,
                    width,
                    i,
                    false,
                    false,
                    crate::grid::Pos { row: i, col: start },
                    crate::attrs::Attrs::default(),
                );
                contents
            })
    }

    /// Returns terminal escape sequences sufficient to set the current
    /// terminal's input modes.
    ///
    /// Supported modes are:
    /// * application keypad
    /// * application cursor
    /// * bracketed paste
    /// * xterm mouse support
    #[must_use]
    pub fn input_mode_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_input_mode_formatted(&mut contents);
        contents
    }

    fn write_input_mode_formatted(&self, contents: &mut Vec<u8>) {
        crate::term::ApplicationKeypad::new(
            self.mode(MODE_APPLICATION_KEYPAD),
        )
        .write_buf(contents);
        crate::term::ApplicationCursor::new(
            self.mode(MODE_APPLICATION_CURSOR),
        )
        .write_buf(contents);
        crate::term::BracketedPaste::new(self.mode(MODE_BRACKETED_PASTE))
            .write_buf(contents);
        crate::term::MouseProtocolMode::new(
            self.mouse_protocol_mode,
            MouseProtocolMode::None,
        )
        .write_buf(contents);
        crate::term::MouseProtocolEncoding::new(
            self.mouse_protocol_encoding,
            MouseProtocolEncoding::Default,
        )
        .write_buf(contents);
    }

    /// Returns terminal escape sequences sufficient to change the previous
    /// terminal's input modes to the input modes enabled in the current
    /// terminal.
    #[must_use]
    pub fn input_mode_diff(&self, prev: &Self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_input_mode_diff(&mut contents, prev);
        contents
    }

    fn write_input_mode_diff(&self, contents: &mut Vec<u8>, prev: &Self) {
        if self.mode(MODE_APPLICATION_KEYPAD)
            != prev.mode(MODE_APPLICATION_KEYPAD)
        {
            crate::term::ApplicationKeypad::new(
                self.mode(MODE_APPLICATION_KEYPAD),
            )
            .write_buf(contents);
        }
        if self.mode(MODE_APPLICATION_CURSOR)
            != prev.mode(MODE_APPLICATION_CURSOR)
        {
            crate::term::ApplicationCursor::new(
                self.mode(MODE_APPLICATION_CURSOR),
            )
            .write_buf(contents);
        }
        if self.mode(MODE_BRACKETED_PASTE) != prev.mode(MODE_BRACKETED_PASTE)
        {
            crate::term::BracketedPaste::new(self.mode(MODE_BRACKETED_PASTE))
                .write_buf(contents);
        }
        crate::term::MouseProtocolMode::new(
            self.mouse_protocol_mode,
            prev.mouse_protocol_mode,
        )
        .write_buf(contents);
        crate::term::MouseProtocolEncoding::new(
            self.mouse_protocol_encoding,
            prev.mouse_protocol_encoding,
        )
        .write_buf(contents);
    }

    /// Returns terminal escape sequences sufficient to set the current
    /// terminal's drawing attributes.
    ///
    /// Supported drawing attributes are:
    /// * fgcolor
    /// * bgcolor
    /// * bold
    /// * dim
    /// * italic
    /// * underline
    /// * inverse
    ///
    /// This is not typically necessary, since
    /// [`contents_formatted`](Self::contents_formatted) will leave
    /// the current active drawing attributes in the correct state, but this
    /// can be useful in the case of drawing additional things on top of a
    /// terminal output, since you will need to restore the terminal state
    /// without the terminal contents necessarily being the same.
    #[must_use]
    pub fn attributes_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_attributes_formatted(&mut contents);
        contents
    }

    fn write_attributes_formatted(&self, contents: &mut Vec<u8>) {
        crate::term::ClearAttrs.write_buf(contents);
        self.attrs.write_escape_code_diff(
            contents,
            &crate::attrs::Attrs::default(),
        );
    }

    /// Returns the current cursor position of the terminal.
    ///
    /// The return value will be (row, col).
    #[must_use]
    pub fn cursor_position(&self) -> (u16, u16) {
        let pos = self.grid().pos();
        (pos.row, pos.col)
    }

    /// Returns terminal escape sequences sufficient to set the current
    /// cursor state of the terminal.
    ///
    /// This is not typically necessary, since
    /// [`contents_formatted`](Self::contents_formatted) will leave
    /// the cursor in the correct state, but this can be useful in the case of
    /// drawing additional things on top of a terminal output, since you will
    /// need to restore the terminal state without the terminal contents
    /// necessarily being the same.
    ///
    /// Note that the bytes returned by this function may alter the active
    /// drawing attributes, because it may require redrawing existing cells in
    /// order to position the cursor correctly (for instance, in the case
    /// where the cursor is past the end of a row). Therefore, you should
    /// ensure to reset the active drawing attributes if necessary after
    /// processing this data, for instance by using
    /// [`attributes_formatted`](Self::attributes_formatted).
    #[must_use]
    pub fn cursor_state_formatted(&self) -> Vec<u8> {
        let mut contents = vec![];
        self.write_cursor_state_formatted(&mut contents);
        contents
    }

    fn write_cursor_state_formatted(&self, contents: &mut Vec<u8>) {
        crate::term::HideCursor::new(self.hide_cursor()).write_buf(contents);
        self.grid()
            .write_cursor_position_formatted(contents, None, None);

        // we don't just call write_attributes_formatted here, because that
        // would still be confusing - consider the case where the user sets
        // their own unrelated drawing attributes (on a different parser
        // instance) and then calls cursor_state_formatted. just documenting
        // it and letting the user handle it on their own is more
        // straightforward.
    }

    /// Returns the [`Cell`](crate::Cell) object at the given location in the
    /// terminal, if it exists.
    #[must_use]
    pub fn cell(&self, row: u16, col: u16) -> Option<&crate::Cell> {
        self.grid().visible_cell(crate::grid::Pos { row, col })
    }

    /// Returns whether the text in row `row` should wrap to the next line.
    #[must_use]
    pub fn row_wrapped(&self, row: u16) -> bool {
        self.grid()
            .visible_row(row)
            .is_some_and(crate::row::Row::wrapped)
    }

    /// Returns whether the alternate screen is currently in use.
    ///
    /// Gated on `allow_alternate_screen` (#88): when the option is off,
    /// `enter_alternate_grid`/`exit_alternate_grid` still use the alt grid
    /// internally as scratch space so `exit_alternate_grid` can flush its
    /// visible rows into the main grid's scrollback (see that function),
    /// so `MODE_ALTERNATE_SCREEN` still gets set for that bookkeeping.
    /// But from the caller's point of view `alternate-screen off` means
    /// psmux deliberately does not honour/expose DEC 1049 at all, so every
    /// consumer of this flag (`#{alternate_on}`, mouse-forwarding-to-child,
    /// zoom/dim decisions) should see "not in alt screen" the whole time,
    /// matching the option's documented contract instead of leaking the
    /// internal scratch-buffer implementation detail.
    #[must_use]
    pub fn alternate_screen(&self) -> bool {
        self.allow_alternate_screen && self.mode(MODE_ALTERNATE_SCREEN)
    }

    /// Returns whether the terminal should be in application keypad mode.
    #[must_use]
    pub fn application_keypad(&self) -> bool {
        self.mode(MODE_APPLICATION_KEYPAD)
    }

    /// Returns whether the terminal should be in application cursor mode.
    #[must_use]
    pub fn application_cursor(&self) -> bool {
        self.mode(MODE_APPLICATION_CURSOR)
    }

    /// Returns whether the terminal should be in hide cursor mode.
    #[must_use]
    pub fn hide_cursor(&self) -> bool {
        self.mode(MODE_HIDE_CURSOR)
    }

    /// Returns whether the terminal should be in bracketed paste mode.
    #[must_use]
    pub fn bracketed_paste(&self) -> bool {
        self.mode(MODE_BRACKETED_PASTE)
    }

    /// Returns the currently active [`MouseProtocolMode`].
    #[must_use]
    pub fn mouse_protocol_mode(&self) -> MouseProtocolMode {
        self.mouse_protocol_mode
    }

    /// Returns the currently active [`MouseProtocolEncoding`].
    #[must_use]
    pub fn mouse_protocol_encoding(&self) -> MouseProtocolEncoding {
        self.mouse_protocol_encoding
    }

    /// Returns the window title set via OSC 0 or OSC 2.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.osc_title
    }

    /// Store a window title set via OSC 0 or OSC 2.
    pub fn set_title(&mut self, raw: &[u8]) {
        if let Ok(s) = std::str::from_utf8(raw) {
            self.osc_title = s.to_string();
        }
    }

    /// Returns the path announced by the shell via OSC 7, if any.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.osc7_path.as_deref()
    }

    /// Store a path announced via OSC 7.
    /// The raw URI is parsed: `file://host/path` → `/path`.
    pub fn set_path(&mut self, raw: &[u8]) {
        if let Ok(s) = std::str::from_utf8(raw) {
            let path = parse_osc7_uri(s);
            if !path.is_empty() {
                self.osc7_path = Some(path);
            }
        }
    }

    /// Store a path announced via ConEmu's `OSC 9 ; 9 ; <cwd>` (issue #615).
    ///
    /// Unlike OSC 7 this payload is a plain filesystem path, not a URL, so it
    /// must NOT be percent-decoded: `%` is a legal Windows filename character
    /// and `C:\tmp\100%20` is a directory name, not an escape.  ConEmu wraps
    /// the value in double quotes and Windows Terminal does not, so both are
    /// accepted.  Shares the OSC 7 slot: both answer the same question, and
    /// the most recent announcement wins.
    pub fn set_path_literal(&mut self, raw: &[u8]) {
        if let Ok(s) = std::str::from_utf8(raw) {
            let path = s.trim().trim_matches('"');
            if !path.is_empty() {
                self.osc7_path = Some(path.to_string());
            }
        }
    }

    /// Returns the most recent OSC 9;4 progress indicator state, if any.
    /// `Some((state, value))` once an OSC 9;4 has been received, even when
    /// state==0 (hide); `None` when none has ever been received. Consumers
    /// (psmux server) forward this to the host terminal so tools like
    /// GitHub Copilot CLI keep working inside a pane.
    #[must_use]
    pub fn progress(&self) -> Option<(u8, u8)> {
        self.osc94_progress
    }

    /// Store an OSC 9;4 progress indicator. State is clamped to 0..=4 and
    /// value to 0..=100 to match the Windows Terminal contract.
    pub fn set_progress(&mut self, state: u8, value: u8) {
        let s = state.min(4);
        let v = value.min(100);
        self.osc94_progress = Some((s, v));
    }

    /// Store an OSC 52 clipboard copy request emitted by the child.
    /// `selector` is the raw selector field (e.g. `b"c"`), `data` is the
    /// base64-encoded payload exactly as received.  Later writes overwrite
    /// earlier ones until [`Screen::take_clipboard`] consumes the slot.
    pub fn set_clipboard(&mut self, selector: &[u8], data: &[u8]) {
        self.osc52_clipboard = Some((selector.to_vec(), data.to_vec()));
    }

    /// Returns and clears any pending OSC 52 clipboard payload.  Consume-once:
    /// after a successful drain this returns `None` until the next OSC 52
    /// arrives.  Used by the psmux server to forward child-emitted clipboard
    /// requests onto `App.clipboard_osc52`, which the client re-emits as an
    /// OSC 52 sequence on its own stdout so the host terminal (Windows
    /// Terminal, etc.) can perform the actual copy.
    pub fn take_clipboard(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        self.osc52_clipboard.take()
    }

    /// Begin an OSC 8 hyperlink: intern `uri` and set it as the current pen's
    /// link so cells written afterwards carry it.  `_id_params` is the OSC 8
    /// id field (e.g. `id=foo`); we key on the URI, which is sufficient for
    /// rendering.  An empty `uri` clears the current link.
    pub fn set_hyperlink(&mut self, _id_params: &[u8], uri: &[u8]) {
        if uri.is_empty() {
            self.attrs.link = 0;
            return;
        }
        let uri = String::from_utf8_lossy(uri);
        let id = if let Some(pos) =
            self.hyperlinks.iter().position(|u| u == uri.as_ref())
        {
            pos + 1
        } else {
            self.hyperlinks.push(uri.into_owned());
            self.hyperlinks.len()
        };
        self.attrs.link = id as u32;
    }

    /// Clear the current pen hyperlink (OSC 8 with an empty URI).
    pub fn clear_hyperlink(&mut self) {
        self.attrs.link = 0;
    }

    /// Resolve a hyperlink id (as stored in `Attrs.link`) to its URI.
    #[must_use]
    pub fn hyperlink_uri(&self, id: u32) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.hyperlinks.get((id - 1) as usize).map(String::as_str)
    }

    /// Peek at the pending OSC 52 clipboard payload without consuming it.
    /// Returns `None` if no copy request is currently staged.
    #[must_use]
    pub fn clipboard(&self) -> Option<(&[u8], &[u8])> {
        self.osc52_clipboard
            .as_ref()
            .map(|(s, d)| (s.as_slice(), d.as_slice()))
    }

    /// Returns the most recently captured shell-integration command identity,
    /// if any, or `None` when the shell is idle. See the
    /// [`Screen::osc_shell_command`] field doc for the OSC sources and clearing
    /// rules.
    ///
    /// This is the authoritative signal for #{pane_current_command} when
    /// shell integration is enabled; consumers fall back to a process-tree
    /// heuristic when this returns `None`.
    #[must_use]
    pub fn shell_command(&self) -> Option<&str> {
        self.osc_shell_command.as_deref()
    }

    /// Set the shell-integration command. Pass `Some(cmd)` to mark a command as
    /// starting, `None` to clear. See the [`Screen::osc_shell_command`] field
    /// doc for which OSC sequences drive each.
    pub(crate) fn set_shell_command(&mut self, cmd: Option<String>) {
        self.osc_shell_command = cmd;
    }

    /// Returns `true` if a screen clear (CSI 2J) was detected while
    /// squelch was pending, signalling that `cls`/`clear` finished.
    /// Calling this does NOT clear the flag; use [`take_squelch_cleared`]
    /// for a consume-style check.
    #[must_use]
    pub fn squelch_cleared(&self) -> bool {
        self.squelch_cleared
    }

    /// Returns `true` and resets the flag if screen clear was detected.
    pub fn take_squelch_cleared(&mut self) -> bool {
        let v = self.squelch_cleared;
        self.squelch_cleared = false;
        v
    }

    /// Returns `true` if one or more audible bells (standalone BEL, not OSC
    /// terminators) were received since the last call.  Resets the counter.
    pub fn take_audible_bell(&mut self) -> bool {
        let v = self.audible_bell_count;
        self.audible_bell_count = 0;
        v > 0
    }

    /// Arm the squelch detector: the next CSI 2J or CSI 3J will
    /// set `squelch_cleared` to `true`.
    pub fn set_squelch_clear_pending(&mut self, v: bool) {
        self.squelch_clear_pending = v;
    }

    /// Internal: fire the squelch signal if armed.
    fn check_squelch_signal(&mut self) {
        if self.squelch_clear_pending {
            self.squelch_cleared = true;
            self.squelch_clear_pending = false;
        }
    }

    /// Returns the currently active foreground color.
    #[must_use]
    pub fn fgcolor(&self) -> crate::Color {
        self.attrs.fgcolor
    }

    /// Returns the currently active background color.
    #[must_use]
    pub fn bgcolor(&self) -> crate::Color {
        self.attrs.bgcolor
    }

    /// Returns whether newly drawn text should be rendered with the bold text
    /// attribute.
    #[must_use]
    pub fn bold(&self) -> bool {
        self.attrs.bold()
    }

    /// Returns whether newly drawn text should be rendered with the dim text
    /// attribute.
    #[must_use]
    pub fn dim(&self) -> bool {
        self.attrs.dim()
    }

    /// Returns whether newly drawn text should be rendered with the italic
    /// text attribute.
    #[must_use]
    pub fn italic(&self) -> bool {
        self.attrs.italic()
    }

    /// Returns whether newly drawn text should be rendered with the
    /// underlined text attribute.
    #[must_use]
    pub fn underline(&self) -> bool {
        self.attrs.underline()
    }

    /// Returns the extended underline style of the current drawing attributes.
    #[must_use]
    pub fn underline_style(&self) -> crate::attrs::UnderlineStyle {
        self.attrs.underline_style()
    }

    /// Returns the underline colour of the current drawing attributes.
    #[must_use]
    pub fn ulcolor(&self) -> crate::Color {
        self.attrs.ulcolor()
    }

    /// Returns whether newly drawn text should be rendered with the inverse
    /// text attribute.
    #[must_use]
    pub fn inverse(&self) -> bool {
        self.attrs.inverse()
    }

    pub(crate) fn grid(&self) -> &crate::grid::Grid {
        if self.mode(MODE_ALTERNATE_SCREEN) {
            &self.alternate_grid
        } else {
            &self.grid
        }
    }

    fn grid_mut(&mut self) -> &mut crate::grid::Grid {
        if self.mode(MODE_ALTERNATE_SCREEN) {
            &mut self.alternate_grid
        } else {
            &mut self.grid
        }
    }

    fn enter_alternate_grid(&mut self) {
        self.grid_mut().set_scrollback(0);
        self.set_mode(MODE_ALTERNATE_SCREEN);
        self.alternate_grid.allocate_rows();
        // Start the alternate screen's DECSC slot clean so a value left by a
        // previous alternate-screen session cannot surface in this one.
        self.alternate_saved_attrs = crate::attrs::Attrs::default();
    }

    fn exit_alternate_grid(&mut self) {
        // Issue #88: when the user has opted in via `alternate-screen
        // off`, append the alt grid's currently-visible rows to the
        // main grid's scrollback BEFORE flipping the mode.  Done in
        // this order so the rows are read off the alt grid (which is
        // still selected by `grid()` while MODE_ALTERNATE_SCREEN is
        // set) and pushed into the main grid's buffer.  A no-op when
        // the option is left at the default `on`.
        if !self.allow_alternate_screen {
            self.copy_alt_visible_to_main_scrollback();
        }
        self.clear_mode(MODE_ALTERNATE_SCREEN);
    }

    /// Append every non-blank visible row of the alt grid to the
    /// main grid's scrollback.  Trailing blank rows are skipped so a
    /// TUI that didn't fill the screen does not leave a tail of empty
    /// lines in scrollback.  Cheap: O(rows × cols) per exit, with the
    /// usual scrollback eviction rules applied by main grid's append.
    fn copy_alt_visible_to_main_scrollback(&mut self) {
        // Snapshot the alt grid's visible rows; we will hand them to
        // the main grid afterwards.
        let alt_rows: Vec<crate::row::Row> = self
            .alternate_grid
            .drawing_rows()
            .cloned()
            .collect();

        // Trim trailing blank rows — they're just empty lines beneath
        // the TUI's last drawn row and would clutter scrollback.
        let last_nonblank = alt_rows
            .iter()
            .rposition(|r| !r.is_blank())
            .map(|i| i + 1)
            .unwrap_or(0);

        for row in alt_rows.into_iter().take(last_nonblank) {
            self.grid.push_row_to_scrollback(row);
        }
    }

    fn save_cursor(&mut self) {
        self.grid_mut().save_cursor();
        // The cursor POSITION already lives in the per-grid slot that
        // `grid_mut()` selects; route the attributes to the matching slot so
        // the two screens cannot overwrite each other (issue #502).
        if self.mode(MODE_ALTERNATE_SCREEN) {
            self.alternate_saved_attrs = self.attrs;
        } else {
            self.saved_attrs = self.attrs;
        }
    }

    fn restore_cursor(&mut self) {
        self.grid_mut().restore_cursor();
        self.attrs = if self.mode(MODE_ALTERNATE_SCREEN) {
            self.alternate_saved_attrs
        } else {
            self.saved_attrs
        };
    }

    fn set_mode(&mut self, mode: u8) {
        self.modes |= mode;
    }

    fn clear_mode(&mut self, mode: u8) {
        self.modes &= !mode;
    }

    fn mode(&self, mode: u8) -> bool {
        self.modes & mode != 0
    }

    fn set_mouse_mode(&mut self, mode: MouseProtocolMode) {
        self.mouse_protocol_mode = mode;
    }

    fn clear_mouse_mode(&mut self, mode: MouseProtocolMode) {
        if self.mouse_protocol_mode == mode {
            self.mouse_protocol_mode = MouseProtocolMode::default();
        }
    }

    fn set_mouse_encoding(&mut self, encoding: MouseProtocolEncoding) {
        self.mouse_protocol_encoding = encoding;
    }

    fn clear_mouse_encoding(&mut self, encoding: MouseProtocolEncoding) {
        if self.mouse_protocol_encoding == encoding {
            self.mouse_protocol_encoding = MouseProtocolEncoding::default();
        }
    }
}

/// U+FE0F VARIATION SELECTOR-16, which requests emoji presentation.
const VS16: char = '\u{FE0F}';

impl Screen {
    /// Does appending the zero-width char `c` turn this cell into a
    /// double-width sequence? (#533)
    ///
    /// Emoji presentation is a property of the *sequence*, not of any single
    /// character: `U+2733` is one column on its own, but `U+2733 U+FE0F` is
    /// two, in real terminals and in tmux alike. Because `text()` measures
    /// width one char at a time, the base settles the cell at one column and
    /// the selector is folded in afterwards as a zero-width mark, so the cell
    /// stays narrow and every column after it drifts left by one.
    ///
    /// The trigger mirrors tmux's `screen_write_combine`, which forces the
    /// stored width to 2 when a VS16 lands on a cell whose width is still 1
    /// (`variation-selector-always-wide`, on by default). The width measured
    /// over the whole cell is checked too, so any other sequence that
    /// `unicode-width` considers double width is promoted as well.
    fn wants_wide_promotion(cell: &crate::Cell, c: char) -> bool {
        use unicode_width::UnicodeWidthStr as _;
        // A cell that is already wide must not be promoted again: tmux only
        // promotes when the stored width is 1, so `📛 + VS16` stays 2 columns
        // rather than growing to 4.
        cell.has_contents()
            && !cell.is_wide()
            && (c == VS16 || cell.contents().width() > 1)
    }

    /// Widen the narrow cell at (`row`, `col`) into a two column cell, taking
    /// the following cell as its continuation and advancing the cursor over
    /// it. The cursor is expected to be sitting on that following cell, which
    /// is the case for every caller (a zero-width char never moves it).
    fn promote_cell_to_wide(
        &mut self,
        row: u16,
        col: u16,
        attrs: crate::attrs::Attrs,
    ) {
        let cont = crate::grid::Pos { row, col: col + 1 };
        if cont.col >= self.grid().size().cols {
            // The base sits in the last column, so there is nowhere to put the
            // continuation. Leave the cell narrow rather than wrapping a
            // half-drawn glyph onto the next row.
            return;
        }

        // If the cell we are taking over is itself the base of a wide glyph,
        // that glyph's own continuation is about to be orphaned, so clear it.
        let clobbers_wide = self
            .grid()
            .drawing_cell(cont)
            .is_some_and(crate::Cell::is_wide);
        if clobbers_wide {
            if let Some(orphan) = self.grid_mut().drawing_cell_mut(
                crate::grid::Pos {
                    row,
                    col: cont.col + 1,
                },
            ) {
                orphan.clear(attrs);
                orphan.set_wide_continuation(false);
            }
        }

        if let Some(base) = self
            .grid_mut()
            .drawing_cell_mut(crate::grid::Pos { row, col })
        {
            base.set_wide(true);
        } else {
            return;
        }
        if let Some(cell) = self.grid_mut().drawing_cell_mut(cont) {
            cell.clear(crate::attrs::Attrs::default());
            cell.set_wide_continuation(true);
        }
        self.grid_mut().col_inc(1);
    }

    pub(crate) fn text(&mut self, c: char) {
        let pos = self.grid().pos();
        let size = self.grid().size();
        let attrs = self.attrs;

        let width = c.width();
        if width.is_none() && (u32::from(c)) < 256 {
            // don't even try to draw control characters
            return;
        }
        let width: u16 = width
            .unwrap_or(1)
            .try_into()
            // width() can only return 0, 1, or 2
            .unwrap();

        // A glyph wider than the whole row can never be represented: there is
        // nowhere to put its continuation, and `size.cols - width` underflows
        // just below (#534, reachable once a pane is shrunk to one column).
        // tmux drops the glyph in this situation, showing nothing for a CJK
        // character in a one column pane, so do the same.
        if width > size.cols {
            return;
        }

        // it doesn't make any sense to wrap if the last column in a row
        // didn't already have contents. don't try to handle the case where a
        // character wraps because there was only one column left in the
        // previous row - literally everything handles this case differently,
        // and this is tmux behavior (and also the simplest). i'm open to
        // reconsidering this behavior, but only with a really good reason
        // (xterm handles this by introducing the concept of triple width
        // cells, which i really don't want to do).
        let mut wrap = false;
        if pos.col > size.cols - width {
            let last_cell = self
                .grid()
                .drawing_cell(crate::grid::Pos {
                    row: pos.row,
                    col: size.cols - 1,
                })
                // pos.row is valid, since it comes directly from
                // self.grid().pos() which we assume to always have a valid
                // row value. size.cols - 1 is also always a valid column.
                .unwrap();
            if last_cell.has_contents() || last_cell.is_wide_continuation() {
                wrap = true;
            }
        }
        self.grid_mut().col_wrap(width, wrap);
        let pos = self.grid().pos();

        if width == 0 {
            if pos.col > 0 {
                let mut base_col = pos.col - 1;
                let mut prev_cell = self
                    .grid_mut()
                    .drawing_cell_mut(crate::grid::Pos {
                        row: pos.row,
                        col: base_col,
                    })
                    // pos.row is valid, since it comes directly from
                    // self.grid().pos() which we assume to always have a
                    // valid row value. pos.col - 1 is valid because we just
                    // checked for pos.col > 0.
                    .unwrap();
                if prev_cell.is_wide_continuation() {
                    base_col = pos.col - 2;
                    prev_cell = self
                        .grid_mut()
                        .drawing_cell_mut(crate::grid::Pos {
                            row: pos.row,
                            col: base_col,
                        })
                        // pos.row is valid, since it comes directly from
                        // self.grid().pos() which we assume to always have a
                        // valid row value. we know pos.col - 2 is valid
                        // because the cell at pos.col - 1 is a wide
                        // continuation character, which means there must be
                        // the first half of the wide character before it.
                        .unwrap();
                }
                prev_cell.append(c);
                if Self::wants_wide_promotion(prev_cell, c) {
                    self.promote_cell_to_wide(pos.row, base_col, attrs);
                }
            } else if pos.row > 0 {
                let prev_row = self
                    .grid()
                    .drawing_row(pos.row - 1)
                    // pos.row is valid, since it comes directly from
                    // self.grid().pos() which we assume to always have a
                    // valid row value. pos.row - 1 is valid because we just
                    // checked for pos.row > 0.
                    .unwrap();
                if prev_row.wrapped() {
                    let mut prev_cell = self
                        .grid_mut()
                        .drawing_cell_mut(crate::grid::Pos {
                            row: pos.row - 1,
                            col: size.cols - 1,
                        })
                        // pos.row is valid, since it comes directly from
                        // self.grid().pos() which we assume to always have a
                        // valid row value. pos.row - 1 is valid because we
                        // just checked for pos.row > 0. col of size.cols - 1
                        // is always valid.
                        .unwrap();
                    if prev_cell.is_wide_continuation() {
                        prev_cell = self
                            .grid_mut()
                            .drawing_cell_mut(crate::grid::Pos {
                                row: pos.row - 1,
                                col: size.cols - 2,
                            })
                            // pos.row is valid, since it comes directly from
                            // self.grid().pos() which we assume to always
                            // have a valid row value. pos.row - 1 is valid
                            // because we just checked for pos.row > 0. col of
                            // size.cols - 2 is valid because the cell at
                            // size.cols - 1 is a wide continuation character,
                            // so it must have the first half of the wide
                            // character before it.
                            .unwrap();
                    }
                    prev_cell.append(c);
                }
            }
        } else {
            // After a resize, cells may be in inconsistent states (e.g.
            // a wide char at the last column without its continuation).
            // Use safe accessors to avoid panics on out-of-bounds.
            if let Some(cell_ref) = self.grid().drawing_cell(pos) {
                if cell_ref.is_wide_continuation() {
                    if let Some(prev_cell) = self
                        .grid_mut()
                        .drawing_cell_mut(crate::grid::Pos {
                            row: pos.row,
                            col: pos.col - 1,
                        })
                    {
                        prev_cell.clear(attrs);
                    }
                }
            }

            let is_wide_at_pos = self
                .grid()
                .drawing_cell(pos)
                .map_or(false, |c| c.is_wide());
            if is_wide_at_pos {
                if let Some(next_cell) = self
                    .grid_mut()
                    .drawing_cell_mut(crate::grid::Pos {
                        row: pos.row,
                        col: pos.col + 1,
                    })
                {
                    next_cell.set(' ', attrs);
                }
            }

            if let Some(cell) = self
                .grid_mut()
                .drawing_cell_mut(pos)
            {
                cell.set(c, attrs);
            } else {
                return;
            }
            self.grid_mut().col_inc(1);
            if width > 1 {
                let pos = self.grid().pos();
                let is_wide_here = self
                    .grid()
                    .drawing_cell(pos)
                    .map_or(false, |c| c.is_wide());
                if is_wide_here {
                    let next_next_pos = crate::grid::Pos {
                        row: pos.row,
                        col: pos.col + 1,
                    };
                    if let Some(next_next_cell) = self
                        .grid_mut()
                        .drawing_cell_mut(next_next_pos)
                    {
                        next_next_cell.clear(attrs);
                        if next_next_pos.col == size.cols - 1 {
                            if let Some(row) = self.grid_mut()
                                .drawing_row_mut(pos.row)
                            {
                                row.wrap(false);
                            }
                        }
                    }
                }
                if let Some(next_cell) = self
                    .grid_mut()
                    .drawing_cell_mut(pos)
                {
                    next_cell.clear(crate::attrs::Attrs::default());
                    next_cell.set_wide_continuation(true);
                }
                self.grid_mut().col_inc(1);
            }
        }
    }

    // control codes

    pub(crate) fn bs(&mut self) {
        self.grid_mut().col_dec(1);
    }

    pub(crate) fn tab(&mut self) {
        self.grid_mut().col_tab();
    }

    pub(crate) fn lf(&mut self) {
        self.grid_mut().row_inc_scroll(1);
    }

    pub(crate) fn vt(&mut self) {
        self.lf();
    }

    pub(crate) fn ff(&mut self) {
        self.lf();
    }

    pub(crate) fn cr(&mut self) {
        self.grid_mut().col_set(0);
    }

    // escape codes

    // ESC 7
    pub(crate) fn decsc(&mut self) {
        self.save_cursor();
    }

    // ESC 8
    pub(crate) fn decrc(&mut self) {
        self.restore_cursor();
    }

    // ESC =
    pub(crate) fn deckpam(&mut self) {
        self.set_mode(MODE_APPLICATION_KEYPAD);
    }

    // ESC >
    pub(crate) fn deckpnm(&mut self) {
        self.clear_mode(MODE_APPLICATION_KEYPAD);
    }

    // ESC M
    pub(crate) fn ri(&mut self) {
        self.grid_mut().row_dec_scroll(1);
    }

    // ESC c
    pub(crate) fn ris(&mut self) {
        *self = Self::new(self.grid.size(), self.grid.scrollback_len());
    }

    // csi codes

    // CSI @
    pub(crate) fn ich(&mut self, count: u16) {
        self.grid_mut().insert_cells(count);
    }

    // CSI A
    pub(crate) fn cuu(&mut self, offset: u16) {
        self.grid_mut().row_dec_clamp(offset);
    }

    // CSI B
    pub(crate) fn cud(&mut self, offset: u16) {
        self.grid_mut().row_inc_clamp(offset);
    }

    // CSI C
    pub(crate) fn cuf(&mut self, offset: u16) {
        self.grid_mut().col_inc_clamp(offset);
    }

    // CSI D
    pub(crate) fn cub(&mut self, offset: u16) {
        self.grid_mut().col_dec(offset);
    }

    // CSI E
    pub(crate) fn cnl(&mut self, offset: u16) {
        self.grid_mut().col_set(0);
        self.grid_mut().row_inc_clamp(offset);
    }

    // CSI F
    pub(crate) fn cpl(&mut self, offset: u16) {
        self.grid_mut().col_set(0);
        self.grid_mut().row_dec_clamp(offset);
    }

    // CSI G
    pub(crate) fn cha(&mut self, col: u16) {
        self.grid_mut().col_set(col - 1);
    }

    // CSI H
    pub(crate) fn cup(&mut self, (row, col): (u16, u16)) {
        self.grid_mut().set_pos(crate::grid::Pos {
            row: row - 1,
            col: col - 1,
        });
    }

    // CSI J
    pub(crate) fn ed(
        &mut self,
        mode: u16,
        mut unhandled: impl FnMut(&mut Self),
    ) {
        let attrs = self.attrs;
        match mode {
            0 => self.grid_mut().erase_all_forward(attrs),
            1 => self.grid_mut().erase_all_backward(attrs),
            2 => {
                self.grid_mut().erase_all(attrs);
                self.check_squelch_signal();
            }
            3 => {
                self.grid_mut().clear_scrollback();
                self.check_squelch_signal();
            }
            _ => unhandled(self),
        }
    }

    // CSI ? J
    pub(crate) fn decsed(
        &mut self,
        mode: u16,
        unhandled: impl FnMut(&mut Self),
    ) {
        self.ed(mode, unhandled);
    }

    // CSI K
    pub(crate) fn el(
        &mut self,
        mode: u16,
        mut unhandled: impl FnMut(&mut Self),
    ) {
        let attrs = self.attrs;
        match mode {
            0 => self.grid_mut().erase_row_forward(attrs),
            1 => self.grid_mut().erase_row_backward(attrs),
            2 => self.grid_mut().erase_row(attrs),
            _ => unhandled(self),
        }
    }

    // CSI ? K
    pub(crate) fn decsel(
        &mut self,
        mode: u16,
        unhandled: impl FnMut(&mut Self),
    ) {
        self.el(mode, unhandled);
    }

    // CSI L
    pub(crate) fn il(&mut self, count: u16) {
        self.grid_mut().insert_lines(count);
    }

    // CSI M
    pub(crate) fn dl(&mut self, count: u16) {
        self.grid_mut().delete_lines(count);
    }

    // CSI P
    pub(crate) fn dch(&mut self, count: u16) {
        self.grid_mut().delete_cells(count);
    }

    // CSI S
    pub(crate) fn su(&mut self, count: u16) {
        self.grid_mut().scroll_up(count);
    }

    // CSI T
    pub(crate) fn sd(&mut self, count: u16) {
        self.grid_mut().scroll_down(count);
    }

    // CSI X
    pub(crate) fn ech(&mut self, count: u16) {
        let attrs = self.attrs;
        self.grid_mut().erase_cells(count, attrs);
    }

    // CSI d
    pub(crate) fn vpa(&mut self, row: u16) {
        self.grid_mut().row_set(row - 1);
    }

    // CSI ? h
    pub(crate) fn decset(
        &mut self,
        params: &vte::Params,
        mut unhandled: impl FnMut(&mut Self),
    ) {
        for param in params {
            match param {
                [1] => self.set_mode(MODE_APPLICATION_CURSOR),
                [6] => self.grid_mut().set_origin_mode(true),
                [9] => self.set_mouse_mode(MouseProtocolMode::Press),
                [25] => self.clear_mode(MODE_HIDE_CURSOR),
                [47] => self.enter_alternate_grid(),
                [1000] => {
                    self.set_mouse_mode(MouseProtocolMode::PressRelease);
                }
                [1002] => {
                    self.set_mouse_mode(MouseProtocolMode::ButtonMotion);
                }
                [1003] => self.set_mouse_mode(MouseProtocolMode::AnyMotion),
                [1005] => {
                    self.set_mouse_encoding(MouseProtocolEncoding::Utf8);
                }
                [1006] => {
                    self.set_mouse_encoding(MouseProtocolEncoding::Sgr);
                }
                [1049] => {
                    self.decsc();
                    self.alternate_grid.clear();
                    self.enter_alternate_grid();
                }
                [2004] => self.set_mode(MODE_BRACKETED_PASTE),
                _ => unhandled(self),
            }
        }
    }

    // CSI ? l
    pub(crate) fn decrst(
        &mut self,
        params: &vte::Params,
        mut unhandled: impl FnMut(&mut Self),
    ) {
        for param in params {
            match param {
                [1] => self.clear_mode(MODE_APPLICATION_CURSOR),
                [6] => self.grid_mut().set_origin_mode(false),
                [9] => self.clear_mouse_mode(MouseProtocolMode::Press),
                [25] => self.set_mode(MODE_HIDE_CURSOR),
                [47] => {
                    self.exit_alternate_grid();
                }
                [1000] => {
                    self.clear_mouse_mode(MouseProtocolMode::PressRelease);
                }
                [1002] => {
                    self.clear_mouse_mode(MouseProtocolMode::ButtonMotion);
                }
                [1003] => {
                    self.clear_mouse_mode(MouseProtocolMode::AnyMotion);
                }
                [1005] => {
                    self.clear_mouse_encoding(MouseProtocolEncoding::Utf8);
                }
                [1006] => {
                    self.clear_mouse_encoding(MouseProtocolEncoding::Sgr);
                }
                [1049] => {
                    self.exit_alternate_grid();
                    self.decrc();
                }
                [2004] => self.clear_mode(MODE_BRACKETED_PASTE),
                _ => unhandled(self),
            }
        }
    }

    // CSI m
    pub(crate) fn sgr(
        &mut self,
        params: &vte::Params,
        mut unhandled: impl FnMut(&mut Self),
    ) {
        // XXX really i want to just be able to pass in a default Params
        // instance with a 0 in it, but vte doesn't allow creating new Params
        // instances
        if params.is_empty() {
            self.attrs = crate::attrs::Attrs::default();
            return;
        }

        let mut iter = params.iter();

        macro_rules! next_param {
            () => {
                match iter.next() {
                    Some(n) => n,
                    _ => return,
                }
            };
        }

        macro_rules! to_u8 {
            ($n:expr) => {
                if let Some(n) = u16_to_u8($n) {
                    n
                } else {
                    return;
                }
            };
        }

        macro_rules! next_param_u8 {
            () => {
                if let &[n] = next_param!() {
                    to_u8!(n)
                } else {
                    return;
                }
            };
        }

        loop {
            match next_param!() {
                [0] => self.attrs = crate::attrs::Attrs::default(),
                [1] => self.attrs.set_bold(),
                [2] => self.attrs.set_dim(),
                [3] => self.attrs.set_italic(true),
                [4] => self.attrs.set_underline(true),
                // SGR 4 with a subparameter: `4:0` .. `4:5` select the
                // extended underline styles (tmux input.c
                // input_csi_dispatch_sgr_colon, cases 0..5).  Windows ConPTY
                // forwards these verbatim, so dropping them here is what made
                // undercurl invisible inside a pane.
                [4, n] => self
                    .attrs
                    .set_underline_style(
                        crate::attrs::UnderlineStyle::from_sgr_subparam(*n),
                    ),
                // Legacy double underline.  ConPTY rewrites `4:2` to `21`, so
                // this arm is the one that actually fires for double
                // underlines coming out of a pane on Windows (tmux input.c
                // case 21).
                [21] => self
                    .attrs
                    .set_underline_style(crate::attrs::UnderlineStyle::Double),
                [5] | [6] => self.attrs.set_blink(true),
                [7] => self.attrs.set_inverse(true),
                [8] => self.attrs.set_hidden(true),
                [9] => self.attrs.set_strikethrough(true),
                [22] => self.attrs.set_normal_intensity(),
                [23] => self.attrs.set_italic(false),
                [24] => self.attrs.set_underline(false),
                [25] => self.attrs.set_blink(false),
                [27] => self.attrs.set_inverse(false),
                [28] => self.attrs.set_hidden(false),
                [29] => self.attrs.set_strikethrough(false),
                [n] if (30..=37).contains(n) => {
                    self.attrs.fgcolor = crate::Color::Idx(to_u8!(*n) - 30);
                }
                [38, 2, r, g, b] => {
                    self.attrs.fgcolor =
                        crate::Color::Rgb(to_u8!(*r), to_u8!(*g), to_u8!(*b));
                }
                [38, 2, _cs, r, g, b] => {
                    self.attrs.fgcolor =
                        crate::Color::Rgb(to_u8!(*r), to_u8!(*g), to_u8!(*b));
                }
                [38, 5, i] => {
                    self.attrs.fgcolor = crate::Color::Idx(to_u8!(*i));
                }
                [38] => match next_param!() {
                    [2] => {
                        let r = next_param_u8!();
                        let g = next_param_u8!();
                        let b = next_param_u8!();
                        self.attrs.fgcolor = crate::Color::Rgb(r, g, b);
                    }
                    [5] => {
                        self.attrs.fgcolor =
                            crate::Color::Idx(next_param_u8!());
                    }
                    _ => {
                        unhandled(self);
                        return;
                    }
                },
                [39] => {
                    self.attrs.fgcolor = crate::Color::Default;
                }
                [n] if (40..=47).contains(n) => {
                    self.attrs.bgcolor = crate::Color::Idx(to_u8!(*n) - 40);
                }
                [48, 2, r, g, b] => {
                    self.attrs.bgcolor =
                        crate::Color::Rgb(to_u8!(*r), to_u8!(*g), to_u8!(*b));
                }
                [48, 2, _cs, r, g, b] => {
                    self.attrs.bgcolor =
                        crate::Color::Rgb(to_u8!(*r), to_u8!(*g), to_u8!(*b));
                }
                [48, 5, i] => {
                    self.attrs.bgcolor = crate::Color::Idx(to_u8!(*i));
                }
                [48] => match next_param!() {
                    [2] => {
                        let r = next_param_u8!();
                        let g = next_param_u8!();
                        let b = next_param_u8!();
                        self.attrs.bgcolor = crate::Color::Rgb(r, g, b);
                    }
                    [5] => {
                        self.attrs.bgcolor =
                            crate::Color::Idx(next_param_u8!());
                    }
                    _ => {
                        unhandled(self);
                        return;
                    }
                },
                [49] => {
                    self.attrs.bgcolor = crate::Color::Default;
                }
                // SGR 58: underline colour.  Both the semicolon form
                // (`58;2;r;g;b`) and the colon forms (`58:2::r:g:b`,
                // `58:5:n`) are accepted, exactly as tmux does in
                // input_csi_dispatch_sgr_colon / _sgr (p[0] == 58).
                [58, 2, r, g, b] => {
                    self.attrs.set_ulcolor(crate::Color::Rgb(
                        to_u8!(*r),
                        to_u8!(*g),
                        to_u8!(*b),
                    ));
                }
                // `58:2::r:g:b` carries an empty colour-space id, which vte
                // reports as a leading zero subparameter.
                [58, 2, _cs, r, g, b] => {
                    self.attrs.set_ulcolor(crate::Color::Rgb(
                        to_u8!(*r),
                        to_u8!(*g),
                        to_u8!(*b),
                    ));
                }
                [58, 5, i] => {
                    self.attrs.set_ulcolor(crate::Color::Idx(to_u8!(*i)));
                }
                [58] => match next_param!() {
                    [2] => {
                        let r = next_param_u8!();
                        let g = next_param_u8!();
                        let b = next_param_u8!();
                        self.attrs
                            .set_ulcolor(crate::Color::Rgb(r, g, b));
                    }
                    [5] => {
                        self.attrs
                            .set_ulcolor(crate::Color::Idx(next_param_u8!()));
                    }
                    _ => {
                        unhandled(self);
                        return;
                    }
                },
                [59] => {
                    self.attrs.set_ulcolor(crate::Color::Default);
                }
                [n] if (90..=97).contains(n) => {
                    self.attrs.fgcolor = crate::Color::Idx(to_u8!(*n) - 82);
                }
                [n] if (100..=107).contains(n) => {
                    self.attrs.bgcolor = crate::Color::Idx(to_u8!(*n) - 92);
                }
                _ => unhandled(self),
            }
        }
    }

    // CSI r
    pub(crate) fn decstbm(&mut self, (top, bottom): (u16, u16)) {
        self.grid_mut().set_scroll_region(top - 1, bottom - 1);
    }
}

fn u16_to_u8(i: u16) -> Option<u8> {
    if i > u16::from(u8::MAX) {
        None
    } else {
        // safe because we just ensured that the value fits in a u8
        Some(i.try_into().unwrap())
    }
}

#[cfg(test)]
#[path = "../../../tests-rs/test_vt100_screen.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests-rs/test_issue155_sgr_attrs.rs"]
mod test_issue155_sgr_attrs;

#[cfg(test)]
#[path = "../../../tests-rs/test_issue361_osc8_hyperlink.rs"]
mod test_issue361_osc8_hyperlink;

