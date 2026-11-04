//! How many statements each thing that runs a chart sends, counted at the driver
//! on a real SQLite connection and through real frames, which is where a chart's
//! first run and its time range meet. A test also counts the runs it started: this
//! scheduler polls a job only once the test parks, so a run the next frame replaced
//! has not reached the driver here, where in the app it already had.

use crate::chart_document::ChartDocument;
use crate::dashboard::{DashboardDocument, DashboardPanelSlot, PANEL_REEXEC_CAP, PanelGridPos};
use crate::handle::DocumentEvent;
use crate::instance_inspector::InspectorPanel;
use crate::keyboard_test_support::init_keyboard_runtime;
use crate::types::DocumentState;
use dbflux_components::chart::{BindingSpec, ChartSpec, InstanceMetricSource};
use dbflux_components::common::time_range::view::TimeRangePanel;
use dbflux_components::saved_chart::{SavedChart, SavedChartRefreshPolicy};
use dbflux_core::chrono::NaiveDate;
use dbflux_core::{
    CancelToken, ColumnKind, ColumnMeta, Connection, ConnectionProfile, DbConfig, DbDriver,
    DbError, DbKind, DriverMetadata, ExecutionSourceContext, QueryHandle, QueryRequest,
    QueryResult, RefreshPolicy, SchemaLoadingStrategy, SchemaSnapshot, SqlDialect, Value,
};
use dbflux_driver_sqlite::SqliteDriver;
use dbflux_storage::bootstrap::StorageRuntime;
use dbflux_ui_base::{AppStateEntity, DashboardPanelDraft};
use gpui::prelude::*;
use gpui::{AnyView, Context, Entity, TestAppContext, VisualTestContext, Window, div};
use gpui_component::calendar::Date;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

const QUERY: &str = "SELECT ts, value FROM samples";

/// A real connection that notes each statement the moment it reaches the driver.
struct CountingConnection {
    inner: Arc<dyn Connection>,
    statements: Arc<Mutex<Vec<String>>>,
    cleanups: Arc<AtomicUsize>,
    cancel_mid_run: Arc<AtomicBool>,
}

impl Connection for CountingConnection {
    fn metadata(&self) -> &DriverMetadata {
        self.inner.metadata()
    }

    fn ping(&self) -> Result<(), DbError> {
        self.inner.ping()
    }

    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }

    fn execute(&self, request: &QueryRequest) -> Result<QueryResult, DbError> {
        if let Ok(mut statements) = self.statements.lock() {
            statements.push(request.sql.clone());
        }

        // SQLite has no instance catalog: answer as a driver's metric dispatch does,
        // with the current sample.
        if matches!(
            request
                .execution_context
                .as_ref()
                .and_then(|context| context.source.as_ref()),
            Some(ExecutionSourceContext::InstanceMetricQuery { .. })
        ) {
            return Ok(QueryResult::table(
                sample_columns().to_vec(),
                vec![vec![
                    Value::Text("2026-01-01 00:00:00".to_string()),
                    Value::Float(1.0),
                ]],
                None,
                Duration::ZERO,
            ));
        }

        self.inner.execute(request)
    }

    /// Takes no notice of `cancel`, like a run the driver has already started.
    fn execute_unattended(
        &self,
        request: &QueryRequest,
        cancel: &CancelToken,
    ) -> Result<QueryResult, DbError> {
        if self.cancel_mid_run.load(Ordering::SeqCst) {
            cancel.cancel();
        }
        self.execute(request)
    }

    fn cleanup_after_cancel(&self) -> Result<(), DbError> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn cancel(&self, handle: &QueryHandle) -> Result<(), DbError> {
        self.inner.cancel(handle)
    }

    fn schema(&self) -> Result<SchemaSnapshot, DbError> {
        self.inner.schema()
    }

    fn kind(&self) -> DbKind {
        self.inner.kind()
    }

    fn schema_loading_strategy(&self) -> SchemaLoadingStrategy {
        self.inner.schema_loading_strategy()
    }

    fn dialect(&self) -> &dyn SqlDialect {
        self.inner.dialect()
    }
}

/// Draws what it holds, as a tab or a dashboard card draws a chart.
struct Host {
    views: Vec<AnyView>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().children(self.views.iter().cloned())
    }
}

struct Rig<'a> {
    window: &'a mut VisualTestContext,
    host: Entity<Host>,
    app_state: Entity<AppStateEntity>,
    profile_id: Uuid,
    statements: Arc<Mutex<Vec<String>>>,
    finished: Rc<Cell<usize>>,
    cleanups: Arc<AtomicUsize>,
    cancel_mid_run: Arc<AtomicBool>,
}

/// What something set off: statements at the driver, and runs that reported an end.
#[derive(Debug, PartialEq, Eq)]
struct Ran {
    statements: usize,
    finished: usize,
}

fn ran(count: usize) -> Ran {
    Ran {
        statements: count,
        finished: count,
    }
}

fn rig(cx: &mut TestAppContext) -> Rig<'_> {
    init_keyboard_runtime(cx);

    let profile = ConnectionProfile::new(
        "real",
        DbConfig::SQLite {
            path: ":memory:".into(),
            connection_id: None,
        },
    );
    let profile_id = profile.id;

    let inner: Arc<dyn Connection> = Arc::from(
        SqliteDriver::new()
            .connect(&profile)
            .expect("in-memory SQLite"),
    );
    inner
        .execute(&QueryRequest::new(
            "CREATE TABLE samples (ts TEXT, value REAL)",
        ))
        .expect("create");
    inner
        .execute(&QueryRequest::new(
            "INSERT INTO samples VALUES ('2026-01-01 00:00:00', 1.0), ('2026-01-01 00:01:00', 2.0)",
        ))
        .expect("insert");

    let statements: Arc<Mutex<Vec<String>>> = Arc::default();
    let cleanups: Arc<AtomicUsize> = Arc::default();
    let cancel_mid_run: Arc<AtomicBool> = Arc::default();
    let connection: Arc<dyn Connection> = Arc::new(CountingConnection {
        inner,
        statements: statements.clone(),
        cleanups: cleanups.clone(),
        cancel_mid_run: cancel_mid_run.clone(),
    });

    let app_state = cx.update(|cx| {
        cx.new(|_| {
            let mut state = AppStateEntity::new_with_storage_runtime(
                StorageRuntime::in_memory().expect("isolated storage runtime"),
            )
            .expect("test storage setup");
            state.add_profile_in_folder(profile.clone(), None);
            state.apply_connect_profile(
                profile,
                connection,
                None,
                None,
                false,
                dbflux_core::WritePrivilege::Unknown,
            );
            state
        })
    });

    let holder: Rc<RefCell<Option<Entity<Host>>>> = Rc::default();
    let holder_for_view = holder.clone();
    let (_, window) = cx.add_window_view(move |window, cx| {
        let host = cx.new(|_| Host { views: Vec::new() });
        holder_for_view.replace(Some(host.clone()));
        gpui_component::Root::new(host, window, cx)
    });
    let host = holder.borrow().clone().expect("host");
    window.run_until_parked();

    Rig {
        window,
        host,
        app_state,
        profile_id,
        statements,
        finished: Rc::default(),
        cleanups,
        cancel_mid_run,
    }
}

fn sample_columns() -> [ColumnMeta; 2] {
    [
        ColumnMeta {
            name: "ts".to_string(),
            type_name: String::new(),
            kind: ColumnKind::Timestamp,
            nullable: true,
            is_primary_key: false,
        },
        ColumnMeta {
            name: "value".to_string(),
            type_name: String::new(),
            kind: ColumnKind::Float,
            nullable: true,
            is_primary_key: false,
        },
    ]
}

fn saved_chart(profile_id: Uuid, name: &str) -> SavedChart {
    SavedChart::new_query(
        name.to_string(),
        profile_id,
        QUERY.to_string(),
        ChartSpec::from_detection(0, vec![1], &sample_columns(), 10_000).expect("a spec"),
        BindingSpec::default(),
    )
}

impl Rig<'_> {
    fn statements(&self) -> usize {
        self.statements.lock().expect("statement log").len()
    }

    fn settle(&mut self) {
        self.window.run_until_parked();
    }

    /// What `act` sets off, once all of it has settled.
    fn count(&mut self, act: impl FnOnce(&mut Self)) -> Ran {
        self.settle();
        let statements = self.statements();
        let finished = self.finished.get();
        act(self);
        self.settle();
        Ran {
            statements: self.statements() - statements,
            finished: self.finished.get() - finished,
        }
    }

    fn watch(&mut self, chart: &Entity<ChartDocument>) {
        let finished = self.finished.clone();
        let chart = chart.clone();
        self.window.update(|_, cx| {
            cx.subscribe(&chart, move |_, event: &DocumentEvent, _| {
                if matches!(event, DocumentEvent::ExecutionFinished) {
                    finished.set(finished.get() + 1);
                }
            })
            .detach();
        });
    }

    fn show(&mut self, view: impl Into<AnyView>) {
        let view = view.into();
        let host = self.host.clone();
        self.window.update(|_, cx| {
            host.update(cx, |host, cx| {
                host.views.push(view);
                cx.notify();
            });
        });
    }

    fn open_new_chart(&mut self) -> Entity<ChartDocument> {
        let app_state = self.app_state.clone();
        let profile_id = self.profile_id;
        let chart = self.window.update(|window, cx| {
            cx.new(|cx| {
                ChartDocument::new(Some(profile_id), QUERY.to_string(), app_state, window, cx)
            })
        });
        self.watch(&chart);
        self.show(chart.clone());
        chart
    }

    fn open_saved_chart(&mut self, saved: &SavedChart) -> Entity<ChartDocument> {
        let app_state = self.app_state.clone();
        let saved = saved.clone();
        let chart = self.window.update(|window, cx| {
            cx.new(|cx| ChartDocument::from_saved(&saved, app_state, window, cx).expect("a chart"))
        });
        self.watch(&chart);
        self.show(chart.clone());
        chart
    }

    fn stored_chart(&mut self, name: &str) -> SavedChart {
        let saved = saved_chart(self.profile_id, name);
        let app_state = self.app_state.clone();
        let written = saved.clone();
        self.window.update(|_, cx| {
            app_state.update(cx, |state, _| {
                state.saved_charts.upsert(written).expect("stored");
            });
        });
        saved
    }

    /// A stored dashboard of `panel_count` stored charts, as the workspace opens
    /// it, not yet drawn.
    fn build_dashboard(
        &mut self,
        panel_count: usize,
        policy: SavedChartRefreshPolicy,
    ) -> (Entity<DashboardDocument>, Vec<Entity<ChartDocument>>) {
        let charts: Vec<SavedChart> = (0..panel_count)
            .map(|index| self.stored_chart(&format!("panel {index}")))
            .collect();

        let app_state = self.app_state.clone();
        let profile_id = self.profile_id;
        let drafts = charts
            .iter()
            .map(|chart| DashboardPanelDraft::Chart {
                saved_chart_id: chart.id,
                layout: None,
            })
            .collect::<Vec<_>>();
        let dashboard_id = self.window.update(|_, cx| {
            app_state.update(cx, |state, _| {
                let id = state
                    .dashboards
                    .create_dashboard("Dashboard".to_string(), None, profile_id, None, policy)
                    .expect("dashboard");
                state
                    .dashboards
                    .append_panels(id, drafts)
                    .expect("panels stored");
                id
            })
        });

        let panels: Rc<RefCell<Vec<Entity<ChartDocument>>>> = Rc::default();
        let panels_ref = panels.clone();
        let dashboard = self.window.update(|window, cx| {
            let slots = charts
                .iter()
                .enumerate()
                .map(|(index, saved)| {
                    let state = app_state.clone();
                    let panel = cx.new(|cx| {
                        let mut doc = ChartDocument::from_saved(saved, state, window, cx)
                            .expect("a query chart");
                        doc.set_embedded(true, cx);
                        doc
                    });
                    panels_ref.borrow_mut().push(panel.clone());
                    DashboardPanelSlot::Loaded {
                        panel,
                        grid_pos: PanelGridPos {
                            grid_row: index as u32,
                            grid_column: 0,
                            grid_width: 12,
                            grid_height: 2,
                        },
                        title_override: None,
                    }
                })
                .collect();
            let shared_time_range = cx.new(|cx| TimeRangePanel::new("24h", Some(3), window, cx));
            cx.new(|cx| {
                DashboardDocument::new(
                    dashboard_id,
                    "Dashboard".to_string(),
                    slots,
                    shared_time_range,
                    None,
                    policy,
                    false,
                    app_state,
                    cx,
                )
            })
        });

        let panels = panels.borrow().clone();
        for panel in &panels {
            self.watch(panel);
        }
        (dashboard, panels)
    }

    fn inflight(&mut self, dashboard: &Entity<DashboardDocument>) -> usize {
        self.window
            .update(|_, cx| dashboard.read(cx).inflight_reexec_count_for_testing())
    }

    fn pick_time_range(&mut self, panel: Entity<TimeRangePanel>, index: usize) {
        self.window.update(|_, cx| {
            panel.update(cx, |panel, cx| panel.select_preset(index, cx));
        });
    }

    fn own_time_range(&mut self, chart: &Entity<ChartDocument>) -> Entity<TimeRangePanel> {
        self.window.update(|_, cx| {
            chart
                .read(cx)
                .time_range_panel
                .clone()
                .expect("the first frame made it")
        })
    }

    fn refresh(&mut self, chart: &Entity<ChartDocument>) {
        let chart = chart.clone();
        self.window.update(|window, cx| {
            chart.update(cx, |chart, cx| chart.run_by_hand(window, cx));
        });
    }

    fn tick(&mut self, every: Duration) {
        self.window.executor().advance_clock(every);
    }

    fn executing(&mut self, panels: &[Entity<ChartDocument>]) -> usize {
        self.window.update(|_, cx| {
            panels
                .iter()
                .filter(|panel| panel.read(cx).state() == DocumentState::Executing)
                .count()
        })
    }

    /// Draws each panel alone once and takes it off the screen again: the first
    /// runs are over, and no frame reaches the panels until the dashboard is shown.
    fn run_first_frames_apart(&mut self, panels: &[Entity<ChartDocument>]) {
        for panel in panels {
            self.show(panel.clone());
        }
        self.settle();
        let host = self.host.clone();
        self.window.update(|_, cx| {
            host.update(cx, |host, cx| {
                host.views.clear();
                cx.notify();
            });
        });
        self.settle();
    }

    fn open_metric_chart(&mut self) -> Entity<ChartDocument> {
        let app_state = self.app_state.clone();
        let profile_id = self.profile_id;
        let chart = self.window.update(|window, cx| {
            cx.new(|cx| {
                ChartDocument::new_with_source(
                    Some(profile_id),
                    "metric".to_string(),
                    Box::new(InstanceMetricSource {
                        metric_id: "pg.tps".to_string(),
                    }),
                    app_state,
                    window,
                    cx,
                )
            })
        });
        self.watch(&chart);
        chart
    }

    fn on_dashboard(
        &mut self,
        dashboard: &Entity<DashboardDocument>,
        act: impl FnOnce(&mut DashboardDocument, &mut Context<DashboardDocument>),
    ) {
        let dashboard = dashboard.clone();
        self.window.update(|_, cx| dashboard.update(cx, act));
    }

    fn request(&mut self, chart: &Entity<ChartDocument>) {
        let chart = chart.clone();
        self.window.update(|window, cx| {
            chart.update(cx, |chart, cx| chart.request_reexecute(window, cx));
        });
    }

    /// Fills in the custom range and presses Apply.
    fn apply_custom_range(&mut self, chart: &Entity<ChartDocument>) {
        let panel = self.own_time_range(chart);
        let chart = chart.clone();
        self.window.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.select_preset(5, cx);
                let start = NaiveDate::from_ymd_opt(2026, 1, 1).expect("a date");
                let end = NaiveDate::from_ymd_opt(2026, 1, 2).expect("a date");
                panel.custom_date_range_picker.update(cx, |picker, cx| {
                    picker.set_date(Date::Range(Some(start), Some(end)), window, cx);
                });
            });
            chart.update(cx, |chart, cx| chart.apply_custom_range(cx));
        });
    }
}

#[gpui::test]
fn opening_a_chart_runs_it_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);

    let ran_on_open = rig.count(|rig| {
        rig.open_new_chart();
    });

    assert_eq!(ran_on_open, ran(1));
}

#[gpui::test]
fn reopening_a_saved_chart_runs_it_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let saved = rig.stored_chart("saved");

    let ran_on_open = rig.count(|rig| {
        rig.open_saved_chart(&saved);
    });

    assert_eq!(ran_on_open, ran(1));

    let again = rig.count(|rig| {
        rig.open_saved_chart(&saved);
    });

    assert_eq!(again, ran(1), "another tab of it");
}

#[gpui::test]
fn a_dashboard_runs_each_panel_once_on_open(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, _panels) = rig.build_dashboard(3, SavedChartRefreshPolicy::Off);

    let ran_on_open = rig.count(|rig| rig.show(dashboard.clone()));

    assert_eq!(ran_on_open, ran(3));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

#[gpui::test]
fn refreshing_a_chart_runs_it_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);

    let opened_and_refreshed = rig.count(|rig| {
        let chart = rig.open_new_chart();
        rig.settle();
        rig.refresh(&chart);
    });

    assert_eq!(opened_and_refreshed, ran(2));
}

#[gpui::test]
fn refreshing_a_dashboard_runs_each_panel_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, _panels) = rig.build_dashboard(3, SavedChartRefreshPolicy::Off);

    let opened_and_refreshed = rig.count(|rig| {
        rig.show(dashboard.clone());
        rig.settle();
        rig.on_dashboard(&dashboard, |dashboard, cx| {
            dashboard.refresh_all_loaded_panels(cx);
        });
    });

    assert_eq!(opened_and_refreshed, ran(6));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

#[gpui::test]
fn each_tick_of_a_chart_timer_runs_it_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);

    let opened_and_ticked = rig.count(|rig| {
        let chart = rig.open_new_chart();
        rig.window.update(|_, cx| {
            chart.update(cx, |chart, cx| {
                chart.set_active_tab(true);
                chart.set_refresh_policy(RefreshPolicy::Interval { every_secs: 30 }, cx);
            });
        });
        rig.settle();
        rig.tick(Duration::from_secs(30));
        rig.settle();
        rig.tick(Duration::from_secs(30));
    });

    assert_eq!(opened_and_ticked, ran(3));
}

#[gpui::test]
fn each_tick_of_a_dashboard_timer_runs_each_panel_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, _panels) =
        rig.build_dashboard(3, SavedChartRefreshPolicy::Interval { every_secs: 30 });

    let opened_and_ticked = rig.count(|rig| {
        rig.show(dashboard.clone());
        rig.settle();
        rig.tick(Duration::from_secs(30));
    });

    assert_eq!(opened_and_ticked, ran(6));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

/// A panel with a timer of its own under a dashboard whose timer already runs it,
/// on another beat: an instance overview's two timers start in one frame, and their
/// ticks join into one run.
#[gpui::test]
fn a_panel_runs_once_per_tick_of_the_timer_of_its_dashboard(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, panels) =
        rig.build_dashboard(2, SavedChartRefreshPolicy::Interval { every_secs: 30 });
    for panel in &panels {
        let panel = panel.clone();
        rig.window.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_active_tab(true);
                panel.set_refresh_policy(RefreshPolicy::Interval { every_secs: 30 }, cx);
            });
        });
    }
    rig.show(dashboard.clone());
    rig.settle();
    rig.tick(Duration::from_secs(10));

    let ticked = rig.count(|rig| rig.tick(Duration::from_secs(30)));

    assert_eq!(ticked, ran(2));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

/// With the dashboard's timer off, a panel's own timer goes on, as it always did.
#[gpui::test]
fn a_panel_timer_keeps_running_under_a_dashboard_without_one(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, panels) = rig.build_dashboard(2, SavedChartRefreshPolicy::Off);
    let panel = panels[0].clone();
    rig.window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.set_active_tab(true);
            panel.set_refresh_policy(RefreshPolicy::Interval { every_secs: 30 }, cx);
        });
    });
    rig.show(dashboard.clone());

    let ticked = rig.count(|rig| rig.tick(Duration::from_secs(30)));

    assert_eq!(ticked, ran(1));
    assert_eq!(rig.inflight(&dashboard), 0, "its run took no slot");
}

#[gpui::test]
fn changing_the_time_range_runs_the_chart_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);

    let opened_and_changed = rig.count(|rig| {
        let chart = rig.open_new_chart();
        rig.settle();
        let panel = rig.own_time_range(&chart);
        rig.pick_time_range(panel, 1);
    });

    assert_eq!(opened_and_changed, ran(2));
}

#[gpui::test]
fn applying_a_custom_range_runs_the_chart_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);

    let opened_and_applied = rig.count(|rig| {
        let chart = rig.open_new_chart();
        rig.settle();
        rig.apply_custom_range(&chart);
    });

    assert_eq!(opened_and_applied, ran(2));
}

#[gpui::test]
fn changing_a_dashboard_time_range_runs_each_panel_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, _panels) = rig.build_dashboard(3, SavedChartRefreshPolicy::Off);

    let opened_and_changed = rig.count(|rig| {
        rig.show(dashboard.clone());
        rig.settle();
        let shared = rig
            .window
            .update(|_, cx| dashboard.read(cx).shared_time_range().clone());
        rig.pick_time_range(shared, 1);
    });

    assert_eq!(opened_and_changed, ran(6));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

#[gpui::test]
fn running_an_edited_query_runs_it_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);

    let opened_and_edited = rig.count(|rig| {
        let chart = rig.open_new_chart();
        rig.settle();
        let typed = chart.clone();
        rig.window.update(|window, cx| {
            typed.update(cx, |chart, cx| {
                chart.type_in_query_box_for_test("SELECT value, ts FROM samples", window, cx);
            });
        });
        rig.refresh(&chart);
    });

    assert_eq!(opened_and_edited, ran(2));
}

#[gpui::test]
fn applying_a_panel_configuration_runs_it_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, _panels) = rig.build_dashboard(2, SavedChartRefreshPolicy::Off);

    let opened_and_applied = rig.count(|rig| {
        rig.show(dashboard.clone());
        rig.settle();
        rig.on_dashboard(&dashboard, |dashboard, cx| {
            dashboard.configure_apply_and_persist(0, cx);
        });
    });

    assert_eq!(opened_and_applied, ran(3));
}

/// A range changed before a panel has drawn: the panel runs once, under the slot
/// the dashboard gave it.
#[gpui::test]
fn a_panel_asked_to_run_before_its_first_frame_runs_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, _panels) = rig.build_dashboard(2, SavedChartRefreshPolicy::Off);

    let ran_on_open = rig.count(|rig| {
        rig.on_dashboard(&dashboard, |dashboard, cx| {
            dashboard.request_reexec_for_slot(0, cx);
        });
        rig.show(dashboard.clone());
    });

    assert_eq!(ran_on_open, ran(2));
    assert_eq!(rig.inflight(&dashboard), 0, "the slot came back");
}

/// Two requests before a frame are one run, as a hidden dashboard's timer makes
/// them, so they can only hold one slot.
#[gpui::test]
fn asking_twice_before_a_frame_takes_one_slot(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, _panels) = rig.build_dashboard(2, SavedChartRefreshPolicy::Off);
    rig.show(dashboard.clone());
    rig.settle();

    let asked = rig.count(|rig| {
        rig.on_dashboard(&dashboard, |dashboard, cx| {
            dashboard.request_reexec_for_slot(0, cx);
            dashboard.request_reexec_for_slot(0, cx);
        });
    });

    assert_eq!(asked, ran(1));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

/// A run that took no slot, such as a panel's first or one from its own timer, has
/// none to give back: the slot another panel holds stays taken.
#[gpui::test]
fn a_run_without_a_slot_gives_none_back(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, panels) = rig.build_dashboard(2, SavedChartRefreshPolicy::Off);
    rig.show(dashboard.clone());
    rig.settle();

    let other = panels[1].clone();
    let holding = dashboard.clone();
    rig.window.update(|_, cx| {
        holding.update(cx, |dashboard, cx| {
            dashboard.request_reexec_for_slot(0, cx);
        });
        other.update(cx, |_, cx| cx.emit(DocumentEvent::ExecutionFinished));
    });

    assert_eq!(
        rig.inflight(&dashboard),
        1,
        "the first panel still holds it"
    );

    rig.settle();
    assert_eq!(
        rig.inflight(&dashboard),
        0,
        "and hands it back when it ends"
    );
}

/// The panels past the cap wait, each runs once, and nothing stays taken.
#[gpui::test]
fn more_panels_than_slots_each_run_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let panel_count = PANEL_REEXEC_CAP + 3;
    let (dashboard, _panels) = rig.build_dashboard(panel_count, SavedChartRefreshPolicy::Off);

    let opened_and_refreshed = rig.count(|rig| {
        rig.show(dashboard.clone());
        rig.settle();
        rig.on_dashboard(&dashboard, |dashboard, cx| {
            dashboard.refresh_all_loaded_panels(cx);
        });
        assert_eq!(rig.inflight(&dashboard), PANEL_REEXEC_CAP);
    });

    assert_eq!(opened_and_refreshed, ran(panel_count * 2));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

/// A panel removed mid-run never reports, so its slot goes with it.
#[gpui::test]
fn removing_a_running_panel_gives_its_slot_back(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, panels) = rig.build_dashboard(3, SavedChartRefreshPolicy::Off);
    rig.show(dashboard.clone());
    rig.settle();
    drop(panels);

    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.request_reexec_for_slot(0, cx);
        dashboard.remove_panel(0, cx);
    });
    rig.settle();

    assert_eq!(rig.inflight(&dashboard), 0);
}

impl Rig<'_> {
    fn samples(&mut self, chart: &Entity<ChartDocument>) -> usize {
        self.window.update(|_, cx| {
            chart
                .read(cx)
                .instance_metric_buffer
                .as_ref()
                .map_or(0, |buffer| buffer.sample_count())
        })
    }

    fn dashboard_with_panel(
        &mut self,
        panel: Entity<ChartDocument>,
        policy: SavedChartRefreshPolicy,
    ) -> Entity<DashboardDocument> {
        let app_state = self.app_state.clone();
        self.window.update(|window, cx| {
            let shared_time_range = cx.new(|cx| TimeRangePanel::new("24h", Some(3), window, cx));
            cx.new(|cx| {
                DashboardDocument::new(
                    Uuid::new_v4(),
                    "Dashboard".to_string(),
                    vec![DashboardPanelSlot::Loaded {
                        panel,
                        grid_pos: PanelGridPos {
                            grid_row: 0,
                            grid_column: 0,
                            grid_width: 12,
                            grid_height: 2,
                        },
                        title_override: None,
                    }],
                    shared_time_range,
                    None,
                    policy,
                    false,
                    app_state,
                    cx,
                )
            })
        })
    }

    fn dashboard_of_one_inspector(&mut self, profile_id: Uuid) -> Entity<DashboardDocument> {
        let app_state = self.app_state.clone();
        let owner = self.profile_id;
        let dashboard_id = self.window.update(|_, cx| {
            app_state.update(cx, |state, _| {
                let id = state
                    .dashboards
                    .create_dashboard(
                        "Dashboard".to_string(),
                        None,
                        owner,
                        None,
                        SavedChartRefreshPolicy::Off,
                    )
                    .expect("dashboard");
                state
                    .dashboards
                    .append_panels(
                        id,
                        vec![DashboardPanelDraft::Inspector {
                            metric_id: "pg.activity".to_string(),
                            layout: None,
                        }],
                    )
                    .expect("panel stored");
                id
            })
        });
        self.window.update(|window, cx| {
            let state = app_state.clone();
            let inspector =
                cx.new(|cx| InspectorPanel::new(profile_id, "pg.activity".to_string(), state, cx));
            let shared_time_range = cx.new(|cx| TimeRangePanel::new("24h", Some(3), window, cx));
            cx.new(|cx| {
                DashboardDocument::new(
                    dashboard_id,
                    "Dashboard".to_string(),
                    vec![DashboardPanelSlot::Inspector {
                        entity: inspector,
                        grid_pos: PanelGridPos {
                            grid_row: 0,
                            grid_column: 0,
                            grid_width: 12,
                            grid_height: 2,
                        },
                        title_override: None,
                    }],
                    shared_time_range,
                    None,
                    SavedChartRefreshPolicy::Off,
                    false,
                    app_state,
                    cx,
                )
            })
        })
    }
}

/// An inspector whose connection is gone starts no run to report the end of, so it
/// takes no slot.
#[gpui::test]
fn an_inspector_without_a_connection_takes_no_slot(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let dashboard = rig.dashboard_of_one_inspector(Uuid::new_v4());

    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.request_reexec_for_slot(0, cx);
    });
    rig.settle();

    assert_eq!(rig.inflight(&dashboard), 0);
}

/// An inspector asked again before its run started: the older run stays off the
/// driver, and both slots come back.
#[gpui::test]
fn a_replaced_inspector_run_gives_its_slot_back(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let profile_id = rig.profile_id;
    let dashboard = rig.dashboard_of_one_inspector(profile_id);

    let asked = rig.count(|rig| {
        rig.on_dashboard(&dashboard, |dashboard, cx| {
            dashboard.request_reexec_for_slot(0, cx);
            dashboard.request_reexec_for_slot(0, cx);
        });
        assert_eq!(rig.inflight(&dashboard), 2);
    });

    assert_eq!(asked.statements, 1);
    rig.show(dashboard.clone());
    rig.settle();
    assert_eq!(rig.inflight(&dashboard), 0);
}

/// Replaced before a thread picked it up: the older run never reaches the driver.
/// This is the only case its token can still stop, and it reports its end all the
/// same, for a dashboard's slot.
#[gpui::test]
fn a_run_replaced_before_it_started_stays_off_the_driver(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let chart = rig.open_new_chart();
    rig.settle();

    let replaced = rig.count(|rig| {
        rig.request(&chart);
        rig.request(&chart);
    });

    assert_eq!(
        replaced,
        Ran {
            statements: 1,
            finished: 2
        }
    );
}

/// The same for a driver's own metric dispatch, which goes to `execute`.
#[gpui::test]
fn a_metric_run_replaced_before_it_started_stays_off_the_driver(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let chart = rig.open_metric_chart();
    rig.show(chart.clone());
    rig.settle();

    let replaced = rig.count(|rig| {
        rig.request(&chart);
        rig.request(&chart);
    });

    assert_eq!(
        replaced,
        Ran {
            statements: 1,
            finished: 2
        }
    );
}

/// A run that took no slot ends after the run that did has started: the slot stays
/// with the run that took it.
#[gpui::test]
fn the_slot_stays_with_the_run_that_took_it(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, panels) = rig.build_dashboard(2, SavedChartRefreshPolicy::Off);
    rig.run_first_frames_apart(&panels);

    // This run reaches the driver and its result waits for a frame.
    rig.request(&panels[0]);
    rig.settle();
    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.request_reexec_for_slot(0, cx);
    });
    assert_eq!(rig.inflight(&dashboard), 1);

    // The frame starts the slotted run and applies the earlier one, without parking.
    rig.show(dashboard.clone());
    assert_eq!(rig.inflight(&dashboard), 1, "the slotted run is under way");

    rig.settle();
    assert_eq!(rig.inflight(&dashboard), 0, "until it ends");
}

/// The same run ending must not let a queued panel past the cap.
#[gpui::test]
fn a_run_without_a_slot_does_not_let_the_queue_past_the_cap(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, panels) =
        rig.build_dashboard(PANEL_REEXEC_CAP + 1, SavedChartRefreshPolicy::Off);
    rig.run_first_frames_apart(&panels);

    rig.request(&panels[0]);
    rig.settle();
    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.refresh_all_loaded_panels(cx);
    });
    rig.show(dashboard.clone());

    assert_eq!(
        rig.executing(&panels[PANEL_REEXEC_CAP..]),
        0,
        "the panel in the queue waits for a slot"
    );
    assert_eq!(rig.inflight(&dashboard), PANEL_REEXEC_CAP);

    rig.settle();
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

/// An inspector's own refresh ending leaves the slot of the run the dashboard asked.
#[gpui::test]
fn an_inspector_slot_stays_with_the_run_that_took_it(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let profile_id = rig.profile_id;
    let dashboard = rig.dashboard_of_one_inspector(profile_id);
    let inspector =
        rig.window.update(
            |_, cx| match &dashboard.read(cx).panel_slots_for_testing()[0] {
                DashboardPanelSlot::Inspector { entity, .. } => entity.clone(),
                _ => unreachable!("the only slot is an inspector"),
            },
        );

    // Its own refresh, as F5 makes, whose result waits for a frame.
    let refreshed = inspector.clone();
    rig.window.update(|_, cx| {
        refreshed.update(cx, |inspector, cx| {
            inspector.request_reexec(cx);
        });
    });
    rig.settle();
    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.request_reexec_for_slot(0, cx);
    });

    rig.show(dashboard.clone());
    assert_eq!(
        rig.inflight(&dashboard),
        1,
        "the run it asked for is under way"
    );

    rig.settle();
    assert_eq!(rig.inflight(&dashboard), 0);
}

/// At the cap, a panel already marked to run has that run coming: asking again does
/// not queue another.
#[gpui::test]
fn at_the_cap_a_marked_panel_does_not_queue_again(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let panel_count = PANEL_REEXEC_CAP + 3;
    let (dashboard, panels) = rig.build_dashboard(panel_count, SavedChartRefreshPolicy::Off);
    rig.run_first_frames_apart(&panels);

    let asked = rig.count(|rig| {
        rig.on_dashboard(&dashboard, |dashboard, cx| {
            dashboard.refresh_all_loaded_panels(cx);
            dashboard.refresh_all_loaded_panels(cx);
        });
        rig.show(dashboard.clone());
    });

    assert_eq!(asked, ran(panel_count));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}

/// A run replaced before it started never touched the driver, so there is nothing
/// to clean up, and on a driver with one client mutex that would block the UI.
#[gpui::test]
fn a_run_that_never_reached_the_driver_cleans_nothing_up(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let chart = rig.open_new_chart();
    rig.settle();

    rig.count(|rig| {
        rig.request(&chart);
        rig.request(&chart);
    });

    assert_eq!(rig.cleanups.load(Ordering::SeqCst), 0);
}

#[gpui::test]
fn an_inspector_run_that_never_reached_the_driver_cleans_nothing_up(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let profile_id = rig.profile_id;
    let dashboard = rig.dashboard_of_one_inspector(profile_id);

    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.request_reexec_for_slot(0, cx);
        dashboard.request_reexec_for_slot(0, cx);
    });
    rig.settle();

    assert_eq!(rig.cleanups.load(Ordering::SeqCst), 0);
}

/// A run cancelled while in the driver still gets its cleanup.
#[gpui::test]
fn a_run_cancelled_in_the_driver_is_cleaned_up(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let chart = rig.open_new_chart();
    rig.settle();

    rig.cancel_mid_run.store(true, Ordering::SeqCst);
    let cancelled = rig.count(|rig| rig.request(&chart));

    assert_eq!(cancelled, ran(1));
    assert_eq!(rig.cleanups.load(Ordering::SeqCst), 1);
}

/// A removed panel's run may still be on the server, so its slot stays taken until
/// the run ends. Its run is cancelled, as nobody wants it any more.
#[gpui::test]
fn a_removed_panel_holds_its_slot_until_its_run_ends(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let (dashboard, panels) = rig.build_dashboard(2, SavedChartRefreshPolicy::Off);
    rig.show(dashboard.clone());
    rig.settle();

    let removed = panels[0].clone();
    drop(panels);
    let statements = rig.statements();
    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.request_reexec_for_slot(0, cx);
    });
    rig.window
        .update(|_, cx| assert!(removed.read(cx).state() == DocumentState::Executing));
    drop(removed);
    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.remove_panel(0, cx);
    });

    assert_eq!(rig.inflight(&dashboard), 1, "its run has not ended");

    rig.settle();
    assert_eq!(rig.inflight(&dashboard), 0, "and now it has");
    assert_eq!(rig.statements(), statements, "cancelled before it started");
}

/// The same for an inspector, whose result would otherwise wait for a frame that
/// never comes.
#[gpui::test]
fn a_removed_inspector_holds_its_slot_until_its_run_ends(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let profile_id = rig.profile_id;
    let dashboard = rig.dashboard_of_one_inspector(profile_id);

    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.request_reexec_for_slot(0, cx);
        dashboard.remove_panel(0, cx);
    });

    assert_eq!(rig.inflight(&dashboard), 1, "its run has not ended");

    rig.settle();
    assert_eq!(rig.inflight(&dashboard), 0, "and now it has");
}

/// The inspector's first fetch is deferred to its first frame. A run asked before
/// that frame is that fetch.
#[gpui::test]
fn an_inspector_asked_before_its_first_frame_runs_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let profile_id = rig.profile_id;
    let dashboard = rig.dashboard_of_one_inspector(profile_id);
    let inspector =
        rig.window.update(
            |_, cx| match &dashboard.read(cx).panel_slots_for_testing()[0] {
                DashboardPanelSlot::Inspector { entity, .. } => entity.clone(),
                _ => unreachable!("the only slot is an inspector"),
            },
        );
    rig.window.update(|_, cx| {
        inspector.update(cx, |inspector, _| inspector.defer_initial_exec());
    });

    let statements = rig.statements();
    rig.on_dashboard(&dashboard, |dashboard, cx| {
        dashboard.request_reexec_for_slot(0, cx);
    });
    rig.settle();
    rig.show(dashboard.clone());
    rig.settle();

    assert_eq!(rig.statements() - statements, 1);
    assert_eq!(rig.inflight(&dashboard), 0);
}

/// A chart keeps collecting samples at its own pace under a dashboard with a much
/// longer interval, and the dashboard's tick does not add a run to that.
#[gpui::test]
fn a_panel_faster_than_its_dashboard_keeps_its_own_timer(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let chart = rig.open_metric_chart();
    let timed = chart.clone();
    rig.window.update(|_, cx| {
        timed.update(cx, |chart, cx| {
            chart.set_embedded(true, cx);
            chart.set_active_tab(true);
            chart.set_refresh_policy(RefreshPolicy::Interval { every_secs: 30 }, cx);
        });
    });
    // Off the dashboard's beat, so that a tick of each cannot join into one run.
    rig.tick(Duration::from_secs(1));
    let dashboard = rig.dashboard_with_panel(
        chart.clone(),
        SavedChartRefreshPolicy::Interval { every_secs: 300 },
    );
    rig.show(dashboard.clone());
    rig.settle();
    let samples = rig.samples(&chart);

    let ticked = rig.count(|rig| {
        for _ in 0..10 {
            rig.tick(Duration::from_secs(30));
            rig.settle();
        }
    });

    assert_eq!(ticked, ran(10));
    assert_eq!(
        rig.samples(&chart) - samples,
        10,
        "the rolling window fills"
    );
    assert_eq!(rig.inflight(&dashboard), 0);
}

/// A dashboard faster than the panel runs it, and the panel's slower timer adds no
/// run on top.
#[gpui::test]
fn a_dashboard_faster_than_its_panel_runs_it(cx: &mut TestAppContext) {
    let mut rig = rig(cx);
    let chart = rig.open_metric_chart();
    let timed = chart.clone();
    rig.window.update(|_, cx| {
        timed.update(cx, |chart, cx| {
            chart.set_embedded(true, cx);
            chart.set_active_tab(true);
            chart.set_refresh_policy(RefreshPolicy::Interval { every_secs: 30 }, cx);
        });
    });
    rig.tick(Duration::from_secs(1));
    let dashboard = rig.dashboard_with_panel(
        chart.clone(),
        SavedChartRefreshPolicy::Interval { every_secs: 10 },
    );
    rig.show(dashboard.clone());
    rig.settle();

    let ticked = rig.count(|rig| {
        for _ in 0..6 {
            rig.tick(Duration::from_secs(10));
            rig.settle();
        }
    });

    assert_eq!(ticked, ran(6));
    assert_eq!(rig.inflight(&dashboard), 0, "no run slot left taken");
}
