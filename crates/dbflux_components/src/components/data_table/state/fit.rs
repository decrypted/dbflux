//! Fitting a column to its content on request (double-click on the header's
//! resize edge, or Fit width in the column menu).

use gpui::Context;

use super::super::model::{CellValue, VisualRowSource};
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
        let null_value = CellValue::null();

        let longest_value = self
            .edit_buffer
            .compute_visual_order()
            .into_iter()
            .filter_map(|source| match source {
                VisualRowSource::Base(row) => {
                    let base = self
                        .model
                        .rows
                        .get(row)
                        .and_then(|row| row.cells.get(col))
                        .unwrap_or(&null_value);
                    Some(self.edit_buffer.get_cell(row, col, base))
                }
                VisualRowSource::Insert(insert) => self
                    .edit_buffer
                    .get_pending_insert_by_idx(insert)
                    .and_then(|cells| cells.get(col)),
            })
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
