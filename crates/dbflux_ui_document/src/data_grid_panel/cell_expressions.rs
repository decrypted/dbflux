//! Cells set to a SQL expression (`NOW()`, `"qty" + 1`) instead of a value:
//! which columns may hold one, the menu entry that edits one, and the
//! assignment a save writes for it.

use std::collections::HashMap;

use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_components::components::data_table::model::{CellValue, ColumnSpec};
use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_components::icons::AppIcon;
use dbflux_core::{ColumnAssignment, ColumnInfo};
use gpui::{App, Context, Window};

use super::{ContextMenuItem, DataGridPanel, DataSource, TableContextMenu};

impl DataGridPanel {
    /// The expressions offered per result column, keyed by column index, for
    /// a table whose driver writes expressions into its row UPDATE and
    /// INSERT statements. Empty for every other source: a query result has
    /// no save path, and a collection's documents are not SQL.
    ///
    /// The column's type comes from the table's schema when it is loaded,
    /// since the result's type names can drop the length that tells a
    /// `char(36)` uuid from other text.
    pub(super) fn cell_expression_suggestions_by_column(
        &self,
        column_details: Option<&[ColumnInfo]>,
        cx: &App,
    ) -> HashMap<usize, Vec<String>> {
        let DataSource::Table { profile_id, .. } = &self.source else {
            return HashMap::new();
        };
        if self.collection.raw.is_some() {
            return HashMap::new();
        }

        let app_state = self.app_state.read(cx);
        let Some(connected) = app_state.connections().get(profile_id) else {
            return HashMap::new();
        };
        let dialect = connected.connection.dialect();

        self.result
            .columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| {
                let type_name = column_details
                    .and_then(|details| details.iter().find(|info| info.name == column.name))
                    .map(|info| info.type_name.as_str())
                    .unwrap_or(column.type_name.as_str());

                dialect
                    .cell_expression_suggestions(&column.name, type_name)
                    .map(|suggestions| (index, suggestions))
            })
            .collect()
    }

    /// Adds "Edit as SQL expression…" after "Edit" when the cell under the
    /// menu can hold an expression.
    pub(super) fn with_edit_as_expression(
        &self,
        mut items: Vec<ContextMenuItem>,
        menu: &TableContextMenu,
        cx: &App,
    ) -> Vec<ContextMenuItem> {
        let accepts = self
            .grid_table
            .table_state
            .as_ref()
            .is_some_and(|state| state.read(cx).accepts_cell_expressions(menu.col));
        let Some(position) = items
            .iter()
            .position(|item| item.action == Some(ContextMenuAction::Edit))
        else {
            return items;
        };

        if accepts && !menu.is_document_view {
            items.insert(
                position + 1,
                ContextMenuItem {
                    label: dbflux_i18n::t!("document.data.context_menu.item.edit_as_expression")
                        .into(),
                    action: Some(ContextMenuAction::EditAsExpression),
                    icon: Some(AppIcon::Code),
                    is_separator: false,
                    is_danger: false,
                    detail: None,
                    disabled: false,
                },
            );
        }

        items
    }

    /// Opens the inline editor on the cell as a SQL expression; on a cell of
    /// a larger selection the expression goes to all of it.
    pub(super) fn handle_edit_as_expression(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                state.start_editing_expression(CellCoord::new(row, col), window, cx);
            });
        }
    }
}

/// The assignment a save writes for `cell` in `column`: the expression the
/// cell holds, unquoted, or its value as a typed literal.
pub(super) fn column_assignment(column: &ColumnSpec, cell: &CellValue) -> ColumnAssignment {
    let type_name = Some(column.type_name.to_string());

    match cell.sql_expression() {
        Some(expression) => {
            ColumnAssignment::expression(column.title.as_ref(), expression, type_name)
        }
        None => {
            ColumnAssignment::new(column.title.as_ref(), cell.to_value()).with_type_opt(type_name)
        }
    }
}
