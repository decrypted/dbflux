use super::*;
use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_core::Value;

fn people_panel(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<DataGridPanel>,
    gpui::Entity<AppStateEntity>,
    &mut VisualTestContext,
) {
    init_test_runtime(cx);

    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let result = QueryResult::table(
        vec![key_column("id", true), key_column("name", false)],
        [(1, "O'Brien"), (2, "Ann"), (3, "O'Brien")]
            .into_iter()
            .map(|(id, name)| vec![Value::Int(id), Value::Text(name.to_string())])
            .collect(),
        None,
        Duration::ZERO,
    );

    let panel = window.update(|window, app| {
        let source = DataSource::QueryResult {
            result: Arc::new(result.clone()),
            original_query: "SELECT id, name FROM people".to_string(),
            profile_id: None,
        };
        let state = app_state.clone();
        let panel =
            app.new(|cx| DataGridPanel::new_internal(source, state, Vec::new(), window, cx));
        panel.update(app, |panel, cx| panel.set_result(result, cx));
        panel
    });

    (panel, app_state, window)
}

fn select_columns(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    first: usize,
    last: usize,
) {
    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| {
            state.click_column(first, gpui::Modifiers::default(), cx);
            let shift = gpui::Modifiers {
                shift: true,
                ..gpui::Modifiers::default()
            };
            state.click_column(last, shift, cx);
        });
    });
}

fn clipboard_text(window: &mut VisualTestContext) -> String {
    window
        .update(|_, cx| cx.read_from_clipboard())
        .and_then(|item| item.text())
        .unwrap_or_default()
}

/// Copy as SQL IN list quotes text with the dialect, doubling the quote in
/// O'Brien, and lists a repeated value once. With two columns selected it
/// lists the leftmost.
#[gpui::test]
fn copy_as_in_list_quotes_and_dedupes_the_column(cx: &mut TestAppContext) {
    let (panel, _app_state, window) = people_panel(cx);

    select_columns(&panel, window, 1, 1);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_in_list(cx)));
    assert_eq!(clipboard_text(window), "('O''Brien', 'Ann')");

    select_columns(&panel, window, 0, 1);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_in_list(cx)));
    assert_eq!(
        clipboard_text(window),
        "(1, 2, 3)",
        "with two columns selected the leftmost one is listed"
    );
}

/// Copy as Markdown table writes a header row, its separator, and one line
/// per selected row.
#[gpui::test]
fn copy_as_markdown_writes_a_header_and_the_rows(cx: &mut TestAppContext) {
    let (panel, _app_state, window) = people_panel(cx);

    select_columns(&panel, window, 0, 1);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_markdown(cx)));

    assert_eq!(
        clipboard_text(window),
        "| id | name |\n| --- | --- |\n| 1 | O'Brien |\n| 2 | Ann |\n| 3 | O'Brien |"
    );
}

/// With Include column names when copying on, the plain copy starts with
/// the copied columns' names; off, it stays values only.
#[gpui::test]
fn plain_copy_adds_column_names_when_the_setting_is_on(cx: &mut TestAppContext) {
    let (panel, app_state, window) = people_panel(cx);
    select_columns(&panel, window, 1, 1);

    let copy = |window: &mut VisualTestContext| {
        window.update(|_, app| {
            panel.update(app, |panel, cx| panel.sync_copy_column_names(cx));
            panel
                .read(app)
                .grid_table
                .table_state
                .as_ref()
                .expect("table state")
                .read(app)
                .copy_selection()
                .unwrap_or_default()
        })
    };

    assert_eq!(copy(window), "O'Brien\nAnn\nO'Brien");

    window.update(|_, app| {
        app_state.update(app, |state, _| {
            let mut settings = state.general_settings().clone();
            settings.grid_copy_column_names = true;
            state.update_general_settings(settings);
        });
    });
    assert_eq!(copy(window), "name\nO'Brien\nAnn\nO'Brien");
}

/// A query result has no table to insert into, so the menu offers the IN
/// list and the Markdown table after Copy as JSON but not the INSERTs.
#[gpui::test]
fn a_query_result_menu_offers_the_extractors_without_inserts(cx: &mut TestAppContext) {
    let (panel, _app_state, window) = people_panel(cx);

    let actions: Vec<Option<ContextMenuAction>> = window.update(|_, app| {
        let items = DataGridPanel::build_context_menu_items(false, false, true, false, true);
        panel
            .read(app)
            .with_copy_extractors(items, app)
            .iter()
            .map(|item| item.action)
            .collect()
    });

    let json = actions
        .iter()
        .position(|action| *action == Some(ContextMenuAction::CopyAsJson))
        .expect("Copy as JSON is listed");
    assert_eq!(
        actions.get(json + 1..json + 3),
        Some(
            &[
                Some(ContextMenuAction::CopyAsSqlInList),
                Some(ContextMenuAction::CopyAsMarkdown),
            ][..]
        )
    );
    assert!(!actions.contains(&Some(ContextMenuAction::CopyAsInsertStatements)));
}

/// With `name` moved in front of `id` on screen, the IN list takes the
/// leftmost column the grid shows, not the lowest model index, and the
/// Markdown table lists the columns in screen order.
#[gpui::test]
fn copy_extractors_follow_the_screen_order_of_columns(cx: &mut TestAppContext) {
    let (panel, _app_state, window) = people_panel(cx);
    window.update(|_, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| {
            state.move_column(1, 0, cx);
        });
    });

    // Screen order is name, id: select both by their headers.
    select_columns(&panel, window, 1, 0);
    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_in_list(cx)));
    assert_eq!(
        clipboard_text(window),
        "('O''Brien', 'Ann')",
        "the leftmost column on screen is `name`"
    );

    window.update(|_, app| panel.update(app, |panel, cx| panel.copy_selection_as_markdown(cx)));
    let markdown = clipboard_text(window);
    assert!(
        markdown.starts_with("| name | id |"),
        "columns in screen order: {markdown}"
    );
}
