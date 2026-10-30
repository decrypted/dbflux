use super::selection_totals::{amounts_grid, with_table};
use dbflux_components::components::data_table::selection::CellCoord;
use gpui::{Modifiers, TestAppContext};

#[gpui::test]
fn the_footer_says_how_many_cells_an_edit_writes(cx: &mut TestAppContext) {
    let (panel, window) = amounts_grid(cx);
    with_table(&panel, window, |state, cx| {
        // A result grid is read-only until it knows its key.
        state.set_pk_columns(vec![1]);
        state.click_column(0, Modifiers::default(), cx);
    });

    window.update(|window, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("the result builds a table");
        table.update(cx, |state, cx| {
            assert!(state.start_editing(CellCoord::new(3, 0), window, cx));
        });
    });
    window.run_until_parked();

    let note = window.update(|_, cx| panel.read(cx).multi_edit_footer(cx));
    assert_eq!(note.as_deref(), Some("Editing 4 cells"));
    assert!(
        window.debug_bounds("footer-multi-edit").is_some(),
        "the footer draws the note"
    );

    with_table(&panel, window, |state, cx| state.stop_editing(false, cx));
    let note = window.update(|_, cx| panel.read(cx).multi_edit_footer(cx));
    assert_eq!(note, None, "the note goes with the editor");
}

/// A three-row `users` table (`id` key, `name` text) under the app keymap,
/// with the `name` column selected and the table focused.
fn keyboard_grid(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<crate::data_grid_panel::DataGridPanel>,
    &mut gpui::VisualTestContext,
) {
    use crate::data_grid_panel::{DataGridPanel, DataSource};
    use crate::keyboard_test_support::{host_document, init_keyboard_runtime};
    use dbflux_core::{ColumnKind, ColumnMeta, Pagination, QueryResult, TableRef, Value};
    use gpui::AppContext as _;

    init_keyboard_runtime(cx);
    let app_state = super::isolated_test_app_state(cx);

    let (host, window) = host_document(
        cx,
        move |window, cx| {
            cx.new(|cx| {
                let source = DataSource::Table {
                    profile_id: uuid::Uuid::nil(),
                    database: Some("app".to_string()),
                    table: TableRef::with_schema("public", "users"),
                    pagination: Pagination::default(),
                    order_by: Vec::new(),
                    total_rows: Some(3),
                };
                let mut panel = DataGridPanel::new_internal(
                    source,
                    app_state.clone(),
                    vec!["id".to_string()],
                    window,
                    cx,
                );
                let column = |name: &str, type_name: &str, kind, is_primary_key| ColumnMeta {
                    name: name.to_string(),
                    type_name: type_name.to_string(),
                    kind,
                    nullable: false,
                    is_primary_key,
                };
                panel.set_result(
                    QueryResult::table(
                        vec![
                            column("id", "int4", ColumnKind::Integer, true),
                            column("name", "text", ColumnKind::Text, false),
                        ],
                        (1..=3)
                            .map(|id| vec![Value::Int(id), Value::Text(format!("user {id}"))])
                            .collect(),
                        None,
                        std::time::Duration::ZERO,
                    ),
                    cx,
                );
                panel
            })
        },
        |panel, cx| panel.active_context(cx),
        DataGridPanel::dispatch_command,
    );
    let panel = window.update(|_, cx| host.read(cx).document.clone());

    window.update(|window, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("the result builds a table");
        table.update(cx, |state, cx| {
            state.set_pk_columns(vec![0]);
            state.click_column(1, Modifiers::default(), cx);
            state.focus(window, cx);
        });
    });
    window.run_until_parked();

    (panel, window)
}

/// Opens the editor with `key` through the app keymap and returns the
/// footer's multi-edit note.
fn note_after_key(cx: &mut TestAppContext, key: &str) -> Option<String> {
    let (panel, window) = keyboard_grid(cx);
    window.simulate_keystrokes(key);
    window.run_until_parked();
    window.update(|_, cx| panel.read(cx).multi_edit_footer(cx))
}

/// Enter reaches the grid through the app keymap and opens one edit for the
/// whole selected column, not just its active cell.
#[gpui::test]
fn enter_opens_an_edit_of_the_whole_selection(cx: &mut TestAppContext) {
    assert_eq!(
        note_after_key(cx, "enter").as_deref(),
        Some("Editing 3 cells")
    );
}

/// F2 does the same as Enter.
#[gpui::test]
fn f2_opens_an_edit_of_the_whole_selection(cx: &mut TestAppContext) {
    assert_eq!(note_after_key(cx, "f2").as_deref(), Some("Editing 3 cells"));
}
