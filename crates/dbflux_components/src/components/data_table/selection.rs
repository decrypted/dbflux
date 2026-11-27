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
/// The selected range is derived from these two points.
#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    /// Current cursor position
    pub active: Option<CellCoord>,
    /// Anchor for range selection
    pub anchor: Option<CellCoord>,
}

impl SelectionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.active = None;
        self.anchor = None;
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_none()
    }

    /// Select a single cell (sets both active and anchor to the same position).
    pub fn select_cell(&mut self, coord: CellCoord) {
        self.active = Some(coord);
        self.anchor = Some(coord);
    }

    /// Extend selection to coord (keeps anchor, moves active).
    pub fn extend_to(&mut self, coord: CellCoord) {
        if self.anchor.is_none() {
            self.anchor = Some(coord);
        }
        self.active = Some(coord);
    }

    /// Get the selected range (normalized so start <= end).
    pub fn selected_range(&self) -> Option<CellRange> {
        match (self.active, self.anchor) {
            (Some(active), Some(anchor)) => Some(CellRange::new(anchor, active)),
            (Some(active), None) => Some(CellRange::single(active)),
            _ => None,
        }
    }

    /// Check if a cell is within the current selection.
    pub fn is_selected(&self, coord: CellCoord) -> bool {
        self.selected_range()
            .map(|r| r.contains(coord))
            .unwrap_or(false)
    }

    /// Check if an entire row is selected (all columns from 0 to col_count-1).
    #[allow(dead_code)]
    pub fn is_full_row_selected(&self, row: usize, col_count: usize) -> bool {
        self.selected_range()
            .map(|r| {
                r.start.row <= row
                    && row <= r.end.row
                    && r.start.col == 0
                    && r.end.col == col_count.saturating_sub(1)
            })
            .unwrap_or(false)
    }

    /// Select an entire row.
    ///
    /// The active cell is the row's first column, so the focus ring, Enter
    /// and the value panel stay at the left edge; the anchor takes the last
    /// column to span the row.
    pub fn select_row(&mut self, row: usize, col_count: usize) {
        if col_count == 0 {
            return;
        }
        self.anchor = Some(CellCoord::new(row, col_count - 1));
        self.active = Some(CellCoord::new(row, 0));
    }

    /// Extend the selection to whole rows, from the anchor's row to `row`.
    ///
    /// The anchor moves to the last column and the active cell to the first,
    /// so a range started on a cell becomes full rows, as a Shift+click on a
    /// row number does in a grid. Without an anchor, `row` alone is selected.
    pub fn extend_to_row(&mut self, row: usize, col_count: usize) {
        if col_count == 0 {
            return;
        }
        let anchor_row = self.anchor.map_or(row, |anchor| anchor.row);
        self.anchor = Some(CellCoord::new(anchor_row, col_count - 1));
        self.active = Some(CellCoord::new(row, 0));
    }

    /// Select all cells in the table.
    pub fn select_all(&mut self, row_count: usize, col_count: usize) {
        if row_count == 0 || col_count == 0 {
            self.clear();
            return;
        }
        self.anchor = Some(CellCoord::new(0, 0));
        self.active = Some(CellCoord::new(row_count - 1, col_count - 1));
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
}
