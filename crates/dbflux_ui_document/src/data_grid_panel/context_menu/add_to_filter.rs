//! Add to filter: narrows a table's WHERE filter to the values of cells, from
//! a middle-click on a cell (Shift excludes its value instead), from `=` on
//! the selection, or from the cell menu's entry.

use super::{DataGridPanel, DataSource};
use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_core::Value;
use dbflux_ui_base::toast::{Toast, now_hms};
use gpui::{App, Context, Window};

/// The values one column contributes to the filter: its distinct literals
/// in selection order, and whether a selected cell holds NULL.
#[derive(Debug, Default, PartialEq)]
pub(super) struct ColumnValues {
    pub(super) literals: Vec<String>,
    pub(super) has_null: bool,
}

/// The condition `values` put on a column: `column = v` for one value,
/// `column IN (...)` for several, `IS NULL` for a NULL; `exclude` negates
/// it (`<>`, `NOT IN`, `IS NOT NULL`). `column` is the quoted name,
/// `comparable` the expression its values compare with.
pub(super) fn column_condition(
    column: &str,
    comparable: &str,
    values: &ColumnValues,
    exclude: bool,
) -> Option<String> {
    let compare = match values.literals.as_slice() {
        [] => None,
        [single] if exclude => Some(format!("{comparable} <> {single}")),
        [single] => Some(format!("{comparable} = {single}")),
        several if exclude => Some(format!("{comparable} NOT IN ({})", several.join(", "))),
        several => Some(format!("{comparable} IN ({})", several.join(", "))),
    };
    let null = values.has_null.then(|| {
        if exclude {
            format!("{column} IS NOT NULL")
        } else {
            format!("{column} IS NULL")
        }
    });

    match (compare, null) {
        (Some(compare), Some(null)) if exclude => Some(format!("{compare} AND {null}")),
        (Some(compare), Some(null)) => Some(format!("({compare} OR {null})")),
        (Some(condition), None) | (None, Some(condition)) => Some(condition),
        (None, None) => None,
    }
}

impl DataGridPanel {
    /// `=` and the cell menu's Add to filter: one condition per selected
    /// column, the column's selected values as `=` or `IN`, AND-combined
    /// with the current filter.
    pub(in crate::data_grid_panel) fn add_selection_to_filter(
        &mut self,
        exclude: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(cells) = self.selected_filter_cells(cx) else {
            return false;
        };
        self.add_cells_to_filter(&cells, exclude, window, cx)
    }

    /// The selected cells as (visual row, column), column by column in
    /// screen order.
    fn selected_filter_cells(&self, cx: &App) -> Option<Vec<(usize, usize)>> {
        let state = self.grid_table.table_state.as_ref()?.read(cx);
        let selection = state.selection();
        let rows = selection.selected_rows(state.edit_buffer().compute_visual_order().len());
        let columns = selection.selected_columns(state.col_count());

        let cells: Vec<(usize, usize)> = columns
            .iter()
            .flat_map(|&col| {
                rows.iter()
                    .filter(move |&&row| selection.is_selected(CellCoord::new(row, col)))
                    .map(move |&row| (row, col))
            })
            .collect();

        (!cells.is_empty()).then_some(cells)
    }

    /// Narrows the WHERE filter of a table to `cells` (visual row, column):
    /// see [`column_condition`]. Other sources have no WHERE to narrow; a
    /// value with no literal form (binary, JSON, nested) only gets a hint.
    /// Returns whether the grid took the request.
    pub(in crate::data_grid_panel) fn add_cells_to_filter(
        &mut self,
        cells: &[(usize, usize)],
        exclude: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_add_to_filter() {
            return false;
        }

        let mut columns: Vec<(usize, ColumnValues)> = Vec::new();
        for &(row, col) in cells {
            let Some(value) = self.resolve_cell_value(row, col, cx) else {
                continue;
            };
            if !matches!(value, Value::Null) && !Self::is_literal_filterable(&value) {
                Toast::info(dbflux_i18n::t!(
                    "document.data.grid.filter.value_not_filterable"
                ))
                .meta_right(now_hms())
                .push(cx);
                return true;
            }

            let index = match columns.iter().position(|(column, _)| *column == col) {
                Some(index) => index,
                None => {
                    columns.push((col, ColumnValues::default()));
                    columns.len() - 1
                }
            };
            let Some((_, values)) = columns.get_mut(index) else {
                continue;
            };
            match self.filter_literal(&value, cx) {
                None => values.has_null = true,
                Some(literal) if !values.literals.contains(&literal) => {
                    values.literals.push(literal);
                }
                Some(_) => {}
            }
        }

        let conditions: Vec<String> = columns
            .iter()
            .filter_map(|(col, values)| {
                let (column, comparable) = self.filter_column_exprs(*col, cx)?;
                column_condition(&column, &comparable, values, exclude)
            })
            .collect();

        if !conditions.is_empty() {
            self.apply_filter_expression(&conditions.join(" AND "), window, cx);
        }
        true
    }

    /// A value the filter can compare as a SQL literal; binary, JSON and
    /// nested values have no such form.
    fn is_literal_filterable(value: &Value) -> bool {
        !matches!(
            value,
            Value::Bytes(_)
                | Value::Json(_)
                | Value::Array(_)
                | Value::Document(_)
                | Value::Unsupported(_)
        )
    }

    /// `value` as a literal of the connection's dialect, `None` for NULL.
    fn filter_literal(&self, value: &Value, cx: &App) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let connection = self.filter_connection(cx)?;
        Some(connection.dialect().value_to_literal(value))
    }

    /// The quoted name of column `col` and the expression its values compare
    /// with, in the connection's dialect.
    fn filter_column_exprs(&self, col: usize, cx: &App) -> Option<(String, String)> {
        let connection = self.filter_connection(cx)?;
        let dialect = connection.dialect();
        let column = self.result.columns.get(col)?;
        let quoted = dialect.quote_identifier(&column.name);
        let comparable =
            dialect.comparison_column_expr(&quoted, &column.type_name.to_ascii_lowercase());
        Some((quoted, comparable))
    }

    fn filter_connection(&self, cx: &App) -> Option<std::sync::Arc<dyn dbflux_core::Connection>> {
        let DataSource::Table { profile_id, .. } = &self.source else {
            return None;
        };
        self.app_state
            .read(cx)
            .connections()
            .get(profile_id)
            .map(|connected| connected.connection.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{ColumnValues, column_condition};

    fn values(literals: &[&str], has_null: bool) -> ColumnValues {
        ColumnValues {
            literals: literals.iter().map(|literal| literal.to_string()).collect(),
            has_null,
        }
    }

    #[test]
    fn one_value_is_an_equality_and_several_an_in_list() {
        assert_eq!(
            column_condition("\"m\"", "\"m\"", &values(&["'a'"], false), false).as_deref(),
            Some("\"m\" = 'a'")
        );
        assert_eq!(
            column_condition("\"m\"", "\"m\"", &values(&["'a'", "'b'"], false), false).as_deref(),
            Some("\"m\" IN ('a', 'b')")
        );
    }

    #[test]
    fn exclude_negates_each_form() {
        assert_eq!(
            column_condition("\"m\"", "\"m\"", &values(&["'a'"], false), true).as_deref(),
            Some("\"m\" <> 'a'")
        );
        assert_eq!(
            column_condition("\"m\"", "\"m\"", &values(&["'a'", "'b'"], false), true).as_deref(),
            Some("\"m\" NOT IN ('a', 'b')")
        );
        assert_eq!(
            column_condition("\"m\"", "\"m\"", &values(&[], true), true).as_deref(),
            Some("\"m\" IS NOT NULL")
        );
    }

    #[test]
    fn null_joins_the_values_with_or_and_stands_alone_without_them() {
        assert_eq!(
            column_condition("\"m\"", "\"m\"", &values(&[], true), false).as_deref(),
            Some("\"m\" IS NULL")
        );
        assert_eq!(
            column_condition("\"m\"", "\"m\"", &values(&["'a'"], true), false).as_deref(),
            Some("(\"m\" = 'a' OR \"m\" IS NULL)")
        );
        assert_eq!(
            column_condition("\"m\"", "\"m\"", &values(&[], false), false),
            None
        );
    }
}
