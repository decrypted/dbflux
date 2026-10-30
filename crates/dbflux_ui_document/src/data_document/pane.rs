//! `PaneHandle` constructor for `DataDocument`.
//!
//! `DataDocument::into_pane` converts a typed `Entity<DataDocument>` into the
//! type-erased `PaneHandle` shell. All closures capture the entity by clone;
//! `Window` and `App` are always passed as per-call parameters.

use super::DataDocument;
use crate::dedup::DocumentKey;
use crate::handle::DocumentEvent;
use crate::pane::{BoxedDocEventCallback, PaneHandle};
use crate::types::{DataSourceKind, DocumentIcon, DocumentKind, DocumentMetaSnapshot};
use gpui::{App, Entity, IntoElement};

impl DataDocument {
    /// Wrap a typed `Entity<DataDocument>` in a `PaneHandle`.
    ///
    /// Reads the document ID synchronously from `cx` then seals all operations
    /// behind `Box<dyn Fn>` closures capturing `entity` by clone.
    ///
    /// The `matches_dedup_key` closure handles both `Table` and `Collection`
    /// keys, mirroring the former `is_table_with_database` and `is_collection`
    /// predicates in `DocumentHandle`.
    pub fn into_pane(entity: Entity<Self>, cx: &App) -> PaneHandle {
        let id = entity.read(cx).id();

        let mut handle = PaneHandle::new_chart(
            id,
            DocumentKind::Data,
            // render
            {
                let e = entity.clone();
                Box::new(move |_w, _cx| e.clone().into_any_element())
            },
            // focus
            {
                let e = entity.clone();
                Box::new(move |w, cx| e.update(cx, |d, cx| d.focus(w, cx)))
            },
            // dispatch_command
            {
                let e = entity.clone();
                Box::new(move |cmd, w, cx| e.update(cx, |d, cx| d.dispatch_command(cmd, w, cx)))
            },
            // meta_snapshot
            {
                let e = entity.clone();
                Box::new(move |cx| {
                    let d = e.read(cx);
                    let icon = match d.source_kind() {
                        DataSourceKind::Table => DocumentIcon::Table,
                        DataSourceKind::Collection => DocumentIcon::Collection,
                        DataSourceKind::QueryResult => DocumentIcon::Table,
                    };
                    DocumentMetaSnapshot {
                        id,
                        kind: DocumentKind::Data,
                        title: d.title(),
                        icon,
                        state: d.state(),
                        closable: true,
                        connection_id: d.connection_id(cx),
                    }
                })
            },
            // tab_title
            {
                let e = entity.clone();
                Box::new(move |cx| e.read(cx).title())
            },
            // can_close
            {
                let e = entity.clone();
                Box::new(move |_cx| e.read(_cx).can_close())
            },
            // connection_id
            {
                let e = entity.clone();
                Box::new(move |cx| e.read(cx).connection_id(cx))
            },
            // active_context
            {
                let e = entity.clone();
                Box::new(move |cx| e.read(cx).active_context(cx))
            },
            // change_summary
            {
                let e = entity.clone();
                Box::new(move |cx| e.read(cx).change_summary(cx))
            },
            // refresh_policy
            {
                let e = entity.clone();
                Box::new(move |cx| e.read(cx).refresh_policy(cx))
            },
            // flush_auto_save — DataDocument has no auto-save
            Box::new(|_cx| {}),
            // set_active_tab
            {
                let e = entity.clone();
                Box::new(move |active, cx| e.update(cx, |d, cx| d.set_active_tab(active, cx)))
            },
            // set_refresh_policy
            {
                let e = entity.clone();
                Box::new(move |policy, cx| e.update(cx, |d, cx| d.set_refresh_policy(policy, cx)))
            },
            // matches_dedup_key — handles Table and Collection variants
            {
                let e = entity.clone();
                Box::new(move |key, cx| {
                    let d = e.read(cx);
                    match key {
                        DocumentKey::Table {
                            profile_id,
                            database,
                            table,
                        } => {
                            d.connection_id(cx) == Some(*profile_id)
                                && d.table_ref(cx).as_ref() == Some(table)
                                && (database.is_none()
                                    || d.database(cx).as_deref() == database.as_deref())
                        }
                        DocumentKey::Collection {
                            profile_id,
                            collection,
                        } => {
                            d.connection_id(cx) == Some(*profile_id)
                                && d.collection_ref(cx).as_ref() == Some(collection)
                        }
                        _ => false,
                    }
                })
            },
            // subscribe — DataDocument emits DocumentEvent directly
            {
                let e = entity.clone();
                Box::new(move |cx, cb: BoxedDocEventCallback| {
                    cx.subscribe(&e, move |_, ev: &DocumentEvent, cx| cb(ev, cx))
                })
            },
        );

        handle.mark_inspector_closed = Some({
            let e = entity.clone();
            Box::new(move |cx| {
                e.update(cx, |d, cx| d.mark_inspector_closed(cx));
            })
        });

        handle.row_inspector_is_tracking = Some({
            let e = entity.clone();
            Box::new(move |cx| e.read(cx).row_inspector_is_tracking(cx))
        });

        handle.set_row_inspector_tracking = Some({
            let e = entity.clone();
            Box::new(move |tracking, cx| {
                e.update(cx, |d, cx| d.set_row_inspector_tracking(tracking, cx));
            })
        });

        handle.value_panel_is_open = Some({
            let e = entity.clone();
            Box::new(move |cx| e.read(cx).value_panel_is_open(cx))
        });

        handle.set_value_panel_open = Some({
            let e = entity.clone();
            Box::new(move |open, cx| {
                e.update(cx, |d, cx| d.set_value_panel_open(open, cx));
            })
        });

        // The grid helpers below look the grid up on every call: a restored
        // tab replaces its grid once its connection is up, and a grid captured
        // here would be the placeholder.

        // Populate optional helper: applying the grid's staged edits for an
        // interrupted close. The tab closes only once every staged edit landed,
        // which the grid reports back as `DocumentEvent::RequestClose`.
        handle.apply_for_close = Some({
            let e = entity.clone();
            Box::new(move |_w, cx| {
                let grid = e.read(cx).data_grid.clone();
                grid.update(cx, |grid, cx| grid.apply_for_close(cx))
            })
        });

        handle.commit_pending_input = Some({
            let e = entity.clone();
            Box::new(move |cx| {
                let grid = e.read(cx).data_grid.clone();
                grid.update(cx, |grid, cx| grid.commit_pending_input(cx))
            })
        });

        handle.side_panels = Some({
            let e = entity.clone();
            Box::new(move |_window, cx| {
                let grid = e.read(cx).data_grid.clone();
                grid.update(cx, |grid, cx| grid.side_panels(cx))
            })
        });

        // Optional helper: positioning the table on one record from outside
        // the document (foreign-key navigation from the workspace).
        handle.apply_table_filter = Some({
            let e = entity.clone();
            Box::new(move |column, value, window, cx| {
                let grid = e.read(cx).data_grid.clone();
                grid.update(cx, |grid, cx| {
                    grid.focus_row_by_key(column, value, window, cx)
                })
            })
        });

        // Optional helper: whether an applied builder spec owns this tab's
        // query. The workspace reads it to keep such a tab's query and open
        // a referenced row beside it instead.
        handle.has_applied_builder_query = Some({
            let e = entity.clone();
            Box::new(move |cx| e.read(cx).data_grid.read(cx).builder_query_applied())
        });

        // Table and collection tabs reopen with the session; query results
        // do not (the snapshot is `None` for them).
        handle.session_tab_snapshot = Some({
            let e = entity.clone();
            Box::new(move |cx| e.read(cx).session_tab_snapshot(cx))
        });

        handle
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports rather than a glob: combining one with `#[gpui::test]`
    // sends the macro expansion into unbounded recursion.
    use super::*;
    use crate::code::CodeDocument;
    use crate::data_document::DataDocument;
    use dbflux_components::theme;
    use dbflux_core::{Pagination, QueryLanguage, TableRef};
    use dbflux_storage::bootstrap::StorageRuntime;
    use dbflux_ui_base::AppStateEntity;
    use dbflux_ui_base::toast::{ToastGlobal, ToastHost};
    use gpui::{AppContext, TestAppContext};
    use gpui_component::Root;
    use std::cell::RefCell;
    use std::rc::Rc;
    use uuid::Uuid;

    fn init_test_runtime(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        cx.update(theme::init);
        cx.update(|cx| {
            let host = cx.new(|_cx| ToastHost::new());
            cx.set_global(ToastGlobal { host });
        });
    }

    fn isolated_test_app_state(cx: &mut TestAppContext) -> gpui::Entity<AppStateEntity> {
        cx.update(|cx| {
            cx.new(|_| {
                let storage_runtime =
                    StorageRuntime::in_memory().expect("isolated storage runtime");
                AppStateEntity::new_with_storage_runtime(storage_runtime)
                    .expect("test storage setup")
            })
        })
    }

    /// A table tab's pending edits are applied to the database, so the dialog
    /// that asks about them says so — and, with nothing staged, the pane reports
    /// that there is nothing to wait on.
    #[gpui::test]
    fn a_table_pane_applies_its_pending_edits(cx: &mut TestAppContext) {
        init_test_runtime(cx);

        let app_state = isolated_test_app_state(cx);
        let holder: Rc<RefCell<Option<PaneHandle>>> = Rc::new(RefCell::new(None));
        let handle = holder.clone();

        let (_, window) = cx.add_window_view(|window, cx| {
            let document = cx.new(|cx| {
                DataDocument::new_for_table(
                    Uuid::nil(),
                    TableRef::with_schema("public", "orders"),
                    Some("app".to_string()),
                    app_state.clone(),
                    window,
                    cx,
                )
            });

            handle.replace(Some(DataDocument::into_pane(document.clone(), cx)));
            Root::new(document, window, cx)
        });

        let pane = holder.borrow_mut().take().expect("the pane is built");

        assert_eq!(
            pane.close_action(),
            dbflux_components::modals::CloseAction::Apply,
            "a grid's pending edits are applied, not saved to a file"
        );
        assert!(
            !window.update(|window, cx| pane.apply_for_close(window, cx)),
            "a grid with nothing staged has no apply to wait on"
        );
    }

    /// A table pane applies a filter handed to it from outside the document:
    /// the pane helper forwards to the grid's key-focus, which rewrites the
    /// filter as a dialect-quoted equality and reloads under it.
    #[gpui::test]
    fn a_table_pane_applies_an_outside_filter(cx: &mut TestAppContext) {
        init_test_runtime(cx);

        let driver = dbflux_test_support::fake_driver::FakeDriver::new(dbflux_core::DbKind::SQLite);
        let (app_state, profile_id) =
            crate::keyboard_test_support::connected_app_state(cx, &driver, "test");
        let holder: Rc<RefCell<Option<PaneHandle>>> = Rc::new(RefCell::new(None));
        let handle = holder.clone();

        let (_, window) = cx.add_window_view(|window, cx| {
            let document = cx.new(|cx| {
                DataDocument::new_for_table(
                    profile_id,
                    TableRef::with_schema("public", "orders"),
                    Some("app".to_string()),
                    app_state.clone(),
                    window,
                    cx,
                )
            });

            handle.replace(Some(DataDocument::into_pane(document.clone(), cx)));
            Root::new(document, window, cx)
        });

        let pane = holder.borrow_mut().take().expect("the pane is built");
        assert!(
            pane.apply_table_filter.is_some(),
            "a table pane must accept an outside filter"
        );

        window.run_until_parked();
        window.update(|window, cx| {
            pane.apply_table_filter("customer_id", &dbflux_core::Value::Int(8), window, cx);
        });
        window.run_until_parked();

        let stats = driver.stats();
        let executed_sql: Vec<&str> = stats
            .executed_requests
            .iter()
            .map(|request| request.sql.as_str())
            .collect();
        assert!(
            executed_sql
                .iter()
                .any(|sql| sql.contains("\"customer_id\" = 8")),
            "the reload must run under the dialect-quoted equality, got {executed_sql:?}"
        );
    }

    /// A code document's pending edits are written to its own file, and the
    /// dialog keeps saying so.
    #[gpui::test]
    fn a_code_pane_saves_its_pending_edits(cx: &mut TestAppContext) {
        init_test_runtime(cx);

        let app_state = isolated_test_app_state(cx);
        let holder: Rc<RefCell<Option<PaneHandle>>> = Rc::new(RefCell::new(None));
        let handle = holder.clone();

        let (_, window) = cx.add_window_view(|window, cx| {
            let document = cx.new(|cx| {
                CodeDocument::new_with_language(
                    app_state.clone(),
                    None,
                    QueryLanguage::Sql,
                    window,
                    cx,
                )
            });

            handle.replace(Some(CodeDocument::into_pane(document.clone(), cx)));
            Root::new(document, window, cx)
        });

        let pane = holder.borrow_mut().take().expect("the pane is built");

        assert_eq!(
            pane.close_action(),
            dbflux_components::modals::CloseAction::Save,
            "a script writes its own file, so its verb is save"
        );
        assert!(
            !window.update(|window, cx| pane.apply_for_close(window, cx)),
            "a script has no staged edits to apply"
        );
    }

    /// A pane that sets no input hook keeps today's close: committing reports
    /// success and leaves its pending changes as they were.
    #[gpui::test]
    fn a_pane_without_an_input_hook_commits_nothing(cx: &mut TestAppContext) {
        init_test_runtime(cx);

        let app_state = isolated_test_app_state(cx);
        let holder: Rc<RefCell<Option<PaneHandle>>> = Rc::new(RefCell::new(None));
        let handle = holder.clone();

        let (_, window) = cx.add_window_view(|window, cx| {
            let document = cx.new(|cx| {
                CodeDocument::new_with_language(
                    app_state.clone(),
                    None,
                    QueryLanguage::Sql,
                    window,
                    cx,
                )
            });

            handle.replace(Some(CodeDocument::into_pane(document.clone(), cx)));
            Root::new(document, window, cx)
        });

        let pane = holder.borrow_mut().take().expect("the pane is built");

        assert!(pane.commit_pending_input.is_none());
        assert!(window.update(|_, cx| pane.commit_pending_input(cx)));
        assert_eq!(window.update(|_, cx| pane.change_summary(cx)), None);
    }

    /// A table document with one editable row, rendered, with its pane and
    /// grid state.
    fn editable_table_document(
        cx: &mut TestAppContext,
    ) -> (
        &mut gpui::VisualTestContext,
        PaneHandle,
        Entity<dbflux_components::components::data_table::DataTableState>,
    ) {
        use dbflux_core::{ColumnKind, ColumnMeta, QueryResult, Value};

        init_test_runtime(cx);

        let app_state = isolated_test_app_state(cx);
        type Built = (PaneHandle, Entity<DataDocument>);

        let holder: Rc<RefCell<Option<Built>>> = Rc::new(RefCell::new(None));
        let handle = holder.clone();

        let (_, window) = cx.add_window_view(|window, cx| {
            let document = cx.new(|cx| {
                DataDocument::new_for_table(
                    Uuid::nil(),
                    TableRef::with_schema("public", "orders"),
                    Some("app".to_string()),
                    app_state.clone(),
                    window,
                    cx,
                )
            });

            handle.replace(Some((
                DataDocument::into_pane(document.clone(), cx),
                document.clone(),
            )));
            Root::new(document, window, cx)
        });
        let (pane, document) = holder.borrow_mut().take().expect("the pane is built");

        let columns = ["id", "note"]
            .iter()
            .map(|name| ColumnMeta {
                name: (*name).to_string(),
                type_name: "text".to_string(),
                kind: ColumnKind::Text,
                nullable: true,
                is_primary_key: *name == "id",
            })
            .collect();
        let rows = vec![vec![Value::Text("1".into()), Value::Text("a".into())]];
        window.update(|_, cx| {
            document
                .read(cx)
                .data_grid_for_tests()
                .clone()
                .update(cx, |grid, cx| {
                    grid.set_result(
                        QueryResult::table(columns, rows, None, std::time::Duration::ZERO),
                        cx,
                    );
                });
        });
        window.run_until_parked();

        let state = window.update(|_, cx| {
            document
                .read(cx)
                .data_grid_for_tests()
                .read(cx)
                .table_state_for_test()
                .expect("the result builds a table")
        });
        window.update(|_, cx| state.update(cx, |state, _| state.set_positional_editing(true)));

        (window, pane, state)
    }

    fn assert_cell_editor_focused(
        window: &mut gpui::VisualTestContext,
        state: &Entity<dbflux_components::components::data_table::DataTableState>,
    ) {
        use gpui::Focusable as _;

        window.update(|window, cx| {
            let input = state
                .read(cx)
                .cell_input()
                .cloned()
                .expect("the cell editor is open");
            assert!(
                input.read(cx).focus_handle(cx).is_focused(window),
                "the cell editor keeps the keyboard"
            );
        });
    }

    /// A double-click focuses the grid, which asks the workspace to focus the
    /// document, and then opens the cell editor. The workspace answers after
    /// the editor is open, so the document's focus must leave the keyboard in
    /// the editor; on the document's root it typed nothing until a second click.
    #[gpui::test]
    fn focusing_the_document_keeps_an_open_cell_editor_focused(cx: &mut TestAppContext) {
        use dbflux_components::components::data_table::selection::CellCoord;

        let (window, pane, state) = editable_table_document(cx);

        // What a double-click on a cell runs, then the workspace's answer to
        // the focus request it raised.
        window.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.focus(window, cx);
                assert!(state.start_editing(CellCoord::new(0, 1), window, cx));
            });
        });
        window.update(|window, cx| pane.focus(window, cx));
        window.run_until_parked();

        assert_cell_editor_focused(window, &state);
    }

    /// F2 and Enter reach the grid as Execute and open the editor of the
    /// selected cell with the keyboard in it.
    #[gpui::test]
    fn execute_opens_the_selected_cell_editor_focused(cx: &mut TestAppContext) {
        use dbflux_app::keymap::Command;
        use dbflux_components::components::data_table::selection::CellCoord;

        let (window, pane, state) = editable_table_document(cx);

        window.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.focus(window, cx);
                state.select_cell(CellCoord::new(0, 1), cx);
            });
        });
        window.run_until_parked();
        window.update(|window, cx| pane.focus(window, cx));
        window.run_until_parked();

        assert!(window.update(|window, cx| pane.dispatch_command(Command::Execute, window, cx)));
        window.run_until_parked();

        assert_cell_editor_focused(window, &state);
    }

    /// A table document with an enum `status` column, hosted under the app
    /// keymap like the workspace hosts it, with its pane and grid state.
    fn enum_table_document(
        cx: &mut TestAppContext,
    ) -> (
        &mut gpui::VisualTestContext,
        PaneHandle,
        Entity<dbflux_components::components::data_table::DataTableState>,
    ) {
        use crate::keyboard_test_support::{host_document, init_keyboard_runtime};
        use dbflux_core::{ColumnKind, ColumnMeta, QueryResult, Value};

        init_keyboard_runtime(cx);
        let app_state = isolated_test_app_state(cx);

        let (host, window) = host_document(
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    DataDocument::new_for_table(
                        Uuid::nil(),
                        TableRef::with_schema("public", "orders"),
                        Some("app".to_string()),
                        app_state.clone(),
                        window,
                        cx,
                    )
                })
            },
            |document, cx| document.active_context(cx),
            DataDocument::dispatch_command,
        );
        let document = window.update(|_, cx| host.read(cx).document.clone());
        let pane = window.update(|_, cx| DataDocument::into_pane(document.clone(), cx));

        let columns = ["id", "status"]
            .iter()
            .map(|name| ColumnMeta {
                name: (*name).to_string(),
                type_name: "text".to_string(),
                kind: ColumnKind::Text,
                nullable: true,
                is_primary_key: *name == "id",
            })
            .collect();
        let rows = (1..=2)
            .map(|id| vec![Value::Text(id.to_string()), Value::Text("open".into())])
            .collect();
        window.update(|_, cx| {
            document
                .read(cx)
                .data_grid_for_tests()
                .clone()
                .update(cx, |grid, cx| {
                    grid.set_result(
                        QueryResult::table(columns, rows, None, std::time::Duration::ZERO),
                        cx,
                    );
                });
        });
        window.run_until_parked();

        let state = window.update(|_, cx| {
            document
                .read(cx)
                .data_grid_for_tests()
                .read(cx)
                .table_state_for_test()
                .expect("the result builds a table")
        });
        window.update(|_, cx| {
            state.update(cx, |state, _| {
                state.set_positional_editing(true);
                state.set_enum_options(1, vec!["open".to_string(), "closed".to_string()]);
            })
        });

        (window, pane, state)
    }

    /// What a double-click on `coord` runs (the grid takes focus, then the
    /// editor opens), then the workspace's answer to the focus request.
    fn double_click_then_focus_the_pane(
        window: &mut gpui::VisualTestContext,
        pane: &PaneHandle,
        state: &Entity<dbflux_components::components::data_table::DataTableState>,
        coord: dbflux_components::components::data_table::selection::CellCoord,
    ) {
        window.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.focus(window, cx);
                state.select_cell(coord, cx);
                assert!(state.start_editing(coord, window, cx));
                assert!(state.is_editing_enum(), "an enum cell opens the dropdown");
            });
        });
        window.update(|window, cx| pane.focus(window, cx));
        window.run_until_parked();
    }

    fn staged_status(
        window: &mut gpui::VisualTestContext,
        state: &Entity<dbflux_components::components::data_table::DataTableState>,
        row: usize,
    ) -> Option<String> {
        window.update(|_, cx| {
            let state = state.read(cx);
            let buffer = state.edit_buffer();
            buffer.is_cell_dirty(row, 1).then(|| {
                let base = dbflux_components::components::data_table::model::CellValue::null();
                buffer.get_cell(row, 1, &base).display_text().to_string()
            })
        })
    }

    /// An enum cell's editor is a dropdown with no input to focus. After the
    /// double-click path and the pane's focus, Down and Enter must still pick
    /// a value in the dropdown, not move the grid cursor or reopen the editor.
    #[gpui::test]
    fn an_enum_editor_opened_by_double_click_keeps_its_keys(cx: &mut TestAppContext) {
        use dbflux_components::components::data_table::selection::CellCoord;

        let (window, pane, state) = enum_table_document(cx);
        double_click_then_focus_the_pane(window, &pane, &state, CellCoord::new(0, 1));

        window.simulate_keystrokes("down");
        window.run_until_parked();
        window.update(|_, cx| {
            let state = state.read(cx);
            assert!(state.is_editing_enum(), "Down stays in the dropdown");
            assert_eq!(
                state.selection().active,
                Some(CellCoord::new(0, 1)),
                "Down does not move the grid cursor under the dropdown"
            );
        });

        window.simulate_keystrokes("enter");
        window.run_until_parked();
        assert!(
            !window.update(|_, cx| state.read(cx).is_editing()),
            "Enter closes the dropdown"
        );
        assert_eq!(
            staged_status(window, &state, 0).as_deref(),
            Some("closed"),
            "Enter picks the value Down highlighted"
        );
    }

    /// Escape closes an enum dropdown opened by double-click without staging
    /// anything, and leaves the grid selection alone.
    #[gpui::test]
    fn escape_cancels_an_enum_editor_opened_by_double_click(cx: &mut TestAppContext) {
        use dbflux_components::components::data_table::selection::CellCoord;

        let (window, pane, state) = enum_table_document(cx);
        double_click_then_focus_the_pane(window, &pane, &state, CellCoord::new(1, 1));

        window.simulate_keystrokes("escape");
        window.run_until_parked();

        window.update(|_, cx| {
            let state = state.read(cx);
            assert!(!state.is_editing(), "Escape closes the dropdown");
            assert_eq!(
                state.selection().active,
                Some(CellCoord::new(1, 1)),
                "Escape cancels the edit rather than clearing the selection"
            );
        });
        assert_eq!(staged_status(window, &state, 1), None);
    }
}
