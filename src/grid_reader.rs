//! Copy mode word motions over the pane's whole grid, ported from tmux's
//! grid-reader.c (tag 3.7c) and `format_grid_word` (format.c).
//!
//! tmux sorts every cell into one of three kinds: whitespace (`WHITESPACE`,
//! a space or a tab), a separator (a character in `word-separators`), or
//! anything else, which includes the second cell of a wide character
//! (`grid_in_set` answers no for padding). A word is a run of one kind other
//! than whitespace, so `foo_bar` is one word, `foo-bar` is three with the
//! default separators, and `日本語abc` is one.
//!
//! Lines are addressed the way tmux's grid is: absolute line 0 is the oldest
//! line of the scrollback and `history + rows - 1` is the bottom of the
//! screen.

use std::collections::HashMap;

/// tmux's default `word-separators` (options-table.c): the printable ASCII
/// characters that are neither letters nor digits, minus the underscore.
pub const DEFAULT_WORD_SEPARATORS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^`{|}~";

/// One cell, as far as word motions care.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    /// The second cell of a wide character.
    Padding,
    /// The text the cell draws. A cell never written holds a space, as
    /// tmux's default cell does.
    Text(String),
}

/// The cells of one line and whether it continues on the next one
/// (`GRID_LINE_WRAPPED`).
#[derive(Clone, Debug)]
pub struct Line {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

/// Where a grid reader gets its lines from.
pub trait Grid {
    /// Lines in the scrollback plus rows on the screen (tmux `hsize + sy`).
    fn total_lines(&self) -> usize;
    /// Columns in a line (tmux `sx`).
    fn width(&self) -> usize;
    /// Absolute line `y`, which is below `total_lines`.
    fn line(&mut self, y: usize) -> &Line;
}

/// A vt100 screen read as a grid. The screen only shows the rows framed by
/// its scroll offset, so a line is read by moving the offset to it; the
/// offset is put back when the grid is dropped.
pub struct ScreenGrid<'a> {
    screen: &'a mut vt100::Screen,
    history: usize,
    rows: usize,
    cols: usize,
    scrollback: usize,
    lines: HashMap<usize, Line>,
}

impl<'a> ScreenGrid<'a> {
    pub fn new(screen: &'a mut vt100::Screen) -> Self {
        let (rows, cols) = screen.size();
        // The history of the grid in use: `scrollback_filled` counts the
        // main grid's even while the alternate grid is up, and the lines
        // above the alternate screen would then all read as its top row.
        let history = screen.active_scrollback_filled();
        let scrollback = screen.scrollback();
        ScreenGrid {
            screen,
            history,
            rows: rows as usize,
            cols: cols as usize,
            scrollback,
            lines: HashMap::new(),
        }
    }

    /// The absolute line shown on screen row `row` at the scroll offset the
    /// grid was made with.
    pub fn abs_of_row(&self, row: u16) -> usize {
        self.top() + row as usize
    }

    /// The absolute line at the top of the screen at that offset.
    pub fn top(&self) -> usize {
        self.history - self.scrollback.min(self.history)
    }

    pub fn history(&self) -> usize {
        self.history
    }

    pub fn rows(&self) -> usize {
        self.rows
    }
}

impl Drop for ScreenGrid<'_> {
    fn drop(&mut self) {
        self.screen.set_scrollback(self.scrollback);
    }
}

impl Grid for ScreenGrid<'_> {
    fn total_lines(&self) -> usize {
        self.history + self.rows
    }

    fn width(&self) -> usize {
        self.cols
    }

    fn line(&mut self, y: usize) -> &Line {
        if !self.lines.contains_key(&y) {
            let (offset, row) = if y < self.history {
                (self.history - y, 0)
            } else {
                (0, (y - self.history) as u16)
            };
            self.screen.set_scrollback(offset);
            let cells = (0..self.cols as u16)
                .map(|col| match self.screen.cell(row, col) {
                    Some(cell) if cell.is_wide_continuation() => Cell::Padding,
                    Some(cell) if cell.has_contents() => Cell::Text(cell.contents().to_string()),
                    _ => Cell::Text(" ".to_string()),
                })
                .collect();
            let wrapped = self.screen.row_wrapped(row);
            self.lines.insert(y, Line { cells, wrapped });
        }
        &self.lines[&y]
    }
}

/// A cursor moving over a grid, tmux's `struct grid_reader`.
pub struct GridReader<'g, G: Grid> {
    pub grid: &'g mut G,
    pub cx: usize,
    pub cy: usize,
}

impl<'g, G: Grid> GridReader<'g, G> {
    pub fn new(grid: &'g mut G, cx: usize, cy: usize) -> Self {
        GridReader { grid, cx, cy }
    }

    fn last_line(&self) -> usize {
        self.grid.total_lines().saturating_sub(1)
    }

    fn cell(&mut self, x: usize, y: usize) -> Cell {
        if y >= self.grid.total_lines() {
            return Cell::Text(" ".to_string());
        }
        self.grid.line(y).cells.get(x).cloned().unwrap_or_else(|| Cell::Text(" ".to_string()))
    }

    fn wrapped(&mut self, y: usize) -> bool {
        y < self.grid.total_lines() && self.grid.line(y).wrapped
    }

    fn padding(&mut self, x: usize, y: usize) -> bool {
        self.cell(x, y) == Cell::Padding
    }

    /// tmux `grid_line_length`: the line without its trailing spaces.
    pub fn line_length(&mut self, y: usize) -> usize {
        let mut px = self.grid.width();
        while px > 0 {
            match self.cell(px - 1, y) {
                Cell::Text(t) if t == " " => px -= 1,
                _ => break,
            }
        }
        px
    }

    /// tmux `grid_in_set(gd, x, y, WHITESPACE)`: the width of whitespace at
    /// the cell, or 0. Tabs are not kept in the grid, so a tab is a space.
    pub fn whitespace_at(&mut self, x: usize, y: usize) -> usize {
        match self.cell(x, y) {
            Cell::Text(t) if t == " " || t == "\t" => 1,
            _ => 0,
        }
    }

    /// tmux `grid_in_set(gd, x, y, set)` for a set other than whitespace:
    /// whether the cell's character is one of `set`. Padding never is.
    pub fn in_set_at(&mut self, x: usize, y: usize, set: &str) -> bool {
        match self.cell(x, y) {
            Cell::Padding => false,
            Cell::Text(t) => {
                let mut chars = t.chars();
                matches!((chars.next(), chars.next()), (Some(c), None) if set.contains(c))
            }
        }
    }

    fn in_whitespace(&mut self) -> usize {
        self.whitespace_at(self.cx, self.cy)
    }

    fn in_set(&mut self, set: &str) -> bool {
        self.in_set_at(self.cx, self.cy, set)
    }

    /// tmux `grid_reader_cursor_right`.
    pub fn cursor_right(&mut self, wrap: bool, all: bool, onemore: bool) {
        let px = if all {
            self.grid.width()
        } else if onemore {
            self.line_length(self.cy)
        } else {
            self.line_length(self.cy).saturating_sub(1)
        };
        if wrap && self.cx >= px && self.cy < self.last_line() {
            self.cursor_start_of_line(false);
            self.cursor_down();
        } else if self.cx < px {
            self.cx += 1;
            while self.cx < px && self.padding(self.cx, self.cy) {
                self.cx += 1;
            }
        }
    }

    /// tmux `grid_reader_cursor_left`.
    pub fn cursor_left(&mut self, wrap: bool) {
        while self.cx > 0 && self.padding(self.cx, self.cy) {
            self.cx -= 1;
        }
        if self.cx == 0 && self.cy > 0 && (wrap || self.wrapped(self.cy - 1)) {
            self.cursor_up();
            self.cursor_end_of_line(false, false);
        } else if self.cx > 0 {
            self.cx -= 1;
        }
    }

    /// tmux `grid_reader_cursor_down`.
    pub fn cursor_down(&mut self) {
        if self.cy < self.last_line() {
            self.cy += 1;
        }
        while self.cx > 0 && self.padding(self.cx, self.cy) {
            self.cx -= 1;
        }
    }

    /// tmux `grid_reader_cursor_up`.
    pub fn cursor_up(&mut self) {
        if self.cy > 0 {
            self.cy -= 1;
        }
        while self.cx > 0 && self.padding(self.cx, self.cy) {
            self.cx -= 1;
        }
    }

    /// tmux `grid_reader_cursor_start_of_line`.
    pub fn cursor_start_of_line(&mut self, wrap: bool) {
        if wrap {
            while self.cy > 0 && self.wrapped(self.cy - 1) {
                self.cy -= 1;
            }
        }
        self.cx = 0;
    }

    /// tmux `grid_reader_cursor_end_of_line`.
    pub fn cursor_end_of_line(&mut self, wrap: bool, all: bool) {
        if wrap {
            let yy = self.last_line();
            while self.cy < yy && self.wrapped(self.cy) {
                self.cy += 1;
            }
        }
        self.cx = if all { self.grid.width() } else { self.line_length(self.cy) };
    }

    /// The last column of line `cy` a word may run to: the whole width when
    /// the line wraps, so a wrapped word is not broken up.
    fn word_limit(&mut self) -> usize {
        if self.wrapped(self.cy) {
            self.grid.width().saturating_sub(1)
        } else {
            self.line_length(self.cy)
        }
    }

    /// tmux `grid_reader_handle_wrap`: move on to the next line while the
    /// cursor is past `xx`. False when that would go past line `yy`.
    fn handle_wrap(&mut self, xx: &mut usize, yy: usize) -> bool {
        while self.cx > *xx {
            if self.cy == yy {
                return false;
            }
            self.cursor_start_of_line(false);
            self.cursor_down();
            *xx = self.word_limit();
        }
        true
    }

    /// tmux `grid_reader_cursor_next_word`. An empty `separators` moves by
    /// whitespace alone (`next-space`).
    pub fn cursor_next_word(&mut self, separators: &str) {
        let mut xx = self.word_limit();
        let yy = self.last_line();
        if !self.handle_wrap(&mut xx, yy) {
            return;
        }
        if self.in_whitespace() == 0 {
            if self.in_set(separators) {
                loop {
                    self.cx += 1;
                    if !(self.handle_wrap(&mut xx, yy) && self.in_set(separators) && self.in_whitespace() == 0) {
                        break;
                    }
                }
            } else {
                loop {
                    self.cx += 1;
                    if !(self.handle_wrap(&mut xx, yy) && !(self.in_set(separators) || self.in_whitespace() != 0)) {
                        break;
                    }
                }
            }
        }
        while self.handle_wrap(&mut xx, yy) {
            let width = self.in_whitespace();
            if width == 0 {
                break;
            }
            self.cx += width;
        }
    }

    /// tmux `grid_reader_cursor_next_word_end`: the cell after the end of the
    /// next word.
    pub fn cursor_next_word_end(&mut self, separators: &str) {
        let mut xx = self.word_limit();
        let yy = self.last_line();
        while self.handle_wrap(&mut xx, yy) {
            if self.in_whitespace() != 0 {
                self.cx += 1;
            } else if self.in_set(separators) {
                loop {
                    self.cx += 1;
                    if !(self.handle_wrap(&mut xx, yy) && self.in_set(separators) && self.in_whitespace() == 0) {
                        break;
                    }
                }
                return;
            } else {
                loop {
                    self.cx += 1;
                    if !(self.handle_wrap(&mut xx, yy) && !(self.in_whitespace() != 0 || self.in_set(separators))) {
                        break;
                    }
                }
                return;
            }
        }
    }

    /// tmux `grid_reader_cursor_previous_word`.
    pub fn cursor_previous_word(&mut self, separators: &str, already: bool, stop_at_eol: bool) {
        let word_is_letters;
        if already || self.in_whitespace() != 0 {
            loop {
                if self.cx > 0 {
                    self.cx -= 1;
                    if self.in_whitespace() == 0 {
                        word_is_letters = !self.in_set(separators);
                        break;
                    }
                } else {
                    if self.cy == 0 {
                        return;
                    }
                    self.cursor_up();
                    self.cursor_end_of_line(false, false);
                    // Stop if separator at EOL.
                    if stop_at_eol && self.cx > 0 {
                        let oldx = self.cx;
                        self.cx -= 1;
                        let at_eol = self.in_whitespace() != 0;
                        self.cx = oldx;
                        if at_eol {
                            word_is_letters = false;
                            break;
                        }
                    }
                }
            }
        } else {
            word_is_letters = !self.in_set(separators);
        }

        // Move back to the beginning of this word.
        let (mut oldx, mut oldy);
        loop {
            oldx = self.cx;
            oldy = self.cy;
            if self.cx == 0 {
                if self.cy == 0 || !self.wrapped(self.cy - 1) {
                    break;
                }
                self.cursor_up();
                self.cursor_end_of_line(false, true);
            }
            if self.cx > 0 {
                self.cx -= 1;
            }
            if !(self.in_whitespace() == 0 && word_is_letters != self.in_set(separators)) {
                break;
            }
        }
        self.cx = oldx;
        self.cy = oldy;
    }
}

impl<'g, G: Grid> GridReader<'g, G> {
    /// `next-word-end` (tmux `window_copy_cursor_next_word_end`). With vi
    /// keys it stops on the last character of the word, so it first steps
    /// off a word it is already in and steps back at the end; with emacs keys
    /// it stops on the cell after the word.
    pub fn cursor_next_word_end_keys(&mut self, separators: &str, vi: bool) {
        if vi {
            if self.in_whitespace() == 0 {
                self.cursor_right(false, false, false);
            }
            self.cursor_next_word_end(separators);
            self.cursor_left(true);
        } else {
            self.cursor_next_word_end(separators);
        }
    }

    /// From the first cell of the word `select-word` takes, move to where the
    /// selection ends (tmux `window_copy_cmd_select_word`): a word of one
    /// character followed by whitespace ends where it starts, anything else
    /// at `next-word-end`, which with emacs keys is the cell after the word,
    /// left out of an emacs selection.
    pub fn select_word_end(&mut self, separators: &str, vi: bool) {
        let (px, py) = (self.cx, self.cy);
        let (mut nx, mut ny) = (px + 1, py);
        if self.wrapped(ny) && nx > self.grid.width().saturating_sub(1) {
            nx = 0;
            ny += 1;
        }
        if px >= self.line_length(py) || self.whitespace_at(nx, ny) == 0 {
            self.cursor_next_word_end_keys(separators, vi);
        }
    }

    /// The kind of the cell at (`x`, `y`) for a word made of one row: 0 for
    /// whitespace, 1 for a separator, 2 for anything else.
    pub fn kind_at(&mut self, x: usize, y: usize, separators: &str) -> u8 {
        if self.whitespace_at(x, y) != 0 {
            0
        } else if self.in_set_at(x, y, separators) {
            1
        } else {
            2
        }
    }
}

/// Whether the cell is a word separator to `format_grid_word`: one of
/// `separators`, or a space (tmux `format_is_word_separator`).
fn is_format_separator<G: Grid>(gr: &mut GridReader<'_, G>, x: usize, y: usize, separators: &str) -> bool {
    gr.whitespace_at(x, y) != 0 || gr.in_set_at(x, y, separators)
}

/// tmux `format_grid_word`: the word at (`x`, `y`), for `#{copy_cursor_word}`
/// and `#{mouse_word}`. It looks left for the separator before the word and
/// then takes the cells after it up to the next separator, following wrapped
/// lines both ways. On a separator that is the word after it.
pub fn format_grid_word<G: Grid>(grid: &mut G, x: usize, y: usize, separators: &str) -> String {
    let mut gr = GridReader::new(grid, x, y);
    let (mut x, mut y) = (x, y);
    let mut found = false;
    loop {
        if !gr.padding(x, y) && is_format_separator(&mut gr, x, y, separators) {
            found = true;
            break;
        }
        if x == 0 {
            if y == 0 || !gr.wrapped(y - 1) {
                break;
            }
            y -= 1;
            x = gr.line_length(y);
            if x == 0 {
                break;
            }
        }
        x -= 1;
    }
    let last = gr.last_line();
    let mut word = String::new();
    loop {
        if found {
            let end = gr.line_length(y);
            if end == 0 || x == end - 1 {
                if y == last || !gr.wrapped(y) {
                    break;
                }
                y += 1;
                x = 0;
            } else {
                x += 1;
            }
        }
        found = true;
        match gr.cell(x, y) {
            Cell::Padding => continue,
            Cell::Text(t) => {
                if is_format_separator(&mut gr, x, y, separators) {
                    break;
                }
                word.push_str(&t);
            }
        }
    }
    word
}

#[cfg(test)]
#[path = "../tests-rs/test_issue862_copy_word_motion_reader.rs"]
mod test_issue862_copy_word_motion_reader;
