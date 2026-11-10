use crate::term::BufWrite as _;

#[derive(Clone, Debug)]
pub struct Grid {
    size: Size,
    pos: Pos,
    saved_pos: Pos,
    rows: Vec<crate::row::Row>,
    scroll_top: u16,
    scroll_bottom: u16,
    origin_mode: bool,
    saved_origin_mode: bool,
    /// Retained history, shared copy on write.
    ///
    /// A row becomes history exactly once, is compacted on the way in
    /// (`scroll_up`, issue #641) and is never written to again, so every copy
    /// of this grid can read the same allocation.  That is what makes psmux's
    /// copy mode snapshot (`Parser::snapshot`, issue #673) cost the visible
    /// screen rather than the whole scrollback: a 50000 line history used to
    /// be deep copied on every entry into copy mode, 186 MB of private bytes
    /// measured at 200 columns.
    ///
    /// Anything that ever needs to CHANGE a retained row must take its own
    /// copy first (`std::sync::Arc::make_mut`), or it would rewrite history
    /// under every snapshot that still holds it.
    scrollback: std::collections::VecDeque<std::sync::Arc<crate::row::Row>>,
    scrollback_len: usize,
    scrollback_offset: usize,
    /// Absolute line number of the OLDEST row currently retained (the front of
    /// `scrollback`, or the top of the drawing area when scrollback is empty).
    /// Starts at 0 and increments by one for every row permanently evicted from
    /// the front of scrollback.  Used to anchor sixel images to a logical line
    /// that survives scrolling (see `crate::image`).  MUST be bumped at every
    /// front-eviction site, so all of them funnel through `evict_front`.
    first_line: u64,
    /// Copy-mode freeze (psmux issue #494): when set, the visible region is
    /// anchored to its current content even at `scrollback_offset == 0` —
    /// rows pushed into scrollback bump the offset so the view does not
    /// follow new output, matching tmux's frozen copy-mode screen.  (At
    /// offset > 0 this anchoring always happens, frozen or not.)
    frozen: bool,
}

impl Grid {
    pub fn new(size: Size, scrollback_len: usize) -> Self {
        Self {
            size,
            pos: Pos::default(),
            saved_pos: Pos::default(),
            rows: vec![],
            scroll_top: 0,
            scroll_bottom: size.rows - 1,
            origin_mode: false,
            saved_origin_mode: false,
            scrollback: std::collections::VecDeque::new(),
            scrollback_len,
            scrollback_offset: 0,
            first_line: 0,
            frozen: false,
        }
    }

    /// Pops the oldest row off the front of scrollback and advances
    /// `first_line` so that absolute line numbers of retained rows stay stable.
    /// This is the single centralised front-eviction primitive; every place
    /// that removes a row from the front of `scrollback` MUST go through here,
    /// otherwise `first_line` drifts and sixel anchors are computed against the
    /// wrong absolute line forever (see the sixel design, risk 3).
    fn evict_front(&mut self) -> Option<std::sync::Arc<crate::row::Row>> {
        let row = self.scrollback.pop_front();
        if row.is_some() {
            self.first_line += 1;
        }
        row
    }

    /// Absolute line number of the oldest retained row (front of scrollback, or
    /// the top of the drawing area when scrollback is empty).
    pub fn first_line(&self) -> u64 {
        self.first_line
    }

    /// Absolute logical line of the current cursor row.
    ///
    /// The scrollback model here keeps evicted rows in `scrollback` (a
    /// `VecDeque` fed from the front) and the live drawing rows in `rows`.  The
    /// top of the drawing area therefore sits at `first_line + scrollback.len()`
    /// absolute lines, and the cursor is `pos.row` rows further down, giving
    /// `first_line + scrollback.len() + pos.row`.  Verified against
    /// `visible_rows`/`scroll_up`: rows leave `rows` into the back of
    /// `scrollback` and are only removed from the front via `evict_front`, so
    /// this relationship holds regardless of scrollback fill or offset.
    pub fn absolute_line_of_cursor(&self) -> u64 {
        self.first_line
            + u64::try_from(self.scrollback.len()).unwrap_or(u64::MAX)
            + u64::from(self.pos.row)
    }

    pub fn frozen(&self) -> bool {
        self.frozen
    }

    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    pub fn allocate_rows(&mut self) {
        if self.rows.is_empty() {
            self.rows.extend(
                std::iter::repeat_with(|| {
                    crate::row::Row::new(self.size.cols)
                })
                .take(usize::from(self.size.rows)),
            );
        }
    }

    fn new_row(&self) -> crate::row::Row {
        crate::row::Row::new(self.size.cols)
    }

    pub fn clear(&mut self) {
        self.pos = Pos::default();
        self.saved_pos = Pos::default();
        for row in self.drawing_rows_mut() {
            row.clear(crate::attrs::Attrs::default());
        }
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows - 1;
        self.origin_mode = false;
        self.saved_origin_mode = false;
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn set_size(&mut self, size: Size) {
        if size.cols != self.size.cols {
            for row in &mut self.rows {
                row.wrap(false);
            }
        }

        if self.scroll_bottom == self.size.rows - 1 {
            self.scroll_bottom = size.rows - 1;
        }

        self.size = size;
        for row in &mut self.rows {
            row.resize(size.cols, crate::Cell::new());
        }
        self.rows.resize(usize::from(size.rows), self.new_row());

        if self.scroll_bottom >= size.rows {
            self.scroll_bottom = size.rows - 1;
        }
        if self.scroll_bottom < self.scroll_top {
            self.scroll_top = 0;
        }

        self.row_clamp_top(false);
        self.row_clamp_bottom(false);
        self.col_clamp();

        if self.saved_pos.row > self.size.rows - 1 {
            self.saved_pos.row = self.size.rows - 1;
        }
        if self.saved_pos.col > self.size.cols - 1 {
            self.saved_pos.col = self.size.cols - 1;
        }
    }

    pub fn pos(&self) -> Pos {
        self.pos
    }

    pub fn set_pos(&mut self, mut pos: Pos) {
        if self.origin_mode {
            pos.row = pos.row.saturating_add(self.scroll_top);
        }
        self.pos = pos;
        self.row_clamp_top(self.origin_mode);
        self.row_clamp_bottom(self.origin_mode);
        self.col_clamp();
    }

    pub fn save_cursor(&mut self) {
        self.saved_pos = self.pos;
        self.saved_origin_mode = self.origin_mode;
    }

    pub fn restore_cursor(&mut self) {
        self.pos = self.saved_pos;
        self.origin_mode = self.saved_origin_mode;
    }

    pub fn visible_rows(&self) -> impl Iterator<Item = &crate::row::Row> {
        let scrollback_len = self.scrollback.len();
        let rows_len = self.rows.len();
        // The `map` that hands out the shared rows comes AFTER the `skip`, so
        // the skip still lands on the deque's own fast path instead of stepping
        // through the history one row at a time.
        self.scrollback
            .iter()
            .skip(scrollback_len - self.scrollback_offset)
            // when scrollback_offset > rows_len (e.g. rows = 3,
            // scrollback_len = 10, offset = 9) the skip(10 - 9)
            // will take 9 rows instead of 3. we need to set
            // the upper bound to rows_len (e.g. 3)
            .take(rows_len)
            .map(|r| &**r)
            // same for rows_len - scrollback_offset (e.g. 3 - 9).
            // it'll panic with overflow. we have to saturate the subtraction.
            .chain(
                self.rows
                    .iter()
                    .take(rows_len.saturating_sub(self.scrollback_offset)),
            )
    }

    pub fn drawing_rows(&self) -> impl Iterator<Item = &crate::row::Row> {
        self.rows.iter()
    }

    pub fn drawing_rows_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut crate::row::Row> {
        self.rows.iter_mut()
    }

    /// The `row`th row of the visible region, indexed rather than walked.
    ///
    /// This is the same sequence `visible_rows` yields, and it has to stay O(1):
    /// `capture-pane -S` asks for one row at a time, so a linear walk per row
    /// turns a 50000 line history into a quadratic read.  (Handing the shared
    /// scrollback rows out through a `map` adaptor did exactly that, and a
    /// capture that took 363 ms started timing out.)
    pub fn visible_row(&self, row: u16) -> Option<&crate::row::Row> {
        let row = usize::from(row);
        let rows_len = self.rows.len();
        if row >= rows_len {
            return None;
        }
        let from_scrollback = self.scrollback_offset.min(rows_len);
        if row < from_scrollback {
            let start = self.scrollback.len() - self.scrollback_offset;
            self.scrollback.get(start + row).map(|r| &**r)
        } else {
            self.rows.get(row - from_scrollback)
        }
    }

    pub fn drawing_row(&self, row: u16) -> Option<&crate::row::Row> {
        self.drawing_rows().nth(usize::from(row))
    }

    pub fn drawing_row_mut(
        &mut self,
        row: u16,
    ) -> Option<&mut crate::row::Row> {
        self.drawing_rows_mut().nth(usize::from(row))
    }

    pub fn current_row_mut(&mut self) -> &mut crate::row::Row {
        self.drawing_row_mut(self.pos.row)
            // we assume self.pos.row is always valid
            .unwrap()
    }

    pub fn visible_cell(&self, pos: Pos) -> Option<&crate::Cell> {
        self.visible_row(pos.row).and_then(|r| r.get(pos.col))
    }

    pub fn drawing_cell(&self, pos: Pos) -> Option<&crate::Cell> {
        self.drawing_row(pos.row).and_then(|r| r.get(pos.col))
    }

    pub fn drawing_cell_mut(&mut self, pos: Pos) -> Option<&mut crate::Cell> {
        self.drawing_row_mut(pos.row)
            .and_then(|r| r.get_mut(pos.col))
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback_len
    }

    pub fn scrollback(&self) -> usize {
        self.scrollback_offset
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        self.scrollback_offset = rows.min(self.scrollback.len());
    }

    /// Returns the number of rows currently held in the scrollback buffer
    /// (distinct from `scrollback_len`, which is the configured maximum).
    pub fn scrollback_filled(&self) -> usize {
        self.scrollback.len()
    }

    /// Approximate resident size of the scrollback in bytes: the cells the
    /// history rows actually store plus the per-row bookkeeping.  Trailing
    /// blank columns are not stored and so do not count, which is the whole
    /// point of the compaction in `scroll_up` (issue #641).  This is what
    /// `#{history_bytes}` reports, matching tmux's field of the same name.
    /// A retained row is shared with every snapshot taken of this grid, so this
    /// is what the history costs once, not once per reader (issue #673).  The
    /// reference counts in front of a shared row are not counted, just as tmux
    /// counts the line struct and its cells and never the allocator's own
    /// bookkeeping, which keeps a fully blank compacted row cheaper than one
    /// cell (issue #641).
    pub fn history_bytes(&self) -> usize {
        let cell = std::mem::size_of::<crate::Cell>();
        let row = std::mem::size_of::<crate::row::Row>();
        self.scrollback
            .iter()
            .map(|r| row + r.stored_cells() * cell)
            .sum()
    }

    /// Updates the scrollback buffer's maximum size.  When `new_len` is
    /// smaller than the current fill, the oldest rows are trimmed away.
    /// Move the retained history out of this grid, leaving it empty.  The
    /// rows are shared (`Arc`), so this moves pointers, never cells.
    pub fn take_scrollback(
        &mut self,
    ) -> std::collections::VecDeque<std::sync::Arc<crate::row::Row>> {
        self.scrollback_offset = 0;
        std::mem::take(&mut self.scrollback)
    }

    /// Install history taken from another grid with `take_scrollback`,
    /// replacing whatever this grid held and trimming the oldest rows to this
    /// grid's limit.
    pub fn put_scrollback(
        &mut self,
        history: std::collections::VecDeque<std::sync::Arc<crate::row::Row>>,
    ) {
        self.scrollback = history;
        self.scrollback_offset = 0;
        while self.scrollback.len() > self.scrollback_len {
            self.evict_front();
        }
    }

    pub fn set_scrollback_len(&mut self, new_len: usize) {
        self.scrollback_len = new_len;
        while self.scrollback.len() > self.scrollback_len {
            self.evict_front();
        }
        if self.scrollback_offset > self.scrollback.len() {
            self.scrollback_offset = self.scrollback.len();
        }
    }

    /// Append a row to the back of scrollback, evicting the oldest if
    /// the cap is reached.  Used by the alt-screen-to-scrollback copy
    /// path (psmux issue #88).  Honours `scrollback_len = 0` (no-op),
    /// matching how the normal in-flow scrolling treats that case.
    pub fn push_row_to_scrollback(&mut self, mut row: crate::row::Row) {
        if self.scrollback_len == 0 {
            return;
        }
        // Scrollback rows are never mutated again, so drop the trailing blank
        // padding before it becomes resident for the life of the history
        // (issue #641).
        row.compact();
        self.scrollback.push_back(std::sync::Arc::new(row));
        while self.scrollback.len() > self.scrollback_len {
            self.evict_front();
        }
        if self.scrollback_offset > 0 || self.frozen {
            self.scrollback_offset =
                self.scrollback.len().min(self.scrollback_offset + 1);
        }
    }

    pub fn write_contents(&self, contents: &mut String) {
        let mut wrapping = false;
        for row in self.visible_rows() {
            row.write_contents(contents, 0, self.size.cols, wrapping);
            if !row.wrapped() {
                contents.push('\n');
            }
            wrapping = row.wrapped();
        }

        while contents.ends_with('\n') {
            contents.truncate(contents.len() - 1);
        }
    }

    pub fn write_contents_formatted(
        &self,
        contents: &mut Vec<u8>,
    ) -> crate::attrs::Attrs {
        crate::term::ClearAttrs.write_buf(contents);
        crate::term::ClearScreen.write_buf(contents);

        let mut prev_attrs = crate::attrs::Attrs::default();
        let mut prev_pos = Pos::default();
        let mut wrapping = false;
        for (i, row) in self.visible_rows().enumerate() {
            // we limit the number of cols to a u16 (see Size), so
            // visible_rows() can never return more rows than will fit
            let i = i.try_into().unwrap();
            let (new_pos, new_attrs) = row.write_contents_formatted(
                contents,
                0,
                self.size.cols,
                i,
                wrapping,
                Some(prev_pos),
                Some(prev_attrs),
            );
            prev_pos = new_pos;
            prev_attrs = new_attrs;
            wrapping = row.wrapped();
        }

        self.write_cursor_position_formatted(
            contents,
            Some(prev_pos),
            Some(prev_attrs),
        );

        prev_attrs
    }

    pub fn write_contents_diff(
        &self,
        contents: &mut Vec<u8>,
        prev: &Self,
        mut prev_attrs: crate::attrs::Attrs,
    ) -> crate::attrs::Attrs {
        let mut prev_pos = prev.pos;
        let mut wrapping = false;
        let mut prev_wrapping = false;
        for (i, (row, prev_row)) in
            self.visible_rows().zip(prev.visible_rows()).enumerate()
        {
            // we limit the number of cols to a u16 (see Size), so
            // visible_rows() can never return more rows than will fit
            let i = i.try_into().unwrap();
            let (new_pos, new_attrs) = row.write_contents_diff(
                contents,
                prev_row,
                0,
                self.size.cols,
                i,
                wrapping,
                prev_wrapping,
                prev_pos,
                prev_attrs,
            );
            prev_pos = new_pos;
            prev_attrs = new_attrs;
            wrapping = row.wrapped();
            prev_wrapping = prev_row.wrapped();
        }

        self.write_cursor_position_formatted(
            contents,
            Some(prev_pos),
            Some(prev_attrs),
        );

        prev_attrs
    }

    pub fn write_cursor_position_formatted(
        &self,
        contents: &mut Vec<u8>,
        prev_pos: Option<Pos>,
        prev_attrs: Option<crate::attrs::Attrs>,
    ) {
        let prev_attrs = prev_attrs.unwrap_or_default();
        // writing a character to the last column of a row doesn't wrap the
        // cursor immediately - it waits until the next character is actually
        // drawn. it is only possible for the cursor to have this kind of
        // position after drawing a character though, so if we end in this
        // position, we need to redraw the character at the end of the row.
        if prev_pos != Some(self.pos) && self.pos.col >= self.size.cols {
            let mut pos = Pos {
                row: self.pos.row,
                col: self.size.cols - 1,
            };
            if self
                .drawing_cell(pos)
                // we assume self.pos.row is always valid, and self.size.cols
                // - 1 is always a valid column
                .unwrap()
                .is_wide_continuation()
            {
                pos.col = self.size.cols - 2;
            }
            let cell =
                // we assume self.pos.row is always valid, and self.size.cols
                // - 2 must be a valid column because self.size.cols - 1 is
                // always valid and we just checked that the cell at
                // self.size.cols - 1 is a wide continuation character, which
                // means that the first half of the wide character must be
                // before it
                self.drawing_cell(pos).unwrap();
            if cell.has_contents() {
                if let Some(prev_pos) = prev_pos {
                    crate::term::MoveFromTo::new(prev_pos, pos)
                        .write_buf(contents);
                } else {
                    crate::term::MoveTo::new(pos).write_buf(contents);
                }
                cell.attrs().write_escape_code_diff(contents, &prev_attrs);
                contents.extend(cell.contents().as_bytes());
                prev_attrs.write_escape_code_diff(contents, cell.attrs());
            } else {
                // if the cell doesn't have contents, we can't have gotten
                // here by drawing a character in the last column. this means
                // that as far as i'm aware, we have to have reached here from
                // a newline when we were already after the end of an earlier
                // row. in the case where we are already after the end of an
                // earlier row, we can just write a few newlines, otherwise we
                // also need to do the same as above to get ourselves to after
                // the end of a row.
                let mut found = false;
                for i in (0..self.pos.row).rev() {
                    pos.row = i;
                    pos.col = self.size.cols - 1;
                    if self
                        .drawing_cell(pos)
                        // i is always less than self.pos.row, which we assume
                        // to be always valid, so it must also be valid.
                        // self.size.cols - 1 is always a valid col.
                        .unwrap()
                        .is_wide_continuation()
                    {
                        pos.col = self.size.cols - 2;
                    }
                    let cell = self
                        .drawing_cell(pos)
                        // i is always less than self.pos.row, which we assume
                        // to be always valid, so it must also be valid.
                        // self.size.cols - 2 is valid because self.size.cols
                        // - 1 is always valid, and col gets set to
                        // self.size.cols - 2 when the cell at self.size.cols
                        // - 1 is a wide continuation character, meaning that
                        // the first half of the wide character must be before
                        // it
                        .unwrap();
                    if cell.has_contents() {
                        if let Some(prev_pos) = prev_pos {
                            if prev_pos.row != i
                                || prev_pos.col < self.size.cols
                            {
                                crate::term::MoveFromTo::new(prev_pos, pos)
                                    .write_buf(contents);
                                cell.attrs().write_escape_code_diff(
                                    contents,
                                    &prev_attrs,
                                );
                                contents.extend(cell.contents().as_bytes());
                                prev_attrs.write_escape_code_diff(
                                    contents,
                                    cell.attrs(),
                                );
                            }
                        } else {
                            crate::term::MoveTo::new(pos).write_buf(contents);
                            cell.attrs().write_escape_code_diff(
                                contents,
                                &prev_attrs,
                            );
                            contents.extend(cell.contents().as_bytes());
                            prev_attrs.write_escape_code_diff(
                                contents,
                                cell.attrs(),
                            );
                        }
                        contents.extend(
                            "\n".repeat(usize::from(self.pos.row - i))
                                .as_bytes(),
                        );
                        found = true;
                        break;
                    }
                }

                // this can happen if you get the cursor off the end of a row,
                // and then do something to clear the end of the current row
                // without moving the cursor (IL, DL, ED, EL, etc). we know
                // there can't be something in the last column because we
                // would have caught that above, so it should be safe to
                // overwrite it.
                if !found {
                    pos = Pos {
                        row: self.pos.row,
                        col: self.size.cols - 1,
                    };
                    if let Some(prev_pos) = prev_pos {
                        crate::term::MoveFromTo::new(prev_pos, pos)
                            .write_buf(contents);
                    } else {
                        crate::term::MoveTo::new(pos).write_buf(contents);
                    }
                    contents.push(b' ');
                    // we know that the cell has no contents, but it still may
                    // have drawing attributes (background color, etc)
                    let end_cell = self
                        .drawing_cell(pos)
                        // we assume self.pos.row is always valid, and
                        // self.size.cols - 1 is always a valid column
                        .unwrap();
                    end_cell
                        .attrs()
                        .write_escape_code_diff(contents, &prev_attrs);
                    crate::term::SaveCursor.write_buf(contents);
                    crate::term::Backspace.write_buf(contents);
                    crate::term::EraseChar::new(1).write_buf(contents);
                    crate::term::RestoreCursor.write_buf(contents);
                    prev_attrs
                        .write_escape_code_diff(contents, end_cell.attrs());
                }
            }
        } else if let Some(prev_pos) = prev_pos {
            crate::term::MoveFromTo::new(prev_pos, self.pos)
                .write_buf(contents);
        } else {
            crate::term::MoveTo::new(self.pos).write_buf(contents);
        }
    }

    pub fn clear_scrollback(&mut self) {
        // Every retained scrollback row is evicted from the front at once, so
        // advance first_line by the whole fill to keep absolute line numbers of
        // the still-visible drawing rows stable (the top of the drawing area was
        // at first_line + scrollback.len() and stays put).
        self.first_line +=
            u64::try_from(self.scrollback.len()).unwrap_or(u64::MAX);
        self.scrollback.clear();
        self.scrollback_offset = 0;
    }

    pub fn erase_all(&mut self, attrs: crate::attrs::Attrs) {
        for row in self.drawing_rows_mut() {
            row.clear(attrs);
        }
    }

    /// Clear the screen with what it showed going into history first, as
    /// tmux's `grid_view_clear_history` does (grid-view.c at tag 3.8): rows
    /// down to the last one holding text, blank ones between them included,
    /// and nothing for a screen with no text. The scroll region plays no part.
    /// The work is at most the height of the screen.
    pub fn erase_all_into_history(&mut self, attrs: crate::attrs::Attrs) {
        if let Some(last) = self.rows.iter().rposition(|row| !row.is_blank()) {
            let moved: Vec<_> = self.rows.drain(..=last).collect();
            for row in moved {
                self.push_row_to_scrollback(row);
            }
            let cols = self.size.cols;
            self.rows.extend(
                std::iter::repeat_with(|| crate::row::Row::new(cols))
                    .take(last + 1),
            );
        }
        self.erase_all(attrs);
    }

    pub fn erase_all_forward(&mut self, attrs: crate::attrs::Attrs) {
        let pos = self.pos;
        for row in self.drawing_rows_mut().skip(usize::from(pos.row) + 1) {
            row.clear(attrs);
        }

        self.erase_row_forward(attrs);
    }

    pub fn erase_all_backward(&mut self, attrs: crate::attrs::Attrs) {
        let pos = self.pos;
        for row in self.drawing_rows_mut().take(usize::from(pos.row)) {
            row.clear(attrs);
        }

        self.erase_row_backward(attrs);
    }

    pub fn erase_row(&mut self, attrs: crate::attrs::Attrs) {
        self.current_row_mut().clear(attrs);
    }

    pub fn erase_row_forward(&mut self, attrs: crate::attrs::Attrs) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in pos.col..size.cols {
            row.erase(col, attrs);
        }
    }

    pub fn erase_row_backward(&mut self, attrs: crate::attrs::Attrs) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in 0..=pos.col.min(size.cols - 1) {
            row.erase(col, attrs);
        }
    }

    pub fn insert_cells(&mut self, count: u16) {
        let size = self.size;
        let pos = self.pos;
        let wide = pos.col < size.cols
            && self
                .drawing_cell(pos)
                // we assume self.pos.row is always valid, and we know we are
                // not off the end of a row because we just checked pos.col <
                // size.cols
                .unwrap()
                .is_wide_continuation();
        let row = self.current_row_mut();
        for _ in 0..count {
            if wide {
                row.get_mut(pos.col).unwrap().set_wide_continuation(false);
            }
            row.insert(pos.col, crate::Cell::new());
            if wide {
                row.get_mut(pos.col).unwrap().set_wide_continuation(true);
            }
        }
        row.truncate(size.cols);
    }

    pub fn delete_cells(&mut self, count: u16) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for _ in 0..(count.min(size.cols - pos.col)) {
            row.remove(pos.col);
        }
        row.resize(size.cols, crate::Cell::new());
    }

    pub fn erase_cells(&mut self, count: u16, attrs: crate::attrs::Attrs) {
        let size = self.size;
        let pos = self.pos;
        let row = self.current_row_mut();
        for col in pos.col..((pos.col.saturating_add(count)).min(size.cols)) {
            row.erase(col, attrs);
        }
    }

    pub fn insert_lines(&mut self, count: u16) {
        for _ in 0..count {
            self.rows.remove(usize::from(self.scroll_bottom));
            self.rows.insert(usize::from(self.pos.row), self.new_row());
            // self.scroll_bottom is maintained to always be a valid row
            self.rows[usize::from(self.scroll_bottom)].wrap(false);
        }
    }

    pub fn delete_lines(&mut self, count: u16) {
        for _ in 0..(count.min(self.size.rows - self.pos.row)) {
            self.rows
                .insert(usize::from(self.scroll_bottom) + 1, self.new_row());
            self.rows.remove(usize::from(self.pos.row));
        }
    }

    pub fn scroll_up(&mut self, count: u16) {
        for _ in 0..(count.min(self.size.rows - self.scroll_top)) {
            self.rows
                .insert(usize::from(self.scroll_bottom) + 1, self.new_row());
            let mut removed = self.rows.remove(usize::from(self.scroll_top));
            if self.scrollback_len > 0 && !self.scroll_region_active() {
                // The row stops being mutable the moment it becomes history,
                // so this is the point to give its trailing blank columns back
                // to the allocator.  tmux does the same in
                // `grid_scroll_history` (grid.c:508).
                removed.compact();
                self.scrollback.push_back(std::sync::Arc::new(removed));
                while self.scrollback.len() > self.scrollback_len {
                    self.evict_front();
                }
                if self.scrollback_offset > 0 || self.frozen {
                    self.scrollback_offset =
                        self.scrollback.len().min(self.scrollback_offset + 1);
                }
            }
        }
    }

    pub fn scroll_down(&mut self, count: u16) {
        for _ in 0..count {
            self.rows.remove(usize::from(self.scroll_bottom));
            self.rows
                .insert(usize::from(self.scroll_top), self.new_row());
            // self.scroll_bottom is maintained to always be a valid row
            self.rows[usize::from(self.scroll_bottom)].wrap(false);
        }
    }

    /// DECSTBM, rows counted from 0. As both console hosts do it: a region
    /// whose bottom is past the screen, or of one line or less, is ignored and
    /// the cursor stays, and a region that is set sends the cursor home, which
    /// is the top of the region in origin mode. tmux differs in three places,
    /// all on purpose here so that psmux's screen stays where the host's is on
    /// the `OpenConsole` route: it homes to the top left of the screen in origin
    /// mode, it cuts a bottom past the screen to the last row, and it ignores
    /// `ESC[0;0r`, which it reads as a one line region (screen-write.c:1775,
    /// input.c:1620 at tag 3.8). Measured with `OpenConsole` and the inbox conhost.
    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        if bottom >= self.size().rows || top >= bottom {
            return;
        }
        self.scroll_top = top;
        self.scroll_bottom = bottom;
        self.set_pos(Pos { row: 0, col: 0 });
    }

    /// The scroll region, its top and bottom rows counted from 0, as
    /// `#{scroll_region_upper}` and `#{scroll_region_lower}` show them.
    pub fn scroll_region(&self) -> (u16, u16) {
        (self.scroll_top, self.scroll_bottom)
    }

    pub fn origin_mode(&self) -> bool {
        self.origin_mode
    }

    /// VPA: to row `row` and keep the column. In origin mode the row counts
    /// from the top of the scroll region and stops at its bottom, as tmux's
    /// `screen_write_cursormove` does with `origin` (screen-write.c:1716 at tag 3.8).
    pub fn row_set_in_origin(&mut self, row: u16) {
        if self.origin_mode {
            let span = self.scroll_bottom - self.scroll_top;
            self.pos.row = self.scroll_top + row.min(span);
        } else {
            self.row_set(row);
        }
    }

    /// DECALN: every cell an `E` with the default attributes, the scroll
    /// region the whole screen, and the cursor at the top left
    /// (`screen_write_alignmenttest`, screen-write.c:1316 at tag 3.8). Origin
    /// mode goes off too, which tmux keeps and both console hosts clear.
    pub fn alignment_pattern(&mut self) {
        let cols = self.size.cols;
        for row in self.drawing_rows_mut() {
            row.clear(crate::attrs::Attrs::default());
            for col in 0..cols {
                if let Some(cell) = row.get_mut(col) {
                    cell.set('E', crate::attrs::Attrs::default());
                }
            }
        }
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows - 1;
        self.origin_mode = false;
        self.pos = Pos { row: 0, col: 0 };
    }

    fn in_scroll_region(&self) -> bool {
        self.pos.row >= self.scroll_top && self.pos.row <= self.scroll_bottom
    }

    fn scroll_region_active(&self) -> bool {
        self.scroll_top != 0 || self.scroll_bottom != self.size.rows - 1
    }

    pub fn set_origin_mode(&mut self, mode: bool) {
        self.origin_mode = mode;
        self.set_pos(Pos { row: 0, col: 0 });
    }

    pub fn row_inc_clamp(&mut self, count: u16) {
        let in_scroll_region = self.in_scroll_region();
        self.pos.row = self.pos.row.saturating_add(count);
        self.row_clamp_bottom(in_scroll_region);
    }

    pub fn row_inc_scroll(&mut self, count: u16) -> u16 {
        let in_scroll_region = self.in_scroll_region();
        self.pos.row = self.pos.row.saturating_add(count);
        let lines = self.row_clamp_bottom(in_scroll_region);
        if in_scroll_region {
            self.scroll_up(lines);
            lines
        } else {
            0
        }
    }

    pub fn row_dec_clamp(&mut self, count: u16) {
        let in_scroll_region = self.in_scroll_region();
        self.pos.row = self.pos.row.saturating_sub(count);
        self.row_clamp_top(in_scroll_region);
    }

    pub fn row_dec_scroll(&mut self, count: u16) {
        let in_scroll_region = self.in_scroll_region();
        // need to account for clamping by both row_clamp_top and by
        // saturating_sub
        let extra_lines = count.saturating_sub(self.pos.row);
        self.pos.row = self.pos.row.saturating_sub(count);
        let lines = self.row_clamp_top(in_scroll_region);
        self.scroll_down(lines + extra_lines);
    }

    /// The scroll region as `(top, bottom)` screen rows, both inclusive.
    pub fn scroll_margins(&self) -> (u16, u16) {
        (self.scroll_top, self.scroll_bottom)
    }

    pub fn row_set(&mut self, i: u16) {
        self.pos.row = i;
        self.row_clamp();
    }

    pub fn col_inc(&mut self, count: u16) {
        self.pos.col = self.pos.col.saturating_add(count);
    }

    pub fn col_inc_clamp(&mut self, count: u16) {
        self.pos.col = self.pos.col.saturating_add(count);
        self.col_clamp();
    }

    pub fn col_dec(&mut self, count: u16) {
        self.pos.col = self.pos.col.saturating_sub(count);
    }

    pub fn col_tab(&mut self) {
        self.pos.col -= self.pos.col % 8;
        self.pos.col += 8;
        self.col_clamp();
    }

    pub fn col_set(&mut self, i: u16) {
        self.pos.col = i;
        self.col_clamp();
    }

    pub fn col_wrap(&mut self, width: u16, wrap: bool) {
        // `cols - width` underflows for a glyph wider than the row, which a
        // pane shrunk to one column produces (#534). Screen::text drops such a
        // glyph before reaching here, but col_wrap is reachable from elsewhere,
        // so keep the arithmetic safe at the source too.
        if self.pos.col > self.size.cols.saturating_sub(width) {
            let mut prev_pos = self.pos;
            self.pos.col = 0;
            let scrolled = self.row_inc_scroll(1);
            let new_pos = self.pos;
            // On a ONE row screen the row that wrapped has just scrolled off
            // (0 - 1): it is history now, or gone with no scrollback, and
            // there is no visible row to flag. The bare subtraction here
            // underflowed and the unwrap below panicked the pane's reader
            // thread (found under #708; psmux allows one row panes, #644).
            let Some(prev_row) = prev_pos.row.checked_sub(scrolled) else {
                return;
            };
            prev_pos.row = prev_row;
            if let Some(row) = self.drawing_row_mut(prev_pos.row) {
                row.wrap(wrap && prev_pos.row + 1 == new_pos.row);
            }
        }
    }

    fn row_clamp_top(&mut self, limit_to_scroll_region: bool) -> u16 {
        if limit_to_scroll_region && self.pos.row < self.scroll_top {
            let rows = self.scroll_top - self.pos.row;
            self.pos.row = self.scroll_top;
            rows
        } else {
            0
        }
    }

    fn row_clamp_bottom(&mut self, limit_to_scroll_region: bool) -> u16 {
        let bottom = if limit_to_scroll_region {
            self.scroll_bottom
        } else {
            self.size.rows - 1
        };
        if self.pos.row > bottom {
            let rows = self.pos.row - bottom;
            self.pos.row = bottom;
            rows
        } else {
            0
        }
    }

    fn row_clamp(&mut self) {
        if self.pos.row > self.size.rows - 1 {
            self.pos.row = self.size.rows - 1;
        }
    }

    fn col_clamp(&mut self) {
        if self.pos.col > self.size.cols - 1 {
            self.pos.col = self.size.cols - 1;
        }
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Pos {
    pub row: u16,
    pub col: u16,
}

#[cfg(test)]
#[path = "../../../tests-rs/test_issue673_scrollback_cow.rs"]
mod test_issue673_scrollback_cow;
