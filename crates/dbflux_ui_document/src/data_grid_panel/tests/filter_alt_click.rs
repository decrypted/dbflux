//! Alt+click on a cell narrows the WHERE filter to its value.

use super::filter_history::{ReadLog, filter_text, hosted_table, reads, register_filter_stub};
use super::*;
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_core::SqlDialect as _;

/// `id` (integer), `method` (text) and `note` (text, NULL in the row).
fn rows() -> QueryResult {
    let column = |name: &str, type_name: &str, kind: ColumnKind| ColumnMeta {
        name: name.to_string(),
        type_name: type_name.to_string(),
        kind,
        nullable: true,
        is_primary_key: name == "id",
    };

    QueryResult::table(
        vec![
            column("id", "int4", ColumnKind::Integer),
            column("method", "varchar", ColumnKind::Text),
            column("note", "text", ColumnKind::Text),
        ],
        vec![vec![
            dbflux_core::Value::Int(1),
            dbflux_core::Value::Text("getbets".to_string()),
            dbflux_core::Value::Null,
        ]],
        None,
        Duration::ZERO,
    )
}

fn quoted(column: &str) -> String {
    dbflux_core::DefaultSqlDialect.quote_identifier(column)
}

/// A table grid on a recording connection, its filter set to `filter`
/// and read once with it.
fn grid_with_filter<'a>(
    cx: &'a mut TestAppContext,
    filter: &str,
) -> (
    gpui::Entity<DataGridPanel>,
    &'a mut VisualTestContext,
    ReadLog,
) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let (profile_id, log) = cx.update(|cx| register_filter_stub(&app_state, rows(), None, cx));

    let (_host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");
    super::filter_history::apply_filter(&panel, window, filter);

    (panel, window, log)
}

/// What the table does for an Alt+click on the cell at `col` of the row.
fn alt_click(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext, col: usize) {
    window.update(|_, cx| {
        let table_state = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("the result builds a table");
        table_state.update(cx, |state, cx| {
            state.click_cell(CellCoord::new(0, col), gpui::Modifiers::alt(), cx)
        });
    });
    window.run_until_parked();
}

#[gpui::test]
fn alt_click_on_a_text_cell_filters_by_it_with_one_read(cx: &mut TestAppContext) {
    let (panel, window, log) = grid_with_filter(cx, "");
    let before = reads(&log).len();

    alt_click(&panel, window, 1);

    let expected = format!("{} = 'getbets'", quoted("method"));
    assert_eq!(filter_text(&panel, window), expected);
    assert_eq!(reads(&log)[before..], [Some(expected)], "one read");
}

#[gpui::test]
fn alt_click_adds_to_an_existing_filter(cx: &mut TestAppContext) {
    let (panel, window, log) = grid_with_filter(cx, "status = 500");
    let before = reads(&log).len();

    alt_click(&panel, window, 1);

    let expected = format!("status = 500 AND {} = 'getbets'", quoted("method"));
    assert_eq!(filter_text(&panel, window), expected);
    assert_eq!(reads(&log)[before..], [Some(expected)], "one read");
}

#[gpui::test]
fn alt_click_on_a_null_cell_adds_is_null(cx: &mut TestAppContext) {
    let (panel, window, _log) = grid_with_filter(cx, "");

    alt_click(&panel, window, 2);

    assert_eq!(
        filter_text(&panel, window),
        format!("{} IS NULL", quoted("note"))
    );
}

#[gpui::test]
fn a_filter_with_a_top_level_or_is_parenthesized(cx: &mut TestAppContext) {
    let (panel, window, _log) = grid_with_filter(cx, "status = 500 OR status = 404");

    alt_click(&panel, window, 1);

    assert_eq!(
        filter_text(&panel, window),
        format!(
            "(status = 500 OR status = 404) AND {} = 'getbets'",
            quoted("method")
        )
    );
}

#[gpui::test]
fn a_condition_already_in_the_filter_is_not_added_again(cx: &mut TestAppContext) {
    let filter = format!("status = 500 AND {} = 'getbets'", quoted("method"));
    let (panel, window, log) = grid_with_filter(cx, &filter);
    let before = reads(&log).len();

    alt_click(&panel, window, 1);

    assert_eq!(filter_text(&panel, window), filter);
    assert_eq!(reads(&log).len(), before, "nothing new to read");
}
