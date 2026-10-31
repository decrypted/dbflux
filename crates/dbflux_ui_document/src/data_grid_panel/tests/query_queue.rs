//! The per-connection query queue, driven through table grids on a
//! FakeDriver connection.

use super::*;
use dbflux_core::{
    ConnectedProfile, DbConfig, MutationPolicy, TaskId, TaskSnapshot, TaskStatus, WorkGate,
    WorkLane,
};
use dbflux_test_support::fake_driver::FakeDriver;
use dbflux_ui_base::work_queue::{QueryWork, spawn_query_db};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Wraps a FakeDriver connection and records, for every statement it runs,
/// how many query slots of its connection were taken at that moment.
struct GateProbe {
    inner: Arc<dyn dbflux_core::Connection>,
    gate: WorkGate,
    lane: WorkLane,
    peak_running: Arc<AtomicUsize>,
    executed: Arc<Mutex<Vec<String>>>,
}

impl dbflux_core::Connection for GateProbe {
    fn metadata(&self) -> &dbflux_core::DriverMetadata {
        self.inner.metadata()
    }

    fn ping(&self) -> Result<(), dbflux_core::DbError> {
        self.inner.ping()
    }

    fn close(&mut self) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn execute(
        &self,
        request: &dbflux_core::QueryRequest,
    ) -> Result<QueryResult, dbflux_core::DbError> {
        self.peak_running
            .fetch_max(self.gate.running_count(self.lane), Ordering::SeqCst);
        if let Ok(mut executed) = self.executed.lock() {
            executed.push(request.sql.clone());
        }
        self.inner.execute(request)
    }

    fn cancel(&self, handle: &dbflux_core::QueryHandle) -> Result<(), dbflux_core::DbError> {
        self.inner.cancel(handle)
    }

    fn schema(&self) -> Result<dbflux_core::SchemaSnapshot, dbflux_core::DbError> {
        self.inner.schema()
    }

    fn kind(&self) -> dbflux_core::DbKind {
        self.inner.kind()
    }

    fn schema_loading_strategy(&self) -> dbflux_core::SchemaLoadingStrategy {
        self.inner.schema_loading_strategy()
    }

    fn dialect(&self) -> &dyn dbflux_core::SqlDialect {
        self.inner.dialect()
    }
}

struct Probe {
    peak_running: Arc<AtomicUsize>,
    executed: Arc<Mutex<Vec<String>>>,
}

impl Probe {
    fn executed(&self) -> Vec<String> {
        self.executed.lock().expect("executed log").clone()
    }

    fn reads_of(&self, table: &str) -> usize {
        self.executed()
            .iter()
            .filter(|sql| sql.contains(table))
            .count()
    }
}

fn set_limits(
    app_state: &gpui::Entity<AppStateEntity>,
    running: usize,
    queued: usize,
    window: &mut VisualTestContext,
) {
    window.update(|_, cx| {
        app_state.update(cx, |state, _| {
            let mut settings = state.general_settings().clone();
            settings.max_concurrent_queries = running;
            settings.max_queued_queries = queued;
            state.update_general_settings(settings);
        });
    });
}

fn connect_probe(
    app_state: &gpui::Entity<AppStateEntity>,
    profile_id: Uuid,
    window: &mut VisualTestContext,
) -> Probe {
    let driver =
        FakeDriver::new(dbflux_core::DbKind::SQLite).with_default_result(QueryResult::table(
            vec![key_column("id", true)],
            vec![vec![dbflux_core::Value::Int(1)]],
            None,
            Duration::ZERO,
        ));
    let peak_running = Arc::new(AtomicUsize::new(0));
    let executed = Arc::new(Mutex::new(Vec::new()));

    window.update(|_, cx| {
        app_state.update(cx, |state, _| {
            let mut profile = dbflux_core::ConnectionProfile::new(
                "queued",
                DbConfig::SQLite {
                    path: std::path::PathBuf::from(":memory:"),
                    connection_id: None,
                },
            );
            profile.id = profile_id;
            let inner = driver
                .connect_arc(&profile)
                .expect("the fake driver connects");
            let connection: Arc<dyn dbflux_core::Connection> = Arc::new(GateProbe {
                inner,
                gate: state.work_gate().clone(),
                lane: WorkLane::Queries(profile_id),
                peak_running: peak_running.clone(),
                executed: executed.clone(),
            });

            state.connections_mut().insert(
                profile_id,
                ConnectedProfile {
                    profile,
                    connection,
                    schema: None,
                    mutation_policy: MutationPolicy::Allowed,
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
        });
    });

    Probe {
        peak_running,
        executed,
    }
}

/// A table tab on `profile_id` whose row count is known, so a refresh reads
/// one page and nothing else.
fn table_tab(
    app_state: &gpui::Entity<AppStateEntity>,
    profile_id: Uuid,
    table: &str,
    window: &mut VisualTestContext,
) -> gpui::Entity<DataGridPanel> {
    window.update(|window, cx| {
        let source = DataSource::Table {
            profile_id,
            database: None,
            table: TableRef::with_schema("main", table),
            pagination: Pagination::default(),
            order_by: Vec::new(),
            total_rows: Some(1),
        };

        cx.new(|cx| {
            DataGridPanel::new_internal(
                source,
                app_state.clone(),
                vec!["id".to_string()],
                window,
                cx,
            )
        })
    })
}

fn open_tabs(
    app_state: &gpui::Entity<AppStateEntity>,
    profile_id: Uuid,
    count: usize,
    window: &mut VisualTestContext,
) -> Vec<gpui::Entity<DataGridPanel>> {
    (0..count)
        .map(|index| table_tab(app_state, profile_id, &format!("t{index}"), window))
        .collect()
}

fn refresh_all(panels: &[gpui::Entity<DataGridPanel>], window: &mut VisualTestContext) {
    window.update(|window, cx| {
        for panel in panels {
            panel.update(cx, |panel, cx| panel.refresh(window, cx));
        }
    });
}

fn task_status(
    app_state: &gpui::Entity<AppStateEntity>,
    task_id: TaskId,
    window: &mut VisualTestContext,
) -> Option<TaskStatus> {
    window.update(|_, cx| {
        app_state
            .read(cx)
            .tasks()
            .get(task_id)
            .map(|task| task.status)
    })
}

fn is_loading(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> bool {
    window.update(|_, cx| panel.read(cx).refresh.state == GridState::Loading)
}

fn has_rows(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> bool {
    window.update(|_, cx| !panel.read(cx).result.rows.is_empty())
}

fn waiting_tasks(
    app_state: &gpui::Entity<AppStateEntity>,
    window: &mut VisualTestContext,
) -> Vec<TaskSnapshot> {
    window.update(|_, cx| {
        app_state
            .read(cx)
            .tasks()
            .running_tasks()
            .into_iter()
            .filter(|task| task.is_waiting())
            .collect()
    })
}

#[gpui::test]
fn five_restored_tabs_read_at_most_two_pages_at_once_and_all_complete(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let profile_id = Uuid::new_v4();
    set_limits(&app_state, 2, 50, window);
    let probe = connect_probe(&app_state, profile_id, window);
    let panels = open_tabs(&app_state, profile_id, 5, window);

    refresh_all(&panels, window);

    let lane = WorkLane::Queries(profile_id);
    let (running, waiting) = window.update(|_, cx| {
        let gate = app_state.read(cx).work_gate();
        (gate.running_count(lane), gate.waiting_count(lane))
    });
    assert_eq!(running, 2, "two reads hold the connection's slots");
    assert_eq!(waiting, 3, "the other three wait in the queue");
    assert_eq!(waiting_tasks(&app_state, window).len(), 3);
    assert!(
        panels.iter().all(|panel| is_loading(panel, window)),
        "a queued tab shows its loading state"
    );

    window.run_until_parked();

    assert!(probe.peak_running.load(Ordering::SeqCst) <= 2);
    for (index, panel) in panels.iter().enumerate() {
        assert_eq!(
            probe.reads_of(&format!("\"t{index}\"")),
            1,
            "tab {index} read once"
        );
        assert!(has_rows(panel, window), "tab {index} shows its page");
        assert!(!is_loading(panel, window));
    }
    assert!(waiting_tasks(&app_state, window).is_empty());
}

#[gpui::test]
fn a_queued_read_cancelled_from_the_tasks_panel_never_runs(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let profile_id = Uuid::new_v4();
    set_limits(&app_state, 1, 50, window);
    let probe = connect_probe(&app_state, profile_id, window);
    let panels = open_tabs(&app_state, profile_id, 2, window);
    refresh_all(&panels, window);

    let queued = waiting_tasks(&app_state, window);
    assert_eq!(queued.len(), 1);
    let queued_task = queued[0].id;
    assert!(queued[0].is_cancellable, "a queued read can be cancelled");

    window.update(|_, cx| {
        app_state.update(cx, |state, _| {
            state.cancel_query_for_task(queued_task);
            state.tasks_mut().cancel(queued_task);
        });
    });
    window.run_until_parked();

    assert_eq!(probe.reads_of("\"t0\""), 1);
    assert_eq!(probe.reads_of("\"t1\""), 0, "the cancelled read never ran");
    assert_eq!(
        task_status(&app_state, queued_task, window),
        Some(TaskStatus::Cancelled)
    );
    assert!(
        !is_loading(&panels[1], window),
        "the tab is ready for a reload"
    );
    assert_eq!(
        window.update(|_, cx| app_state
            .read(cx)
            .tasks()
            .get(queued_task)
            .map(|t| t.waiting_for)),
        Some(None)
    );
}

#[gpui::test]
fn a_fourth_background_read_on_a_full_queue_is_cancelled_with_a_notice(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let profile_id = Uuid::new_v4();
    set_limits(&app_state, 1, 2, window);
    let probe = connect_probe(&app_state, profile_id, window);
    let panels = open_tabs(&app_state, profile_id, 4, window);

    refresh_all(&panels, window);

    let titles = window.update(|_, cx| cx.global::<ToastGlobal>().host.read(cx).toast_titles());
    assert!(
        titles
            .iter()
            .any(|title| title == "queued: 2 queries already waiting; this one was cancelled"),
        "the full queue says why the read was cancelled: {titles:?}"
    );

    window.run_until_parked();

    assert_eq!(probe.reads_of("\"t3\""), 0, "the refused read never ran");
    assert!(
        !is_loading(&panels[3], window),
        "the refused tab can be reloaded"
    );
    for (index, panel) in panels.iter().take(3).enumerate() {
        assert!(has_rows(panel, window), "tab {index} still loads");
    }
}

#[gpui::test]
fn interactive_work_on_a_full_queue_evicts_the_oldest_background_read(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let profile_id = Uuid::new_v4();
    set_limits(&app_state, 1, 2, window);
    let probe = connect_probe(&app_state, profile_id, window);
    let panels = open_tabs(&app_state, profile_id, 3, window);
    refresh_all(&panels, window);

    let connection = window.update(|_, cx| {
        app_state
            .read(cx)
            .connections()
            .get(&profile_id)
            .map(|connected| connected.connection.clone())
            .expect("connected")
    });
    let save = window.update(|_, cx| {
        spawn_query_db(
            &app_state,
            QueryWork::interactive(profile_id).described("Save row"),
            async move { connection.execute(&dbflux_core::QueryRequest::new("UPDATE \"saved\"")) },
            cx,
        )
    });
    window.run_until_parked();
    drop(save);

    assert_eq!(
        probe.reads_of("\"t1\""),
        0,
        "the oldest queued read was evicted"
    );
    assert_eq!(probe.reads_of("\"saved\""), 1, "the save got its place");
    assert_eq!(probe.reads_of("\"t2\""), 1);
    assert!(!is_loading(&panels[1], window));
}

#[gpui::test]
fn interactive_work_starts_before_queued_background_reads(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let profile_id = Uuid::new_v4();
    set_limits(&app_state, 1, 50, window);
    let probe = connect_probe(&app_state, profile_id, window);
    let panels = open_tabs(&app_state, profile_id, 3, window);
    refresh_all(&panels, window);

    let connection = window.update(|_, cx| {
        app_state
            .read(cx)
            .connections()
            .get(&profile_id)
            .map(|connected| connected.connection.clone())
            .expect("connected")
    });
    let _run = window.update(|_, cx| {
        spawn_query_db(
            &app_state,
            QueryWork::interactive(profile_id).described("Run"),
            async move { connection.execute(&dbflux_core::QueryRequest::new("SELECT \"run\"")) },
            cx,
        )
    });
    window.run_until_parked();

    let order: Vec<String> = probe
        .executed()
        .into_iter()
        .filter(|sql| {
            ["\"t0\"", "\"t1\"", "\"t2\"", "\"run\""]
                .iter()
                .any(|t| sql.contains(t))
        })
        .collect();
    assert_eq!(order.len(), 4, "{order:?}");
    assert!(
        order[0].contains("\"t0\""),
        "the running read finishes first: {order:?}"
    );
    assert!(
        order[1].contains("\"run\""),
        "the explicit run goes next: {order:?}"
    );
}

#[gpui::test]
fn clear_queue_cancels_the_waiting_reads_and_lets_the_running_one_finish(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let profile_id = Uuid::new_v4();
    set_limits(&app_state, 1, 50, window);
    let probe = connect_probe(&app_state, profile_id, window);
    let panels = open_tabs(&app_state, profile_id, 5, window);
    refresh_all(&panels, window);
    let queued: Vec<TaskId> = waiting_tasks(&app_state, window)
        .into_iter()
        .map(|task| task.id)
        .collect();
    assert_eq!(queued.len(), 4);

    let cleared =
        window.update(|_, cx| app_state.update(cx, |state, _| state.clear_query_queue(profile_id)));
    window.run_until_parked();

    assert_eq!(cleared, 4);
    for task_id in queued {
        assert_eq!(
            task_status(&app_state, task_id, window),
            Some(TaskStatus::Cancelled)
        );
    }
    assert!(has_rows(&panels[0], window), "the running read completes");
    for (index, panel) in panels.iter().enumerate().skip(1) {
        assert_eq!(
            probe.reads_of(&format!("\"t{index}\"")),
            0,
            "tab {index} never ran"
        );
        assert!(
            !is_loading(panel, window),
            "tab {index} shows the cancelled state"
        );
    }
}

#[gpui::test]
fn a_busy_connection_does_not_hold_up_another(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let busy = Uuid::new_v4();
    let other = Uuid::new_v4();
    set_limits(&app_state, 1, 50, window);
    let _busy_probe = connect_probe(&app_state, busy, window);
    let _other_probe = connect_probe(&app_state, other, window);
    let busy_tabs = open_tabs(&app_state, busy, 3, window);
    let other_tab = table_tab(&app_state, other, "elsewhere", window);

    refresh_all(&busy_tabs, window);
    refresh_all(std::slice::from_ref(&other_tab), window);

    window.update(|_, cx| {
        let gate = app_state.read(cx).work_gate();
        assert_eq!(gate.waiting_count(WorkLane::Queries(busy)), 2);
        assert_eq!(gate.running_count(WorkLane::Queries(other)), 1);
        assert_eq!(gate.waiting_count(WorkLane::Queries(other)), 0);
    });
}

#[gpui::test]
fn closing_a_tab_drops_its_queued_read(cx: &mut TestAppContext) {
    init_test_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let window = cx.add_empty_window();
    let profile_id = Uuid::new_v4();
    set_limits(&app_state, 1, 50, window);
    let probe = connect_probe(&app_state, profile_id, window);
    let panels = open_tabs(&app_state, profile_id, 2, window);
    refresh_all(&panels, window);

    window.update(|_, cx| panels[1].update(cx, |panel, _| panel.drop_queued_work()));
    window.run_until_parked();

    assert_eq!(probe.reads_of("\"t1\""), 0);
    assert!(waiting_tasks(&app_state, window).is_empty());
}
