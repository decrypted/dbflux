//! The grid menu's Copy as SQL IN list, Copy as INSERT statements and Copy as
//! Markdown table: text built from the selection for pasting elsewhere, next
//! to Copy as CSV and Copy as JSON, which go through the exporter instead.

use super::super::{ContextMenuItem, DataGridPanel, DataSource};
use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_components::icons::AppIcon;
use dbflux_core::{
    ColumnAssignment, ColumnMeta, DefaultSqlDialect, MutationRequest, QueryGenerator,
    QueryLanguage, QueryResult, RowInsert, SqlDialect, TableRef, Value,
};
use dbflux_ui_base::toast::{Toast, now_hms};
use gpui::{App, Context};
use std::collections::HashSet;

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
/// Copy as Query skips them. `expressions` marks, per row and column, a
/// cell staged as a SQL expression, whose text is written unquoted as the
/// save writes it; an empty slice marks none.
pub(super) fn insert_statements(
    generator: &dyn QueryGenerator,
    table: &TableRef,
    columns: &[ColumnMeta],
    rows: &[Vec<Value>],
    expressions: &[Vec<bool>],
) -> Vec<String> {
    rows.iter()
        .enumerate()
        .filter(|(_, values)| {
            !values
                .iter()
                .any(|value| matches!(value, Value::Unsupported(_)))
        })
        .filter_map(|(row, values)| {
            let assignments = columns
                .iter()
                .zip(values)
                .enumerate()
                .map(|(col, (column, value))| {
                    let is_expression = expressions
                        .get(row)
                        .and_then(|marks| marks.get(col))
                        .copied()
                        .unwrap_or(false);
                    match value {
                        Value::Text(sql) if is_expression => ColumnAssignment::expression(
                            column.name.clone(),
                            sql.clone(),
                            Some(column.type_name.clone()),
                        ),
                        value => ColumnAssignment::typed(
                            column.name.clone(),
                            value.clone(),
                            column.type_name.clone(),
                        ),
                    }
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
/// cells are empty and the other values are written as the exporters write
/// them (see [`markdown_value`]); pipes, line breaks, `&` and `<` inside
/// values are escaped so each row stays on its own line and no value reads
/// as HTML.
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
                value => markdown_cell(&markdown_value(value)),
            })
            .collect();
        markdown_row(&cells)
    }));

    lines.join("\n")
}

/// A value as Copy as CSV writes it (bytes as `\x` hex, dates in ISO
/// form), except arrays and documents, which take the plain JSON of Copy as
/// JSON rather than the CSV writer's tagged form.
fn markdown_value(value: &Value) -> String {
    match value {
        Value::Array(_) | Value::Document(_) => Value::to_serde_json(value).to_string(),
        value => dbflux_export::value_to_csv_field(value),
    }
}

fn markdown_row(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

fn markdown_cell(text: &str) -> String {
    // `&` first, so the entities written for `<` stay as they are, and both
    // before the line breaks, so a literal `<br>` in a value reads as text.
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('\\', "\\\\")
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

        let mut extractors = Vec::new();
        if self.offers_copy_as_in_list(cx) {
            extractors.push(copy_item(
                dbflux_i18n::t!("document.data.context_menu.item.copy_as_sql_in_list"),
                ContextMenuAction::CopyAsSqlInList,
                AppIcon::Parentheses,
            ));
        }
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
        // Cells of the first column outside the selection come back NULL,
        // and an IN list leaves NULLs out.
        let Some(values) = self.selection_result(cx).map(|result| {
            result
                .rows
                .into_iter()
                .filter_map(|row| row.into_iter().next())
                .collect::<Vec<_>>()
        }) else {
            return;
        };

        let connection = self.copy_connection(cx);
        let text = match &connection {
            Some(connection) => in_list_text(&values, connection.dialect()),
            None => in_list_text(&values, &DefaultSqlDialect),
        };

        match text {
            Some(text) => self.finish_clipboard_text(text, "SQL IN list", None, cx),
            // The clipboard keeps what it held, so say why nothing changed.
            None => {
                Toast::warning(crate::labels::copy_as_in_list_only_nulls_toast())
                    .meta_right(now_hms())
                    .push(cx);
            }
        }
    }

    /// Copy as INSERT statements: one INSERT into the table per selected row,
    /// with the columns that hold a selected cell and each row's values for
    /// them as the grid shows them, selected or not.
    pub(in crate::data_grid_panel) fn copy_selection_as_inserts(&mut self, cx: &mut Context<Self>) {
        let DataSource::Table { table, .. } = &self.source else {
            return;
        };
        let table = table.clone();
        let Some(selected) = self.selection_values(cx, false) else {
            return;
        };
        let Some(connection) = self.copy_connection(cx) else {
            return;
        };
        let Some(generator) = connection.query_generator() else {
            return;
        };

        let expressions = self.selection_expressions(cx, &selected);
        let statements = insert_statements(
            generator,
            &table,
            &selected.columns,
            &selected.rows,
            &expressions,
        );
        if statements.is_empty() {
            return;
        }

        let count = statements.len();
        self.finish_clipboard_text(statements.join("\n"), "SQL INSERT", Some(count), cx);
    }

    /// Copy as Markdown table: every row and column the grid shows holding a
    /// selected cell, with the cells outside the selection empty, as Copy as
    /// CSV takes them.
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

    /// Whether the menu offers Copy as SQL IN list: a table, or a query
    /// result that came from SQL (or from no connection, which falls back to
    /// standard SQL literals). A collection's documents are not SQL rows.
    pub(super) fn offers_copy_as_in_list(&self, cx: &App) -> bool {
        match &self.source {
            DataSource::Table { .. } => true,
            DataSource::Collection { .. } => false,
            DataSource::QueryResult { .. } => self.copy_connection(cx).is_none_or(|connection| {
                connection.metadata().query_language == QueryLanguage::Sql
            }),
        }
    }

    /// Whether the menu offers Copy as INSERT statements: a table whose
    /// driver generates SQL, the same generator Copy as Query uses, showing
    /// rows of that table alone. Grouped rows and a builder join hold
    /// columns the table does not have, so the predicate is the one that
    /// lets the grid add rows to a builder result.
    pub(super) fn offers_copy_as_inserts(&self, cx: &App) -> bool {
        matches!(self.source, DataSource::Table { .. })
            && !self.is_grouped_result()
            && self.builder_rows_insertable()
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

        let statements = insert_statements(&generator, &table, &columns, &rows, &[]);

        assert_eq!(
            statements,
            vec![
                "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (1, 'Ann');",
                "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (2, NULL);",
            ]
        );
    }

    #[test]
    fn markdown_table_writes_bytes_and_arrays_as_the_exporter_does() {
        let result = QueryResult::table(
            vec![column("blob", "bytea"), column("tags", "text[]")],
            vec![vec![
                Value::Bytes(vec![0xde, 0xad]),
                Value::Array(vec![Value::Text("a".to_string()), Value::Int(2)]),
            ]],
            None,
            Duration::ZERO,
        );

        assert_eq!(
            markdown_table(&result),
            "| blob | tags |\n| --- | --- |\n| \\\\xdead | [\"a\",2] |"
        );
    }

    #[test]
    fn markdown_cell_escapes_html_and_keeps_a_literal_br_apart() {
        assert_eq!(markdown_cell("a<b & c"), "a&lt;b &amp; c");
        assert_eq!(
            markdown_cell("x<br>y\nz"),
            "x&lt;br>y<br>z",
            "a literal <br> no longer reads like an escaped line break"
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
