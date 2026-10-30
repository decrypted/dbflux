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
