use super::*;
use dbflux_core::{OrderByColumn, SortDirection, Value};

fn order_names(order_by: &[OrderByColumn]) -> Vec<(String, SortDirection)> {
    order_by
        .iter()
        .map(|column| (column.column.name.clone(), column.direction))
        .collect()
}

/// A click on one sort arrow and an Alt+click on another sort a table by both
/// columns, in that order, in the query the page reloads with; the reloaded
/// headers then show both keys.
#[gpui::test]
fn alt_click_adds_a_second_order_by_key_to_a_table(cx: &mut TestAppContext) {
    init_test_runtime(cx);

    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let panel = table_panel(window, app_state);

    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| state.cycle_sort(1, cx));
    });
    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| {
            state.cycle_sort_key(2, cx);
            state.cycle_sort_key(2, cx);
        });
    });

    let expected = vec![
        ("name".to_string(), SortDirection::Ascending),
        ("email".to_string(), SortDirection::Descending),
    ];
    window.update(|_, app| {
        let panel = panel.read(app);
        let DataSource::Table { order_by, .. } = &panel.source else {
            panic!("a table source");
        };
        assert_eq!(order_names(order_by), expected);
        assert_eq!(
            order_names(
                &panel
                    .pending
                    .requery
                    .as_ref()
                    .expect("the sort queues a reload")
                    .order_by
            ),
            expected,
            "the reload asks for both keys"
        );
    });

    window.update(|_, app| {
        panel.update(app, |panel, cx| {
            panel.apply_table_result(
                Uuid::nil(),
                TableRef::with_schema("public", "users"),
                Pagination::default(),
                vec![
                    OrderByColumn::from_name("name", SortDirection::Ascending),
                    OrderByColumn::from_name("email", SortDirection::Descending),
                ],
                Some(2),
                reload_result(&["id", "name", "email"], 2),
                cx,
            );
        });
    });

    window.update(|_, app| {
        let keys = panel
            .read(app)
            .grid_table
            .table_state
            .as_ref()
            .expect("table state")
            .read(app)
            .sort_keys();
        assert_eq!(
            keys,
            vec![TableSortState::ascending(1), TableSortState::descending(2)],
            "both keys stay on the headers after the reload"
        );
    });
}

fn query_result_panel(
    cx: &mut TestAppContext,
) -> (gpui::Entity<DataGridPanel>, &mut VisualTestContext) {
    init_test_runtime(cx);

    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let result = QueryResult::table(
        vec![key_column("city", false), key_column("name", false)],
        [("Lyon", "b"), ("Oslo", "a"), ("Lyon", "c"), ("Oslo", "d")]
            .into_iter()
            .map(|(city, name)| vec![Value::Text(city.to_string()), Value::Text(name.to_string())])
            .collect(),
        None,
        Duration::ZERO,
    );

    let panel = window.update(|window, app| {
        let source = DataSource::QueryResult {
            result: Arc::new(result.clone()),
            original_query: "SELECT city, name FROM people".to_string(),
            profile_id: None,
        };
        let panel =
            app.new(|cx| DataGridPanel::new_internal(source, app_state, Vec::new(), window, cx));
        panel.update(app, |panel, cx| panel.set_result(result, cx));
        panel
    });

    (panel, window)
}

fn names_in_order(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> Vec<String> {
    window.update(|_, app| {
        panel
            .read(app)
            .result
            .rows
            .iter()
            .filter_map(|row| match row.get(1) {
                Some(Value::Text(name)) => Some(name.clone()),
                _ => None,
            })
            .collect()
    })
}

/// A query result sorts in memory by every key: rows tied on the first key
/// follow the second.
#[gpui::test]
fn a_query_result_sorts_in_memory_by_two_keys(cx: &mut TestAppContext) {
    let (panel, window) = query_result_panel(cx);

    window.update(|_, app| {
        panel.update(app, |panel, cx| {
            panel.apply_local_sort(0, SortDirection::Ascending, cx);
            panel.handle_add_sort_key(1, SortDirection::Descending, cx);
        });
    });

    assert_eq!(names_in_order(&panel, window), vec!["c", "b", "d", "a"]);
    let keys = window.update(|_, app| panel.read(app).local_sort_keys());
    assert_eq!(
        keys,
        vec![TableSortState::ascending(0), TableSortState::descending(1)]
    );

    window.update(|_, app| panel.update(app, |panel, cx| panel.handle_sort_clear(cx)));
    assert_eq!(
        names_in_order(&panel, window),
        vec!["b", "a", "c", "d"],
        "clearing the sort puts the rows back in the order they came in"
    );
}

fn header_sort_keys(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> Vec<TableSortState> {
    window.update(|_, app| {
        panel
            .read(app)
            .grid_table
            .table_state
            .as_ref()
            .expect("table state")
            .read(app)
            .sort_keys()
    })
}

fn stage_cell_edit(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) {
    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, _| {
            state.stage_base_cell_value(
                0,
                1,
                dbflux_components::components::data_table::model::CellValue::text("edited"),
            );
        });
    });
}

fn users_with_id_key() -> dbflux_core::TableInfo {
    let column = |name: &str, is_primary_key: bool| dbflux_core::ColumnInfo {
        name: name.to_string(),
        type_name: "text".to_string(),
        nullable: !is_primary_key,
        is_primary_key,
        default_value: None,
        enum_values: None,
    };

    dbflux_core::TableInfo {
        name: "users".to_string(),
        schema: Some("public".to_string()),
        columns: Some(vec![
            column("id", true),
            column("name", false),
            column("email", false),
        ]),
        indexes: None,
        foreign_keys: None,
        constraints: None,
        sample_fields: None,
        presentation: Default::default(),
        child_items: None,
        storage_hints: None,
        pseudo_columns: Box::default(),
    }
}

/// On a table still in its primary-key order, an Alt+click on another
/// column's arrow replaces that order: the reload sorts by the new column
/// alone and the headers show only it.
#[gpui::test]
fn alt_click_on_a_table_in_key_order_replaces_the_key_order(cx: &mut TestAppContext) {
    let (app_state, profile_id) = register_stub_connection(cx, Arc::new(StubConnection));
    cx.update(|cx| {
        app_state.update(cx, |app, _| {
            app.set_table_details(
                profile_id,
                "app".to_string(),
                Some("public".to_string()),
                "users".to_string(),
                users_with_id_key(),
            );
        });
    });

    let window = cx.add_empty_window();
    let panel = table_panel(window, app_state);
    window.update(|_, app| {
        panel.update(app, |panel, cx| {
            panel.apply_table_result(
                profile_id,
                TableRef::with_schema("public", "users"),
                Pagination::default(),
                vec![OrderByColumn::asc("id")],
                Some(2),
                reload_result(&["id", "name", "email"], 2),
                cx,
            );
        });
    });
    assert_eq!(
        header_sort_keys(&panel, window),
        vec![TableSortState::ascending(0)],
        "the table opens in its key order"
    );

    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| state.cycle_sort_key(1, cx));
    });

    let expected = vec![("name".to_string(), SortDirection::Ascending)];
    window.update(|_, app| {
        let panel = panel.read(app);
        let DataSource::Table { order_by, .. } = &panel.source else {
            panic!("a table source");
        };
        assert_eq!(order_names(order_by), expected);
        assert_eq!(
            order_names(
                &panel
                    .pending
                    .requery
                    .as_ref()
                    .expect("the sort queues a reload")
                    .order_by
            ),
            expected,
            "the key order is dropped from the reload"
        );
    });
    assert_eq!(
        header_sort_keys(&panel, window),
        vec![TableSortState::ascending(1)],
        "the headers show the order the reload asks for"
    );
}

/// A sort by several keys refused for unsaved edits leaves every key the
/// rows were loaded with on the headers, not just the first.
#[gpui::test]
fn a_refused_table_sort_keeps_every_loaded_key_on_the_headers(cx: &mut TestAppContext) {
    init_test_runtime(cx);

    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let panel = table_panel(window, app_state);
    window.update(|_, app| {
        panel.update(app, |panel, cx| {
            panel.apply_table_result(
                Uuid::nil(),
                TableRef::with_schema("public", "users"),
                Pagination::default(),
                vec![
                    OrderByColumn::from_name("name", SortDirection::Ascending),
                    OrderByColumn::from_name("email", SortDirection::Descending),
                ],
                Some(2),
                reload_result(&["id", "name", "email"], 2),
                cx,
            );
        });
    });
    stage_cell_edit(&panel, window);

    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| state.cycle_sort_key(0, cx));
    });

    window.update(|_, app| {
        assert!(
            panel.read(app).pending.requery.is_none(),
            "the sort must not re-query"
        );
    });
    assert_eq!(
        header_sort_keys(&panel, window),
        vec![TableSortState::ascending(1), TableSortState::descending(2)]
    );
    assert_eq!(
        last_toast_title(window),
        Some(crate::labels::grid_reload_blocked_by_pending_edits())
    );
}

/// The same for an in-memory sort: a result without a primary key refuses
/// to reorder rows with edits on them, and the headers go back to every key.
#[gpui::test]
fn a_refused_local_sort_keeps_every_key_on_the_headers(cx: &mut TestAppContext) {
    let (panel, window) = query_result_panel(cx);

    window.update(|_, app| {
        panel.update(app, |panel, cx| {
            panel.apply_local_sort(0, SortDirection::Ascending, cx);
            panel.handle_add_sort_key(1, SortDirection::Descending, cx);
        });
    });
    stage_cell_edit(&panel, window);

    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| state.cycle_sort(1, cx));
    });

    assert_eq!(
        names_in_order(&panel, window),
        vec!["c", "b", "d", "a"],
        "the rows must not move"
    );
    assert_eq!(
        header_sort_keys(&panel, window),
        vec![TableSortState::ascending(0), TableSortState::descending(1)]
    );
}

/// Clearing "Search in results" puts the hidden rows back and sorts them
/// again by every key, not only the first.
#[gpui::test]
fn clearing_a_result_search_sorts_again_by_every_key(cx: &mut TestAppContext) {
    let (panel, window) = query_result_panel(cx);

    window.update(|_, app| {
        panel.update(app, |panel, cx| {
            panel.apply_local_sort(0, SortDirection::Ascending, cx);
            panel.handle_add_sort_key(1, SortDirection::Descending, cx);
            panel.set_result_search("lyon", cx);
        });
    });
    assert_eq!(names_in_order(&panel, window), vec!["c", "b"]);

    window.update(|_, app| panel.update(app, |panel, cx| panel.set_result_search("", cx)));
    assert_eq!(names_in_order(&panel, window), vec!["c", "b", "d", "a"]);
    let keys = window.update(|_, app| panel.read(app).local_sort_keys());
    assert_eq!(
        keys,
        vec![TableSortState::ascending(0), TableSortState::descending(1)]
    );
}

/// The footer names every key of a multi-column sort, since its arrow shows
/// the first key's direction only.
#[gpui::test]
fn the_footer_names_every_sort_key(cx: &mut TestAppContext) {
    let (panel, window) = query_result_panel(cx);

    window.update(|_, app| {
        panel.update(app, |panel, cx| {
            panel.apply_local_sort(0, SortDirection::Ascending, cx);
        });
    });
    let info = window.update(|_, app| panel.read(app).current_sort_info());
    assert_eq!(
        info,
        Some(("city".to_string(), SortDirection::Ascending, false))
    );

    window.update(|_, app| {
        panel.update(app, |panel, cx| {
            panel.handle_add_sort_key(1, SortDirection::Descending, cx);
        });
    });
    let info = window.update(|_, app| panel.read(app).current_sort_info());
    assert_eq!(
        info,
        Some((
            "city ASC, name DESC".to_string(),
            SortDirection::Ascending,
            false
        ))
    );
}
