use crate::term::BufWrite as _;

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
    /// Where the cursor was on the main screen when the alternate screen was
    /// entered, `None` while it is not in use (tmux's `saved_cx`/`saved_cy`,
    /// set by screen_alternate_on and reset by screen_alternate_off).
    alternate_saved_cursor: Option<(u16, u16)>,

    modes: u8,
    mouse_protocol_mode: MouseProtocolMode,
    mouse_protocol_encoding: MouseProtocolEncoding,

    /// Which of DECSET 1005 and 1006 the application currently has ON, kept
    /// apart from `mouse_protocol_encoding` because they are two independent
    /// switches and the encoding is only one answer.  tmux stores them as two
    /// bits of the pane's screen mode (`MODE_MOUSE_UTF8` 0x100 and
    /// `MODE_MOUSE_SGR` 0x200, tmux.h:683) and reports each on its own through
    /// `#{mouse_utf8_flag}` / `#{mouse_sgr_flag}` (format.c:2015, 1991), so an
    /// application that turned both on has both flags set.  Issue #662.
    mouse_encoding_flags: u8,

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
    program_status: crate::ProgramStatuses,
    program_status_enabled: bool,
    capture_queries: bool,
    pending_queries: Vec<crate::TerminalQuery>,

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

    /// Per pane colour palette set by OSC 4 and emptied by OSC 104 / RIS
    /// (issue #685).  `None` until the first entry is set, so a pane that
    /// never touches its palette costs one pointer and renders byte for byte
    /// as it did before.  tmux keeps the same thing on the pane
    /// (`tmux.h:1379`) and allocates it lazily too (`colour.c:1280`).
    palette: Option<Box<crate::palette::ColourPalette>>,

    /// Bumped on every change to `palette`.  The pane reader thread compares
    /// it to the value it last published so it can mirror the low sixteen
    /// entries for the OSC 4 query responder without locking on every batch.
    palette_generation: u64,

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

    /// True once the output stream asked its terminal for win32 input mode
    /// (`CSI ?9001h`).  On a psmux pane that request comes from the pane's
    /// console host itself, as the very first bytes it writes (`ESC[?9001h
    /// ESC[?1004h` on build 26200), and it is the host's way of saying it can
    /// parse `ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _` key records written into its
    /// input.  A host that predates win32 input mode (the Windows 10 19041
    /// inbox conhost, OpenConsole 1.0) never sends it.  Issue #845.
    ///
    /// Sticky: `CSI ?9001l` does not clear it, because a program in the pane
    /// that turns the mode off again on exit says nothing about the host, and
    /// RIS keeps it because a reset of the screen is not a new host.
    win32_input_requested: bool,
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
            alternate_saved_cursor: None,

            modes: 0,
            mouse_protocol_mode: MouseProtocolMode::default(),
            mouse_protocol_encoding: MouseProtocolEncoding::default(),
            mouse_encoding_flags: 0,
            osc_title: String::new(),
            osc7_path: None,
            osc94_progress: None,
            program_status: crate::ProgramStatuses::default(),
            program_status_enabled: false,
            capture_queries: false,
            pending_queries: Vec::new(),
            osc52_clipboard: None,
            hyperlinks: Vec::new(),
            osc_shell_command: None,
            palette: None,
            palette_generation: 0,
            squelch_cleared: false,
            squelch_clear_pending: false,
            audible_bell_count: 0,
            allow_alternate_screen: true,
            win32_input_requested: false,
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

    /// Returns the number of rows held in the scrollback of the grid that
    /// is in use, the one `cell` and `set_scrollback` work on: none while
    /// the alternate grid is up, since it keeps no scrollback.  A reader
    /// that walks lines by moving the scrollback offset needs this count,
    /// not `scrollback_filled`, or it reads rows the offset cannot reach.
    #[must_use]
    pub fn active_scrollback_filled(&self) -> usize {
        self.grid().scrollback_filled()
    }

    /// Returns roughly how many bytes the main grid's scrollback holds: the
    /// cells the history rows actually store, plus per-row bookkeeping.  Since
    /// a row is compacted to its used width when it becomes history (issue
    /// #641), this tracks the text retained rather than the pane's width.
    /// Reported as tmux's `#{history_bytes}`.
    #[must_use]
    pub fn history_bytes(&self) -> usize {
        self.grid.history_bytes()
    }

    /// Reset the screen for a new process while KEEPING the main grid's
    /// history, the way tmux's `screen_reinit` (screen.c) does when
    /// `respawn-pane` / `respawn-window` reuse a pane (spawn.c, psmux issue
    /// #708).
    ///
    /// Exactly what tmux resets goes: the visible rows are cleared WITHOUT
    /// being pushed into history (`grid_clear_lines(hsize, sy)`), the cursor
    /// goes home, the scroll region, saved cursor, modes (cursor keys, keypad,
    /// mouse, bracketed paste, ...), attributes, title and progress are reset,
    /// and the alternate screen is left.  The history above the visible rows
    /// survives, still under the grid's own limit, and so does the hyperlink
    /// table those history cells point into, plus the `alternate-screen`
    /// option, which belongs to the pane rather than to the process.
    ///
    /// The OSC 4 palette survives as well: tmux keeps it on the
    /// `window_pane` and clears it only on RIS, OSC 104 and `send-keys -R`
    /// (input.c, cmd-send-keys.c), never on a respawn.
    pub fn reinit_keep_history(&mut self) {
        let size = self.grid.size();
        let scrollback_len = self.grid.scrollback_len();
        let history = self.grid.take_scrollback();
        let hyperlinks = std::mem::take(&mut self.hyperlinks);
        let palette = self.palette.take();
        let palette_generation = self.palette_generation;
        let allow_alternate_screen = self.allow_alternate_screen;
        let mut program_status = std::mem::take(&mut self.program_status);
        program_status.clear();
        let program_status_enabled = self.program_status_enabled;
        *self = Self::new(size, scrollback_len);
        self.program_status = program_status;
        self.program_status_enabled = program_status_enabled;
        self.grid.put_scrollback(history);
        self.hyperlinks = hyperlinks;
        self.palette = palette;
        self.palette_generation = palette_generation;
        self.allow_alternate_screen = allow_alternate_screen;
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

    /// Whether the output stream has asked for win32 input mode
    /// (`CSI ?9001h`) at any point.  See the field for why it is sticky.
    #[must_use]
    pub fn win32_input_requested(&self) -> bool {
        self.win32_input_requested
    }

    /// Carry [`Self::win32_input_requested`] into a fresh screen that replaces
    /// this one for the same output stream (clear-history swaps the parser
    /// while the console host behind it stays the same).
    pub fn set_win32_input_requested(&mut self, requested: bool) {
        self.win32_input_requested = requested;
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

    /// Is the application asking for mouse tracking at all?  tmux's
    /// `#{mouse_any_flag}`, `wp->base.mode & ALL_MOUSE_MODES` where
    /// `ALL_MOUSE_MODES` is `MODE_MOUSE_STANDARD|MODE_MOUSE_BUTTON|
    /// MODE_MOUSE_ALL` (format.c:1952, tmux.h:698).  The reporting ENCODING
    /// (1005/1006) is deliberately not part of it: an encoding on its own
    /// reports nothing.
    #[must_use]
    pub fn mouse_any_flag(&self) -> bool {
        self.mouse_protocol_mode != MouseProtocolMode::None
    }

    /// DECSET 1000, tmux's `#{mouse_standard_flag}` / `MODE_MOUSE_STANDARD`
    /// (format.c:2003).  X10 tracking (DECSET 9) counts here too: tmux does
    /// not implement 9 at all, this parser does, and it is the same
    /// press-only report `1000` extends.
    #[must_use]
    pub fn mouse_standard_flag(&self) -> bool {
        matches!(
            self.mouse_protocol_mode,
            MouseProtocolMode::Press | MouseProtocolMode::PressRelease
        )
    }

    /// DECSET 1002, tmux's `#{mouse_button_flag}` / `MODE_MOUSE_BUTTON`
    /// (format.c:1964): motion reported while a button is held.
    #[must_use]
    pub fn mouse_button_flag(&self) -> bool {
        self.mouse_protocol_mode == MouseProtocolMode::ButtonMotion
    }

    /// DECSET 1003, tmux's `#{mouse_all_flag}` / `MODE_MOUSE_ALL`
    /// (format.c:1940): motion reported with no button held.
    #[must_use]
    pub fn mouse_all_flag(&self) -> bool {
        self.mouse_protocol_mode == MouseProtocolMode::AnyMotion
    }

    /// DECSET 1005, tmux's `#{mouse_utf8_flag}` / `MODE_MOUSE_UTF8`
    /// (format.c:2015).
    #[must_use]
    pub fn mouse_utf8_flag(&self) -> bool {
        self.mouse_encoding_flags & Self::ENCODING_UTF8 != 0
    }

    /// DECSET 1006, tmux's `#{mouse_sgr_flag}` / `MODE_MOUSE_SGR`
    /// (format.c:1991).
    #[must_use]
    pub fn mouse_sgr_flag(&self) -> bool {
        self.mouse_encoding_flags & Self::ENCODING_SGR != 0
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

    /// Live OSC 7501 records, independent of the active terminal grid.
    pub fn program_status(&self) -> &crate::ProgramStatuses {
        &self.program_status
    }

    /// Mutable records for acknowledgement and attached-process exit cleanup.
    pub fn program_status_mut(&mut self) -> &mut crate::ProgramStatuses {
        &mut self.program_status
    }

    /// Whether OSC 7501 is accepted. Off, the default, reports are not stored
    /// and the capability query draws no reply: the sequence is handled as an
    /// unknown OSC, exactly as before OSC 7501 support. Turning it off
    /// withdraws every stored record.
    pub fn set_program_status_enabled(&mut self, enabled: bool) {
        if self.program_status_enabled == enabled { return; }
        self.program_status_enabled = enabled;
        if !enabled { self.program_status.clear(); }
    }

    /// See [`Screen::set_program_status_enabled`].
    pub fn program_status_enabled(&self) -> bool {
        self.program_status_enabled
    }

    /// Enable ordered query capture. The host must drain queries after processing.
    pub fn capture_queries(&mut self, enabled: bool) {
        self.capture_queries = enabled;
        if !enabled { self.pending_queries.clear(); }
    }

    /// Drain queries in parser dispatch order. Snapshots do not capture queries.
    pub fn take_queries(&mut self) -> Vec<crate::TerminalQuery> {
        std::mem::take(&mut self.pending_queries)
    }

    pub(crate) fn query(&mut self, query: crate::TerminalQuery) {
        if self.capture_queries { self.pending_queries.push(query); }
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

    // ---- OSC 4 / OSC 104 per pane colour palette (issue #685) ----

    /// This pane's palette, or `None` when no OSC 4 has ever been accepted.
    ///
    /// Serialisers take this once per pane per frame and hand it to
    /// [`resolve_colour`](Self::resolve_colour), so a pane without a palette
    /// pays a single null check for the whole frame.
    #[must_use]
    pub fn colour_palette(&self) -> Option<&crate::palette::ColourPalette> {
        self.palette.as_deref()
    }

    /// The RGB override for one index, if set.  tmux's `colour_palette_get`
    /// (`colour.c:1251`).
    #[must_use]
    pub fn palette_entry(&self, idx: u8) -> Option<(u8, u8, u8)> {
        self.palette.as_ref().and_then(|p| p.get(idx))
    }

    /// Set or unset one palette entry.  Returns `true` when the stored value
    /// changed, mirroring `colour_palette_set` (`colour.c:1272`), whose return
    /// is what makes tmux schedule a full redraw.
    pub fn set_palette_entry(
        &mut self,
        idx: u8,
        rgb: Option<(u8, u8, u8)>,
    ) -> bool {
        if rgb.is_none() && self.palette.is_none() {
            // Nothing allocated, nothing to unset: tmux returns 0 here too
            // rather than allocating 256 slots for a reset.
            return false;
        }
        let palette = self
            .palette
            .get_or_insert_with(|| Box::new(crate::palette::ColourPalette::default()));
        if palette.set(idx, rgb) {
            self.palette_generation = self.palette_generation.wrapping_add(1);
            true
        } else {
            false
        }
    }

    /// Drop the whole palette (`colour_palette_clear`, `colour.c:1227`).
    /// Returns `true` when something was dropped.
    pub fn clear_palette(&mut self) -> bool {
        let dropped = self.palette.take().is_some_and(|p| !p.is_empty());
        if dropped {
            self.palette_generation = self.palette_generation.wrapping_add(1);
        }
        dropped
    }

    /// A counter bumped on every accepted palette change.  The pane reader
    /// thread uses it to notice that the mirror it keeps for the OSC 4 query
    /// responder is stale, without walking 256 entries per output batch.
    #[must_use]
    pub fn palette_generation(&self) -> u64 {
        self.palette_generation
    }

    /// Replace an indexed colour with this pane's palette RGB.  This is the
    /// substitution tmux performs in `tty_check_fg` / `tty_check_bg` /
    /// `tty_check_us` (`tty.c:2822`, `2892`, `2945`) just before the cell
    /// reaches the terminal.  A pane with no palette returns `c` untouched.
    #[inline]
    #[must_use]
    pub fn resolve_colour(&self, c: crate::attrs::Color) -> crate::attrs::Color {
        match &self.palette {
            Some(p) => p.resolve(c),
            None => c,
        }
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
        if !self.mode(MODE_ALTERNATE_SCREEN) {
            let pos = self.grid.pos();
            self.alternate_saved_cursor = Some((pos.col, pos.row));
        }
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
        self.alternate_saved_cursor = None;
    }

    /// The main screen cursor `(col, row)` saved when the alternate screen
    /// was entered, `None` when the alternate screen is not in use.
    #[must_use]
    pub fn alternate_saved_cursor(&self) -> Option<(u16, u16)> {
        if self.alternate_screen() {
            self.alternate_saved_cursor
        } else {
            None
        }
    }

    /// While the alternate screen is in use, show the main screen it covers
    /// (tmux's saved grid) instead. Meant for a snapshot, never the live
    /// screen: the alternate screen stays allocated and comes back on the
    /// next write.
    pub(crate) fn show_saved_normal_grid(&mut self) {
        self.clear_mode(MODE_ALTERNATE_SCREEN);
        self.alternate_saved_cursor = None;
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

    /// DECRST for a mouse tracking mode.  tmux turns the whole family off for
    /// any of `1000`, `1001`, `1002` and `1003`
    /// (`screen_write_mode_clear(sctx, ALL_MOUSE_MODES)`, input.c:1959), and
    /// so does xterm: the three modes are one setting with three spellings,
    /// so an application that enabled `1003` and disables with a bare
    /// `ESC[?1000l` really is asking for the mouse to go quiet.  Matching only
    /// the exact mode left such a pane reporting tracking that nobody wanted,
    /// and `#{mouse_all_flag}` would have said so (#662).
    fn clear_mouse_mode(&mut self, _mode: MouseProtocolMode) {
        self.mouse_protocol_mode = MouseProtocolMode::default();
    }

    fn set_mouse_encoding(&mut self, encoding: MouseProtocolEncoding) {
        self.mouse_encoding_flags |= Self::encoding_bit(encoding);
        self.mouse_protocol_encoding = encoding;
    }

    fn clear_mouse_encoding(&mut self, encoding: MouseProtocolEncoding) {
        self.mouse_encoding_flags &= !Self::encoding_bit(encoding);
        if self.mouse_protocol_encoding == encoding {
            // Fall back to whichever of the two is still on rather than
            // straight to the default: `1005h 1006h 1006l` leaves UTF-8
            // reporting enabled, because `1005` was never withdrawn.
            self.mouse_protocol_encoding =
                if self.mouse_encoding_flags & Self::ENCODING_SGR != 0 {
                    MouseProtocolEncoding::Sgr
                } else if self.mouse_encoding_flags & Self::ENCODING_UTF8 != 0 {
                    MouseProtocolEncoding::Utf8
                } else {
                    MouseProtocolEncoding::default()
                };
        }
    }

    const ENCODING_UTF8: u8 = 0b01;
    const ENCODING_SGR: u8 = 0b10;

    fn encoding_bit(encoding: MouseProtocolEncoding) -> u8 {
        match encoding {
            MouseProtocolEncoding::Utf8 => Self::ENCODING_UTF8,
            MouseProtocolEncoding::Sgr => Self::ENCODING_SGR,
            MouseProtocolEncoding::Default => 0,
        }
    }
}

impl Screen {
    /// Fold `c` into the cell before the cursor when tmux would, and say
    /// whether `c` has been dealt with (#749). The decision is
    /// `crate::width::join`, tmux's `screen_write_combine`, which
    /// `screen_write_cell` calls before anything else; so it runs ahead of the
    /// wrap decision here too, and a character combining onto the last column
    /// never wraps.
    ///
    /// Measured one character at a time, each wide member of an emoji cluster
    /// took its own two columns, so a family of four spent eight where the
    /// child process had counted two, and everything after it drifted right.
    fn combine(&mut self, c: char, width: u16) -> bool {
        // The cell to combine with is the one the cursor follows: one column
        // back, or two when that column is the continuation of a wide cell.
        // It must really start there, so a cursor parked in the middle of a
        // wide glyph combines with nothing.
        let pos = self.grid().pos();
        let mut base = None;
        if pos.col > 0 {
            let mut base_col = pos.col - 1;
            let mut last = self.grid().drawing_cell(crate::grid::Pos {
                row: pos.row,
                col: base_col,
            });
            if pos.col != 1 && last.is_some_and(crate::Cell::is_wide_continuation) {
                base_col = pos.col - 2;
                last = self.grid().drawing_cell(crate::grid::Pos {
                    row: pos.row,
                    col: base_col,
                });
            }
            if let Some(last) = last {
                let last_width = if last.is_wide() { 2 } else { 1 };
                if pos.col - base_col == last_width && !last.is_wide_continuation() {
                    base = Some((base_col, last, usize::from(last_width)));
                }
            }
        }

        let decision = crate::width::join(
            base.map(|(_, cell, w)| (cell.contents(), w)),
            c,
            usize::from(width),
        );
        let base_col = base.map(|(col, _, _)| col);
        match (decision, base_col) {
            (crate::width::Join::Discard, _) => true,
            (crate::width::Join::NewCell, _) | (_, None) => false,
            (crate::width::Join::Combine { widen }, Some(base_col)) => {
                let appended = self
                    .grid_mut()
                    .drawing_cell_mut(crate::grid::Pos {
                        row: pos.row,
                        col: base_col,
                    })
                    .is_some_and(|cell| cell.append(c));
                if !appended {
                    // Only the cluster table refusing a new entry gets here;
                    // tmux then writes the character on its own, which for a
                    // zero width one shows nothing.
                    return width == 0;
                }
                // Force the width to 2 for modifiers and the variation
                // selector, but only from 1: a cell already two wide is left
                // alone, so `📛 VS16` stays two columns rather than four.
                if widen {
                    self.promote_cell_to_wide(pos.row, base_col, self.attrs);
                }
                true
            }
        }
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

        let width = crate::width::char_width(c);
        if width.is_none() && (u32::from(c)) < 256 {
            // don't even try to draw control characters
            return;
        }
        let width: u16 = width
            .unwrap_or(1)
            .try_into()
            // width() can only return 0, 1, or 2
            .unwrap();

        if self.combine(c, width) {
            return;
        }

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
        let mut statuses = std::mem::take(&mut self.program_status);
        statuses.clear();
        let queries = std::mem::take(&mut self.pending_queries);
        let capture = self.capture_queries;
        let program_status_enabled = self.program_status_enabled;
        let win32_input_requested = self.win32_input_requested;
        *self = Self::new(self.grid.size(), self.grid.scrollback_len());
        self.program_status = statuses;
        self.pending_queries = queries;
        self.capture_queries = capture;
        self.program_status_enabled = program_status_enabled;
        self.win32_input_requested = win32_input_requested;
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
                [9001] => self.win32_input_requested = true,
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

