use std::sync::Arc;

use super::column_layout::ColumnLayout;

/// Coordinate of a cell in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellCoord {
    pub row: usize,
    pub col: usize,
}

impl CellCoord {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// A rectangular range of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    /// Top-left corner (inclusive)
    pub start: CellCoord,
    /// Bottom-right corner (inclusive)
    pub end: CellCoord,
}

#[allow(dead_code)]
impl CellRange {
    /// Create a range from two corners, normalizing so start <= end.
    pub fn new(a: CellCoord, b: CellCoord) -> Self {
        Self {
            start: CellCoord {
                row: a.row.min(b.row),
                col: a.col.min(b.col),
            },
            end: CellCoord {
                row: a.row.max(b.row),
                col: a.col.max(b.col),
            },
        }
    }

    /// Create a range containing a single cell.
    pub fn single(coord: CellCoord) -> Self {
        Self {
            start: coord,
            end: coord,
        }
    }

    /// Check if a cell is within this range.
    pub fn contains(&self, coord: CellCoord) -> bool {
        coord.row >= self.start.row
            && coord.row <= self.end.row
            && coord.col >= self.start.col
            && coord.col <= self.end.col
    }

    /// Check if this range contains an entire row.
    pub fn contains_row(&self, row: usize) -> bool {
        row >= self.start.row && row <= self.end.row
    }

    pub fn row_count(&self) -> usize {
        self.end.row - self.start.row + 1
    }

    pub fn col_count(&self) -> usize {
        self.end.col - self.start.col + 1
    }

    /// Iterate over all cells in this range (row-major order).
    pub fn iter(&self) -> impl Iterator<Item = CellCoord> {
        let start = self.start;
        let end = self.end;
        (start.row..=end.row)
            .flat_map(move |row| (start.col..=end.col).map(move |col| CellCoord { row, col }))
    }
}

/// Selection state using active + anchor pattern.
///
/// - `active`: Current cursor position (moves with arrows, updated on click)
/// - `anchor`: Starting point for range selection (set on click without shift, stays fixed)
///
/// The selected range is derived from these two points. Ctrl+click
/// (Cmd+click on macOS) keeps earlier ranges in `extra`; the keyboard, Shift
/// and drag act on the anchor/active range only.
///
/// Every coordinate is a model column, but a range spans the columns between
/// its corners as the screen shows them: with reordered or hidden columns
/// (`columns`), dragging from one header to another selects what lies
/// between them on screen. [`Self::ranges`] hands such a range out as the
/// runs of adjacent model columns it covers, so code reading cells by model
/// index never sees a hidden column or one from outside the dragged area.
#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    /// Current cursor position
    pub active: Option<CellCoord>,
    /// Anchor for range selection
    pub anchor: Option<CellCoord>,
    /// Further ranges added with Ctrl+click, oldest first. Their corners are
    /// ordered by screen position, like [`Self::selected_range`].
    extra: Vec<CellRange>,
    /// Screen order of the columns; `None` while it is the model's own.
    columns: Option<Arc<ColumnLayout>>,
}

impl SelectionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.active = None;
        self.anchor = None;
        self.extra.clear();
    }

    /// Sets the screen order ranges follow, `None` for the model's own. The
    /// Ctrl+click ranges are dropped: their corners were ordered for the
    /// previous screen.
    pub fn set_column_layout(&mut self, columns: Option<Arc<ColumnLayout>>) {
        self.columns = columns.filter(|layout| !layout.is_identity());
        self.extra.clear();
    }

    /// Screen position of model column `col`, `None` while it is hidden.
    pub fn view_position(&self, col: usize) -> Option<usize> {
        match &self.columns {
            Some(layout) => layout.view_position(col),
            None => Some(col),
        }
    }

    /// The model column at screen position `position`, for a grid of
    /// `col_count` model columns.
    fn model_column(&self, position: usize, col_count: usize) -> Option<usize> {
        match &self.columns {
            Some(layout) => layout.model_column(position),
            None => (position < col_count).then_some(position),
        }
    }

    /// The first and last column on screen, for a grid of `col_count` model
    /// columns.
    fn screen_edges(&self, col_count: usize) -> Option<(usize, usize)> {
        let shown = match &self.columns {
            Some(layout) => layout.visible_count().min(col_count),
            None => col_count,
        };
        let last = shown.checked_sub(1)?;
        Some((
            self.model_column(0, col_count)?,
            self.model_column(last, col_count)?,
        ))
    }

    /// Orders `cols` (model columns) as the screen shows them, hidden ones
    /// last, so text copied from the selection reads like the grid.
    pub fn sort_by_screen_order(&self, cols: &mut [usize]) {
        cols.sort_by_key(|col| (self.view_position(*col).unwrap_or(usize::MAX), *col));
    }

    /// The range between two cells, its corners ordered by screen position.
    fn range_between(&self, a: CellCoord, b: CellCoord) -> CellRange {
        let (Some(a_pos), Some(b_pos)) = (self.view_position(a.col), self.view_position(b.col))
        else {
            return CellRange::new(a, b);
        };
        let (left, right) = if a_pos <= b_pos {
            (a.col, b.col)
        } else {
            (b.col, a.col)
        };

        CellRange {
            start: CellCoord::new(a.row.min(b.row), left),
            end: CellCoord::new(a.row.max(b.row), right),
        }
    }

    /// Screen positions a range spans, `None` when a corner is hidden.
    fn screen_span(&self, range: &CellRange) -> Option<(usize, usize)> {
        Some((
            self.view_position(range.start.col)?,
            self.view_position(range.end.col)?,
        ))
    }

    fn range_contains(&self, range: &CellRange, coord: CellCoord) -> bool {
        if self.columns.is_none() {
            return range.contains(coord);
        }
        let (Some((left, right)), Some(position)) =
            (self.screen_span(range), self.view_position(coord.col))
        else {
            return false;
        };

        range.contains_row(coord.row) && left <= position && position <= right
    }

    /// A range as runs of adjacent model columns, for readers that walk
    /// `start.col..=end.col`.
    fn model_runs(&self, range: CellRange) -> Vec<CellRange> {
        let Some(layout) = &self.columns else {
            return vec![range];
        };
        let Some((left, right)) = self.screen_span(&range) else {
            return Vec::new();
        };

        let mut cols: Vec<usize> = (left..=right)
            .filter_map(|position| layout.model_column(position))
            .collect();
        cols.sort_unstable();

        let mut runs: Vec<CellRange> = Vec::new();
        for col in cols {
            match runs.last_mut() {
                Some(run) if run.end.col + 1 == col => run.end.col = col,
                _ => runs.push(CellRange {
                    start: CellCoord::new(range.start.row, col),
                    end: CellCoord::new(range.end.row, col),
                }),
            }
        }
        runs
    }

    /// Drops the Ctrl+click ranges, keeping the anchor/active range.
    pub fn clear_extra(&mut self) {
        self.extra.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_none()
    }

    /// Select a single cell (sets both active and anchor to the same position).
    pub fn select_cell(&mut self, coord: CellCoord) {
        self.active = Some(coord);
        self.anchor = Some(coord);
        self.extra.clear();
    }

    /// Ctrl+click on a cell: keep what is selected and start a new range
    /// there, or, when the cell is selected on its own already, deselect it.
    ///
    /// Returns whether the cell was added, so a press that deselected does
    /// not start a drag from it.
    pub fn toggle_cell(&mut self, coord: CellCoord) -> bool {
        self.toggle_range(CellRange::single(coord))
    }

    /// Ctrl+click on a row number: keep what is selected and add the row, or
    /// deselect it when it is selected on its own already.
    pub fn toggle_row(&mut self, row: usize, col_count: usize) {
        let Some((first, last)) = self.screen_edges(col_count) else {
            return;
        };
        self.toggle_range(
            self.range_between(CellCoord::new(row, first), CellCoord::new(row, last)),
        );
    }

    /// Ctrl+click on a column header: keep what is selected and add the
    /// column, or deselect it when it is selected on its own already.
    pub fn toggle_column(&mut self, col: usize, row_count: usize) {
        if row_count == 0 {
            return;
        }
        self.toggle_range(
            self.range_between(CellCoord::new(0, col), CellCoord::new(row_count - 1, col)),
        );
    }

    /// Adds `range` as the current range, or takes it out when it is
    /// selected on its own already. Returns whether it was added.
    fn toggle_range(&mut self, range: CellRange) -> bool {
        if let Some(index) = self.extra.iter().position(|extra| *extra == range) {
            self.extra.remove(index);
            return false;
        }
        if self.selected_range() == Some(range) {
            if self.extra.is_empty() {
                self.clear();
            } else {
                self.restore_last_extra();
            }
            return false;
        }
        self.push_current_range();
        // The cursor at the range's top-left, as select_row and
        // select_column put it, the anchor spanning.
        self.anchor = Some(range.end);
        self.active = Some(range.start);
        true
    }

    fn push_current_range(&mut self) {
        if let Some(range) = self.selected_range()
            && !self.extra.contains(&range)
        {
            self.extra.push(range);
        }
    }

    fn restore_last_extra(&mut self) {
        if let Some(range) = self.extra.pop() {
            self.anchor = Some(range.end);
            self.active = Some(range.start);
        }
    }

    /// Every selected range: the anchor/active one first, then the Ctrl+click
    /// ones. With columns reordered or hidden on screen, each range comes as
    /// the runs of adjacent model columns it covers on screen, so
    /// `start.col..=end.col` of every returned range holds only selected,
    /// shown columns.
    pub fn ranges(&self) -> Vec<CellRange> {
        self.screen_ranges()
            .into_iter()
            .flat_map(|range| self.model_runs(range))
            .collect()
    }

    /// The ranges as selected on screen, corners ordered by screen position.
    fn screen_ranges(&self) -> Vec<CellRange> {
        self.selected_range()
            .into_iter()
            .chain(self.extra.iter().copied())
            .collect()
    }

    /// Extend selection to coord (keeps anchor, moves active).
    pub fn extend_to(&mut self, coord: CellCoord) {
        if self.anchor.is_none() {
            self.anchor = Some(coord);
        }
        self.active = Some(coord);
    }

    /// Get the selected range (normalized so start <= end, the columns by
    /// screen position).
    pub fn selected_range(&self) -> Option<CellRange> {
        match (self.active, self.anchor) {
            (Some(active), Some(anchor)) => Some(self.range_between(anchor, active)),
            (Some(active), None) => Some(CellRange::single(active)),
            _ => None,
        }
    }

    /// Check if a cell is within the current selection.
    pub fn is_selected(&self, coord: CellCoord) -> bool {
        self.screen_ranges()
            .iter()
            .any(|r| self.range_contains(r, coord))
    }

    /// Check if an entire row is selected (every column on screen).
    pub fn is_full_row_selected(&self, row: usize, col_count: usize) -> bool {
        let Some((first, last)) = self.screen_edges(col_count) else {
            return false;
        };
        self.screen_ranges().iter().any(|r| {
            r.start.row <= row && row <= r.end.row && r.start.col == first && r.end.col == last
        })
    }

    /// Select an entire row.
    ///
    /// The active cell is the row's first column, so the focus ring, Enter
    /// and the value panel stay at the left edge; the anchor takes the last
    /// column to span the row.
    pub fn select_row(&mut self, row: usize, col_count: usize) {
        let Some((first, last)) = self.screen_edges(col_count) else {
            return;
        };
        self.anchor = Some(CellCoord::new(row, last));
        self.active = Some(CellCoord::new(row, first));
        self.extra.clear();
    }

    /// Extend the selection to whole rows, from the anchor's row to `row`.
    ///
    /// The anchor moves to the last column and the active cell to the first,
    /// so a range started on a cell becomes full rows, as a Shift+click on a
    /// row number does in a grid. Without an anchor, `row` alone is selected.
    pub fn extend_to_row(&mut self, row: usize, col_count: usize) {
        let Some((first, last)) = self.screen_edges(col_count) else {
            return;
        };
        let anchor_row = self.anchor.map_or(row, |anchor| anchor.row);
        self.anchor = Some(CellCoord::new(anchor_row, last));
        self.active = Some(CellCoord::new(row, first));
    }

    /// Select an entire column.
    ///
    /// The active cell is the column's first row, so the value panel and the
    /// row inspector stay at the top; the anchor takes the last row to span
    /// the column.
    pub fn select_column(&mut self, col: usize, row_count: usize) {
        if row_count == 0 {
            return;
        }
        self.anchor = Some(CellCoord::new(row_count - 1, col));
        self.active = Some(CellCoord::new(0, col));
        self.extra.clear();
    }

    /// Extend the selection to whole columns, from the anchor's column to
    /// `col`, as a Shift+click on a column header does. Ctrl+click ranges
    /// stay, as they do for a Shift+click on a row number.
    pub fn extend_to_column(&mut self, col: usize, row_count: usize) {
        if row_count == 0 {
            return;
        }
        let anchor_col = self.anchor.map_or(col, |anchor| anchor.col);
        self.anchor = Some(CellCoord::new(row_count - 1, anchor_col));
        self.active = Some(CellCoord::new(0, col));
    }

    /// Check if an entire column is selected (all rows).
    pub fn is_full_column_selected(&self, col: usize, row_count: usize) -> bool {
        self.screen_ranges().iter().any(|r| {
            r.start.row == 0
                && r.end.row == row_count.saturating_sub(1)
                && self.range_contains(r, CellCoord::new(0, col))
        })
    }

    /// Select all cells in the table.
    pub fn select_all(&mut self, row_count: usize, col_count: usize) {
        let edges = self.screen_edges(col_count);
        let (Some((first, last)), false) = (edges, row_count == 0) else {
            self.clear();
            return;
        };
        self.anchor = Some(CellCoord::new(0, first));
        self.active = Some(CellCoord::new(row_count - 1, last));
        self.extra.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_range_contains() {
        let range = CellRange::new(CellCoord::new(1, 1), CellCoord::new(3, 3));
        assert!(range.contains(CellCoord::new(2, 2)));
        assert!(range.contains(CellCoord::new(1, 1)));
        assert!(range.contains(CellCoord::new(3, 3)));
        assert!(!range.contains(CellCoord::new(0, 0)));
        assert!(!range.contains(CellCoord::new(4, 4)));
    }

    #[test]
    fn test_cell_range_normalization() {
        let range = CellRange::new(CellCoord::new(3, 3), CellCoord::new(1, 1));
        assert_eq!(range.start, CellCoord::new(1, 1));
        assert_eq!(range.end, CellCoord::new(3, 3));
    }

    #[test]
    fn test_selection_state() {
        let mut sel = SelectionState::new();
        assert!(sel.is_empty());

        sel.select_cell(CellCoord::new(2, 3));
        assert!(!sel.is_empty());
        assert!(sel.is_selected(CellCoord::new(2, 3)));
        assert!(!sel.is_selected(CellCoord::new(0, 0)));

        sel.extend_to(CellCoord::new(4, 5));
        assert!(sel.is_selected(CellCoord::new(3, 4)));
    }

    #[test]
    fn ctrl_click_adds_and_removes_cells_and_rows() {
        let mut sel = SelectionState::new();
        sel.select_cell(CellCoord::new(0, 0));
        sel.toggle_cell(CellCoord::new(2, 1));
        assert!(sel.is_selected(CellCoord::new(0, 0)), "earlier cell kept");
        assert!(sel.is_selected(CellCoord::new(2, 1)), "new cell added");
        assert!(!sel.is_selected(CellCoord::new(1, 0)));
        assert_eq!(sel.ranges().len(), 2);

        // Ctrl+click on a cell selected on its own takes it out again.
        sel.toggle_cell(CellCoord::new(0, 0));
        assert!(!sel.is_selected(CellCoord::new(0, 0)));
        assert!(sel.is_selected(CellCoord::new(2, 1)));
        sel.toggle_cell(CellCoord::new(2, 1));
        assert!(!sel.is_selected(CellCoord::new(2, 1)), "last one too");

        sel.select_row(1, 3);
        sel.toggle_row(4, 3);
        assert!(sel.is_full_row_selected(1, 3));
        assert!(sel.is_full_row_selected(4, 3));
        assert!(!sel.is_full_row_selected(2, 3));

        // A plain selection starts over.
        sel.select_cell(CellCoord::new(0, 0));
        assert_eq!(sel.ranges().len(), 1);
        assert!(!sel.is_full_row_selected(4, 3));
    }

    /// With columns reordered and one hidden, a range spans what lies
    /// between its corners on screen, and `ranges` hands it out as runs of
    /// model columns without the hidden one.
    #[test]
    fn ranges_follow_the_screen_order_of_columns() {
        // Model columns 0..5; screen shows 3, 0, 4, 1 and hides 2.
        let mut layout = ColumnLayout::identity(5);
        assert!(layout.move_to(3, 0));
        assert!(layout.move_to(4, 1));
        assert!(layout.hide(2));
        assert_eq!(layout.visible_columns(), &[3, 0, 4, 1]);

        let mut sel = SelectionState::new();
        sel.set_column_layout(Some(Arc::new(layout)));

        // Drag from the first screen column (model 3) to the third (model 4).
        sel.select_cell(CellCoord::new(0, 3));
        sel.extend_to(CellCoord::new(1, 4));
        assert!(
            sel.is_selected(CellCoord::new(1, 0)),
            "model 0 lies between"
        );
        assert!(
            !sel.is_selected(CellCoord::new(1, 1)),
            "model 1 is right of it"
        );
        assert!(!sel.is_selected(CellCoord::new(1, 2)), "model 2 is hidden");

        let runs: Vec<(usize, usize)> = sel
            .ranges()
            .iter()
            .map(|range| (range.start.col, range.end.col))
            .collect();
        assert_eq!(runs, [(0, 0), (3, 4)]);

        // A whole row is the screen's first to last column, with the
        // cursor in the first one on screen.
        sel.select_row(2, 5);
        assert_eq!(sel.active, Some(CellCoord::new(2, 3)));
        assert_eq!(sel.anchor, Some(CellCoord::new(2, 1)));
        assert!(sel.is_full_row_selected(2, 5));
        assert!(!sel.is_selected(CellCoord::new(2, 2)));

        let mut cols = vec![0, 1, 2, 3, 4];
        sel.sort_by_screen_order(&mut cols);
        assert_eq!(cols, [3, 0, 4, 1, 2]);
    }

    #[test]
    fn select_row_keeps_the_active_cell_in_the_first_column() {
        let mut sel = SelectionState::new();
        sel.select_row(2, 4);

        assert_eq!(sel.active, Some(CellCoord::new(2, 0)));
        assert_eq!(sel.anchor, Some(CellCoord::new(2, 3)));
        assert!(sel.is_full_row_selected(2, 4));
        assert!(!sel.is_full_row_selected(1, 4));
    }

    #[test]
    fn extend_to_row_upward_selects_every_row_back_to_the_anchor() {
        let mut sel = SelectionState::new();
        sel.select_row(5, 3);
        sel.extend_to_row(2, 3);

        assert_eq!(sel.active, Some(CellCoord::new(2, 0)));
        for row in 2..=5 {
            assert!(sel.is_full_row_selected(row, 3), "row {row}");
        }
        assert!(!sel.is_full_row_selected(1, 3));
        assert!(!sel.is_full_row_selected(6, 3));
    }

    #[test]
    fn extend_to_row_from_a_cell_widens_the_range_to_full_rows() {
        let mut sel = SelectionState::new();
        sel.select_cell(CellCoord::new(1, 2));
        sel.extend_to_row(3, 4);

        assert_eq!(sel.active, Some(CellCoord::new(3, 0)));
        for row in 1..=3 {
            assert!(sel.is_full_row_selected(row, 4), "row {row}");
        }
    }

    #[test]
    fn extend_to_row_without_an_anchor_selects_that_row() {
        let mut sel = SelectionState::new();
        sel.extend_to_row(4, 3);

        assert_eq!(sel.active, Some(CellCoord::new(4, 0)));
        assert!(sel.is_full_row_selected(4, 3));
        assert!(!sel.is_full_row_selected(3, 3));
    }

    #[test]
    fn row_selection_without_columns_changes_nothing() {
        let mut sel = SelectionState::new();
        sel.select_row(1, 0);
        sel.extend_to_row(2, 0);

        assert!(sel.is_empty());
    }

    #[test]
    fn ctrl_click_on_the_current_cell_restores_the_previous_range() {
        let mut sel = SelectionState::new();
        sel.select_cell(CellCoord::new(0, 0));
        assert!(sel.toggle_cell(CellCoord::new(2, 1)), "added");
        assert!(!sel.toggle_cell(CellCoord::new(2, 1)), "taken out");

        assert_eq!(sel.active, Some(CellCoord::new(0, 0)));
        assert_eq!(sel.ranges(), vec![CellRange::single(CellCoord::new(0, 0))]);

        sel.select_row(1, 3);
        sel.toggle_row(3, 3);
        assert_eq!(sel.active, Some(CellCoord::new(3, 0)), "cursor at the left");
        sel.toggle_row(3, 3);
        assert_eq!(sel.active, Some(CellCoord::new(1, 0)));
        assert!(sel.is_full_row_selected(1, 3));
        assert!(!sel.is_full_row_selected(3, 3));
        assert_eq!(sel.ranges().len(), 1);
    }

    #[test]
    fn ctrl_click_on_an_earlier_range_takes_it_out() {
        let mut sel = SelectionState::new();
        sel.select_cell(CellCoord::new(0, 0));
        sel.toggle_cell(CellCoord::new(2, 1));
        sel.toggle_cell(CellCoord::new(4, 0));

        assert!(!sel.toggle_cell(CellCoord::new(0, 0)));
        assert!(!sel.is_selected(CellCoord::new(0, 0)));
        assert!(sel.is_selected(CellCoord::new(2, 1)));
        assert_eq!(sel.active, Some(CellCoord::new(4, 0)), "cursor unchanged");
    }

    #[test]
    fn shift_on_a_row_number_keeps_the_ctrl_click_ranges() {
        let mut sel = SelectionState::new();
        sel.select_row(1, 2);
        sel.toggle_row(3, 2);
        sel.extend_to_row(5, 2);

        assert!(sel.is_full_row_selected(1, 2), "the earlier row stays");
        for row in 3..=5 {
            assert!(sel.is_full_row_selected(row, 2), "row {row}");
        }
        assert!(!sel.is_full_row_selected(2, 2));
    }

    #[test]
    fn select_column_keeps_the_active_cell_in_the_first_row() {
        let mut sel = SelectionState::new();
        sel.select_column(1, 4);

        assert_eq!(sel.active, Some(CellCoord::new(0, 1)));
        assert!(sel.is_full_column_selected(1, 4));
        assert!(!sel.is_full_column_selected(0, 4));
        assert!(!sel.is_full_row_selected(0, 3));
    }

    #[test]
    fn extend_to_column_after_a_cell_selects_whole_columns() {
        let mut sel = SelectionState::new();
        sel.select_cell(CellCoord::new(2, 3));
        sel.extend_to_column(1, 5);

        assert_eq!(sel.active, Some(CellCoord::new(0, 1)));
        for col in 1..=3 {
            assert!(sel.is_full_column_selected(col, 5), "column {col}");
        }
        assert!(!sel.is_full_column_selected(0, 5));
        assert!(!sel.is_full_column_selected(4, 5));
    }

    #[test]
    fn extend_to_column_keeps_the_ctrl_click_ranges() {
        let mut sel = SelectionState::new();
        sel.select_cell(CellCoord::new(4, 0));
        sel.toggle_column(2, 5);
        sel.extend_to_column(3, 5);

        assert!(
            sel.is_selected(CellCoord::new(4, 0)),
            "the earlier cell stays"
        );
        assert!(sel.is_full_column_selected(2, 5));
        assert!(sel.is_full_column_selected(3, 5));
        assert!(!sel.is_full_column_selected(1, 5));
    }

    #[test]
    fn toggle_column_adds_and_takes_out_a_column() {
        let mut sel = SelectionState::new();
        sel.select_column(0, 3);
        sel.toggle_column(2, 3);
        assert!(sel.is_full_column_selected(0, 3));
        assert!(sel.is_full_column_selected(2, 3));
        assert!(!sel.is_full_column_selected(1, 3));
        assert_eq!(sel.active, Some(CellCoord::new(0, 2)));

        sel.toggle_column(2, 3);
        assert!(!sel.is_full_column_selected(2, 3));
        assert!(sel.is_full_column_selected(0, 3));
    }

    #[test]
    fn column_selection_without_rows_changes_nothing() {
        let mut sel = SelectionState::new();
        sel.select_column(1, 0);
        sel.extend_to_column(2, 0);
        sel.toggle_column(0, 0);

        assert!(sel.is_empty());
        assert!(!sel.is_full_column_selected(0, 0));
    }
}
