//! The column header menu's Column section and the keyboard path to it.

use super::*;
use crate::keyboard_test_support::{host_document, init_keyboard_runtime};
use dbflux_components::components::data_table::ContextMenuAction;

/// A query result with a short `id` column and a `note` column whose values
/// are longer than the width a column opens with.
fn host_wide_result(
    cx: &mut TestAppContext,
) -> (gpui::Entity<DataGridPanel>, &mut VisualTestContext) {
    let app_state = isolated_test_app_state(cx);
    let (host, window) = host_document(
        cx,
        move |window, cx| {
            cx.new(|cx| {
                DataGridPanel::new_for_result(
                    Arc::new(QueryResult::table(
                        vec![key_column("id", false), key_column("note", false)],
                        (1..=3)
                            .map(|id| {
                                vec![
                                    dbflux_core::Value::Int(id),
                                    dbflux_core::Value::Text("n".repeat(60)),
                                ]
                            })
                            .collect(),
                        None,
                        Duration::ZERO,
                    )),
                    "SELECT id, note FROM notes".to_string(),
                    None,
                    app_state,
                    window,
                    cx,
                )
            })
        },
        |panel, cx| panel.active_context(cx),
        DataGridPanel::dispatch_command,
    );
    let panel = window.update(|_, cx| host.read(cx).document.clone());

    window.update(|window, cx| {
        let table_state = table_state(&panel, cx);
        table_state.update(cx, |state, cx| {
            state.select_cell(
                dbflux_components::components::data_table::selection::CellCoord::new(0, 1),
                cx,
            );
        });
        let focus_handle = table_state.read(cx).focus_handle().clone();
        focus_handle.focus(window, cx);
    });
    window.run_until_parked();

    (panel, window)
}

fn table_state(
    panel: &gpui::Entity<DataGridPanel>,
    cx: &gpui::App,
) -> gpui::Entity<dbflux_components::components::data_table::DataTableState> {
    panel
        .read(cx)
        .grid_table
        .table_state
        .clone()
        .expect("the result builds a table")
}

/// `Shift+m` opens the column header menu of the active cell's column, and
/// its Fit width entry sizes that column to its values.
#[gpui::test]
fn shift_m_opens_the_column_menu_and_fit_width_fits_the_column(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let (panel, window) = host_wide_result(cx);

    window.simulate_keystrokes("shift-m");
    window.run_until_parked();

    let menu = window.update(|_, cx| {
        panel
            .read(cx)
            .context_menu
            .as_ref()
            .map(|menu| (menu.is_column_header, menu.col))
    });
    assert_eq!(menu, Some((true, 1)), "the menu of the `note` column opens");

    let fit_entry = window.update(|_, cx| {
        panel
            .read(cx)
            .column_menu_items(1, cx)
            .iter()
            .any(|item| item.action == ContextMenuAction::FitColumnWidth)
    });
    assert!(fit_entry, "the Column section offers Fit width");

    let before = window.update(|_, cx| table_state(&panel, cx).read(cx).column_widths()[1]);
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.handle_context_menu_action(ContextMenuAction::FitColumnWidth, window, cx);
        });
    });
    let after = window.update(|_, cx| table_state(&panel, cx).read(cx).column_widths()[1]);

    assert!(
        after > before,
        "the 60-character values fit: {before} -> {after}"
    );
    assert!(
        window.update(|_, cx| panel.read(cx).context_menu.is_none()),
        "the menu closes"
    );
}
