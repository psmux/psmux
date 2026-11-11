use crate::term::BufWrite as _;

/// A single row of the grid.
///
/// The row's logical width is `cols`, which is **not** the same thing as the
/// number of cells it stores.  `cells` holds the columns up to and including the
/// last one that differs from `Cell::blank()`; every column from `cells.len()`
/// up to `cols` reads as that shared blank cell.  This is tmux's split between
/// `grid_line.cellsize` (what is allocated) and the grid's width: `grid_set_cell`
/// grows the allocation only as far as the column actually written (tmux
/// grid.c:662 via `grid_expand_line`, grid.c:564), and `grid_get_cell` hands back
/// `grid_default_cell` for anything past it (tmux grid.c:650).
///
/// Rows in the visible grid are created dense and stay dense, because that is
/// the mutation hot path.  Rows leaving the visible grid for the scrollback are
/// run through [`Row::compact`], which is why a deep `history-limit` on a wide
/// pane now costs bytes per retained character instead of `cols * 44` per line
/// (psmux issue #641).
#[derive(Clone, Debug)]
pub struct Row {
    cells: Vec<crate::Cell>,
    cols: u16,
    wrapped: bool,
}

impl Row {
    pub fn new(cols: u16) -> Self {
        Self {
            cells: vec![crate::Cell::new(); usize::from(cols)],
            cols,
            wrapped: false,
        }
    }

    /// A row of `cols` copies of `cell`, built in one pass, for a line
    /// scrolled in or inserted with a background (`Grid::blank_row`).
    pub fn filled(cols: u16, cell: crate::Cell) -> Self {
        Self {
            cells: vec![cell; usize::from(cols)],
            cols,
            wrapped: false,
        }
    }

    fn cols(&self) -> u16 {
        self.cols
    }

    /// Materialise storage for the columns below `len` so a mutation can
    /// address them.  tmux's `grid_expand_line` (grid.c:564).
    fn expand(&mut self, len: u16) {
        if self.cells.len() < usize::from(len) {
            self.cells.resize(usize::from(len), crate::Cell::new());
        }
    }

    /// The number of cells this row actually stores, i.e. tmux's `cellsize`.
    /// Reads for the columns between this and `cols()` are served from the
    /// shared blank cell.
    pub fn stored_cells(&self) -> usize {
        self.cells.len()
    }

    /// Drop the trailing cells that are indistinguishable from the blank cell
    /// reads already synthesise, and hand the freed bytes back to the
    /// allocator.  Called when a row is evicted from the visible grid into the
    /// scrollback, the point at which it stops being mutable, mirroring tmux's
    /// `grid_compact_line` call in `grid_scroll_history` (grid.c:508).
    ///
    /// A wide glyph can never be cut in half here: its continuation cell
    /// carries the `IS_WIDE_CONTINUATION` flag in the length byte, so it does
    /// not compare equal to the blank cell and is retained.
    pub fn compact(&mut self) {
        let blank = crate::Cell::blank();
        let used = self
            .cells
            .iter()
            .rposition(|cell| cell != blank)
            .map_or(0, |last| last + 1);
        if used == self.cells.len() && self.cells.capacity() == used {
            return;
        }
        // Reallocating to the exact length is what actually returns the
        // padding to the allocator; `Vec::truncate` alone would only move the
        // length and keep the full-width block resident.  `with_capacity(0)`
        // does not allocate at all, so a blank row costs nothing.
        let old = std::mem::take(&mut self.cells);
        let mut cells = Vec::with_capacity(used);
        cells.extend(old.into_iter().take(used));
        self.cells = cells;
    }

    pub fn clear(&mut self, attrs: crate::attrs::Attrs) {
        if attrs == crate::attrs::Attrs::default() {
            // Every column now reads as the blank cell we synthesise, so there
            // is nothing left worth storing.  The capacity is kept because the
            // visible grid will write into this row again immediately.
            self.cells.clear();
        } else {
            // A non-default background has to be remembered per cell, so this
            // row stays dense.
            self.expand(self.cols);
            for cell in &mut self.cells {
                cell.clear(attrs);
            }
        }
        self.wrapped = false;
    }

    /// True when every cell on this row holds no glyph.  Used by the
    /// alt-screen-to-scrollback copy path (issue #88) to skip the
    /// trailing blank rows a TUI did not draw into.
    pub fn is_blank(&self) -> bool {
        !self.cells.iter().any(|c| c.has_contents())
    }

    /// Every logical column of the row, the stored prefix followed by the
    /// blank cells the unstored tail reads as.  Callers see exactly the same
    /// sequence they saw when every row was dense.
    fn cells(&self) -> impl Iterator<Item = &crate::Cell> {
        let pad = usize::from(self.cols).saturating_sub(self.cells.len());
        self.cells
            .iter()
            .chain(std::iter::repeat(crate::Cell::blank()).take(pad))
    }

    pub fn get(&self, col: u16) -> Option<&crate::Cell> {
        if let Some(cell) = self.cells.get(usize::from(col)) {
            return Some(cell);
        }
        // Past the stored prefix but still inside the row: blank, the same
        // answer tmux's grid_get_cell gives past `cellsize` (grid.c:650).
        if col < self.cols {
            Some(crate::Cell::blank())
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, col: u16) -> Option<&mut crate::Cell> {
        if usize::from(col) >= self.cells.len() {
            if col >= self.cols {
                return None;
            }
            self.expand(col + 1);
        }
        self.cells.get_mut(usize::from(col))
    }

    pub fn insert(&mut self, i: u16, cell: crate::Cell) {
        self.expand(self.cols);
        self.cells.insert(usize::from(i), cell);
        self.wrapped = false;
    }

    pub fn remove(&mut self, i: u16) {
        self.clear_wide(i);
        self.expand(self.cols);
        self.cells.remove(usize::from(i));
        self.wrapped = false;
    }

    pub fn erase(&mut self, i: u16, attrs: crate::attrs::Attrs) {
        let wide = self
            .cells
            .get(usize::from(i))
            .is_some_and(crate::Cell::is_wide);
        self.clear_wide(i);
        if attrs == crate::attrs::Attrs::default() {
            // Erasing back to the default is what the unstored tail already
            // reads as, so an unstored column needs no allocation.
            if let Some(cell) = self.cells.get_mut(usize::from(i)) {
                cell.clear(attrs);
            }
        } else if i < self.cols {
            self.expand(i + 1);
            self.cells[usize::from(i)].clear(attrs);
        }
        // A row that was shrunk through a wide glyph's continuation can hand us
        // an orphaned wide cell, and in a one column row `cols() - 2` underflows
        // (#534). Saturating is correct rather than merely safe: if the logical
        // last column would be negative there is no last column to match, so the
        // comparison should simply not fire.
        if i == self.cols().saturating_sub(if wide { 2 } else { 1 }) {
            self.wrapped = false;
        }
    }

    pub fn truncate(&mut self, len: u16) {
        self.cells.truncate(usize::from(len));
        self.cols = len;
        self.wrapped = false;
        if len == 0 {
            return;
        }
        // The last column may be in the unstored tail, where there is no wide
        // glyph to orphan.
        if let Some(last_cell) = self.cells.get_mut(usize::from(len) - 1) {
            if last_cell.is_wide() {
                last_cell.clear(*last_cell.attrs());
            }
        }
    }

    pub fn resize(&mut self, len: u16, cell: crate::Cell) {
        let shrinking = len < self.cols;
        if usize::from(len) < self.cells.len() {
            self.cells.truncate(usize::from(len));
        } else if len > self.cols && &cell != crate::Cell::blank() {
            // Growing with a non-blank filler has to be materialised; growing
            // with a blank one does not, because the unstored tail already
            // reads as blank.
            self.expand(self.cols);
            self.cells.resize(usize::from(len), cell);
        }
        self.cols = len;
        self.wrapped = false;
        // Shrinking can cut away the continuation of a wide glyph, leaving the
        // last cell flagged wide with nothing after it. `truncate` above already
        // clears that; `resize` must too, or the row keeps a cell that claims a
        // width the row cannot hold (#534). `Cell::clear` zeroes the flag byte,
        // so this drops the wide flag along with the contents, matching tmux,
        // which shows nothing for a CJK glyph once the pane is one column wide.
        if shrinking && len > 0 {
            if let Some(last_cell) = self.cells.get_mut(usize::from(len) - 1) {
                if last_cell.is_wide() {
                    last_cell.clear(*last_cell.attrs());
                }
            }
        }
    }

    pub fn wrap(&mut self, wrap: bool) {
        self.wrapped = wrap;
    }

    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    pub fn clear_wide(&mut self, col: u16) {
        // An unstored column holds the blank cell, which is neither wide nor a
        // wide continuation, so there is nothing to clear.
        let (wide, continuation) = match self.cells.get(usize::from(col)) {
            Some(cell) => (cell.is_wide(), cell.is_wide_continuation()),
            None => return,
        };
        if wide {
            let next = usize::from(col + 1);
            if let Some(cell) = self.cells.get_mut(next) {
                let attrs = *cell.attrs();
                cell.clear(attrs);
            }
        } else if continuation && col > 0 {
            let prev = usize::from(col - 1);
            if let Some(cell) = self.cells.get_mut(prev) {
                let attrs = *cell.attrs();
                cell.clear(attrs);
            }
        }
    }

    pub fn write_contents(
        &self,
        contents: &mut String,
        start: u16,
        width: u16,
        wrapping: bool,
    ) {
        let mut prev_was_wide = false;

        let mut prev_col = start;
        for (col, cell) in self
            .cells()
            .enumerate()
            .skip(usize::from(start))
            .take(usize::from(width))
        {
            if prev_was_wide {
                prev_was_wide = false;
                continue;
            }
            prev_was_wide = cell.is_wide();

            // we limit the number of cols to a u16 (see Size)
            let col: u16 = col.try_into().unwrap();
            if cell.has_contents() {
                for _ in 0..(col - prev_col) {
                    contents.push(' ');
                }
                prev_col += col - prev_col;

                contents.push_str(cell.contents());
                prev_col += if cell.is_wide() { 2 } else { 1 };
            }
        }
        if prev_col == start && wrapping {
            contents.push('\n');
        }
    }

    pub fn write_contents_formatted(
        &self,
        contents: &mut Vec<u8>,
        start: u16,
        width: u16,
        row: u16,
        wrapping: bool,
        prev_pos: Option<crate::grid::Pos>,
        prev_attrs: Option<crate::attrs::Attrs>,
    ) -> (crate::grid::Pos, crate::attrs::Attrs) {
        let mut prev_was_wide = false;
        let default_cell = crate::Cell::new();

        let mut prev_pos = prev_pos.unwrap_or_else(|| {
            if wrapping {
                crate::grid::Pos {
                    row: row - 1,
                    col: self.cols(),
                }
            } else {
                crate::grid::Pos { row, col: start }
            }
        });
        let mut prev_attrs = prev_attrs.unwrap_or_default();

        // `start` can fall in the unstored tail of a compacted row, so go
        // through `get`, which synthesises the blank cell rather than indexing.
        let first_cell = self.get(start).unwrap_or(&default_cell);
        if wrapping && first_cell == &default_cell {
            let default_attrs = default_cell.attrs();
            if &prev_attrs != default_attrs {
                default_attrs.write_escape_code_diff(contents, &prev_attrs);
                prev_attrs = *default_attrs;
            }
            contents.push(b' ');
            crate::term::Backspace.write_buf(contents);
            crate::term::EraseChar::new(1).write_buf(contents);
            prev_pos = crate::grid::Pos { row, col: 0 };
        }

        let mut erase: Option<(u16, &crate::attrs::Attrs)> = None;
        for (col, cell) in self
            .cells()
            .enumerate()
            .skip(usize::from(start))
            .take(usize::from(width))
        {
            if prev_was_wide {
                prev_was_wide = false;
                continue;
            }
            prev_was_wide = cell.is_wide();

            // we limit the number of cols to a u16 (see Size)
            let col: u16 = col.try_into().unwrap();
            let pos = crate::grid::Pos { row, col };

            if let Some((prev_col, attrs)) = erase {
                if cell.has_contents() || cell.attrs() != attrs {
                    let new_pos = crate::grid::Pos { row, col: prev_col };
                    if wrapping
                        && prev_pos.row + 1 == new_pos.row
                        && prev_pos.col >= self.cols()
                    {
                        if new_pos.col > 0 {
                            contents.extend(
                                " ".repeat(usize::from(new_pos.col))
                                    .as_bytes(),
                            );
                        } else {
                            contents.extend(b" ");
                            crate::term::Backspace.write_buf(contents);
                        }
                    } else {
                        crate::term::MoveFromTo::new(prev_pos, new_pos)
                            .write_buf(contents);
                    }
                    prev_pos = new_pos;
                    if &prev_attrs != attrs {
                        attrs.write_escape_code_diff(contents, &prev_attrs);
                        prev_attrs = *attrs;
                    }
                    crate::term::EraseChar::new(pos.col - prev_col)
                        .write_buf(contents);
                    erase = None;
                }
            }

            if cell != &default_cell {
                let attrs = cell.attrs();
                if cell.has_contents() {
                    if pos != prev_pos {
                        if !wrapping
                            || prev_pos.row + 1 != pos.row
                            || prev_pos.col
                                < self.cols() - u16::from(cell.is_wide())
                            || pos.col != 0
                        {
                            crate::term::MoveFromTo::new(prev_pos, pos)
                                .write_buf(contents);
                        }
                        prev_pos = pos;
                    }

                    if &prev_attrs != attrs {
                        attrs.write_escape_code_diff(contents, &prev_attrs);
                        prev_attrs = *attrs;
                    }

                    prev_pos.col += if cell.is_wide() { 2 } else { 1 };
                    let cell_contents = cell.contents();
                    contents.extend(cell_contents.as_bytes());
                } else if erase.is_none() {
                    erase = Some((pos.col, attrs));
                }
            }
        }
        if let Some((prev_col, attrs)) = erase {
            let new_pos = crate::grid::Pos { row, col: prev_col };
            if wrapping
                && prev_pos.row + 1 == new_pos.row
                && prev_pos.col >= self.cols()
            {
                if new_pos.col > 0 {
                    contents.extend(
                        " ".repeat(usize::from(new_pos.col)).as_bytes(),
                    );
                } else {
                    contents.extend(b" ");
                    crate::term::Backspace.write_buf(contents);
                }
            } else {
                crate::term::MoveFromTo::new(prev_pos, new_pos)
                    .write_buf(contents);
            }
            prev_pos = new_pos;
            if &prev_attrs != attrs {
                attrs.write_escape_code_diff(contents, &prev_attrs);
                prev_attrs = *attrs;
            }
            crate::term::ClearRowForward.write_buf(contents);
        }

        (prev_pos, prev_attrs)
    }

    // while it's true that most of the logic in this is identical to
    // write_contents_formatted, i can't figure out how to break out the
    // common parts without making things noticeably slower.
    pub fn write_contents_diff(
        &self,
        contents: &mut Vec<u8>,
        prev: &Self,
        start: u16,
        width: u16,
        row: u16,
        wrapping: bool,
        prev_wrapping: bool,
        mut prev_pos: crate::grid::Pos,
        mut prev_attrs: crate::attrs::Attrs,
    ) -> (crate::grid::Pos, crate::attrs::Attrs) {
        let mut prev_was_wide = false;
        let default_cell = crate::Cell::new();

        // Either row may be compacted, so read through `get` rather than
        // indexing the stored prefix.
        let first_cell = self.get(start).unwrap_or(&default_cell);
        let prev_first_cell = prev.get(start).unwrap_or(&default_cell);
        if wrapping
            && !prev_wrapping
            && first_cell == prev_first_cell
            && prev_pos.row + 1 == row
            && prev_pos.col
                >= self.cols() - u16::from(prev_first_cell.is_wide())
        {
            let first_cell_attrs = first_cell.attrs();
            if &prev_attrs != first_cell_attrs {
                first_cell_attrs
                    .write_escape_code_diff(contents, &prev_attrs);
                prev_attrs = *first_cell_attrs;
            }
            let mut cell_contents = prev_first_cell.contents();
            let need_erase = if cell_contents.is_empty() {
                cell_contents = " ";
                true
            } else {
                false
            };
            contents.extend(cell_contents.as_bytes());
            crate::term::Backspace.write_buf(contents);
            if prev_first_cell.is_wide() {
                crate::term::Backspace.write_buf(contents);
            }
            if need_erase {
                crate::term::EraseChar::new(1).write_buf(contents);
            }
            prev_pos = crate::grid::Pos { row, col: 0 };
        }

        let mut erase: Option<(u16, &crate::attrs::Attrs)> = None;
        for (col, (cell, prev_cell)) in self
            .cells()
            .zip(prev.cells())
            .enumerate()
            .skip(usize::from(start))
            .take(usize::from(width))
        {
            if prev_was_wide {
                prev_was_wide = false;
                continue;
            }
            prev_was_wide = cell.is_wide();

            // we limit the number of cols to a u16 (see Size)
            let col: u16 = col.try_into().unwrap();
            let pos = crate::grid::Pos { row, col };

            if let Some((prev_col, attrs)) = erase {
                if cell.has_contents() || cell.attrs() != attrs {
                    let new_pos = crate::grid::Pos { row, col: prev_col };
                    if wrapping
                        && prev_pos.row + 1 == new_pos.row
                        && prev_pos.col >= self.cols()
                    {
                        if new_pos.col > 0 {
                            contents.extend(
                                " ".repeat(usize::from(new_pos.col))
                                    .as_bytes(),
                            );
                        } else {
                            contents.extend(b" ");
                            crate::term::Backspace.write_buf(contents);
                        }
                    } else {
                        crate::term::MoveFromTo::new(prev_pos, new_pos)
                            .write_buf(contents);
                    }
                    prev_pos = new_pos;
                    if &prev_attrs != attrs {
                        attrs.write_escape_code_diff(contents, &prev_attrs);
                        prev_attrs = *attrs;
                    }
                    crate::term::EraseChar::new(pos.col - prev_col)
                        .write_buf(contents);
                    erase = None;
                }
            }

            if cell != prev_cell {
                let attrs = cell.attrs();
                if cell.has_contents() {
                    if pos != prev_pos {
                        if !wrapping
                            || prev_pos.row + 1 != pos.row
                            || prev_pos.col
                                < self.cols() - u16::from(cell.is_wide())
                            || pos.col != 0
                        {
                            crate::term::MoveFromTo::new(prev_pos, pos)
                                .write_buf(contents);
                        }
                        prev_pos = pos;
                    }

                    if &prev_attrs != attrs {
                        attrs.write_escape_code_diff(contents, &prev_attrs);
                        prev_attrs = *attrs;
                    }

                    prev_pos.col += if cell.is_wide() { 2 } else { 1 };
                    contents.extend(cell.contents().as_bytes());
                } else if erase.is_none() {
                    erase = Some((pos.col, attrs));
                }
            }
        }
        if let Some((prev_col, attrs)) = erase {
            let new_pos = crate::grid::Pos { row, col: prev_col };
            if wrapping
                && prev_pos.row + 1 == new_pos.row
                && prev_pos.col >= self.cols()
            {
                if new_pos.col > 0 {
                    contents.extend(
                        " ".repeat(usize::from(new_pos.col)).as_bytes(),
                    );
                } else {
                    contents.extend(b" ");
                    crate::term::Backspace.write_buf(contents);
                }
            } else {
                crate::term::MoveFromTo::new(prev_pos, new_pos)
                    .write_buf(contents);
            }
            prev_pos = new_pos;
            if &prev_attrs != attrs {
                attrs.write_escape_code_diff(contents, &prev_attrs);
                prev_attrs = *attrs;
            }
            crate::term::ClearRowForward.write_buf(contents);
        }

        // if this row is going from wrapped to not wrapped, we need to erase
        // and redraw the last character to break wrapping. if this row is
        // wrapped, we need to redraw the last character without erasing it to
        // position the cursor after the end of the line correctly so that
        // drawing the next line can just start writing and be wrapped.
        if (!self.wrapped && prev.wrapped) || (!prev.wrapped && self.wrapped)
        {
            let end_pos = if self
                .get(self.cols() - 1)
                .is_some_and(crate::Cell::is_wide_continuation)
            {
                crate::grid::Pos {
                    row,
                    col: self.cols() - 2,
                }
            } else {
                crate::grid::Pos {
                    row,
                    col: self.cols() - 1,
                }
            };
            crate::term::MoveFromTo::new(prev_pos, end_pos)
                .write_buf(contents);
            prev_pos = end_pos;
            if !self.wrapped {
                crate::term::EraseChar::new(1).write_buf(contents);
            }
            let end_cell = self.get(end_pos.col).unwrap_or(&default_cell);
            if end_cell.has_contents() {
                let attrs = end_cell.attrs();
                if &prev_attrs != attrs {
                    attrs.write_escape_code_diff(contents, &prev_attrs);
                    prev_attrs = *attrs;
                }
                contents.extend(end_cell.contents().as_bytes());
                prev_pos.col += if end_cell.is_wide() { 2 } else { 1 };
            }
        }

        (prev_pos, prev_attrs)
    }
}
