use super::*;
use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_core::{Connection as _, Value};

fn people_result() -> QueryResult {
    QueryResult::table(
        vec![key_column("id", true), key_column("name", false)],
        [(1, "O'Brien"), (2, "Ann"), (3, "O'Brien")]
            .into_iter()
            .map(|(id, name)| vec![Value::Int(id), Value::Text(name.to_string())])
            .collect(),
        None,
        Duration::ZERO,
    )
}

fn people_panel(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<DataGridPanel>,
    gpui::Entity<AppStateEntity>,
    &mut VisualTestContext,
) {
    query_result_panel_of(cx, people_result())
}

/// A query result with no connection behind it, so the IN list falls back to
/// standard SQL literals (`DefaultSqlDialect`).
fn query_result_panel_of(
    cx: &mut TestAppContext,
    result: QueryResult,
) -> (
    gpui::Entity<DataGridPanel>,
    gpui::Entity<AppStateEntity>,
    &mut VisualTestContext,
) {
    init_test_runtime(cx);

    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();

    let panel = window.update(|window, app| {
        let source = DataSource::QueryResult {
            result: Arc::new(result.clone()),
            original_query: "SELECT id, name FROM people".to_string(),
            profile_id: None,
        };
        let state = app_state.clone();
        let panel =
            app.new(|cx| DataGridPanel::new_internal(source, state, Vec::new(), window, cx));
        panel.update(app, |panel, cx| panel.set_result(result, cx));
        panel
    });

    (panel, app_state, window)
}

fn select_columns(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    first: usize,
    last: usize,
) {
    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| {
            state.click_column(first, gpui::Modifiers::default(), cx);
            let shift = gpui::Modifiers {
                shift: true,
                ..gpui::Modifiers::default()
            };
            state.click_column(last, shift, cx);
        });
    });
}

fn clipboard_text(window: &mut VisualTestContext) -> String {
    window
        .update(|_, cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .unwrap_or_default()
}

/// Copy as SQL IN list quotes text with the dialect, doubling the quote in
/// O'Brien, and lists a repeated value once. With two columns selected it
/// lists the leftmost. The result has no connection, so this is the
/// `DefaultSqlDialect` fallback.
#[gpui::test]
fn copy_as_in_list_quotes_and_dedupes_the_column(cx: &mut TestAppContext) {
    let (panel, _app_state, window) = people_panel(cx);

    select_columns(&panel, window, 1, 1);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_in_list(cx)));
    assert_eq!(clipboard_text(window), "('O''Brien', 'Ann')");

    select_columns(&panel, window, 0, 1);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_in_list(cx)));
    assert_eq!(
        clipboard_text(window),
        "(1, 2, 3)",
        "with two columns selected the leftmost one is listed"
    );
}

/// Copy as Markdown table writes a header row, its separator, and one line
/// per selected row.
#[gpui::test]
fn copy_as_markdown_writes_a_header_and_the_rows(cx: &mut TestAppContext) {
    let (panel, _app_state, window) = people_panel(cx);

    select_columns(&panel, window, 0, 1);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_markdown(cx)));

    assert_eq!(
        clipboard_text(window),
        "| id | name |\n| --- | --- |\n| 1 | O'Brien |\n| 2 | Ann |\n| 3 | O'Brien |"
    );
}

/// With Include column names when copying on, the plain copy starts with
/// the copied columns' names; off, it stays values only.
#[gpui::test]
fn plain_copy_adds_column_names_when_the_setting_is_on(cx: &mut TestAppContext) {
    let (panel, app_state, window) = people_panel(cx);
    select_columns(&panel, window, 1, 1);

    let copy = |window: &mut VisualTestContext| {
        window.update(|_, app| {
            panel.update(app, |panel, cx| panel.sync_copy_column_names(cx));
            panel
                .read(app)
                .grid_table
                .table_state
                .as_ref()
                .expect("table state")
                .read(app)
                .copy_selection()
                .unwrap_or_default()
        })
    };

    assert_eq!(copy(window), "O'Brien\nAnn\nO'Brien");

    window.update(|_, app| {
        app_state.update(app, |state, _| {
            let mut settings = state.general_settings().clone();
            settings.grid_copy_column_names = true;
            state.update_general_settings(settings);
        });
    });
    assert_eq!(copy(window), "name\nO'Brien\nAnn\nO'Brien");
}

/// A query result has no table to insert into, so the menu offers the IN
/// list and the Markdown table after Copy as JSON but not the INSERTs.
#[gpui::test]
fn a_query_result_menu_offers_the_extractors_without_inserts(cx: &mut TestAppContext) {
    let (panel, _app_state, window) = people_panel(cx);

    let actions: Vec<Option<ContextMenuAction>> = window.update(|_, app| {
        let items = DataGridPanel::build_context_menu_items(false, false, true, false, true);
        panel
            .read(app)
            .with_copy_extractors(items, app)
            .iter()
            .map(|item| item.action)
            .collect()
    });

    let json = actions
        .iter()
        .position(|action| *action == Some(ContextMenuAction::CopyAsJson))
        .expect("Copy as JSON is listed");
    assert_eq!(
        actions.get(json + 1..json + 3),
        Some(
            &[
                Some(ContextMenuAction::CopyAsSqlInList),
                Some(ContextMenuAction::CopyAsMarkdown),
            ][..]
        )
    );
    assert!(!actions.contains(&Some(ContextMenuAction::CopyAsInsertStatements)));
}

static STUB: StubConnection = StubConnection;
static GENERATOR: dbflux_core::SqlMutationGenerator =
    dbflux_core::SqlMutationGenerator::new(&dbflux_core::DefaultSqlDialect);

/// A relational connection that generates SQL INSERTs with standard SQL
/// literals and answers no query.
struct SqlTableConnection;

impl dbflux_core::Connection for SqlTableConnection {
    fn metadata(&self) -> &dbflux_core::DriverMetadata {
        STUB.metadata()
    }

    fn kind(&self) -> dbflux_core::DbKind {
        STUB.kind()
    }

    fn schema_loading_strategy(&self) -> dbflux_core::SchemaLoadingStrategy {
        STUB.schema_loading_strategy()
    }

    fn dialect(&self) -> &dyn dbflux_core::SqlDialect {
        &dbflux_core::DefaultSqlDialect
    }

    fn query_generator(&self) -> Option<&dyn dbflux_core::QueryGenerator> {
        Some(&GENERATOR)
    }

    fn ping(&self) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn execute(&self, _: &dbflux_core::QueryRequest) -> Result<QueryResult, dbflux_core::DbError> {
        Err(dbflux_core::DbError::NotSupported("stub".to_string()))
    }

    fn cancel(&self, _: &dbflux_core::QueryHandle) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn schema(&self) -> Result<dbflux_core::SchemaSnapshot, dbflux_core::DbError> {
        Ok(dbflux_core::SchemaSnapshot::default())
    }
}

fn typed_column(name: &str, type_name: &str, is_primary_key: bool) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        type_name: type_name.to_string(),
        kind: ColumnKind::Unknown,
        nullable: !is_primary_key,
        is_primary_key,
    }
}

/// `public.users` on a connection that generates INSERTs: three rows, the
/// second with a `shape` the driver cannot write.
fn users_table_panel(
    cx: &mut TestAppContext,
) -> (gpui::Entity<DataGridPanel>, &mut VisualTestContext) {
    let (app_state, profile_id) = register_stub_connection(cx, Arc::new(SqlTableConnection));
    let window = cx.add_empty_window();
    let result = QueryResult::table(
        vec![
            typed_column("id", "int4", true),
            typed_column("name", "text", false),
            typed_column("shape", "geometry", false),
        ],
        vec![
            vec![
                Value::Int(1),
                Value::Text("O'Brien".to_string()),
                Value::Null,
            ],
            vec![
                Value::Int(2),
                Value::Text("Ann".to_string()),
                Value::Unsupported("geometry".to_string()),
            ],
            vec![Value::Int(3), Value::Text("Bo".to_string()), Value::Null],
        ],
        None,
        Duration::ZERO,
    );

    let panel = window.update(|window, app| {
        let source = DataSource::Table {
            profile_id,
            database: Some("app".to_string()),
            table: TableRef::with_schema("public", "users"),
            pagination: Pagination::default(),
            order_by: Vec::new(),
            total_rows: Some(3),
        };
        let panel = app.new(|cx| {
            DataGridPanel::new_internal(source, app_state, vec!["id".to_string()], window, cx)
        });
        panel.update(app, |panel, cx| panel.set_result(result, cx));
        panel
    });

    (panel, window)
}

fn menu_actions(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> Vec<Option<ContextMenuAction>> {
    window.update(|_, app| {
        let items = DataGridPanel::build_context_menu_items(false, false, true, false, true);
        panel
            .read(app)
            .with_copy_extractors(items, app)
            .iter()
            .map(|item| item.action)
            .collect()
    })
}

/// Selects the cells from `(first_row, first_col)` to `(last_row, last_col)`.
fn select_cells(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    (first_row, first_col): (usize, usize),
    (last_row, last_col): (usize, usize),
) {
    use dbflux_components::components::data_table::selection::CellCoord;

    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| {
            state.select_cell(CellCoord::new(first_row, first_col), cx);
            state.extend_selection(CellCoord::new(last_row, last_col), cx);
        });
    });
}

/// Adds an unsaved row below the first loaded row, so the grid shows
/// `1, (new), 2, 3`.
fn insert_row_after_the_first(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) {
    use dbflux_components::components::data_table::model::CellValue;

    window.update(|_, app| {
        let base_rows = panel.read(app).result.rows.len();
        let width = panel.read(app).result.columns.len();
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| {
            let buffer = state.edit_buffer_mut();
            buffer.set_base_row_count(base_rows);
            let mut row = vec![CellValue::int(9), CellValue::text("pending")];
            row.resize(width, CellValue::null());
            buffer.add_pending_insert_after(0, row);
            cx.notify();
        });
    });
}

/// On a table whose driver generates SQL, Copy as INSERT statements is
/// offered and writes one INSERT per selected row with the selected
/// columns only; a row holding a value the driver cannot write is left out.
#[gpui::test]
fn copy_as_inserts_writes_the_selected_columns_and_skips_unwritable_rows(cx: &mut TestAppContext) {
    let (panel, window) = users_table_panel(cx);
    assert!(
        menu_actions(&panel, window).contains(&Some(ContextMenuAction::CopyAsInsertStatements))
    );

    select_columns(&panel, window, 0, 1);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_inserts(cx)));
    assert_eq!(
        clipboard_text(window),
        [
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (1, 'O''Brien');",
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (2, 'Ann');",
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (3, 'Bo');",
        ]
        .join("\n")
    );

    select_columns(&panel, window, 0, 2);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_inserts(cx)));
    let text = clipboard_text(window);
    assert_eq!(
        text.lines().count(),
        2,
        "row 2 has an unwritable shape: {text}"
    );
    assert!(text.contains("VALUES (1, 'O''Brien', NULL);"), "{text}");
    assert!(text.contains("VALUES (3, 'Bo', NULL);"), "{text}");
}

/// Grouped rows and a builder join hold columns the table does not have, so
/// neither offers Copy as INSERT statements; a plain builder query does.
#[gpui::test]
fn copy_as_inserts_is_not_offered_on_grouped_or_joined_builder_rows(cx: &mut TestAppContext) {
    let (panel, window) = users_table_panel(cx);
    let offers_inserts = |window: &mut VisualTestContext, spec: VisualQuerySpec| {
        window.update(|_, app| {
            panel.update(app, |panel, _| {
                panel.builder.current_visual_spec = Some(spec)
            });
        });
        menu_actions(&panel, window).contains(&Some(ContextMenuAction::CopyAsInsertStatements))
    };

    assert!(!offers_inserts(window, make_grouped_spec()), "grouped rows");

    let mut joined = make_test_spec();
    joined.joins.push(dbflux_core::JoinStep {
        kind: dbflux_core::JoinKind::Inner,
        from_alias: "users".to_string(),
        to_schema: None,
        to_table: "orders".to_string(),
        to_alias: "orders".to_string(),
        on: dbflux_core::JoinOn::FkPath {
            from_column: "id".to_string(),
            to_column: "user_id".to_string(),
        },
    });
    assert!(!offers_inserts(window, joined), "a join");

    assert!(offers_inserts(window, make_test_spec()), "one table");
}

/// A row added in the grid and not saved sits among the loaded rows; the
/// extractors copy the rows the grid shows, not the rows at the same index
/// of the result, with the unsaved row at its place and unsaved edits
/// applied, as Copy as CSV does.
#[gpui::test]
fn the_extractors_copy_the_rows_shown_around_a_pending_insert(cx: &mut TestAppContext) {
    let (panel, _app_state, window) = people_panel(cx);
    insert_row_after_the_first(&panel, window);

    select_cells(&panel, window, (2, 1), (3, 1));
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_in_list(cx)));
    assert_eq!(clipboard_text(window), "('Ann', 'O''Brien')");

    select_cells(&panel, window, (0, 0), (2, 1));
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_markdown(cx)));
    assert_eq!(
        clipboard_text(window),
        "| id | name |\n| --- | --- |\n| 1 | O'Brien |\n| 9 | pending |\n| 2 | Ann |",
        "the unsaved row at its place"
    );

    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| {
            state.edit_buffer_mut().set_cell(
                1,
                1,
                dbflux_components::components::data_table::model::CellValue::text("Edited"),
            );
            cx.notify();
        });
    });
    select_cells(&panel, window, (2, 1), (3, 1));
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_in_list(cx)));
    assert_eq!(
        clipboard_text(window),
        "('Edited', 'O''Brien')",
        "an unsaved edit is copied with its new value"
    );

    let (panel, window) = users_table_panel(cx);
    insert_row_after_the_first(&panel, window);
    select_cells(&panel, window, (1, 0), (3, 1));
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_inserts(cx)));
    assert_eq!(
        clipboard_text(window),
        [
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (9, 'pending');",
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (2, 'Ann');",
            "INSERT INTO \"public\".\"users\" (\"id\", \"name\") VALUES (3, 'Bo');",
        ]
        .join("\n")
    );
}

/// A selection of NULLs alone has no IN list: the clipboard keeps what it
/// held and a toast says why.
#[gpui::test]
fn copy_as_in_list_of_nulls_keeps_the_clipboard_and_says_why(cx: &mut TestAppContext) {
    let result = QueryResult::table(
        vec![key_column("id", true), key_column("note", false)],
        vec![
            vec![Value::Int(1), Value::Null],
            vec![Value::Int(2), Value::Null],
        ],
        None,
        Duration::ZERO,
    );
    let (panel, _app_state, window) = query_result_panel_of(cx, result);
    window.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into())));

    select_columns(&panel, window, 1, 1);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_in_list(cx)));

    assert_eq!(clipboard_text(window), "before");
    assert_eq!(
        last_toast_title(window),
        Some(crate::labels::copy_as_in_list_only_nulls_toast())
    );
}

/// A collection's documents are not SQL rows, so its menu has no SQL IN
/// list; the Markdown table stays.
#[gpui::test]
fn a_collection_menu_offers_no_sql_in_list(cx: &mut TestAppContext) {
    init_test_runtime(cx);

    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let panel = window.update(|window, app| {
        let source = DataSource::Collection {
            profile_id: Uuid::nil(),
            collection: dbflux_core::CollectionRef::new("db", "items"),
            pagination: Pagination::default(),
            total_docs: None,
        };
        let panel =
            app.new(|cx| DataGridPanel::new_internal(source, app_state, vec![], window, cx));
        panel.update(app, |panel, cx| panel.set_result(people_result(), cx));
        panel
    });

    let actions = menu_actions(&panel, window);
    assert!(!actions.contains(&Some(ContextMenuAction::CopyAsSqlInList)));
    assert!(actions.contains(&Some(ContextMenuAction::CopyAsMarkdown)));
}
