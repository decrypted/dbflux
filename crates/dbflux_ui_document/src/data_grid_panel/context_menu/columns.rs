//! The Column section at the bottom of the column header menu.
//!
//! Its entries change how the grid shows a column, never the query behind
//! it, so they act on the table state alone.

use super::sections::MenuRowCursor;
use super::{DataGridEvent, DataGridPanel, TableContextMenu};
use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_components::composites::{MenuItem, render_menu_header, render_separator};
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
        let Some(state) = self
            .grid_table
            .table_state
            .as_ref()
            .map(|state| state.read(cx))
        else {
            return Vec::new();
        };
        if col >= state.col_count() {
            return Vec::new();
        }

        let item = |label: String, action, icon| ColumnMenuItem {
            label,
            action,
            icon,
        };
        let mut items = vec![item(
            dbflux_i18n::t!("document.data.context_menu.column.fit_width"),
            ContextMenuAction::FitColumnWidth,
            AppIcon::ArrowLeftRight,
        )];
        if !self.offers_column_layout(cx) {
            return items;
        }

        let layout = state.column_layout();
        let position = layout.view_position(col);
        let shown = layout.visible_count();

        items.push(if layout.is_pinned(col) {
            item(
                dbflux_i18n::t!("document.data.context_menu.column.unpin"),
                ContextMenuAction::UnpinColumn,
                AppIcon::Pin,
            )
        } else {
            item(
                dbflux_i18n::t!("document.data.context_menu.column.pin"),
                ContextMenuAction::PinColumn,
                AppIcon::Pin,
            )
        });

        // Entries that would do nothing are left out rather than disabled,
        // so the keyboard never lands on a dead row. Moving stops at the
        // pinned edge, which Pin and Unpin cross.
        let neighbor_on_same_side = |step: isize| {
            position
                .and_then(|position| position.checked_add_signed(step))
                .and_then(|target| layout.model_column(target))
                .is_some_and(|target| layout.is_pinned(target) == layout.is_pinned(col))
        };
        if shown > 1 && position.is_some() {
            items.push(item(
                dbflux_i18n::t!("document.data.context_menu.column.hide"),
                ContextMenuAction::HideColumn,
                AppIcon::EyeOff,
            ));
        }
        items.push(item(
            dbflux_i18n::t!("document.data.context_menu.column.choose"),
            ContextMenuAction::ChooseColumns,
            AppIcon::Columns,
        ));
        if neighbor_on_same_side(-1) {
            items.push(item(
                dbflux_i18n::t!("document.data.context_menu.column.move_left"),
                ContextMenuAction::MoveColumnLeft,
                AppIcon::ChevronLeft,
            ));
        }
        if neighbor_on_same_side(1) {
            items.push(item(
                dbflux_i18n::t!("document.data.context_menu.column.move_right"),
                ContextMenuAction::MoveColumnRight,
                AppIcon::ChevronRight,
            ));
        }

        items
    }

    /// Appends the Column section to the column header menu: a separator, its
    /// heading and one row per entry, numbered on from the menu's earlier
    /// rows so the keyboard walks them in the same order.
    pub(super) fn push_column_menu_rows(
        &self,
        menu: &TableContextMenu,
        cursor: MenuRowCursor<'_>,
        cx: &mut Context<Self>,
    ) {
        let items = self.column_menu_items(menu.col, cx);
        if items.is_empty() {
            return;
        }

        cursor.rows.push(render_separator(cx).into_any_element());
        cursor.rows.push(
            render_menu_header(
                &MenuItem::header(dbflux_i18n::t!("document.data.context_menu.column.title"))
                    .icon(AppIcon::Columns),
                cx,
            )
            .into_any_element(),
        );

        for item in items {
            cursor.rows.push(Self::column_menu_action_row(
                item.label,
                item.action,
                item.icon,
                false,
                *cursor.visual_index,
                cursor.selected_index,
                cx,
            ));
            *cursor.visual_index += 1;
        }
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

        table_state.update(cx, |state, cx| {
            let moved = match action {
                ContextMenuAction::FitColumnWidth => {
                    state.fit_column_width(col, cx);
                    false
                }
                ContextMenuAction::HideColumn => {
                    state.hide_column(col, cx);
                    false
                }
                ContextMenuAction::MoveColumnLeft => state.move_column_by(col, -1, cx),
                ContextMenuAction::MoveColumnRight => state.move_column_by(col, 1, cx),
                ContextMenuAction::PinColumn => {
                    state.pin_column(col, cx);
                    false
                }
                // Unpinned, the column joins the scrolling ones, which may
                // be scrolled past it.
                ContextMenuAction::UnpinColumn => state.unpin_column(col, cx),
                _ => false,
            };

            // A moved column can leave the viewport; follow it.
            if moved {
                state.scroll_to_column(col);
            }
        });
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

        let (row, col, column_left) = {
            let state = table_state.read(cx);
            let active = state.selection().active;
            let (row, col) = active.map_or((0, 0), |cell| (cell.row, cell.col));
            let Some(column_left) = state.column_screen_left(col) else {
                return false;
            };

            (row, col, column_left)
        };

        // The toolbar height is the same estimate the cell menu uses.
        let toolbar_height = px(36.0);
        let position = Point {
            x: self.panel_origin.x + column_left + px(20.0),
            y: self.panel_origin.y + toolbar_height + fonts::grid_header_height(cx),
        };

        self.context_menu = Some(TableContextMenu {
            row,
            col,
            position,
            sql_submenu_open: false,
            copy_query_submenu_open: false,
            filter_submenu_open: false,
            order_submenu_open: false,
            toolbar_submenu_open: false,
            selected_index: 0,
            submenu_selected_index: 0,
            is_document_view: false,
            is_column_header: true,
            doc_field_path: None,
            doc_field_value: None,
            row_actions: Vec::new(),
        });

        self.focus.context_menu_focus.focus(window, cx);
        cx.emit(DataGridEvent::Focused);
        cx.notify();
        true
    }
}
