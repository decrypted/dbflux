// Explicit imports rather than a glob: combining one with `#[gpui::test]`
// sends the macro expansion into unbounded recursion.
use crate::data_document::DataDocument;
use crate::data_grid_panel::TableSessionView;
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_core::{CollectionRef, OrderByColumn, TableRef};
use dbflux_test_support::fake_driver::FakeDriver;
use dbflux_ui_base::toast::ToastGlobal;
use dbflux_ui_base::{AppStateChanged, AppStateEntity};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use uuid::Uuid;

/// An app state that knows the profile `stage` but has not connected it, as
/// startup does before any reconnect.
fn disconnected_app_state(cx: &mut TestAppContext) -> (Entity<AppStateEntity>, Uuid) {
    let profile = sqlite_profile();
    let profile_id = profile.id;

    let app_state = cx.update(|cx| {
        cx.new(|_| {
            let runtime =
                dbflux_storage::bootstrap::StorageRuntime::in_memory().expect("in-memory storage");
            AppStateEntity::new_with_storage_runtime(runtime).expect("test storage setup")
        })
    });

    cx.update(|cx| {
        app_state.update(cx, |state, _| state.add_profile_in_folder(profile, None));
    });

    (app_state, profile_id)
}

fn sqlite_profile() -> dbflux_core::ConnectionProfile {
    dbflux_core::ConnectionProfile::new(
        "stage",
        dbflux_core::DbConfig::SQLite {
            path: std::path::PathBuf::from(":memory:"),
            connection_id: None,
        },
    )
}

/// Connects `profile_id` through `driver` and announces it the way a finished
/// connect does.
fn connect(
    window: &mut VisualTestContext,
    app_state: &Entity<AppStateEntity>,
    driver: &FakeDriver,
    profile_id: Uuid,
) {
    let profile = sqlite_profile();
    let connection = driver.connect_arc(&profile).expect("fake connection");

    window.update(|_, cx| {
        app_state.update(cx, |state, cx| {
            state.connections_mut().insert(
                profile_id,
                dbflux_core::ConnectedProfile {
                    profile,
                    connection,
                    schema: None,
                    mutation_policy: dbflux_core::MutationPolicy::default(),
                    read_only_reason: None,
                    database_schemas: Default::default(),
                    table_details: Default::default(),
                    collection_children: Default::default(),
                    schema_types: Default::default(),
                    schema_columns: Default::default(),
                    schema_indexes: Default::default(),
                    schema_foreign_keys: Default::default(),
                    schema_routines: Default::default(),
                    dependents_cache: Default::default(),
                    active_database: None,
                    redis_key_cache: Default::default(),
                    database_connections: Default::default(),
                    proxy_tunnel: None,
                },
            );
            cx.emit(AppStateChanged);
        });
    });
    window.run_until_parked();
}

fn executed_sql(driver: &FakeDriver) -> Vec<String> {
    driver
        .stats()
        .executed_requests
        .iter()
        .map(|request| request.sql.clone())
        .collect()
}

fn last_toast_title(window: &mut VisualTestContext) -> Option<String> {
    window.update(|_, cx| cx.global::<ToastGlobal>().host.read(cx).last_toast_title())
}

fn errors_view() -> TableSessionView {
    TableSessionView {
        filter: Some("status = 500".to_string()),
        limit: Some(25),
        order_by: vec![OrderByColumn::desc("created_at")],
    }
}

/// A table tab restored before its connection is up keeps its place without
/// reading or complaining, then reads its first page, under the filter, limit
/// and sort it was saved with, as soon as the connection arrives.
#[gpui::test]
fn a_restored_table_waits_for_its_connection_then_reads_its_first_page(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite);
    let (app_state, profile_id) = disconnected_app_state(cx);

    let (document, window) = cx.add_window_view({
        let app_state = app_state.clone();
        move |window, cx| {
            DataDocument::restore_table(
                profile_id,
                TableRef::with_schema("main", "api_call_log"),
                None,
                errors_view(),
                app_state,
                window,
                cx,
            )
        }
    });
    window.run_until_parked();

    window.update(|_, cx| {
        let document = document.read(cx);
        assert!(document.is_awaiting_connection());
        assert_eq!(document.title(), "main.api_call_log");
        assert_eq!(document.connection_id(cx), Some(profile_id));
        assert_eq!(
            document.table_ref(cx),
            Some(TableRef::with_schema("main", "api_call_log"))
        );
    });
    assert!(
        executed_sql(&driver).is_empty(),
        "nothing can be read before the connection exists"
    );
    assert_eq!(
        last_toast_title(window),
        None,
        "a waiting tab must not report the missing connection as an error"
    );

    // Saving the session again while waiting keeps what it was restored with.
    let waiting_snapshot = window
        .update(|_, cx| document.read(cx).session_tab_snapshot(cx))
        .expect("a table tab is part of the session");
    assert_eq!(waiting_snapshot.kind, DataDocument::TABLE_SESSION_TAB_KIND);
    assert_eq!(
        TableSessionView::from_json(waiting_snapshot.view_state_json.as_deref()),
        errors_view()
    );

    connect(window, &app_state, &driver, profile_id);

    window.update(|_, cx| assert!(!document.read(cx).is_awaiting_connection()));

    let sql = executed_sql(&driver);
    let browse = sql
        .iter()
        .find(|sql| sql.contains("api_call_log") && sql.contains("status = 500"))
        .unwrap_or_else(|| panic!("the first page must be read under the filter, got {sql:?}"));
    assert!(browse.contains("25"), "the saved limit applies: {browse}");
    assert!(
        browse.contains("created_at") && browse.to_ascii_uppercase().contains("DESC"),
        "the saved sort applies: {browse}"
    );
}

/// Once loaded, the session records the read the grid applied, so the next
/// launch reopens the same rows.
#[gpui::test]
fn a_loaded_table_tab_saves_the_read_it_applied(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite);
    let (app_state, profile_id) = disconnected_app_state(cx);

    let (document, window) = cx.add_window_view({
        let app_state = app_state.clone();
        move |window, cx| {
            DataDocument::restore_table(
                profile_id,
                TableRef::with_schema("main", "api_error_log"),
                Some("main".to_string()),
                errors_view(),
                app_state,
                window,
                cx,
            )
        }
    });
    connect(window, &app_state, &driver, profile_id);

    let snapshot = window
        .update(|_, cx| document.read(cx).session_tab_snapshot(cx))
        .expect("a table tab is part of the session");

    assert_eq!(snapshot.kind, DataDocument::TABLE_SESSION_TAB_KIND);
    assert_eq!(snapshot.exec_ctx.connection_id, Some(profile_id));
    assert_eq!(snapshot.exec_ctx.database.as_deref(), Some("main"));
    assert_eq!(snapshot.exec_ctx.schema.as_deref(), Some("main"));
    assert_eq!(
        snapshot.exec_ctx.container.as_deref(),
        Some("api_error_log")
    );

    let view = TableSessionView::from_json(snapshot.view_state_json.as_deref());
    assert_eq!(view.filter.as_deref(), Some("status = 500"));
    assert_eq!(view.limit, Some(25));
    assert_eq!(view.order_by, vec![OrderByColumn::desc("created_at")]);
}

/// A collection tab is restored by its database and name, and also waits for
/// its connection.
#[gpui::test]
fn a_restored_collection_waits_for_its_connection(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let (app_state, profile_id) = disconnected_app_state(cx);

    let (document, window) = cx.add_window_view({
        let app_state = app_state.clone();
        move |window, cx| {
            DataDocument::restore_collection(
                profile_id,
                CollectionRef::new("shop", "orders"),
                app_state,
                window,
                cx,
            )
        }
    });
    window.run_until_parked();

    let snapshot = window.update(|_, cx| {
        let document = document.read(cx);
        assert!(document.is_awaiting_connection());
        assert_eq!(
            document.collection_ref(cx),
            Some(CollectionRef::new("shop", "orders"))
        );
        document.session_tab_snapshot(cx)
    });
    let snapshot = snapshot.expect("a collection tab is part of the session");

    assert_eq!(snapshot.kind, DataDocument::COLLECTION_SESSION_TAB_KIND);
    assert_eq!(snapshot.exec_ctx.database.as_deref(), Some("shop"));
    assert_eq!(snapshot.exec_ctx.container.as_deref(), Some("orders"));
    assert_eq!(last_toast_title(window), None);
}

/// Stored view state that cannot be read reopens the table with the defaults
/// instead of losing the tab.
#[test]
fn unreadable_view_state_falls_back_to_the_defaults() {
    assert_eq!(
        TableSessionView::from_json(Some("not json")),
        TableSessionView::default()
    );
    assert_eq!(
        TableSessionView::from_json(None),
        TableSessionView::default()
    );
    assert_eq!(
        TableSessionView::from_json(errors_view().to_json().as_deref()),
        errors_view()
    );
}
