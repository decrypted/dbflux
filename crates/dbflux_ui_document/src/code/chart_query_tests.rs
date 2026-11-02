//! What "Open in chart" hands the chart, and what the chart makes of it.

use super::CodeDocument;
use crate::chart_document::ChartDocument;
use crate::data_grid_panel::DataGridEvent;
use crate::handle::DocumentEvent;
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_components::chart::{BindingSpec, ChartKind, ChartSpec};
use dbflux_components::controls::InputPosition;
use dbflux_components::saved_chart::SavedChart;
use dbflux_core::{
    ColumnKind, ColumnMeta, ConnectionProfile, DbConfig, DbKind, ExecutionContext, QueryLanguage,
    QueryResult, RefreshPolicy, Value,
};
use dbflux_storage::bootstrap::StorageRuntime;
use dbflux_test_support::fake_driver::{FakeDriver, FakeQueryOutcome};
use dbflux_ui_base::AppStateEntity;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use uuid::Uuid;

/// The shape of the reported buffer: a statement above the one the cursor
/// is on, and a table with a timestamp and a measure under the cursor.
const FIRST_STATEMENT: &str = "select id, name from users;";
const SECOND_STATEMENT: &str = "select * from api_call_log acl";

fn buffer() -> String {
    format!("{FIRST_STATEMENT}\n\n\n\n{SECOND_STATEMENT}")
}

fn column(name: &str, kind: ColumnKind) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        type_name: String::new(),
        kind,
        nullable: true,
        is_primary_key: false,
    }
}

fn users_result() -> QueryResult {
    let rows = (1..=3)
        .map(|id| vec![Value::Int(id), Value::Text(format!("user {id}"))])
        .collect();

    QueryResult::table(
        vec![
            column("id", ColumnKind::Integer),
            column("name", ColumnKind::Text),
        ],
        rows,
        None,
        Duration::ZERO,
    )
}

fn api_call_log_result() -> QueryResult {
    let rows = (0..4)
        .map(|index| {
            vec![
                Value::Int(100 + index),
                Value::Int(index * 1_000),
                Value::Int(0),
                Value::Float(index as f64 / 2.0),
            ]
        })
        .collect();

    QueryResult::table(
        vec![
            column("id", ColumnKind::Integer),
            column("created_at", ColumnKind::Timestamp),
            column("is_error", ColumnKind::Integer),
            column("runtime", ColumnKind::Float),
        ],
        rows,
        None,
        Duration::ZERO,
    )
}

/// A connection answering a whole buffer of two statements the way MySQL does:
/// with the first statement's rows as the primary result set.
fn driver(kind: DbKind) -> FakeDriver {
    FakeDriver::new(kind)
        .with_query_result(buffer(), users_result())
        .with_query_result(SECOND_STATEMENT, api_call_log_result())
        .with_query_result(FIRST_STATEMENT, users_result())
}

struct Fixture<'a> {
    window: &'a mut VisualTestContext,
    app_state: Entity<AppStateEntity>,
    document: Entity<CodeDocument>,
    driver: FakeDriver,
    profile_id: Uuid,
    events: Rc<RefCell<Vec<DocumentEvent>>>,
}

fn console_with<'a>(cx: &'a mut TestAppContext, buffer: &str) -> Fixture<'a> {
    console_on(cx, DbKind::MySQL, buffer)
}

fn console_on<'a>(cx: &'a mut TestAppContext, kind: DbKind, buffer: &str) -> Fixture<'a> {
    init_keyboard_runtime(cx);

    let driver = driver(kind);
    let profile = ConnectionProfile::new(
        "test",
        DbConfig::SQLite {
            path: ":memory:".into(),
            connection_id: None,
        },
    );
    let profile_id = profile.id;
    let connection = driver.connect_arc(&profile).expect("fake connection");

    let app_state = cx.update(|cx| {
        cx.new(|_| {
            let storage_runtime = StorageRuntime::in_memory().expect("isolated storage runtime");
            let mut state = AppStateEntity::new_with_storage_runtime(storage_runtime)
                .expect("test storage setup");
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

    let content = buffer.to_string();
    let state_for_doc = app_state.clone();
    let (document, window) = cx.add_window_view(move |window, cx| {
        let mut document = CodeDocument::new_with_language(
            state_for_doc,
            Some(profile_id),
            QueryLanguage::Sql,
            window,
            cx,
        );
        document.set_content(&content, window, cx);
        document
    });

    let events: Rc<RefCell<Vec<DocumentEvent>>> = Rc::default();
    let sink = events.clone();
    window.update(|_, cx| {
        cx.subscribe(&document, move |_, event: &DocumentEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
        .detach();
    });

    Fixture {
        window,
        app_state,
        document,
        driver,
        profile_id,
        events,
    }
}

impl Fixture<'_> {
    fn run_chart(&mut self, chart: &Entity<ChartDocument>) {
        let chart = chart.clone();
        self.window.update(|window, cx| {
            chart.update(cx, |chart, cx| chart.request_reexecute(window, cx));
        });
        self.window.run_until_parked();
        self.window.update(|_, cx| {
            chart.update(cx, |chart, cx| chart.apply_pending_result_for_test(cx));
        });
    }

    fn put_cursor_on_line(&mut self, line: u32) {
        let document = self.document.clone();
        self.window.update(|window, cx| {
            let input = document.read(cx).editor.input_state.clone();
            input.update(cx, |state, cx| {
                state.set_cursor_position(InputPosition::new(line, 0), window, cx);
            });
        });
    }

    fn click_open_in_chart(&mut self) {
        let document = self.document.clone();
        self.window.update(|_, cx| {
            document.update(cx, |document, cx| document.emit_chart_this_query(cx));
        });
        self.window.run_until_parked();
    }

    /// What the button sent: the query, the connection and the context.
    fn sent(&self) -> Option<(String, Option<Uuid>, ExecutionContext)> {
        self.events.borrow().iter().find_map(|event| match event {
            DocumentEvent::ChartThisQuery {
                query,
                connection_id,
                context,
            } => Some((query.clone(), *connection_id, context.clone())),
            _ => None,
        })
    }

    fn executed(&self) -> Vec<String> {
        self.driver
            .stats()
            .executed_requests
            .iter()
            .map(|request| request.sql.clone())
            .collect()
    }

    /// Opens the chart the workspace would open for the button and runs it.
    fn open_the_chart(&mut self) -> Entity<ChartDocument> {
        let (query, connection_id, context) = self.sent().expect("the button asked for a chart");
        let app_state = self.app_state.clone();
        let chart = self.window.update(|window, cx| {
            cx.new(|cx| {
                let mut chart = ChartDocument::new(connection_id, query, app_state, window, cx);
                chart.set_execution_context(context);
                chart
            })
        });
        self.run_chart(&chart);

        chart
    }

    fn set_active_database(&mut self, database: &str) {
        let app_state = self.app_state.clone();
        let profile_id = self.profile_id;
        let database = database.to_string();
        self.window.update(|_, cx| {
            app_state.update(cx, |state, _| {
                state.set_active_database(profile_id, Some(database));
            });
        });
    }

    fn bindings(&mut self, chart: &Entity<ChartDocument>) -> BindingSpec {
        self.window
            .update(|_, cx| chart.read(cx).active_bindings(cx))
    }

    fn message(&mut self, chart: &Entity<ChartDocument>) -> String {
        self.window
            .update(|_, cx| chart.read(cx).degraded_message(cx))
    }

    fn draws(&mut self, chart: &Entity<ChartDocument>) -> bool {
        self.window.update(|_, cx| {
            chart
                .read(cx)
                .chart_shell_for_test()
                .read(cx)
                .chart_view()
                .is_some()
        })
    }
}

/// Reported: with a statement above the cursor, Run reads `api_call_log` and
/// the chart button draws nothing, `X = id`, `Y = none`, "No time column",
/// because the buffer went to the driver and a script's primary result set is
/// its first statement's.
#[gpui::test]
fn the_chart_runs_the_statement_run_would_run(cx: &mut TestAppContext) {
    let mut console = console_with(cx, &buffer());
    console.put_cursor_on_line(4);
    console.click_open_in_chart();

    let chart = console.open_the_chart();
    assert_eq!(console.executed(), vec![SECOND_STATEMENT.to_string()]);

    let bindings = console.bindings(&chart);
    assert_eq!(bindings.x, 1, "created_at is the X axis");
    assert_eq!(bindings.y, vec![3], "runtime is the Y axis, not the id");
    assert!(console.draws(&chart));
}

#[gpui::test]
fn the_cursor_picks_the_statement_it_is_on(cx: &mut TestAppContext) {
    let mut console = console_with(cx, &buffer());
    console.put_cursor_on_line(0);
    console.click_open_in_chart();

    let (query, _, _) = console.sent().expect("the button asked for a chart");
    assert_eq!(
        query,
        FIRST_STATEMENT.trim_end_matches(';'),
        "as Run sends it"
    );
}

#[gpui::test]
fn a_selection_is_charted_as_it_is(cx: &mut TestAppContext) {
    let mut console = console_with(cx, &buffer());
    let document = console.document.clone();
    let start = FIRST_STATEMENT.len() + 4;
    let end = start + "select *".len();
    console.window.update(|_, cx| {
        let input = document.read(cx).editor.input_state.clone();
        input.update(cx, |state, cx| state.set_selected_range(start..end, cx));
    });
    console.click_open_in_chart();

    let (query, _, _) = console.sent().expect("the button asked for a chart");
    assert_eq!(query, "select *");
}

/// An unqualified table resolves where the console runs it. The chart used to
/// run with no database at all, on whatever the shared connection last used.
#[gpui::test]
fn the_chart_runs_in_the_database_the_console_runs_in(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    console.set_active_database("analytics");
    console.click_open_in_chart();

    let (_, connection_id, context) = console.sent().expect("the button asked for a chart");
    assert_eq!(connection_id, Some(console.profile_id));
    assert_eq!(context.database.as_deref(), Some("analytics"));

    // The console's database stays with the chart when the connection moves on.
    console.set_active_database("other");
    console.open_the_chart();

    let request = console
        .driver
        .stats()
        .executed_requests
        .last()
        .cloned()
        .expect("the chart ran");
    assert_eq!(request.database.as_deref(), Some("analytics"));
    assert_eq!(
        request
            .execution_context
            .and_then(|context| context.database)
            .as_deref(),
        Some("analytics")
    );
}

/// Run caps the rows of `SELECT *`; the chart runs the same statement and must
/// not read the whole table where Run read 10,000 rows.
#[gpui::test]
fn the_chart_takes_the_row_limit_run_takes(cx: &mut TestAppContext) {
    // The fake's Postgres advertises the row limit; its MySQL does not.
    let mut console = console_on(cx, DbKind::Postgres, SECOND_STATEMENT);
    let limit = console.window.update(|_, cx| {
        console
            .app_state
            .read(cx)
            .general_settings()
            .editor_row_limit
    });
    console.click_open_in_chart();
    console.open_the_chart();

    let request = console
        .driver
        .stats()
        .executed_requests
        .last()
        .cloned()
        .expect("the chart ran");
    assert_eq!(request.limit, Some(limit as u32));
}

/// A chart re-runs its query with nobody watching, so a statement that changes
/// data never reaches it. This used to run without the confirmation Run asks.
#[gpui::test]
fn a_statement_that_changes_data_is_not_charted(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "delete from users where id = 1");
    console.click_open_in_chart();

    assert!(console.sent().is_none());
    assert!(console.executed().is_empty());
}

#[gpui::test]
fn a_transaction_boundary_is_not_charted(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "begin;\nselect 1");
    console.put_cursor_on_line(0);
    console.click_open_in_chart();

    assert!(console.sent().is_none());
}

#[gpui::test]
fn a_selection_of_two_statements_is_not_charted(cx: &mut TestAppContext) {
    let buffer = buffer();
    let mut console = console_with(cx, &buffer);
    let document = console.document.clone();
    console.window.update(|_, cx| {
        let input = document.read(cx).editor.input_state.clone();
        input.update(cx, |state, cx| {
            state.set_selected_range(0..buffer.len(), cx);
        });
    });
    console.click_open_in_chart();

    assert!(console.sent().is_none());
}

#[gpui::test]
fn no_connection_opens_no_chart(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    let document = console.document.clone();
    console.window.update(|_, cx| {
        document.update(cx, |document, _| document.connection_id = None);
    });
    console.click_open_in_chart();

    assert!(console.sent().is_none());
}

/// The results menu's "Chart this query" reached the document and stopped
/// there, on a comment saying the menu never offers it.
#[gpui::test]
fn the_results_menu_charts_the_statement_of_the_result(cx: &mut TestAppContext) {
    let mut console = console_with(cx, &buffer());
    console.set_active_database("analytics");
    console.put_cursor_on_line(4);

    let document = console.document.clone();
    console.window.update(|window, cx| {
        document.update(cx, |document, cx| document.run_query(window, cx));
    });
    console.window.run_until_parked();
    console.window.update(|window, _| window.refresh());
    console.window.run_until_parked();

    console.set_active_database("other");

    let grid = console
        .window
        .update(|_, cx| document.read(cx).active_result_grid())
        .expect("the run made a result grid");
    console.window.update(|_, cx| {
        grid.update(cx, |grid, cx| {
            let query = grid
                .chart_host_current_query(cx)
                .expect("the result's query");
            let connection_id = grid.chart_host_connection_id(cx);
            cx.emit(DataGridEvent::ChartThisQuery {
                query,
                connection_id,
            });
        });
    });
    console.window.run_until_parked();

    let (query, connection_id, context) = console.sent().expect("the menu asked for a chart");
    assert_eq!(query, SECOND_STATEMENT);
    assert_eq!(connection_id, Some(console.profile_id));
    assert_eq!(context.database.as_deref(), Some("analytics"));
}

/// A chart of a result without a time column says what is missing and draws
/// once a Y column is picked, against the numeric X the picker offers.
#[gpui::test]
fn without_a_time_column_the_chart_asks_for_y_and_then_draws(cx: &mut TestAppContext) {
    let mut console = console_with(cx, FIRST_STATEMENT);
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    assert_eq!(
        console.bindings(&chart).x,
        0,
        "id is the first numeric column"
    );
    assert!(!console.draws(&chart));
    let message = console.message(&chart);
    assert!(
        message.contains("Y column") && message.contains("id"),
        "names the next step and the X column: {message}"
    );

    console.window.update(|_, cx| {
        chart.update(cx, |chart, cx| {
            chart.apply_binding_spec(
                BindingSpec {
                    x: 0,
                    y: vec![0],
                    ..BindingSpec::default()
                },
                cx,
            );
        });
    });
    assert!(!console.draws(&chart), "Y equal to X has nothing to plot");
    assert!(console.message(&chart).contains("Y column"));

    // A numeric X with a numeric Y draws, and a re-run keeps the picks.
    let result = QueryResult::table(
        vec![
            column("bucket", ColumnKind::Integer),
            column("hits", ColumnKind::Integer),
        ],
        vec![
            vec![Value::Int(1), Value::Int(7)],
            vec![Value::Int(2), Value::Int(9)],
        ],
        None,
        Duration::ZERO,
    );
    console
        .driver
        .set_query_outcome(FIRST_STATEMENT, FakeQueryOutcome::success(result));
    console.run_chart(&chart);
    console.window.update(|_, cx| {
        chart.update(cx, |chart, cx| {
            chart.apply_binding_spec(
                BindingSpec {
                    x: 0,
                    y: vec![1],
                    ..BindingSpec::default()
                },
                cx,
            );
        });
    });
    assert!(console.draws(&chart));

    console.run_chart(&chart);
    assert!(console.draws(&chart), "the picks survive a re-run");
}

#[gpui::test]
fn a_query_error_stays_visible_in_the_chart(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "select broken");
    console.driver.set_query_outcome(
        "select broken",
        FakeQueryOutcome::Error("no such column".into()),
    );
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    assert!(console.message(&chart).contains("no such column"));
}

#[gpui::test]
fn a_query_without_rows_says_so(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "select nothing");
    console.driver.set_query_outcome(
        "select nothing",
        FakeQueryOutcome::success(QueryResult::table(
            vec![column("ts", ColumnKind::Timestamp)],
            Vec::new(),
            None,
            Duration::ZERO,
        )),
    );
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    assert!(console.message(&chart).contains("no rows"));
}

/// The connection dropped between the button and the run.
#[gpui::test]
fn a_missing_connection_stays_visible_in_the_chart(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    let app_state = console.app_state.clone();
    let chart = console.window.update(|window, cx| {
        cx.new(|cx| {
            ChartDocument::new(
                Some(Uuid::new_v4()),
                SECOND_STATEMENT.to_string(),
                app_state,
                window,
                cx,
            )
        })
    });
    console.run_chart(&chart);

    assert!(console.message(&chart).contains("not found"));
}

/// Opens a chart of `query` the way a saved chart or a dashboard panel does,
/// past the button's check, and runs it twice as a refresh would.
fn run_saved_chart(console: &mut Fixture<'_>, query: &str) -> Entity<ChartDocument> {
    let saved = SavedChart::new_query(
        "saved".to_string(),
        console.profile_id,
        query.to_string(),
        ChartSpec::from_detection(1, vec![3], &api_call_log_result().columns, 10_000)
            .expect("a spec"),
        BindingSpec::default(),
    );
    let app_state = console.app_state.clone();
    let chart = console.window.update(|window, cx| {
        cx.new(|cx| {
            ChartDocument::from_saved(&saved, app_state, window, cx).expect("a query chart")
        })
    });
    console.run_chart(&chart);
    console.run_chart(&chart);

    chart
}

/// A saved chart stores no database. It keeps running where it always did, and
/// never follows the database selected in the sidebar from one refresh to the
/// next, which would draw a same-named table from somewhere else.
#[gpui::test]
fn a_saved_chart_does_not_follow_the_active_database(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    console.set_active_database("analytics");
    let chart = run_saved_chart(&mut console, SECOND_STATEMENT);
    console.set_active_database("other");
    console.run_chart(&chart);

    let requests = console.driver.stats().executed_requests;
    assert_eq!(requests.len(), 3);
    assert!(
        requests.iter().all(|request| request.database.is_none()),
        "{requests:?}"
    );
}

/// The check sits where the chart runs, so nothing opens around it.
#[gpui::test]
fn a_saved_chart_that_changes_data_never_runs(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    let chart = run_saved_chart(&mut console, "delete from users where id = 1");

    assert!(console.executed().is_empty());
    assert!(console.message(&chart).contains("read data"));
}

/// An interval on a refused query would only repeat the refusal.
#[gpui::test]
fn a_refused_query_stops_auto_refresh(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    let chart = run_saved_chart(&mut console, "delete from users where id = 1");

    console.window.update(|_, cx| {
        chart.update(cx, |chart, cx| {
            chart.set_refresh_policy(RefreshPolicy::Interval { every_secs: 30 }, cx);
        });
    });
    console.run_chart(&chart);

    assert_eq!(
        console
            .window
            .update(|_, cx| chart.read(cx).refresh_policy()),
        RefreshPolicy::Manual
    );
    assert!(console.executed().is_empty());
}

/// A profile set to read-only after the chart was saved is asked too.
#[gpui::test]
fn a_saved_chart_is_held_to_a_read_only_profile(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    let app_state = console.app_state.clone();
    let profile_id = console.profile_id;
    console.window.update(|_, cx| {
        app_state.update(cx, |state, _| {
            if let Some(connected) = state.connections_mut().get_mut(&profile_id) {
                connected.read_only_reason = Some(dbflux_core::ReadOnlyReason::ProfileSetting);
            }
        });
    });
    let chart = run_saved_chart(&mut console, "delete from users where id = 1");

    assert!(console.executed().is_empty());
    assert!(console.message(&chart).contains("read-only"));
}

/// A buffer saved whole before the chart took one statement drew only the
/// first statement's rows, and cannot be told from a batch with a write.
#[gpui::test]
fn a_saved_chart_of_two_statements_says_why_it_stopped(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    let chart = run_saved_chart(&mut console, &buffer());

    assert!(console.executed().is_empty());
    assert!(console.message(&chart).contains("one statement"));
}

/// Writes in the languages core can classify are refused like SQL writes, not
/// only the ones the danger check calls dangerous.
#[gpui::test]
fn a_redis_write_is_not_charted(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::Redis, "");
    for write in ["INCR page_hits", "SET k v", "EVAL \"return 1\" 0"] {
        let chart = run_saved_chart(&mut console, write);
        assert!(console.message(&chart).contains("read data"), "{write}");
    }
    assert!(console.executed().is_empty());

    run_saved_chart(&mut console, "GET k");
    assert!(!console.executed().is_empty(), "a read still runs");
}

#[gpui::test]
fn a_mongo_write_is_not_charted(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::MongoDB, "");
    for write in [
        "db.users.insertOne({})",
        "db.users.updateOne({}, {})",
        "db.users.deleteOne({})",
    ] {
        let chart = run_saved_chart(&mut console, write);
        assert!(console.message(&chart).contains("read data"), "{write}");
    }
    assert!(console.executed().is_empty());

    run_saved_chart(&mut console, "db.users.find({})");
    assert!(!console.executed().is_empty(), "a read still runs");
}

/// A driver with one connection per database may still be opening this one.
#[gpui::test]
fn a_database_still_connecting_says_so_in_the_chart(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::Postgres, SECOND_STATEMENT);
    console.set_active_database("analytics");
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    assert!(console.executed().is_empty());
    assert!(console.message(&chart).contains("analytics"));
}

/// Save keeps what the chart shows, and a saved chart reopens with it. Both
/// used to fall back to whatever detection found.
#[gpui::test]
fn a_saved_chart_reopens_with_its_columns_and_kind(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    let saved = console.window.update(|_, cx| {
        chart.update(cx, |chart, cx| {
            chart.apply_binding_spec(
                BindingSpec {
                    x: 1,
                    y: vec![2],
                    ..BindingSpec::default()
                },
                cx,
            );
            chart
                .chart_shell_for_test()
                .clone()
                .update(cx, |shell, cx| {
                    shell.set_chart_kind(ChartKind::Bar, cx);
                });
        });

        let chart = chart.read(cx);
        let columns = chart.last_result_columns().unwrap_or_default();
        let spec = chart
            .chart_shell_for_test()
            .read(cx)
            .current_chart_spec(&columns);
        let bindings = spec.binding.clone();
        SavedChart::new_query(
            "calls".to_string(),
            console.profile_id,
            SECOND_STATEMENT.to_string(),
            spec,
            bindings,
        )
    });

    let app_state = console.app_state.clone();
    let reopened = console.window.update(|window, cx| {
        cx.new(|cx| {
            ChartDocument::from_saved(&saved, app_state, window, cx).expect("a query chart")
        })
    });
    console.run_chart(&reopened);

    assert_eq!(console.bindings(&reopened).y, vec![2]);
    assert_eq!(
        console
            .window
            .update(|_, cx| reopened.read(cx).chart_kind(cx)),
        ChartKind::Bar
    );
}
