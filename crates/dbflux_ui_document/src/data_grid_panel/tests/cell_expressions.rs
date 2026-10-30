use super::*;
use dbflux_components::components::data_table::model::CellValue;
use dbflux_core::{
    CellExpressionVocabulary, DefaultSqlDialect, PlaceholderStyle, SqlDialect, SqlQueryBuilder,
    Value,
};
use dbflux_test_support::fake_driver::{CrudOp, FakeDriver};

/// The ANSI dialect plus a vocabulary, standing in for a driver whose save
/// path writes expressions.
pub(super) struct ExpressionDialect;

static VOCABULARY: CellExpressionVocabulary = CellExpressionVocabulary {
    timestamp: &["NOW()"],
    date: &[],
    time: &[],
    uuid: &[],
    uuid_text: &[],
    uuid_binary: &[],
    text: &["UPPER({col})"],
    supports_default: true,
};

impl SqlDialect for ExpressionDialect {
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

    fn cell_expression_vocabulary(&self) -> Option<&'static CellExpressionVocabulary> {
        Some(&VOCABULARY)
    }
}

pub(super) static EXPRESSION_DIALECT: ExpressionDialect = ExpressionDialect;

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

#[gpui::test]
fn the_grid_offers_the_dialects_expressions_per_column(cx: &mut TestAppContext) {
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite).with_dialect(&EXPRESSION_DIALECT);
    let (panel, window) = events_grid(cx, &driver);

    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        let state = table.read(cx);
        assert_eq!(
            state.cell_expression_suggestions(1),
            ["NOW()", "NULL", "DEFAULT"]
        );
        assert_eq!(
            state.cell_expression_suggestions(2),
            ["UPPER(\"note\")", "NULL", "DEFAULT"]
        );
    });
}

#[gpui::test]
fn a_dialect_without_expressions_offers_none(cx: &mut TestAppContext) {
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite);
    let (panel, window) = events_grid(cx, &driver);

    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        assert!(!table.read(cx).accepts_cell_expressions(1));
    });
}

#[gpui::test]
fn a_saved_expression_reaches_the_driver_unquoted(cx: &mut TestAppContext) {
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite).with_dialect(&EXPRESSION_DIALECT);
    let (panel, window) = events_grid(cx, &driver);

    stage(&panel, window, 0, 1, CellValue::expression("NOW()"));
    stage(&panel, window, 0, 2, CellValue::text("NOW()"));
    window.update(|_, cx| panel.update(cx, |panel, cx| panel.handle_save_row(0, false, cx)));
    window.run_until_parked();

    let patch = driver
        .stats()
        .crud_ops
        .into_iter()
        .find_map(|op| match op {
            CrudOp::Update(patch) => Some(patch),
            _ => None,
        })
        .expect("the save updates the row");

    assert_eq!(
        SqlQueryBuilder::new(&EXPRESSION_DIALECT)
            .build_update(&patch, false)
            .as_deref(),
        Some(
            "UPDATE \"public\".\"events\" SET \"seen_at\" = NOW(), \"note\" = 'NOW()' \
             WHERE \"id\" = 1"
        )
    );
}

#[gpui::test]
fn an_expression_in_a_new_row_reaches_the_insert(cx: &mut TestAppContext) {
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite).with_dialect(&EXPRESSION_DIALECT);
    let (panel, window) = events_grid(cx, &driver);

    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        table.update(cx, |state, cx| {
            state.edit_buffer_mut().add_pending_insert(vec![
                CellValue::int(3),
                CellValue::expression("NOW()"),
                CellValue::null(),
            ]);
            cx.notify();
        });
        panel.update(cx, |panel, cx| panel.handle_commit_insert(0, false, cx));
    });
    window.run_until_parked();

    let insert = driver
        .stats()
        .crud_ops
        .into_iter()
        .find_map(|op| match op {
            CrudOp::Insert(insert) => Some(insert),
            _ => None,
        })
        .expect("the save inserts the row");

    assert_eq!(
        SqlQueryBuilder::new(&EXPRESSION_DIALECT)
            .build_insert(&insert, false)
            .as_deref(),
        Some("INSERT INTO \"public\".\"events\" (\"id\", \"seen_at\") VALUES (3, NOW())")
    );
}
