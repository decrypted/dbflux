//! Fitting a column to its content on request (double-click on the header's
//! resize edge, or Fit width in the column menu).

use gpui::Context;

use super::super::theme::{MAX_FIT_COLUMN_WIDTH, MIN_COLUMN_WIDTH};
use super::{DataTableState, GridTextMetrics};

impl DataTableState {
    /// Sizes `col` to its header and to the longest value among every loaded
    /// row, pending inserts and staged edits included, as the user sees them.
    ///
    /// Unlike the width a column opens with, which samples the first rows,
    /// this reads them all: it runs once per request, not per reload. Content
    /// counts up to `MAX_FIT_COLUMN_WIDTH`, so one long JSON value does not
    /// push the rest of the grid off screen; a wider header still fits.
    ///
    /// The fitted width counts as set by the user, so a grid font change
    /// keeps it, like a dragged width.
    pub fn fit_column_width(&mut self, col: usize, cx: &mut Context<Self>) {
        let Some(width) = self.fitted_column_width(col, &GridTextMetrics::current(cx)) else {
            return;
        };

        self.set_column_width(col, width, cx);
    }

    fn fitted_column_width(&self, col: usize, metrics: &GridTextMetrics) -> Option<f32> {
        let column = self.model.columns.get(col)?;
        let visual_order = self.edit_buffer.compute_visual_order();

        let longest_value = (0..visual_order.len())
            .filter_map(|row| self.displayed_cell(&visual_order, row, col))
            .map(|cell| cell.display_text().chars().count())
            .max()
            .unwrap_or(0);

        let content_width = Self::content_width(longest_value, metrics).min(MAX_FIT_COLUMN_WIDTH);

        Some(
            Self::header_width(column, metrics)
                .max(content_width)
                .max(MIN_COLUMN_WIDTH)
                .ceil(),
        )
    }
}
