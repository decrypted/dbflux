//! Filter changes are steps of the grid's Undo / Redo.

use super::filter_history::{
    ReadLog, apply_filter, filter_text, hosted_table, method_rows, reads, register_filter_stub,
};
use super::*;
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_components::components::data_table::actions;
use dbflux_components::components::data_table::model::CellValue;

/// A table grid read with `filters` in turn, the grid holding the keyboard.
/// `rows` is what every read returns.
fn grid_read_with<'a>(
    cx: &'a mut TestAppContext,
    rows: QueryResult,
    filters: &[&str],
) -> (
    gpui::Entity<DataGridPanel>,
    &'a mut VisualTestContext,
    ReadLog,
) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let (profile_id, log) = cx.update(|cx| register_filter_stub(&app_state, rows, None, cx));

    let (_host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");
    for filter in filters {
        apply_filter(&panel, window, filter);
    }

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

/// Undo or Redo as the grid's key sends it, to the focused table.
fn press<A: gpui::Action>(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    action: A,
) {
    let state = table_state(panel, window);
    window.update(|window, cx| {
        let focus = state.read(cx).focus_handle().clone();
        focus.focus(window, cx);
    });
    window.run_until_parked();
    window.dispatch_action(action);
    window.run_until_parked();
}

fn dirty_rows(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> usize {
    let state = table_state(panel, window);
    window.update(|_, cx| state.read(cx).edit_buffer().dirty_row_count())
}

fn edit_method(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext, text: &str) {
    let state = table_state(panel, window);
    window.update(|_, cx| {
        state.update(cx, |state, _| {
            state
                .edit_buffer_mut()
                .set_cell(0, 1, CellValue::text(text));
        });
    });
    window.run_until_parked();
}

#[gpui::test]
fn undo_reads_the_previous_filter_and_redo_the_next(cx: &mut TestAppContext) {
    let (panel, window, log) =
        grid_read_with(cx, method_rows(), &["", "status = 404", "status = 500"]);

    let before = reads(&log).len();
    press(&panel, window, actions::Undo);
    assert_eq!(filter_text(&panel, window), "status = 404");
    assert_eq!(
        reads(&log)[before..],
        [Some("status = 404".to_string())],
        "one read, with the previous filter"
    );

    press(&panel, window, actions::Undo);
    assert_eq!(filter_text(&panel, window), "");

    press(&panel, window, actions::Redo);
    press(&panel, window, actions::Redo);
    assert_eq!(filter_text(&panel, window), "status = 500");
    assert_eq!(reads(&log).last(), Some(&Some("status = 500".to_string())));
}

#[gpui::test]
fn undo_steps_back_through_edits_before_the_filter(cx: &mut TestAppContext) {
    let (panel, window, log) =
        grid_read_with(cx, method_rows(), &["", "status = 404", "status = 500"]);
    edit_method(&panel, window, "first");
    edit_method(&panel, window, "second");
    let before = reads(&log).len();

    press(&panel, window, actions::Undo);
    assert_eq!(
        dirty_rows(&panel, window),
        1,
        "the newest edit is undone first"
    );
    assert_eq!(filter_text(&panel, window), "status = 500");

    press(&panel, window, actions::Undo);
    assert_eq!(dirty_rows(&panel, window), 0, "then the older edit");
    assert_eq!(reads(&log).len(), before, "no read while edits are undone");

    press(&panel, window, actions::Undo);
    assert_eq!(
        filter_text(&panel, window),
        "status = 404",
        "then the filter"
    );
    assert_eq!(reads(&log)[before..], [Some("status = 404".to_string())]);
}

#[gpui::test]
fn a_filter_undo_never_drops_pending_edits(cx: &mut TestAppContext) {
    let (panel, window, log) =
        grid_read_with(cx, method_rows(), &["", "status = 404", "status = 500"]);
    edit_method(&panel, window, "kept");

    // While a save runs the host makes the table read-only: its own
    // history refuses, and the filter step would reload over the edit.
    let state = table_state(&panel, window);
    window.update(|_, cx| {
        state.update(cx, |state, _| {
            state.set_positional_editing(false);
            state.set_insertable(false);
        });
    });
    let before = reads(&log).len();

    press(&panel, window, actions::Undo);

    assert_eq!(dirty_rows(&panel, window), 1, "the edit is kept");
    assert_eq!(filter_text(&panel, window), "status = 500");
    assert_eq!(reads(&log).len(), before, "the filter step is refused");
}

#[gpui::test]
fn a_read_only_grid_still_undoes_its_filters(cx: &mut TestAppContext) {
    // Without the key column the rows cannot be edited.
    let read_only = QueryResult::table(
        vec![ColumnMeta {
            name: "method".to_string(),
            type_name: "varchar".to_string(),
            kind: ColumnKind::Text,
            nullable: true,
            is_primary_key: false,
        }],
        vec![vec![dbflux_core::Value::Text("getbets".to_string())]],
        None,
        Duration::ZERO,
    );
    let (panel, window, log) = grid_read_with(cx, read_only, &["", "status = 404", "status = 500"]);
    let state = table_state(&panel, window);
    assert!(!window.update(|_, cx| state.read(cx).is_editable()));

    press(&panel, window, actions::Undo);

    assert_eq!(filter_text(&panel, window), "status = 404");
    assert_eq!(reads(&log).last(), Some(&Some("status = 404".to_string())));
}

/// A multi-cell edit is one undo group: the first Undo takes back every cell
/// it wrote, and only the next one steps back through the filters.
#[gpui::test]
fn a_multi_cell_edit_is_one_step_before_the_filter(cx: &mut TestAppContext) {
    let (panel, window, log) = grid_read_with(cx, method_rows(), &["", "status = 404"]);
    let state = table_state(&panel, window);
    let dirty_cells = |window: &mut VisualTestContext| {
        window.update(|_, cx| {
            let buffer = state.read(cx).edit_buffer();
            (0..2).filter(|col| buffer.is_cell_dirty(0, *col)).count()
        })
    };
    window.update(|_, cx| {
        state.update(cx, |state, _| {
            let buffer = state.edit_buffer_mut();
            buffer.begin_undo_group();
            buffer.set_cell(0, 0, CellValue::text("same"));
            buffer.set_cell(0, 1, CellValue::text("same"));
            buffer.end_undo_group();
        });
    });
    window.run_until_parked();
    assert_eq!(dirty_cells(window), 2);
    let before = reads(&log).len();

    press(&panel, window, actions::Undo);
    assert_eq!(dirty_cells(window), 0, "the whole edit at once");
    assert_eq!(filter_text(&panel, window), "status = 404");
    assert_eq!(reads(&log).len(), before, "no read for the edit");

    press(&panel, window, actions::Undo);
    assert_eq!(filter_text(&panel, window), "", "then the filter");
}
