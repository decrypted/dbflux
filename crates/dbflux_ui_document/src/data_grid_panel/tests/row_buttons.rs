//! The edit bar's `+` and `−`: add a row, and mark every row with a
//! selected cell for delete.

use super::filter_history::{hosted_table, register_filter_stub};
use super::*;
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_components::accessibility::{FrameCapture, accessibility_node};
use dbflux_components::components::data_table::selection::CellCoord;
use gpui::Entity;
use std::sync::Arc;

/// `id` and `method` over three rows.
fn three_rows() -> QueryResult {
    let column = |name: &str, is_primary_key: bool| ColumnMeta {
        name: name.to_string(),
        type_name: "varchar".to_string(),
        kind: ColumnKind::Text,
        nullable: true,
        is_primary_key,
    };

    QueryResult::table(
        vec![column("id", true), column("method", false)],
        (1..=3)
            .map(|id| {
                vec![
                    dbflux_core::Value::Text(id.to_string()),
                    dbflux_core::Value::Text(format!("method-{id}")),
                ]
            })
            .collect(),
        None,
        Duration::ZERO,
    )
}

fn editable_table(
    cx: &mut TestAppContext,
) -> (
    Entity<DataGridPanel>,
    &mut VisualTestContext,
    Arc<FrameCapture>,
) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let (profile_id, _log) =
        cx.update(|cx| register_filter_stub(&app_state, three_rows(), None, cx));
    let (_host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");
    window.update(|_, cx| panel.update(cx, |panel, cx| panel.set_result(three_rows(), cx)));
    window.run_until_parked();
    let capture = FrameCapture::observe(window);
    (panel, window, capture)
}

fn table_state(
    panel: &Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> Entity<dbflux_components::components::data_table::DataTableState> {
    window.update(|_, cx| {
        panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("the result builds a table")
    })
}

/// Whether the button `id` is drawn disabled, `None` when it is not drawn.
fn disabled(capture: &FrameCapture, window: &mut VisualTestContext, id: &str) -> Option<bool> {
    let frame = capture.frame(window);
    accessibility_node(&frame, id).map(|node| node.is_disabled())
}

fn click(capture: &FrameCapture, window: &mut VisualTestContext, id: &str) {
    let frame = capture.frame(window);
    let bounds = frame
        .nodes()
        .find(|(_, node)| node.id() == id)
        .map(|(_, node)| node.bounds())
        .unwrap_or_else(|| panic!("{id} is drawn"));
    window.simulate_click(bounds.center(), gpui::Modifiers::default());
    window.run_until_parked();
}

fn pending(panel: &Entity<DataGridPanel>, window: &mut VisualTestContext) -> (usize, usize) {
    let table_state = table_state(panel, window);
    window.update(|_, cx| {
        let buffer = table_state.read(cx).edit_buffer();
        (
            buffer.pending_insert_rows().len(),
            buffer.pending_delete_rows().len(),
        )
    })
}

#[gpui::test]
fn plus_adds_a_pending_insert_row(cx: &mut TestAppContext) {
    let (panel, window, capture) = editable_table(cx);
    assert_eq!(disabled(&capture, window, "add-row-btn"), Some(false));

    click(&capture, window, "add-row-btn");

    assert_eq!(
        pending(&panel, window),
        (1, 0),
        "one new row, nothing deleted"
    );
}

/// `−` marks every row that holds a selected cell, not just the active one,
/// and leaves the rows outside the selection alone.
#[gpui::test]
fn minus_marks_every_selected_row_for_delete(cx: &mut TestAppContext) {
    let (panel, window, capture) = editable_table(cx);
    let table_state = table_state(&panel, window);
    window.update(|_, cx| {
        table_state.update(cx, |state, cx| {
            state.select_cell(CellCoord::new(0, 1), cx);
            state.click_cell(CellCoord::new(2, 0), gpui::Modifiers::secondary_key(), cx);
        });
    });
    window.run_until_parked();
    assert_eq!(disabled(&capture, window, "delete-rows-btn"), Some(false));

    click(&capture, window, "delete-rows-btn");

    let marked = window.update(|_, cx| {
        let mut rows = table_state.read(cx).edit_buffer().pending_delete_rows();
        rows.sort_unstable();
        rows
    });
    assert_eq!(marked, vec![0, 2]);
}

/// With no cell selected `−` has no rows to act on and is drawn disabled;
/// `+` still adds.
#[gpui::test]
fn minus_is_disabled_without_a_selection(cx: &mut TestAppContext) {
    let (panel, window, capture) = editable_table(cx);
    let table_state = table_state(&panel, window);
    window.update(|_, cx| table_state.update(cx, |state, cx| state.clear_selection(cx)));
    window.run_until_parked();

    assert_eq!(disabled(&capture, window, "delete-rows-btn"), Some(true));
    assert_eq!(disabled(&capture, window, "add-row-btn"), Some(false));

    click(&capture, window, "delete-rows-btn");
    assert_eq!(
        pending(&panel, window),
        (0, 0),
        "a disabled button does nothing"
    );
}

/// A read-only result draws no edit bar, so neither button.
#[gpui::test]
fn a_read_only_result_has_no_row_buttons(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let (_host, _panel, window) = host_result_grid(cx, app_state);
    let capture = FrameCapture::observe(window);

    assert_eq!(disabled(&capture, window, "add-row-btn"), None);
    assert_eq!(disabled(&capture, window, "delete-rows-btn"), None);
}

/// The Delete key marks the selected rows too, so it is the keyboard path
/// of `−`.
#[gpui::test]
fn the_delete_key_marks_every_selected_row(cx: &mut TestAppContext) {
    let (panel, window, _capture) = editable_table(cx);
    let table_state = table_state(&panel, window);
    window.update(|window, cx| {
        table_state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_cell(CellCoord::new(0, 0), cx);
            state.extend_selection(CellCoord::new(1, 1), cx);
        });
    });
    window.run_until_parked();

    window.simulate_keystrokes("delete");
    window.run_until_parked();

    assert_eq!(pending(&panel, window), (0, 2));
}
