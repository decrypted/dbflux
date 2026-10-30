//! The grid menu's Copy as SQL IN list, Copy as INSERT statements and Copy as
//! Markdown table: text built from the selection for pasting elsewhere, next
//! to Copy as CSV and Copy as JSON, which go through the exporter instead.

use super::super::{ContextMenuItem, DataGridPanel, DataSource};
use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_components::icons::AppIcon;
use dbflux_core::{
    ColumnAssignment, ColumnMeta, DefaultSqlDialect, MutationRequest, QueryGenerator, QueryResult,
    RowInsert, SqlDialect, TableRef, Value,
};
use gpui::{App, Context};
use std::collections::{BTreeSet, HashSet};

/// Formats `values` as a parenthesized SQL list, `('a', 'b')` or `(1, 2)`,
/// with the dialect's literals. NULLs are left out, since `x IN (NULL)` never
/// matches, and a value repeated in the selection is listed once.
pub(super) fn in_list_text(values: &[Value], dialect: &dyn SqlDialect) -> Option<String> {
    let mut seen = HashSet::new();
    let literals: Vec<String> = values
        .iter()
        .filter(|value| !value.is_null())
        .map(|value| dialect.value_to_literal(value))
        .filter(|literal| seen.insert(literal.clone()))
        .collect();

    (!literals.is_empty()).then(|| format!("({})", literals.join(", ")))
}

/// One INSERT per row into `table`, for `columns`, through the driver's
/// generator; rows holding a value the driver cannot write are skipped, as
/// Copy as Query skips them.
pub(super) fn insert_statements(
    generator: &dyn QueryGenerator,
    table: &TableRef,
    columns: &[ColumnMeta],
    rows: &[Vec<Value>],
) -> Vec<String> {
    rows.iter()
        .filter(|values| {
            !values
                .iter()
                .any(|value| matches!(value, Value::Unsupported(_)))
        })
        .filter_map(|values| {
            let assignments = columns
                .iter()
                .zip(values)
                .map(|(column, value)| ColumnAssignment {
                    name: column.name.clone(),
                    value: value.clone(),
                    type_name: Some(column.type_name.clone()),
                })
                .collect();
            let insert = RowInsert::with_typed_assignments(
                table.name.clone(),
                table.schema.clone(),
                assignments,
            );

            generator
                .generate_mutation(&MutationRequest::SqlInsert(insert))
                .map(|generated| terminated(generated.text))
        })
        .collect()
}

fn terminated(statement: String) -> String {
    if statement.trim_end().ends_with(';') {
        statement
    } else {
        format!("{statement};")
    }
}

/// A GitHub-flavored Markdown table with a header row of column names. NULL
/// cells are empty; pipes and line breaks inside values are escaped so each
/// row stays on its own line.
pub(super) fn markdown_table(result: &QueryResult) -> String {
    let header: Vec<String> = result
        .columns
        .iter()
        .map(|column| markdown_cell(&column.name))
        .collect();
    let mut lines = vec![
        markdown_row(&header),
        markdown_row(&vec!["---".to_string(); header.len()]),
    ];

    lines.extend(result.rows.iter().map(|row| {
        let cells: Vec<String> = row
            .iter()
            .map(|value| match value {
                Value::Null => String::new(),
                value => markdown_cell(&value.as_display_string_truncated(usize::MAX)),
            })
            .collect();
        markdown_row(&cells)
    }));

    lines.join("\n")
}

fn markdown_row(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

fn markdown_cell(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace(['\n', '\r'], "<br>")
}

fn copy_item(label: String, action: ContextMenuAction, icon: AppIcon) -> ContextMenuItem {
    ContextMenuItem {
        label: label.into(),
        action: Some(action),
        icon: Some(icon),
        is_separator: false,
        is_danger: false,
        detail: None,
        disabled: false,
    }
}

impl DataGridPanel {
    /// Puts the extractors after Copy as JSON, so every "Copy as" entry sits
    /// together; a menu without Copy as JSON (the document view) gets none.
    pub(in crate::data_grid_panel) fn with_copy_extractors(
        &self,
        mut items: Vec<ContextMenuItem>,
        cx: &App,
    ) -> Vec<ContextMenuItem> {
        let Some(json_index) = items
            .iter()
            .position(|item| item.action == Some(ContextMenuAction::CopyAsJson))
        else {
            return items;
        };

        let mut extractors = vec![copy_item(
            dbflux_i18n::t!("document.data.context_menu.item.copy_as_sql_in_list"),
            ContextMenuAction::CopyAsSqlInList,
            AppIcon::Parentheses,
        )];
        if self.offers_copy_as_inserts(cx) {
            extractors.push(copy_item(
                dbflux_i18n::t!("document.data.context_menu.item.copy_as_insert_statements"),
                ContextMenuAction::CopyAsInsertStatements,
                AppIcon::Database,
            ));
        }
        extractors.push(copy_item(
            dbflux_i18n::t!("document.data.context_menu.item.copy_as_markdown"),
            ContextMenuAction::CopyAsMarkdown,
            AppIcon::Table,
        ));

        let insert_at = json_index + 1;
        items.splice(insert_at..insert_at, extractors);
        items
    }

    /// Carries Settings > General > Include column names when copying to the
    /// table, which owns Ctrl+C but cannot read settings itself. Run on every
    /// render, so a saved change applies to open grids at once.
    pub(in crate::data_grid_panel) fn sync_copy_column_names(&mut self, cx: &mut Context<Self>) {
        let enabled = self
            .app_state
            .read(cx)
            .general_settings()
            .grid_copy_column_names;
        let Some(table_state) = self.grid_table.table_state.clone() else {
            return;
        };

        if table_state.read(cx).copy_column_names() != enabled {
            table_state.update(cx, |state, _| state.set_copy_column_names(enabled));
        }
    }

    /// Copy as SQL IN list: the selected values of one column. With several
    /// columns selected it takes the leftmost, since an IN list compares one
    /// column.
    pub(in crate::data_grid_panel) fn copy_selection_as_in_list(&mut self, cx: &mut Context<Self>) {
        let Some(values) = self.selected_values_of_first_column(cx) else {
            return;
        };

        let connection = self.copy_connection(cx);
        let text = match &connection {
            Some(connection) => in_list_text(&values, connection.dialect()),
            None => in_list_text(&values, &DefaultSqlDialect),
        };

        if let Some(text) = text {
            self.finish_clipboard_text(text, "SQL IN list", None, cx);
        }
    }

    /// Copy as INSERT statements: one INSERT into the table per selected row,
    /// with the columns that hold a selected cell and each row's values for
    /// them, selected or not.
    pub(in crate::data_grid_panel) fn copy_selection_as_inserts(&mut self, cx: &mut Context<Self>) {
        let DataSource::Table { table, .. } = &self.source else {
            return;
        };
        let table = table.clone();
        let Some((columns, rows)) = self.selected_rows_and_columns(cx) else {
            return;
        };
        let Some(connection) = self.copy_connection(cx) else {
            return;
        };
        let Some(generator) = connection.query_generator() else {
            return;
        };

        let statements = insert_statements(generator, &table, &columns, &rows);
        if statements.is_empty() {
            return;
        }

        let count = statements.len();
        self.finish_clipboard_text(statements.join("\n"), "SQL INSERT", Some(count), cx);
    }

    /// Copy as Markdown table: the same rows and columns as Copy as CSV.
    pub(in crate::data_grid_panel) fn copy_selection_as_markdown(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self.selection_result(cx) else {
            return;
        };

        let count = selected.rows.len();
        self.finish_clipboard_text(markdown_table(&selected), "Markdown", Some(count), cx);
    }

    /// Whether the menu offers Copy as INSERT statements: a table whose
    /// driver generates SQL, the same generator Copy as Query uses.
    pub(super) fn offers_copy_as_inserts(&self, cx: &App) -> bool {
        matches!(self.source, DataSource::Table { .. })
            && self
                .copy_connection(cx)
                .is_some_and(|connection| connection.query_generator().is_some())
    }

    fn copy_connection(&self, cx: &App) -> Option<std::sync::Arc<dyn dbflux_core::Connection>> {
        let profile_id = match &self.source {
            DataSource::Table { profile_id, .. } | DataSource::Collection { profile_id, .. } => {
                Some(*profile_id)
            }
            DataSource::QueryResult { profile_id, .. } => *profile_id,
        }?;

        self.app_state
            .read(cx)
            .connections()
            .get(&profile_id)
            .map(|connected| connected.connection.clone())
    }

    fn selected_values_of_first_column(&self, cx: &App) -> Option<Vec<Value>> {
        let table_state = self.grid_table.table_state.as_ref()?;
        let selection = table_state.read(cx).selection();
        let ranges = selection.ranges();
        let column = ranges
            .iter()
            .map(|range| range.start.col)
            .filter(|&col| col < self.result.columns.len())
            .min()?;
        let rows: BTreeSet<usize> = ranges
            .iter()
            .flat_map(|range| range.start.row..=range.end.row)
            .collect();

        Some(
            rows.into_iter()
                .filter(|&row| selection.is_selected(CellCoord::new(row, column)))
                .filter_map(|row| self.result.rows.get(row)?.get(column).cloned())
                .collect(),
        )
    }

    /// The columns holding a selected cell and, for every row holding one,
    /// that row's values in those columns.
    fn selected_rows_and_columns(&self, cx: &App) -> Option<(Vec<ColumnMeta>, Vec<Vec<Value>>)> {
        let table_state = self.grid_table.table_state.as_ref()?;
        let ranges = table_state.read(cx).selection().ranges();
        if ranges.is_empty() {
            return None;
        }
        let rows: BTreeSet<usize> = ranges
            .iter()
            .flat_map(|range| range.start.row..=range.end.row)
            .collect();
        let cols: BTreeSet<usize> = ranges
            .iter()
            .flat_map(|range| range.start.col..=range.end.col)
            .filter(|&col| col < self.result.columns.len())
            .collect();

        let columns = cols
            .iter()
            .filter_map(|&col| self.result.columns.get(col).cloned())
            .collect();
        let values = rows
            .iter()
            .filter_map(|&row| {
                let values = self.result.rows.get(row)?;
                Some(
                    cols.iter()
                        .map(|&col| values.get(col).cloned().unwrap_or(Value::Null))
                        .collect(),
                )
            })
            .collect();

        Some((columns, values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbflux_core::{ColumnKind, SqlMutationGenerator};
    use std::time::Duration;

    static DIALECT: DefaultSqlDialect = DefaultSqlDialect;

    fn column(name: &str, type_name: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.to_string(),
            type_name: type_name.to_string(),
            kind: ColumnKind::Unknown,
            nullable: true,
            is_primary_key: false,
        }
    }

    #[test]
    fn in_list_quotes_text_dedupes_and_drops_nulls() {
        let values = [
            Value::Text("O'Brien".to_string()),
            Value::Text("Ann".to_string()),
            Value::Null,
            Value::Text("O'Brien".to_string()),
        ];

        assert_eq!(
            in_list_text(&values, &DIALECT).as_deref(),
            Some("('O''Brien', 'Ann')")
        );
        assert_eq!(
            in_list_text(&[Value::Int(1), Value::Int(2), Value::Int(1)], &DIALECT).as_deref(),
            Some("(1, 2)")
        );
        assert_eq!(in_list_text(&[Value::Null], &DIALECT), None);
    }

    #[test]
    fn insert_statements_write_one_statement_per_row() {
        let generator = SqlMutationGenerator::new(&DIALECT);
        let table = TableRef::with_schema("public", "users");
        let columns = [column("id", "int4"), column("name", "text")];
        let rows = vec![
            vec![Value::Int(1), Value::Text("Ann".to_string())],
            vec![Value::Int(2), Value::Null],
        ];

        let statements = insert_statements(&generator, &table, &columns, &rows);

        assert_eq!(
            statements,
            vec![
                "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (1, 'Ann');",
                "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (2, NULL);",
            ]
        );
    }

    #[test]
    fn markdown_table_escapes_pipes_and_line_breaks() {
        let result = QueryResult::table(
            vec![column("id", "int4"), column("note", "text")],
            vec![
                vec![Value::Int(1), Value::Text("a|b\nc".to_string())],
                vec![Value::Int(2), Value::Null],
            ],
            None,
            Duration::ZERO,
        );

        assert_eq!(
            markdown_table(&result),
            "| id | note |\n| --- | --- |\n| 1 | a\\|b<br>c |\n| 2 |  |"
        );
    }
}
