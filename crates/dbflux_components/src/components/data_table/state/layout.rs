//! Hiding, showing and moving columns on screen.
//!
//! These change the screen only: the model, its column indices and the query
//! behind it stay as they are. Each user change emits
//! [`DataTableEvent::ColumnLayoutChanged`] so the host can keep the layout.

use std::sync::Arc;

use gpui::{Context, Pixels, px};

use super::super::column_layout::{ColumnLayout, ColumnLayoutSpec};
use super::super::events::DataTableEvent;
use super::super::selection::CellCoord;
use super::super::theme::ROW_NUMBER_WIDTH;
use super::DataTableState;

/// The largest share of the grid's width the pinned strip, row-number gutter
/// included, may cover; the rest stays for the scrolling columns.
const MAX_PINNED_SHARE: f32 = 2.0 / 3.0;

impl DataTableState {
    pub fn column_layout(&self) -> &ColumnLayout {
        &self.column_layout
    }

    /// Shown columns, as model indices, in screen order.
    pub fn visible_columns(&self) -> &[usize] {
        self.column_layout.visible_columns()
    }

    /// The layout by column name, for the host to keep.
    pub fn column_layout_spec(&self) -> ColumnLayoutSpec {
        self.column_layout.to_spec(&self.column_titles())
    }

    /// Lays the columns out as `spec` says, for a host restoring a kept
    /// layout. Emits no layout change, since nothing new needs keeping.
    pub fn set_column_layout_spec(&mut self, spec: &ColumnLayoutSpec, cx: &mut Context<Self>) {
        let layout = ColumnLayout::from_spec(spec, &self.column_titles());
        if layout != *self.column_layout {
            self.install_column_layout(layout);
            cx.emit(DataTableEvent::SelectionChanged(self.selection.clone()));
            cx.notify();
        }
    }

    pub fn hide_column(&mut self, col: usize, cx: &mut Context<Self>) -> bool {
        self.change_column_layout(cx, |layout| layout.hide(col))
    }

    pub fn show_column(&mut self, col: usize, cx: &mut Context<Self>) -> bool {
        self.change_column_layout(cx, |layout| layout.show(col))
    }

    pub fn show_all_columns(&mut self, cx: &mut Context<Self>) -> bool {
        self.change_column_layout(cx, ColumnLayout::show_all)
    }

    /// Puts every column back in model order and shows them all.
    pub fn reset_column_layout(&mut self, cx: &mut Context<Self>) -> bool {
        let col_count = self.col_count();
        self.change_column_layout(cx, |layout| {
            let identity = ColumnLayout::identity(col_count);
            let changed = *layout != identity;
            *layout = identity;
            changed
        })
    }

    /// Moves `col` into the place of `target` on screen (both model
    /// indices), as dropping its header onto `target`'s does.
    pub fn move_column(&mut self, col: usize, target: usize, cx: &mut Context<Self>) -> bool {
        self.change_column_layout(cx, |layout| layout.move_to(col, target))
    }

    /// Pins `col` to the left edge, where it stays while the grid scrolls
    /// sideways.
    pub fn pin_column(&mut self, col: usize, cx: &mut Context<Self>) -> bool {
        self.change_column_layout(cx, |layout| layout.pin(col))
    }

    pub fn unpin_column(&mut self, col: usize, cx: &mut Context<Self>) -> bool {
        self.change_column_layout(cx, |layout| layout.unpin(col))
    }

    /// How many of the shown columns the grid draws in the pinned strip: the
    /// pinned ones, as many as fit, with the row-number gutter, in
    /// [`MAX_PINNED_SHARE`] of the grid's width. Pinned columns past that
    /// scroll with the others until the grid is wide enough again, so wide
    /// pinned columns never cover every scrolling one. All of them before the
    /// grid has been measured.
    pub fn pinned_shown_count(&self) -> usize {
        let pinned = self.column_layout.pinned_count();
        let viewport: f32 = self.viewport_size.width.into();
        if viewport <= 0.0 {
            return pinned;
        }

        let gutter: f32 = ROW_NUMBER_WIDTH.into();
        let budget = viewport * MAX_PINNED_SHARE - gutter;
        (0..=pinned)
            .rev()
            .find(|count| {
                self.column_offsets
                    .get(*count)
                    .is_some_and(|width| *width <= budget)
            })
            .unwrap_or(0)
    }

    /// Total width of the columns drawn in the pinned strip, the row-number
    /// gutter not included.
    pub fn pinned_width(&self) -> f32 {
        self.column_offsets
            .get(self.pinned_shown_count())
            .copied()
            .unwrap_or(0.0)
    }

    /// Where `col`'s left edge is drawn, from the grid's left edge (the
    /// row-number gutter included) after the horizontal scroll, which a
    /// pinned column does not follow. `None` for a hidden column.
    pub fn column_screen_left(&self, col: usize) -> Option<Pixels> {
        let left = ROW_NUMBER_WIDTH + px(self.column_left(col)?);
        let pinned = self
            .column_layout
            .view_position(col)
            .is_some_and(|position| position < self.pinned_shown_count());

        Some(if pinned {
            left
        } else {
            left - self.horizontal_offset
        })
    }

    /// Moves `col` one shown column left (`step` < 0) or right.
    pub fn move_column_by(&mut self, col: usize, step: isize, cx: &mut Context<Self>) -> bool {
        self.change_column_layout(cx, |layout| layout.move_by(col, step))
    }

    fn change_column_layout(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut ColumnLayout) -> bool,
    ) -> bool {
        let mut layout = (*self.column_layout).clone();
        if !change(&mut layout) {
            return false;
        }

        // An editor left open on a column that leaves the screen could not
        // be seen or closed; commit it as selecting another cell does.
        self.commit_edit_in_progress(cx);
        self.install_column_layout(layout);
        cx.emit(DataTableEvent::ColumnLayoutChanged);
        cx.emit(DataTableEvent::SelectionChanged(self.selection.clone()));
        cx.notify();
        true
    }

    pub(super) fn install_column_layout(&mut self, layout: ColumnLayout) {
        self.column_layout = Arc::new(layout);
        self.selection
            .set_column_layout(Some(Arc::clone(&self.column_layout)));
        self.keep_selection_on_screen();
        self.column_offsets = self.layout_offsets();
    }

    /// Moves a cursor or anchor that sits on a hidden column to the nearest
    /// shown one, looking right first, so the keyboard keeps a visible cell.
    pub(super) fn keep_selection_on_screen(&mut self) {
        let layout = Arc::clone(&self.column_layout);
        let on_screen = |coord: CellCoord| {
            if !layout.is_hidden(coord.col) {
                return coord;
            }
            let order = layout.order();
            let index = order.iter().position(|col| *col == coord.col).unwrap_or(0);
            let right = order
                .iter()
                .skip(index)
                .find(|col| !layout.is_hidden(**col));
            let left = order
                .iter()
                .take(index)
                .rev()
                .find(|col| !layout.is_hidden(**col));
            right
                .or(left)
                .map_or(coord, |col| CellCoord::new(coord.row, *col))
        };

        self.selection.active = self.selection.active.map(on_screen);
        self.selection.anchor = self.selection.anchor.map(on_screen);
    }

    /// Left edges of the shown columns in screen order, plus the right edge
    /// of the last: `[0, w0, w0+w1, ...]` with `w` taken in screen order.
    pub(super) fn layout_offsets(&self) -> Vec<f32> {
        let widths: Vec<f32> = self
            .column_layout
            .visible_columns()
            .iter()
            .map(|col| self.column_widths.get(*col).copied().unwrap_or(0.0))
            .collect();

        Self::calculate_offsets(&widths)
    }

    /// The model column `step` shown columns away from `col`, stopping at
    /// the first and last.
    pub(super) fn screen_neighbor(&self, col: usize, step: isize) -> usize {
        let layout = &self.column_layout;
        let Some(last) = layout.visible_count().checked_sub(1) else {
            return col;
        };
        let position = layout.view_position(col).unwrap_or(0);
        let target = position.saturating_add_signed(step).min(last);

        layout.model_column(target).unwrap_or(col)
    }

    /// The first and last column on screen, `None` without columns.
    pub(super) fn screen_edge_columns(&self) -> Option<(usize, usize)> {
        let visible = self.column_layout.visible_columns();
        Some((*visible.first()?, *visible.last()?))
    }

    pub(super) fn column_titles(&self) -> Vec<Arc<str>> {
        self.model
            .columns
            .iter()
            .map(|column| column.title.clone())
            .collect()
    }
}
