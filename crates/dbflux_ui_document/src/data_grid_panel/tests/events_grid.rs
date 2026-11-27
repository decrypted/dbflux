//! `events(id, seen_at, note)`: a small table grid behind a fake
//! connection, for the tests that save, review and generate cell values.

use super::*;
use dbflux_components::components::data_table::model::CellValue;
use dbflux_core::{DefaultSqlDialect, PlaceholderStyle, SqlDialect, Value};
use dbflux_test_support::fake_driver::FakeDriver;

/// The ANSI dialect, quoting the way the expected statements are written.
pub(super) struct AnsiDialect;

impl SqlDialect for AnsiDialect {
    fn quote_identifier(&self, name: &str) -> String {
        DefaultSqlDialect.quote_identifier(name)
    }

    fn qualified_table(&self, schema: Option<&str>, table: &str) -> String {
        DefaultSqlDialect.qualified_table(schema, table)
    }

    fn value_to_literal(&self, value: &Value) -> String {
        DefaultSqlDialect.value_to_literal(value)
    }

    fn escape_string(&self, s: &str) -> String {
        DefaultSqlDialect.escape_string(s)
    }

    fn placeholder_style(&self) -> PlaceholderStyle {
        PlaceholderStyle::QuestionMark
    }
}

pub(super) static ANSI_DIALECT: AnsiDialect = AnsiDialect;

fn column(name: &str, type_name: &str, is_primary_key: bool) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        type_name: type_name.to_string(),
        kind: ColumnKind::Text,
        nullable: !is_primary_key,
        is_primary_key,
    }
}

/// `events(id, seen_at, note)` with two rows, behind a connection whose
/// dialect is `driver`'s. The connection is registered before the result
/// arrives because the grid reads the dialect when it builds the table.
pub(super) fn events_grid<'a>(
    cx: &'a mut TestAppContext,
    driver: &FakeDriver,
) -> (gpui::Entity<DataGridPanel>, &'a mut VisualTestContext) {
    init_test_runtime(cx);

    let app_state = isolated_test_app_state(cx);
    let holder: Rc<RefCell<Option<gpui::Entity<DataGridPanel>>>> = Rc::new(RefCell::new(None));
    let handle = holder.clone();

    let (_, window) = cx.add_window_view(|window, cx| {
        let panel = cx.new(|cx| {
            let source = DataSource::Table {
                profile_id: Uuid::nil(),
                database: Some("app".to_string()),
                table: TableRef::with_schema("public", "events"),
                pagination: Pagination::default(),
                order_by: Vec::new(),
                total_rows: None,
            };
            DataGridPanel::new_internal(
                source,
                app_state.clone(),
                vec!["id".to_string()],
                window,
                cx,
            )
        });
        handle.replace(Some(panel.clone()));
        Root::new(panel, window, cx)
    });
    let panel = holder.borrow().clone().expect("panel should be created");

    register_fake_connection(&panel, window, driver);

    let result = QueryResult::table(
        vec![
            column("id", "int4", true),
            column("seen_at", "timestamp", false),
            column("note", "text", false),
        ],
        (1..=2)
            .map(|id| {
                vec![
                    Value::Int(id),
                    Value::Text("2026-01-01 00:00:00".to_string()),
                    Value::Text("note".to_string()),
                ]
            })
            .collect(),
        None,
        Duration::ZERO,
    );
    window.update(|_, cx| panel.update(cx, |panel, cx| panel.set_result(result, cx)));
    window.run_until_parked();

    (panel, window)
}

pub(super) fn stage(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    row: usize,
    col: usize,
    cell: CellValue,
) {
    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        table.update(cx, |state, cx| {
            state.stage_cell_value(row, col, cell);
            cx.notify();
        });
    });
}
