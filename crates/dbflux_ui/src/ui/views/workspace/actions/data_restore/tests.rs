// Explicit imports, not `use super::*`: the parent glob together with
// `#[gpui::test]` sends the macro expansion into unbounded recursion.
use crate::ui::document::{DataDocument, Tab, TableSessionView};
use crate::ui::views::workspace::Workspace;
use crate::ui::views::workspace::actions::TabPlacement;
use dbflux_storage::repositories::state::sessions::{
    RestoredTab, WorkspaceSessionManifest, WorkspaceTab,
};
use dbflux_test_support::fake_driver::FakeDriver;
use dbflux_ui_base::{AppStateChanged, AppStateEntity};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use uuid::Uuid;

fn sqlite_profile() -> dbflux_core::ConnectionProfile {
    dbflux_core::ConnectionProfile::new(
        "stage-new",
        dbflux_core::DbConfig::SQLite {
            path: std::path::PathBuf::from(":memory:"),
            connection_id: None,
        },
    )
}

/// An app state, backed by in-memory storage, that knows one profile.
fn app_state_with_profile(cx: &mut TestAppContext) -> (Entity<AppStateEntity>, Uuid) {
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

/// A workspace on `app_state`, built the way the app builds it at startup,
/// so the session restore it runs is the real one.
fn launch_workspace(
    cx: &mut TestAppContext,
    app_state: Entity<AppStateEntity>,
) -> (Entity<Workspace>, &mut VisualTestContext) {
    let holder: Rc<RefCell<Option<Entity<Workspace>>>> = Rc::new(RefCell::new(None));
    let workspace_ref = holder.clone();

    let (_, window) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new(app_state.clone(), window, cx));
        workspace_ref.replace(Some(workspace.clone()));
        gpui_component::Root::new(workspace, window, cx)
    });
    window.run_until_parked();

    let workspace = holder
        .borrow()
        .clone()
        .expect("workspace should be created");
    (workspace, window)
}

fn init(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    cx.update(dbflux_components::theme::init);
    cx.update(dbflux_ui_base::keymap::init_keymap);
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

fn disconnect(
    window: &mut VisualTestContext,
    app_state: &Entity<AppStateEntity>,
    profile_id: Uuid,
) {
    window.update(|_, cx| {
        app_state.update(cx, |state, cx| {
            state.connections_mut().remove(&profile_id);
            cx.emit(AppStateChanged);
        });
    });
    window.run_until_parked();
}

fn open_table(
    window: &mut VisualTestContext,
    workspace: &Entity<Workspace>,
    profile_id: Uuid,
    name: &str,
) {
    window.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_table_document(
                profile_id,
                dbflux_core::TableRef::with_schema("main", name),
                Some("main".to_string()),
                TabPlacement::NewTab,
                window,
                cx,
            );
        });
    });
    window.run_until_parked();
}

fn session_tabs(
    window: &mut VisualTestContext,
    app_state: &Entity<AppStateEntity>,
) -> Vec<RestoredTab> {
    window.update(|_, cx| {
        let runtime = app_state.read(cx).storage_runtime();
        runtime
            .sessions()
            .restore_session(runtime.artifacts())
            .expect("the session must be readable")
            .map(|session| session.tabs)
            .unwrap_or_default()
    })
}

/// Titles and connections of the workspace's tabs, and the active title.
fn tabs_of(
    window: &mut VisualTestContext,
    workspace: &Entity<Workspace>,
) -> (Vec<(String, Option<Uuid>)>, Option<String>) {
    window.update(|_, cx| {
        let manager = workspace.read(cx).tab_manager.read(cx);
        let tabs = manager
            .documents()
            .iter()
            .map(|tab| {
                let meta = tab.meta_snapshot(cx);
                (meta.title, meta.connection_id)
            })
            .collect();
        let active = manager.active_id().and_then(|id| {
            manager
                .documents()
                .iter()
                .find(|tab| tab.id() == id)
                .map(|tab| tab.tab_title(cx))
        });
        (tabs, active)
    })
}

fn browse_count(driver: &FakeDriver, table: &str) -> usize {
    driver
        .stats()
        .executed_requests
        .iter()
        .filter(|request| request.sql.contains(table))
        .count()
}

/// Table tabs are saved with the session and come back on the next launch in
/// their order, with the active one active again, even though their
/// connection is not up yet. They read their first page once it is.
#[gpui::test]
fn table_tabs_reopen_in_place_and_load_once_connected(cx: &mut TestAppContext) {
    init(cx);
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite);
    let (app_state, profile_id) = app_state_with_profile(cx);

    {
        let (workspace, window) = launch_workspace(cx, app_state.clone());
        connect(window, &app_state, &driver, profile_id);
        open_table(window, &workspace, profile_id, "api_call_log");
        open_table(window, &workspace, profile_id, "api_error_log");

        let saved = session_tabs(window, &app_state);
        let kinds: Vec<&str> = saved.iter().map(|tab| tab.tab_kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                DataDocument::TABLE_SESSION_TAB_KIND,
                DataDocument::TABLE_SESSION_TAB_KIND
            ],
            "both table tabs must be recorded in the session"
        );

        // The next launch starts with no connection.
        disconnect(window, &app_state, profile_id);
    }

    let reads_before_restore = browse_count(&driver, "api_call_log");
    let (workspace, window) = launch_workspace(cx, app_state.clone());

    let (tabs, active) = tabs_of(window, &workspace);
    assert_eq!(
        tabs,
        vec![
            ("main.api_call_log".to_string(), Some(profile_id)),
            ("main.api_error_log".to_string(), Some(profile_id)),
        ],
        "the restored tabs keep their table, connection and order"
    );
    assert_eq!(active.as_deref(), Some("main.api_error_log"));
    assert_eq!(
        browse_count(&driver, "api_call_log"),
        reads_before_restore,
        "a tab whose connection is down reads nothing"
    );

    connect(window, &app_state, &driver, profile_id);

    assert!(
        browse_count(&driver, "api_call_log") > reads_before_restore,
        "the restored tab reads its first page once its connection is up"
    );
    assert_eq!(
        tabs_of(window, &workspace).0.len(),
        2,
        "loading replaces no tab and opens none"
    );
}

fn table_tab(profile: Uuid, name: &str, position: usize) -> WorkspaceTab {
    WorkspaceTab {
        id: Uuid::new_v4().to_string(),
        tab_kind: DataDocument::TABLE_SESSION_TAB_KIND.to_string(),
        language: "sql".to_string(),
        exec_ctx: dbflux_core::ExecutionContext {
            connection_id: Some(profile),
            database: Some("main".to_string()),
            schema: Some("main".to_string()),
            container: Some(name.to_string()),
            source: None,
        },
        scratch_path: None,
        shadow_path: None,
        file_path: None,
        title: format!("main.{name}"),
        position,
        is_pinned: false,
        view_state_json: None,
    }
}

fn save_session(
    cx: &mut TestAppContext,
    app_state: &Entity<AppStateEntity>,
    manifest: WorkspaceSessionManifest,
) {
    cx.update(|cx| {
        let runtime = app_state.read(cx).storage_runtime();
        runtime
            .sessions()
            .save_workspace_session(&manifest)
            .expect("the session must be storable");
    });
}

/// A table tab whose connection was deleted since the last session is not
/// restored; the tabs around it are.
#[gpui::test]
fn a_table_tab_of_a_deleted_connection_is_dropped(cx: &mut TestAppContext) {
    init(cx);
    let (app_state, profile_id) = app_state_with_profile(cx);

    cx.update(|cx| {
        let runtime = app_state.read(cx).storage_runtime();
        runtime
            .sessions()
            .save_workspace_session(&WorkspaceSessionManifest {
                version: 1,
                active_index: Some(0),
                tabs: vec![
                    table_tab(Uuid::new_v4(), "gone", 0),
                    table_tab(profile_id, "kept", 1),
                ],
            })
            .expect("the session must be storable");
    });

    let (workspace, window) = launch_workspace(cx, app_state);

    let (tabs, _) = tabs_of(window, &workspace);
    assert_eq!(tabs, vec![("main.kept".to_string(), Some(profile_id))]);
}

/// A tab dropped from the session does not move the active tab onto its
/// neighbor: the tab that was active comes back active.
#[gpui::test]
fn a_dropped_tab_before_the_active_one_keeps_the_active_tab(cx: &mut TestAppContext) {
    init(cx);
    let (app_state, profile_id) = app_state_with_profile(cx);

    save_session(
        cx,
        &app_state,
        WorkspaceSessionManifest {
            version: 1,
            active_index: Some(1),
            tabs: vec![
                table_tab(Uuid::new_v4(), "gone", 0),
                table_tab(profile_id, "active", 1),
                table_tab(profile_id, "last", 2),
            ],
        },
    );

    let (workspace, window) = launch_workspace(cx, app_state);

    let (tabs, active) = tabs_of(window, &workspace);
    assert_eq!(
        tabs,
        vec![
            ("main.active".to_string(), Some(profile_id)),
            ("main.last".to_string(), Some(profile_id)),
        ]
    );
    assert_eq!(active.as_deref(), Some("main.active"));
}

/// A filter run after the last tab event is not in the session until the
/// shutdown write, which records it.
#[gpui::test]
fn the_shutdown_write_records_a_filter_run_after_the_last_tab_event(cx: &mut TestAppContext) {
    init(cx);
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite);
    let (app_state, profile_id) = app_state_with_profile(cx);

    let (workspace, window) = launch_workspace(cx, app_state.clone());
    connect(window, &app_state, &driver, profile_id);

    // Opened the way `open_table_document` opens it, keeping the document
    // to reach its grid.
    let grid = window.update(|window, cx| {
        let document = cx.new(|cx| {
            DataDocument::new_for_table(
                profile_id,
                dbflux_core::TableRef::with_schema("main", "api_call_log"),
                Some("main".to_string()),
                app_state.clone(),
                window,
                cx,
            )
        });
        let grid = document.read(cx).data_grid_for_tests().clone();
        let pane = DataDocument::into_pane(document, cx);
        workspace.update(cx, |workspace, cx| {
            workspace
                .tab_manager
                .update(cx, |mgr, cx| mgr.open(Tab::Pane(Box::new(pane)), cx));
        });
        grid
    });
    window.run_until_parked();

    window.update(|window, cx| {
        grid.update(cx, |grid, cx| {
            grid.set_filter_for_tests("status = 500", window, cx);
            grid.refresh(window, cx);
        });
    });
    window.run_until_parked();

    let saved_filter = |window: &mut VisualTestContext| {
        session_tabs(window, &app_state)
            .first()
            .map(|tab| TableSessionView::from_json(tab.view_state_json.as_deref()).filter)
            .expect("the table tab is part of the session")
    };
    assert_eq!(
        saved_filter(window),
        None,
        "running a filter is no tab event, so the session has not seen it yet"
    );

    window.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.persist_session(cx)));

    assert_eq!(saved_filter(window).as_deref(), Some("status = 500"));
}
