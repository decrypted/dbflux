//! The Column section at the bottom of the column header menu.
//!
//! Its entries change how the grid shows a column, never the query behind
//! it, so they act on the table state alone.

use super::{ContextMenuTarget, DataGridEvent, DataGridPanel};
use dbflux_components::components::data_table::{ContextMenuAction, ROW_NUMBER_WIDTH};
use dbflux_components::fonts;
use dbflux_components::icons::AppIcon;
use gpui::*;

/// One entry of the Column section.
pub(in crate::data_grid_panel) struct ColumnMenuItem {
    pub(in crate::data_grid_panel) label: String,
    pub(in crate::data_grid_panel) action: ContextMenuAction,
    pub(in crate::data_grid_panel) icon: AppIcon,
}

impl DataGridPanel {
    /// The Column section's entries for the column at model index `col`, in
    /// menu order. Empty without a grid to act on.
    pub(in crate::data_grid_panel) fn column_menu_items(
        &self,
        col: usize,
        cx: &App,
    ) -> Vec<ColumnMenuItem> {
        let has_column = self
            .grid_table
            .table_state
            .as_ref()
            .is_some_and(|state| col < state.read(cx).col_count());
        if !has_column {
            return Vec::new();
        }

        vec![ColumnMenuItem {
            label: dbflux_i18n::t!("document.data.context_menu.column.fit_width"),
            action: ContextMenuAction::FitColumnWidth,
            icon: AppIcon::ArrowLeftRight,
        }]
    }

    /// Runs a Column section entry on the column at model index `col`.
    pub(super) fn handle_column_menu_action(
        &mut self,
        action: ContextMenuAction,
        col: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(table_state) = self.grid_table.table_state.clone() else {
            return;
        };

        if action == ContextMenuAction::FitColumnWidth {
            table_state.update(cx, |state, cx| state.fit_column_width(col, cx));
        }
    }

    /// Opens the column header menu of the active cell's column, under its
    /// header, as a right-click on the header does. Returns `false` when the
    /// grid has no column to open it for.
    pub(in crate::data_grid_panel) fn open_column_menu_at_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(table_state) = &self.grid_table.table_state else {
            return false;
        };

        let (row, col, column_left, horizontal_offset) = {
            let state = table_state.read(cx);
            let active = state.selection().active;
            let (row, col) = active.map_or((0, 0), |cell| (cell.row, cell.col));
            let Some(column_left) = state.column_left(col) else {
                return false;
            };

            (row, col, column_left, state.horizontal_offset())
        };

        // The toolbar height is the same estimate the cell menu uses.
        let toolbar_height = px(36.0);
        let position = Point {
            x: self.panel_origin.x + ROW_NUMBER_WIDTH + px(column_left) - horizontal_offset
                + px(20.0),
            y: self.panel_origin.y + toolbar_height + fonts::grid_header_height(cx),
        };

        self.context_menu = Some(self.new_context_menu(
            ContextMenuTarget {
                row,
                col,
                position,
                is_document_view: false,
                is_column_header: true,
                doc_field_path: None,
                doc_field_value: None,
                row_actions: Vec::new(),
            },
            cx,
        ));

        self.focus.context_menu_focus.focus(window, cx);
        cx.emit(DataGridEvent::Focused);
        cx.notify();
        true
    }
}
