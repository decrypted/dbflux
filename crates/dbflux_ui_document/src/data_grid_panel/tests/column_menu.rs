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

/// Hide column takes the column off the screen without touching the
/// result, and the Columns chooser shows it again: its first row, Show all
/// columns, runs with Enter while the chooser stays open, and Escape closes
/// it.
#[gpui::test]
fn hide_column_and_the_columns_chooser_show_it_again(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let (panel, window) = host_wide_result(cx);

    window.simulate_keystrokes("shift-m");
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.handle_context_menu_action(ContextMenuAction::HideColumn, window, cx);
        });
    });
    let shown = |window: &mut VisualTestContext| {
        window.update(|_, cx| table_state(&panel, cx).read(cx).visible_columns().to_vec())
    };
    assert_eq!(shown(window), [0], "`note` is hidden");
    assert_eq!(
        window.update(|_, cx| panel.read(cx).result.columns.len()),
        2,
        "the result keeps both columns"
    );

    window.simulate_keystrokes("shift-m");
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.handle_context_menu_action(ContextMenuAction::ChooseColumns, window, cx);
        });
    });
    window.run_until_parked();
    assert!(window.update(|_, cx| panel.read(cx).column_chooser.is_some()));
    assert!(
        window.update(|_, cx| panel.read(cx).context_menu.is_none()),
        "the chooser replaces the menu"
    );

    window.simulate_keystrokes("enter");
    window.run_until_parked();
    assert_eq!(shown(window), [0, 1], "Show all columns brings `note` back");
    assert!(
        window.update(|_, cx| panel.read(cx).column_chooser.is_some()),
        "the chooser stays open"
    );

    window.simulate_keystrokes("escape");
    window.run_until_parked();
    assert!(window.update(|_, cx| panel.read(cx).column_chooser.is_none()));
}

/// A table panel over the `public.users` table of the profile `nil`.
fn table_panel(
    cx: &mut TestAppContext,
    app_state: gpui::Entity<AppStateEntity>,
) -> (gpui::Entity<DataGridPanel>, &mut VisualTestContext) {
    let (host, window) = host_document(
        cx,
        move |window, cx| {
            let source = DataSource::Table {
                profile_id: Uuid::nil(),
                database: Some("app".to_string()),
                table: TableRef::with_schema("public", "users"),
                pagination: Pagination::default(),
                order_by: Vec::new(),
                total_rows: Some(1),
            };
            cx.new(|cx| {
                let mut panel = DataGridPanel::new_internal(
                    source,
                    app_state,
                    vec!["id".to_string()],
                    window,
                    cx,
                );
                panel.set_result(
                    QueryResult::table(
                        vec![
                            key_column("id", true),
                            key_column("name", false),
                            key_column("email", false),
                        ],
                        vec![vec![
                            dbflux_core::Value::Int(1),
                            dbflux_core::Value::Text("ada".into()),
                            dbflux_core::Value::Text("ada@example.com".into()),
                        ]],
                        None,
                        Duration::ZERO,
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

    (panel, window)
}

/// A table's layout is kept per table: a second grid over the same table,
/// as after closing and reopening it or a restart, opens with it, and a
/// layout back to the table's own order removes the kept row.
#[gpui::test]
fn a_table_reopens_with_its_column_layout(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);

    let (first, window) = table_panel(cx, app_state.clone());
    let key = window.update(|_, cx| first.read(cx).column_layout_key().expect("a table key"));
    window.update(|_, cx| {
        table_state(&first, cx).update(cx, |state, cx| {
            state.hide_column(1, cx);
            state.move_column(2, 0, cx);
        });
    });
    window.run_until_parked();

    let (second, window) = table_panel(cx, app_state.clone());
    let shown = window.update(|_, cx| table_state(&second, cx).read(cx).visible_columns().to_vec());
    assert_eq!(shown, [2, 0], "email, id; name hidden");

    window.update(|_, cx| {
        table_state(&second, cx).update(cx, |state, cx| {
            state.reset_column_layout(cx);
        });
    });
    window.run_until_parked();
    let stored = window.update(|_, cx| {
        app_state
            .read(cx)
            .storage_runtime()
            .ui_state()
            .get(&key)
            .expect("the store reads")
    });
    assert_eq!(stored, None, "the default layout keeps no row");
}

/// Pin column from the column menu keeps the column at the left edge, the
/// menu then offers Unpin column, and a reopened table comes back with the
/// column pinned.
#[gpui::test]
fn pin_column_is_kept_with_the_table_layout(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);

    let (first, window) = table_panel(cx, app_state.clone());
    window.update(|window, cx| {
        let table_state = table_state(&first, cx);
        table_state.update(cx, |state, cx| {
            state.select_cell(
                dbflux_components::components::data_table::selection::CellCoord::new(0, 2),
                cx,
            );
        });
        let focus_handle = table_state.read(cx).focus_handle().clone();
        focus_handle.focus(window, cx);
    });
    window.run_until_parked();
    window.simulate_keystrokes("shift-m");
    window.update(|window, cx| {
        first.update(cx, |panel, cx| {
            panel.handle_context_menu_action(ContextMenuAction::PinColumn, window, cx);
        });
    });
    window.run_until_parked();

    let (shown, offers_unpin) = window.update(|_, cx| {
        let panel = first.read(cx);
        (
            table_state(&first, cx).read(cx).visible_columns().to_vec(),
            panel
                .column_menu_items(2, cx)
                .iter()
                .any(|item| item.action == ContextMenuAction::UnpinColumn),
        )
    });
    assert_eq!(shown, [2, 0, 1], "the pinned `email` leads the screen");
    assert!(offers_unpin, "the menu offers Unpin column");

    let (second, window) = table_panel(cx, app_state);
    let pinned = window.update(|_, cx| {
        let state = table_state(&second, cx);
        let state = state.read(cx);
        (
            state.visible_columns().to_vec(),
            state.column_layout().pinned_count(),
        )
    });
    assert_eq!(
        pinned,
        (vec![2, 0, 1], 1),
        "the reopened table keeps the pin"
    );
}
