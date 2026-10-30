//! Add to filter: a middle-click on a cell, `=` on the selection and the
//! cell menu's entry narrow the WHERE filter to the cells' values.

use super::filter_history::{ReadLog, filter_text, hosted_table, reads, register_filter_stub};
use super::*;
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_components::components::data_table::DataTableEvent;
use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_core::SqlDialect as _;

/// `id` (integer), `method` (text) and `note` (text, NULL in the first
/// row), over three rows of which two share their method.
fn rows() -> QueryResult {
    let column = |name: &str, type_name: &str, kind: ColumnKind| ColumnMeta {
        name: name.to_string(),
        type_name: type_name.to_string(),
        kind,
        nullable: true,
        is_primary_key: name == "id",
    };
    let row = |id: i64, method: &str, note: Option<&str>| {
        vec![
            dbflux_core::Value::Int(id),
            dbflux_core::Value::Text(method.to_string()),
            note.map_or(dbflux_core::Value::Null, |note| {
                dbflux_core::Value::Text(note.to_string())
            }),
        ]
    };

    QueryResult::table(
        vec![
            column("id", "int4", ColumnKind::Integer),
            column("method", "varchar", ColumnKind::Text),
            column("note", "text", ColumnKind::Text),
        ],
        vec![
            row(1, "getbets", None),
            row(2, "putbet", Some("x")),
            row(3, "getbets", Some("y")),
        ],
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

fn table_state(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> gpui::Entity<dbflux_components::components::data_table::DataTableState> {
    window.update(|_, cx| {
        panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("the result builds a table")
    })
}

/// What the table reports for a middle-click on the cell at `row`, `col`
/// (with Shift when `exclude`): see the table's own middle-click test.
fn middle_click(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    (row, col): (usize, usize),
    exclude: bool,
) {
    let table_state = table_state(panel, window);
    window.update(|_, cx| {
        table_state.update(cx, |state, cx| {
            state.select_cell(CellCoord::new(row, col), cx);
            cx.emit(DataTableEvent::AddToFilterRequested { row, col, exclude });
        });
    });
    window.run_until_parked();
}

/// Selects the cells from `first` to `last` and puts the keyboard on the
/// table.
fn select(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    first: (usize, usize),
    last: (usize, usize),
) {
    let table_state = table_state(panel, window);
    window.update(|window, cx| {
        table_state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_cell(CellCoord::new(first.0, first.1), cx);
            state.extend_selection(CellCoord::new(last.0, last.1), cx);
        });
    });
    window.run_until_parked();
}

fn keys(window: &mut VisualTestContext, keystrokes: &str) {
    for keystroke in keystrokes.split(' ') {
        window.simulate_keystrokes(keystroke);
        window.run_until_parked();
    }
}

#[gpui::test]
fn a_middle_click_on_a_text_cell_filters_by_it_with_one_read(cx: &mut TestAppContext) {
    let (panel, window, log) = grid_with_filter(cx, "");
    let before = reads(&log).len();

    middle_click(&panel, window, (0, 1), false);

    let expected = format!("{} = 'getbets'", quoted("method"));
    assert_eq!(filter_text(&panel, window), expected);
    assert_eq!(reads(&log)[before..], [Some(expected)], "one read");
}

#[gpui::test]
fn a_shift_middle_click_excludes_the_value(cx: &mut TestAppContext) {
    let (panel, window, _log) = grid_with_filter(cx, "");

    middle_click(&panel, window, (0, 1), true);
    assert_eq!(
        filter_text(&panel, window),
        format!("{} <> 'getbets'", quoted("method"))
    );
}

#[gpui::test]
fn a_shift_middle_click_on_null_adds_is_not_null(cx: &mut TestAppContext) {
    let (panel, window, _log) = grid_with_filter(cx, "");

    middle_click(&panel, window, (0, 2), true);
    assert_eq!(
        filter_text(&panel, window),
        format!("{} IS NOT NULL", quoted("note"))
    );
}

#[gpui::test]
fn a_middle_click_adds_to_an_existing_filter(cx: &mut TestAppContext) {
    let (panel, window, log) = grid_with_filter(cx, "status = 500");
    let before = reads(&log).len();

    middle_click(&panel, window, (0, 1), false);

    let expected = format!("status = 500 AND {} = 'getbets'", quoted("method"));
    assert_eq!(filter_text(&panel, window), expected);
    assert_eq!(reads(&log)[before..], [Some(expected)], "one read");
}

#[gpui::test]
fn a_middle_click_on_a_null_cell_adds_is_null(cx: &mut TestAppContext) {
    let (panel, window, _log) = grid_with_filter(cx, "");

    middle_click(&panel, window, (0, 2), false);

    assert_eq!(
        filter_text(&panel, window),
        format!("{} IS NULL", quoted("note"))
    );
}

#[gpui::test]
fn a_filter_with_a_top_level_or_is_parenthesized(cx: &mut TestAppContext) {
    let (panel, window, _log) = grid_with_filter(cx, "status = 500 OR status = 404");

    middle_click(&panel, window, (0, 1), false);

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

    middle_click(&panel, window, (0, 1), false);

    assert_eq!(filter_text(&panel, window), filter);
    assert_eq!(reads(&log).len(), before, "nothing new to read");
}

/// `=` on cells of one column filters by all their values, each once.
#[gpui::test]
fn equals_on_cells_of_one_column_builds_an_in_list(cx: &mut TestAppContext) {
    let (panel, window, log) = grid_with_filter(cx, "");
    let before = reads(&log).len();

    select(&panel, window, (0, 1), (2, 1));
    keys(window, "=");

    let expected = format!("{} IN ('getbets', 'putbet')", quoted("method"));
    assert_eq!(filter_text(&panel, window), expected);
    assert_eq!(reads(&log)[before..], [Some(expected)], "one read");
}

/// `=` on cells of two columns adds one condition per column, joined with
/// AND.
#[gpui::test]
fn equals_on_two_columns_ands_one_condition_each(cx: &mut TestAppContext) {
    let (panel, window, _log) = grid_with_filter(cx, "");

    select(&panel, window, (1, 0), (1, 1));
    keys(window, "=");

    assert_eq!(
        filter_text(&panel, window),
        format!("{} = 2 AND {} = 'putbet'", quoted("id"), quoted("method"))
    );
}

/// The cell menu's Add to filter, reached with the menu keys, adds the cell
/// under the menu as `=` does.
#[gpui::test]
fn the_menus_add_to_filter_adds_the_cell(cx: &mut TestAppContext) {
    let (panel, window, _log) = grid_with_filter(cx, "");

    select(&panel, window, (1, 1), (1, 1));
    // Copy, Copy as, View value, Add to filter.
    keys(window, "m j j j enter");

    assert_eq!(
        filter_text(&panel, window),
        format!("{} = 'putbet'", quoted("method"))
    );
    assert!(window.update(|_, cx| panel.read(cx).context_menu.is_none()));
}
