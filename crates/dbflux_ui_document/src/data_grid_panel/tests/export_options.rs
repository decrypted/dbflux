//! The Rows, Columns and CSV options of the export menu, driven the way a
//! user drives them: Ctrl+E, the menu keys, then a format.

use super::host_result_grid;
use crate::data_grid_panel::DataGridPanel;
use crate::data_grid_panel::context_menu::ExportMenuEntry;
use crate::data_grid_panel::export_options::{
    ExportColumns, ExportDelimiter, ExportOptionRow, ExportOptions, ExportRows,
};
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_components::components::data_table::DataTableState;
use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_storage::bootstrap::StorageRuntime;
use dbflux_ui_base::AppStateEntity;
use dbflux_ui_base::keyboard_coverage::FrameCapture;
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

fn app_state(cx: &mut TestAppContext) -> Entity<AppStateEntity> {
    cx.update(|cx| {
        cx.new(|_| {
            AppStateEntity::new_with_storage_runtime(
                StorageRuntime::in_memory().expect("isolated storage runtime"),
            )
            .expect("test storage setup")
        })
    })
}

fn keys(window: &mut VisualTestContext, keystrokes: &str) {
    for keystroke in keystrokes.split(' ') {
        window.simulate_keystrokes(keystroke);
        window.update(|window, _| window.refresh());
        window.run_until_parked();
    }
}

fn table_state(
    panel: &Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> Entity<DataTableState> {
    window.update(|_, cx| {
        panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("a result grid has a table")
    })
}

fn clipboard(window: &mut VisualTestContext) -> String {
    window
        .update(|_, cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .unwrap_or_default()
}

fn options(panel: &Entity<DataGridPanel>, window: &mut VisualTestContext) -> ExportOptions {
    window.update(|_, cx| panel.read(cx).chrome.export_options)
}

fn selected_entry(
    panel: &Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> Option<ExportMenuEntry> {
    window.update(|_, cx| {
        let panel = panel.read(cx);
        panel
            .export_menu_entries()
            .get(panel.chrome.export_menu_selected)
            .copied()
    })
}

/// The grid `host_result_grid` builds is `id, name` over rows 1..=3. With
/// `name` moved first, rows 1 and 2 selected and the options set from the
/// keyboard to selected rows, selected columns and no header, Copy as CSV
/// writes exactly those cells, `name` first, and the toast says so.
#[gpui::test]
fn the_menu_keys_export_the_selected_cells_in_screen_order(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let state = app_state(cx);
    let (_host, panel, window) = host_result_grid(cx, state);
    let table = table_state(&panel, window);
    window.update(|_, cx| {
        table.update(cx, |state, cx| {
            assert!(state.move_column(1, 0, cx), "name moves before id");
            state.select_cell(CellCoord::new(0, 1), cx);
            state.extend_selection(CellCoord::new(1, 0), cx);
        })
    });

    keys(window, "ctrl-e");
    assert_eq!(
        selected_entry(&panel, window),
        Some(ExportMenuEntry::Save(dbflux_export::ExportFormat::Csv)),
        "the menu opens on the first format, under the options"
    );

    // Up to Rows: Loaded steps to Selected (a query result has no All).
    keys(window, "k k k k");
    assert_eq!(
        selected_entry(&panel, window),
        Some(ExportMenuEntry::Option(ExportOptionRow::Rows))
    );
    keys(window, "enter");
    assert_eq!(options(&panel, window).rows, ExportRows::Selected);

    // Columns: Visible steps to Selected; then the header checkbox.
    keys(window, "j enter j enter");
    let chosen = options(&panel, window);
    assert_eq!(chosen.columns, ExportColumns::Selected);
    assert!(!chosen.csv_header, "Enter unticks the header row");
    assert!(
        window.update(|_, cx| panel.read(cx).chrome.export_menu_open),
        "an option leaves the menu open"
    );

    // Past the delimiter and the three save rows to Copy CSV.
    keys(window, "j j j j j enter");
    assert_eq!(clipboard(window), "row 1,1\nrow 2,2\n");
    assert_eq!(
        super::last_toast_title(window),
        Some(crate::labels::context_menu_clipboard_copied_scope_toast(
            "2 selected rows, 2 columns",
            "CSV",
            16,
        ))
    );
}

/// With nothing selected, the Selected segments are disabled, Enter steps
/// over them and a click cannot pick them.
#[gpui::test]
fn selected_rows_are_disabled_with_nothing_selected(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let state = app_state(cx);
    let (_host, panel, window) = host_result_grid(cx, state);
    let capture = FrameCapture::observe(window);
    let table = table_state(&panel, window);
    window.update(|_, cx| table.update(cx, |state, cx| state.clear_selection(cx)));

    keys(window, "ctrl-e");
    let frame = capture.frame(window);
    let node = |id: &str| {
        let (_, node) = frame
            .nodes()
            .find(|(_, node)| node.id() == id)
            .unwrap_or_else(|| panic!("{id} is drawn"));
        frame
            .accessibility_node(node)
            .unwrap_or_else(|| panic!("{id} is accessible"))
            .clone()
    };

    let selected_rows = node("segmented-export-rows-selected");
    assert!(selected_rows.is_disabled());
    assert!(!selected_rows.supports_action(gpui::AccessibleAction::Click));
    assert!(node("segmented-export-columns-selected").is_disabled());
    assert!(!node("segmented-export-rows-loaded").is_disabled());
    assert!(
        frame
            .nodes()
            .all(|(_, node)| node.id() != "segmented-export-rows-all"),
        "a query result cannot be read again, so All is not offered"
    );

    window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.set_export_rows(ExportRows::Selected, cx);
            panel.activate_export_option(ExportOptionRow::Rows, cx);
        })
    });
    assert_eq!(
        options(&panel, window).rows,
        ExportRows::Loaded,
        "neither a click nor Enter picks Selected rows"
    );

    // Copy CSV writes every loaded row.
    window.update(|_, cx| panel.update(cx, |panel, _| panel.chrome.export_menu_selected = 7));
    assert_eq!(
        selected_entry(&panel, window),
        Some(ExportMenuEntry::Copy(dbflux_export::ExportFormat::Csv))
    );
    keys(window, "enter");
    assert_eq!(clipboard(window), "id,name\n1,row 1\n2,row 2\n3,row 3\n");
}

/// Under Visible columns a hidden column is not written; under All columns
/// it is, in result order. The delimiter applies to the clipboard too.
#[gpui::test]
fn visible_columns_leave_hidden_ones_out(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let state = app_state(cx);
    let (_host, panel, window) = host_result_grid(cx, state);
    let table = table_state(&panel, window);
    window.update(|_, cx| table.update(cx, |state, cx| assert!(state.hide_column(0, cx))));

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.copy_to_clipboard_with_format(dbflux_export::ExportFormat::Csv, window, cx)
        })
    });
    assert_eq!(clipboard(window), "name\nrow 1\nrow 2\nrow 3\n");

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.set_export_columns(ExportColumns::All, cx);
            panel.set_export_delimiter(ExportDelimiter::Semicolon, cx);
            panel.copy_to_clipboard_with_format(dbflux_export::ExportFormat::Csv, window, cx)
        })
    });
    assert_eq!(clipboard(window), "id;name\n1;row 1\n2;row 2\n3;row 3\n");
}

/// The choices are the user's, not the grid's: another grid on the same
/// store opens its menu with them.
#[gpui::test]
fn the_choices_are_remembered_across_grids(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let state = app_state(cx);
    let (_host, first, window) = host_result_grid(cx, state.clone());

    keys(window, "ctrl-e k enter escape");
    let chosen = options(&first, window);
    assert_eq!(chosen.csv_delimiter, ExportDelimiter::Semicolon);

    let (_host, second, window) = host_result_grid(cx, state);
    assert_eq!(
        options(&second, window),
        ExportOptions::default(),
        "a new grid starts from the defaults"
    );
    keys(window, "ctrl-e");
    assert_eq!(
        options(&second, window),
        chosen,
        "opening the menu reads the store"
    );
}

/// Selected rows are written as the grid shows them, as Copy as CSV takes
/// them: a row added in the grid and not saved at its place, and an unsaved
/// edit with its new value, not the loaded row at the same index.
#[gpui::test]
fn selected_rows_export_the_rows_the_grid_shows(cx: &mut TestAppContext) {
    use dbflux_components::components::data_table::model::CellValue;
    use dbflux_core::Value;

    init_keyboard_runtime(cx);
    let state = app_state(cx);
    let (_host, panel, window) = host_result_grid(cx, state);
    let table = table_state(&panel, window);
    window.update(|_, cx| {
        let base_rows = panel.read(cx).result.rows.len();
        table.update(cx, |state, cx| {
            let buffer = state.edit_buffer_mut();
            buffer.set_base_row_count(base_rows);
            buffer.add_pending_insert_after(0, vec![CellValue::int(9), CellValue::text("new")]);
            buffer.set_cell(1, 1, CellValue::text("edited"));
            // Visual rows 1 and 2: the added row, then loaded row 2.
            state.select_cell(CellCoord::new(1, 0), cx);
            state.extend_selection(CellCoord::new(2, 1), cx);
        });
        panel.update(cx, |panel, _| {
            panel.chrome.export_options.rows = ExportRows::Selected;
            panel.chrome.export_options.columns = ExportColumns::All;
        });
    });

    let (result, _) = window.update(|_, cx| panel.read(cx).export_menu_payload(cx));
    assert_eq!(
        result.rows,
        vec![
            vec![Value::Int(9), Value::Text("new".to_string())],
            vec![Value::Int(2), Value::Text("edited".to_string())],
        ]
    );
}
