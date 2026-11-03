//! What "Open in chart" hands the chart, and what the chart makes of it.

use crate::chart_document::ChartDocument;
use crate::code::CodeDocument;
use crate::data_grid_panel::DataGridEvent;
use crate::handle::DocumentEvent;
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_components::chart::{
    BindingSpec, ChartKind, ChartSpec, InstanceMetricSource, MetricSource, YScale,
};
use dbflux_components::controls::InputPosition;
use dbflux_components::saved_chart::MetricSeries;
use dbflux_components::saved_chart::{SavedChart, SavedChartContext};
use dbflux_core::{
    ColumnKind, ColumnMeta, ConnectionProfile, DbConfig, DbKind, ExecutionContext,
    ExecutionSourceContext, QueryLanguage, QueryResult, RefreshPolicy, Value,
};
use dbflux_storage::bootstrap::StorageRuntime;
use dbflux_test_support::fake_driver::{FakeDriver, FakeQueryOutcome};
use dbflux_ui_base::AppStateEntity;
use dbflux_ui_base::toast::ToastGlobal;
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

    console_for_driver(cx, driver(kind), buffer)
}

fn console_for_driver<'a>(
    cx: &'a mut TestAppContext,
    driver: FakeDriver,
    buffer: &str,
) -> Fixture<'a> {
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
                ..
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
/// Runs each query as a saved chart: `reads` must reach the driver, `writes`
/// must not, and a refused one says why.
fn assert_gate(console: &mut Fixture<'_>, reads: &[&str], writes: &[&str]) {
    for write in writes {
        let before = console.executed().len();
        let chart = run_saved_chart(console, write);
        assert_eq!(console.executed().len(), before, "{write} ran");
        assert!(console.message(&chart).contains("read data"), "{write}");
    }

    for read in reads {
        let before = console.executed().len();
        let chart = run_saved_chart(console, read);
        assert!(console.executed().len() > before, "{read} was refused");
        assert!(!console.message(&chart).contains("read data"), "{read}");
    }
}

#[gpui::test]
fn redis_reads_chart_and_writes_do_not(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::Redis, "");
    assert_gate(
        &mut console,
        &[
            "GET k",
            "HGETALL stats",
            "LRANGE events 0 -1",
            "ZRANGE temps 0 -1",
            "ZRANGEBYSCORE temps 0 +inf WITHSCORES",
            "XRANGE events - +",
            "SCAN 0",
            "TS.RANGE cpu - +",
        ],
        &[
            "INCR page_hits",
            "SET k v",
            "HSET h f v",
            "XADD s * f v",
            "ZADD z 1 m",
            "EVAL \"return 1\" 0",
            "FLUSHALL",
        ],
    );
}

/// Mongo's real language service, with the core classifier behind it.
#[gpui::test]
fn mongo_reads_chart_and_writes_do_not(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::MongoDB, "");
    assert_gate(
        &mut console,
        &[
            "db.users.find({})",
            "db.users.aggregate([{ $match: { active: true } }])",
            "db.users.countDocuments({})",
        ],
        &[
            "db.users.insertOne({})",
            "db.users.updateOne({}, {})",
            "db.users.deleteOne({})",
            "db.users.drop()",
            "db.users.aggregate([{ $out: \"copy\" }])",
        ],
    );
}

/// CloudWatch's metadata says SQL, so a Logs Insights query judged by that
/// language looked like a write. No query mode is stored with a saved chart, so
/// the driver's default applies, as it does when the query runs.
#[gpui::test]
fn logs_insights_and_opensearch_queries_chart(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::CloudWatchLogs, "");
    assert_gate(
        &mut console,
        &[
            "stats count(*) by bin(5m)",
            "fields @timestamp, @message | filter @message like /error/",
            "source logs | stats count() by host",
        ],
        &[],
    );
}

#[gpui::test]
fn a_console_query_mode_names_the_language_of_the_chart(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::CloudWatchLogs, "SELECT * FROM logs");
    let document = console.document.clone();
    console.window.update(|_, cx| {
        document.update(cx, |document, _| {
            document.source.exec_ctx.source = Some(ExecutionSourceContext::CollectionWindow {
                targets: vec!["group".to_string()],
                start_ms: 0,
                end_ms: 1,
                query_mode: Some("sql".to_string()),
            });
        });
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
    let source = request.execution_context.and_then(|context| context.source);
    assert!(
        matches!(
            source,
            Some(ExecutionSourceContext::CollectionWindow { query_mode: Some(ref mode), .. })
                if mode == "sql"
        ),
        "{source:?}"
    );
}

/// A read-only profile is held to the same language: its Logs Insights charts
/// keep reading.
#[gpui::test]
fn a_read_only_profile_does_not_refuse_logs_insights(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::CloudWatchLogs, "");
    let app_state = console.app_state.clone();
    let profile_id = console.profile_id;
    console.window.update(|_, cx| {
        app_state.update(cx, |state, _| {
            if let Some(connected) = state.connections_mut().get_mut(&profile_id) {
                connected.read_only_reason = Some(dbflux_core::ReadOnlyReason::ProfileSetting);
            }
        });
    });

    assert_gate(&mut console, &["stats count(*) by bin(5m)"], &[]);
}

#[gpui::test]
fn influx_reads_chart_and_writes_do_not(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::InfluxDB, "");
    assert_gate(
        &mut console,
        &[
            "SELECT mean(v) FROM cpu WHERE $timeFilter GROUP BY time(1m)",
            "from(bucket: \"b\") |> range(start: -1h)",
        ],
        &["DROP MEASUREMENT cpu", "DELETE FROM cpu"],
    );
}

/// Metric sources are the driver's own dispatch, not a query anyone wrote: they
/// carry no text to judge, on a language that would refuse any.
#[gpui::test]
fn metric_sources_keep_running(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::CloudWatchLogs, "");
    let app_state = console.app_state.clone();
    let profile_id = console.profile_id;

    let instance = console.window.update(|window, cx| {
        cx.new(|cx| {
            ChartDocument::new_with_source(
                Some(profile_id),
                "metric".to_string(),
                Box::new(InstanceMetricSource {
                    metric_id: "connections".to_string(),
                }),
                app_state.clone(),
                window,
                cx,
            )
        })
    });
    console.window.update(|_, cx| {
        instance.update(cx, |chart, cx| {
            chart.on_time_range_changed(Some(0), Some(1_000), cx);
        });
    });
    console.run_chart(&instance);
    assert_eq!(console.driver.stats().executed_requests.len(), 1);
    assert!(!console.message(&instance).contains("read data"));

    let metric = console.window.update(|window, cx| {
        cx.new(|cx| {
            ChartDocument::new_with_source(
                Some(profile_id),
                "metric".to_string(),
                Box::new(MetricSource {
                    series: vec![MetricSeries {
                        namespace: "AWS/RDS".to_string(),
                        metric_name: "CPUUtilization".to_string(),
                        dimensions: Vec::new(),
                        period_seconds: 60,
                        statistic: "Average".to_string(),
                        region: None,
                        label: None,
                    }],
                }),
                app_state,
                window,
                cx,
            )
        })
    });
    console.window.update(|_, cx| {
        metric.update(cx, |chart, cx| {
            chart.on_time_range_changed(Some(0), Some(1_000), cx);
        });
    });
    console.run_chart(&metric);
    assert_eq!(console.driver.stats().executed_requests.len(), 2);
}

/// Counts how often `chart` reports a finished run.
fn finishes_of(console: &mut Fixture<'_>, chart: &Entity<ChartDocument>) -> Rc<RefCell<usize>> {
    let count: Rc<RefCell<usize>> = Rc::default();
    let sink = count.clone();
    console.window.update(|_, cx| {
        cx.subscribe(chart, move |_, event: &DocumentEvent, _| {
            if matches!(event, DocumentEvent::ExecutionFinished) {
                *sink.borrow_mut() += 1;
            }
        })
        .detach();
    });
    count
}

/// A dashboard holds a run slot until the panel reports back, so a request that
/// never starts has to report back too: once, whatever stopped it.
#[gpui::test]
fn every_request_that_starts_no_run_reports_finished_once(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::Postgres, "");
    let app_state = console.app_state.clone();
    let profile_id = console.profile_id;

    // (profile, query, database)
    let cases: [(Option<Uuid>, &str, Option<&str>); 5] = [
        (Some(profile_id), "delete from users", None),
        (None, SECOND_STATEMENT, None),
        (Some(Uuid::new_v4()), SECOND_STATEMENT, None),
        (Some(profile_id), "", None),
        (Some(profile_id), SECOND_STATEMENT, Some("still_connecting")),
    ];

    for (profile, query, database) in cases {
        let app_state = app_state.clone();
        let chart = console.window.update(|window, cx| {
            cx.new(|cx| {
                let mut chart =
                    ChartDocument::new(profile, query.to_string(), app_state, window, cx);
                chart.set_execution_context(ExecutionContext {
                    database: database.map(str::to_string),
                    ..ExecutionContext::default()
                });
                chart
            })
        });
        let finishes = finishes_of(&mut console, &chart);

        let window_handle = chart.clone();
        console.window.update(|window, cx| {
            window_handle.update(cx, |chart, cx| chart.request_reexecute(window, cx));
        });
        console.window.run_until_parked();

        assert_eq!(*finishes.borrow(), 1, "{profile:?} {query:?} {database:?}");
    }
    assert!(console.executed().is_empty());

    // A source that cannot build its request: a metric chart without a window.
    let chart = console.window.update(|window, cx| {
        cx.new(|cx| {
            ChartDocument::new_with_source(
                Some(profile_id),
                "metric".to_string(),
                Box::new(MetricSource { series: Vec::new() }),
                app_state,
                window,
                cx,
            )
        })
    });
    let finishes = finishes_of(&mut console, &chart);
    let handle = chart.clone();
    console.window.update(|window, cx| {
        handle.update(cx, |chart, cx| chart.request_reexecute(window, cx));
    });
    console.window.run_until_parked();
    assert_eq!(*finishes.borrow(), 1);
}

/// A finished run reports once, from the result; a cancelled one has no result
/// to report from, and reports when it ends.
#[gpui::test]
fn a_run_reports_finished_once_when_it_ends_or_is_cancelled(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    let finishes = finishes_of(&mut console, &chart);
    console.run_chart(&chart);
    assert_eq!(*finishes.borrow(), 1, "a run that ends");

    let handle = chart.clone();
    console.window.update(|window, cx| {
        handle.update(cx, |chart, cx| {
            chart.request_reexecute(window, cx);
            chart.cancel_run_for_test(cx);
        });
    });
    console.window.run_until_parked();
    assert_eq!(*finishes.borrow(), 2, "a run that was cancelled");
}

/// The same refusal on every tick of a dashboard's timer is one notice.
#[gpui::test]
fn a_repeated_refusal_is_reported_once(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    let refusals = |window: &mut VisualTestContext| {
        window.update(|_, cx| {
            cx.global::<ToastGlobal>()
                .host
                .read(cx)
                .toast_titles()
                .iter()
                .filter(|title| title.contains("read data"))
                .count()
        })
    };

    // Runs twice already.
    let chart = run_saved_chart(&mut console, "delete from users");
    console.run_chart(&chart);

    assert!(console.message(&chart).contains("read data"));
    assert_eq!(refusals(console.window), 1);
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
                    shell.set_y_scale(YScale::Log, cx);
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
    assert_eq!(
        console
            .window
            .update(|_, cx| reopened.read(cx).chart_shell_for_test().read(cx).y_scale()),
        YScale::Log
    );
}

// ---------------------------------------------------------------------------
// Where a saved chart runs
// ---------------------------------------------------------------------------

const CALLS_QUERY: &str = "select * from api_call_log";

fn database_list(names: &[&str]) -> dbflux_core::SchemaSnapshot {
    dbflux_core::SchemaSnapshot::relational(dbflux_core::RelationalSchema {
        databases: names
            .iter()
            .map(|name| dbflux_core::DatabaseInfo {
                name: (*name).to_string(),
                is_current: false,
            })
            .collect(),
        ..dbflux_core::RelationalSchema::default()
    })
}

impl Fixture<'_> {
    /// What the connection reports having listed when it connected.
    fn list_databases(&mut self, names: &[&str]) {
        let app_state = self.app_state.clone();
        let profile_id = self.profile_id;
        let schema = database_list(names);
        self.window.update(|_, cx| {
            app_state.update(cx, |state, _| {
                state
                    .connections_mut()
                    .get_mut(&profile_id)
                    .expect("connected")
                    .schema = Some(schema);
            });
        });
    }

    /// Saves `chart` under a name through the same path as the Save prompt and
    /// returns the record storage now holds.
    fn save(&mut self, chart: &Entity<ChartDocument>, name: &str) -> SavedChart {
        let chart = chart.clone();
        let app_state = self.app_state.clone();
        let name = name.to_string();
        let profile_id = self.profile_id;
        self.window.update(|_, cx| {
            // A saved chart references its profile's row, which connecting alone
            // does not write.
            app_state.update(cx, |state, _| {
                if !state
                    .profiles()
                    .iter()
                    .any(|profile| profile.id == profile_id)
                {
                    let profile = state.connections()[&profile_id].profile.clone();
                    state.add_profile_in_folder(profile, None);
                }
            });
            chart.update(cx, |chart, cx| chart.save_as(name.clone(), cx));
            app_state
                .read(cx)
                .saved_charts
                .all_charts()
                .iter()
                .find(|saved| saved.name == name)
                .cloned()
                .expect("the chart was saved")
        })
    }

    fn reopen(&mut self, saved: &SavedChart) -> Entity<ChartDocument> {
        let app_state = self.app_state.clone();
        let saved = saved.clone();
        self.window.update(|window, cx| {
            cx.new(|cx| ChartDocument::from_saved(&saved, app_state, window, cx).expect("a chart"))
        })
    }

    fn last_request(&self) -> dbflux_core::QueryRequest {
        self.driver
            .stats()
            .executed_requests
            .last()
            .cloned()
            .expect("the chart ran")
    }

    fn saved_with(&self, context: SavedChartContext) -> SavedChart {
        let mut saved = SavedChart::new_query(
            "saved".to_string(),
            self.profile_id,
            CALLS_QUERY.to_string(),
            ChartSpec::from_detection(1, vec![3], &api_call_log_result().columns, 10_000)
                .expect("a spec"),
            BindingSpec::default(),
        );
        saved.context = context;
        saved
    }

    /// One request to `chart`, as a refresh makes.
    fn request(&mut self, chart: &Entity<ChartDocument>) {
        let chart = chart.clone();
        self.window.update(|window, cx| {
            chart.update(cx, |chart, cx| chart.request_reexecute(window, cx));
        });
    }

    fn register_driver(&mut self) {
        let app_state = self.app_state.clone();
        let profile_id = self.profile_id;
        let driver = self.driver.clone().as_driver_arc();
        self.window.update(|_, cx| {
            app_state.update(cx, |state, _| {
                let driver_id = state
                    .connections()
                    .get(&profile_id)
                    .expect("connected")
                    .profile
                    .driver_id();
                state.facade.connections.drivers.insert(driver_id, driver);
            });
        });
    }
}

fn database_context(database: &str) -> SavedChartContext {
    SavedChartContext {
        database: Some(database.to_string()),
        ..SavedChartContext::default()
    }
}

fn log_groups(request: &dbflux_core::QueryRequest) -> Vec<String> {
    match request
        .execution_context
        .as_ref()
        .and_then(|context| context.source.as_ref())
    {
        Some(ExecutionSourceContext::CollectionWindow { targets, .. }) => targets.clone(),
        other => panic!("no window source: {other:?}"),
    }
}

/// The chart from a console keeps the console's database through Save, and the
/// reopened chart sends it. Without this a reopened chart ran on the
/// connection's default database.
#[gpui::test]
fn a_saved_chart_reopens_on_its_database_and_schema(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    console.set_active_database("analytics");
    let document = console.document.clone();
    console.window.update(|_, cx| {
        document.update(cx, |document, _| {
            document.source.exec_ctx.schema = Some("reporting".to_string());
        });
    });
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    let saved = console.save(&chart, "calls");
    assert_eq!(saved.context.database.as_deref(), Some("analytics"));
    assert_eq!(saved.context.schema.as_deref(), Some("reporting"));

    console.set_active_database("other");
    let reopened = console.reopen(&saved);
    console.run_chart(&reopened);

    let request = console.last_request();
    assert_eq!(request.database.as_deref(), Some("analytics"));
    let context = request.execution_context.expect("a context");
    assert_eq!(context.database.as_deref(), Some("analytics"));
    assert_eq!(context.schema.as_deref(), Some("reporting"));
}

/// A Logs Insights chart keeps its log groups and query mode, without which the
/// driver answers "Select at least one CloudWatch log group".
#[gpui::test]
fn a_saved_logs_chart_reopens_with_its_log_groups_and_mode(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::CloudWatchLogs, SECOND_STATEMENT);
    let document = console.document.clone();
    console.window.update(|_, cx| {
        document.update(cx, |document, _| {
            document.source.exec_ctx.source = Some(ExecutionSourceContext::CollectionWindow {
                targets: vec!["/aws/app".to_string(), "/aws/worker".to_string()],
                start_ms: 1,
                end_ms: 2,
                query_mode: Some("sql".to_string()),
            });
        });
    });
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    let saved = console.save(&chart, "logs");
    assert_eq!(saved.context.targets, ["/aws/app", "/aws/worker"]);
    assert_eq!(saved.context.query_mode.as_deref(), Some("sql"));

    let reopened = console.reopen(&saved);
    console.run_chart(&reopened);

    let request = console.last_request();
    assert_eq!(log_groups(&request), ["/aws/app", "/aws/worker"]);
    match request.execution_context.and_then(|context| context.source) {
        Some(ExecutionSourceContext::CollectionWindow {
            start_ms,
            end_ms,
            query_mode,
            ..
        }) => {
            assert_eq!(query_mode.as_deref(), Some("sql"));
            assert!(start_ms < end_ms, "{start_ms}..{end_ms}");
            assert_ne!((start_ms, end_ms), (1, 2), "bounds are not stored");
        }
        other => panic!("{other:?}"),
    }
}

/// The driver's own answer about a log group that is gone reaches the chart.
#[gpui::test]
fn a_driver_error_about_log_groups_stays_in_the_chart(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::CloudWatchLogs, "");
    let saved = console.saved_with(SavedChartContext {
        targets: vec!["/aws/gone".to_string()],
        ..SavedChartContext::default()
    });
    console.driver.set_query_outcome(
        CALLS_QUERY,
        FakeQueryOutcome::Error("/aws/gone not found".into()),
    );

    let chart = console.reopen(&saved);
    console.run_chart(&chart);

    assert!(console.message(&chart).contains("/aws/gone"));
}

/// A chart stays on its database while the sidebar's moves, including through a
/// refresh and a Save.
#[gpui::test]
fn a_chart_on_a_stored_database_keeps_reading_it(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    let saved = console.saved_with(database_context("analytics"));
    console.set_active_database("app");

    let chart = console.reopen(&saved);
    console.run_chart(&chart);
    console.set_active_database("other");
    console.run_chart(&chart);

    let requests = console.driver.stats().executed_requests;
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.database.as_deref() == Some("analytics")),
        "{requests:?}"
    );

    let resaved = console.save(&chart, "saved again");
    assert_eq!(resaved.context, database_context("analytics"));
}

/// A chart saved before this stored nothing: it sends no database, and saving it
/// again does not invent one from whatever the sidebar selects.
#[gpui::test]
fn a_chart_without_a_stored_context_runs_as_before(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    console.set_active_database("analytics");
    console.list_databases(&["app"]);
    let saved = console.saved_with(SavedChartContext::default());

    let chart = console.reopen(&saved);
    console.run_chart(&chart);

    assert!(console.last_request().database.is_none());
    assert_eq!(
        console.save(&chart, "still nothing").context,
        SavedChartContext::default()
    );
}

/// A database the connection does not have stops the chart and is named, where
/// MongoDB would answer with no rows and another driver could read elsewhere.
#[gpui::test]
fn a_stored_database_the_connection_lacks_is_named_and_reported_once(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    console.list_databases(&["app", "billing"]);
    let saved = console.saved_with(database_context("gone"));

    let chart = console.reopen(&saved);
    let finishes = finishes_of(&mut console, &chart);
    console.request(&chart);
    console.window.run_until_parked();

    assert_eq!(*finishes.borrow(), 1);
    assert!(console.executed().is_empty(), "nothing ran elsewhere");
    assert!(console.message(&chart).contains("gone"));

    // Back on the list after a refresh of the connection, it runs.
    console.list_databases(&["app", "gone"]);
    console.run_chart(&chart);
    assert_eq!(console.last_request().database.as_deref(), Some("gone"));
}

/// A list that says nothing leaves the answer to the driver, and the check is
/// only for a stored database: a console's own is live and already ran there.
#[gpui::test]
fn only_a_stored_database_on_a_known_list_is_refused(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    console.list_databases(&[]);
    let saved = console.saved_with(database_context("analytics"));
    let chart = console.reopen(&saved);
    console.run_chart(&chart);
    assert_eq!(
        console.last_request().database.as_deref(),
        Some("analytics")
    );

    console.list_databases(&["app"]);
    console.set_active_database("created_just_now");
    console.click_open_in_chart();
    console.open_the_chart();
    assert_eq!(
        console.last_request().database.as_deref(),
        Some("created_just_now")
    );
}

/// The driver's refusal of a database reaches the chart too.
#[gpui::test]
fn a_driver_error_about_the_database_stays_in_the_chart(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    let saved = console.saved_with(database_context("gone"));
    console.driver.set_query_outcome(
        CALLS_QUERY,
        FakeQueryOutcome::Error("USE database failed: Unknown database 'gone'".into()),
    );

    let chart = console.reopen(&saved);
    let finishes = finishes_of(&mut console, &chart);
    console.run_chart(&chart);

    assert!(console.message(&chart).contains("gone"));
    assert_eq!(*finishes.borrow(), 1);
}

/// A driver with one fixed database has none to remember, and behaves as today.
#[gpui::test]
fn a_driver_without_databases_stores_none(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::SQLite, SECOND_STATEMENT);
    console.set_active_database("main");
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    let saved = console.save(&chart, "local");

    assert_eq!(saved.context, SavedChartContext::default());
}

/// With one connection per database nothing opens the database of a saved chart
/// after a restart, so the chart does, and runs there once it can.
#[gpui::test]
fn a_chart_opens_the_connection_its_database_needs(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let driver = driver(DbKind::Postgres).with_per_database_connections();
    let mut console = console_for_driver(cx, driver, "");
    console.register_driver();
    console.list_databases(&["app", "analytics"]);
    let saved = console.saved_with(database_context("analytics"));

    let chart = console.reopen(&saved);
    let finishes = finishes_of(&mut console, &chart);
    console.request(&chart);
    assert_eq!(*finishes.borrow(), 0, "the run still owes its report");
    assert!(console.message(&chart).contains("analytics"));

    // A request meanwhile has no run of its own.
    console.request(&chart);
    assert_eq!(*finishes.borrow(), 1);

    console.window.run_until_parked();
    let handle = chart.clone();
    console.window.update(|_, cx| {
        handle.update(cx, |chart, cx| chart.apply_pending_result_for_test(cx));
    });

    assert_eq!(*finishes.borrow(), 2, "the run that waited reports once");
    assert_eq!(console.executed(), [CALLS_QUERY]);
    assert_eq!(
        console.last_request().database.as_deref(),
        Some("analytics")
    );
    let opened = console.window.update(|_, cx| {
        console
            .app_state
            .read(cx)
            .connections()
            .get(&console.profile_id)
            .expect("connected")
            .database_connection("analytics")
            .is_some()
    });
    assert!(opened);
}

/// A database that cannot be opened says so, reports once, and only a refresh by
/// hand tries again: a dashboard's timer would repeat the failure each tick.
#[gpui::test]
fn a_database_that_cannot_open_is_named_and_not_retried_by_a_timer(cx: &mut TestAppContext) {
    // No driver is registered, which fails the same way a refused connect does.
    init_keyboard_runtime(cx);
    let driver = driver(DbKind::Postgres).with_per_database_connections();
    let mut console = console_for_driver(cx, driver, "");
    let saved = console.saved_with(database_context("analytics"));

    let chart = console.reopen(&saved);
    let finishes = finishes_of(&mut console, &chart);
    console.request(&chart);
    console.window.run_until_parked();

    assert_eq!(*finishes.borrow(), 1);
    assert!(console.message(&chart).contains("analytics"));

    console.register_driver();
    console.request(&chart);
    console.window.run_until_parked();
    assert_eq!(*finishes.borrow(), 2);
    assert!(console.executed().is_empty(), "the timer does not retry");

    let handle = chart.clone();
    console.window.update(|window, cx| {
        handle.update(cx, |chart, cx| {
            chart.retry_database_on_next_run();
            chart.request_reexecute(window, cx);
        });
    });
    console.window.run_until_parked();
    assert_eq!(console.executed(), [CALLS_QUERY]);
    let handle = chart.clone();
    console.window.update(|_, cx| {
        handle.update(cx, |chart, cx| chart.apply_pending_result_for_test(cx));
    });
    assert_eq!(*finishes.borrow(), 3);
}

/// Panels run where their saved charts stored, and one that cannot run hands its
/// run slot back, so the panels queued behind it still get their turn.
#[gpui::test]
fn dashboard_panels_use_the_stored_context_and_free_their_slots(cx: &mut TestAppContext) {
    use crate::dashboard::{DashboardDocument, DashboardPanelSlot, PANEL_REEXEC_CAP, PanelGridPos};
    use dbflux_components::common::time_range::view::TimeRangePanel;
    use dbflux_components::saved_chart::SavedChartRefreshPolicy;

    let mut console = console_with(cx, "");
    console.list_databases(&["app", "analytics"]);

    let panel_count = PANEL_REEXEC_CAP + 2;
    let saved: Vec<SavedChart> = (0..panel_count)
        .map(|index| {
            console.saved_with(database_context(if index == 0 {
                "analytics"
            } else {
                "gone"
            }))
        })
        .collect();

    let charts: Vec<Entity<ChartDocument>> =
        saved.iter().map(|saved| console.reopen(saved)).collect();
    let slots: Vec<DashboardPanelSlot> = charts
        .iter()
        .map(|chart| DashboardPanelSlot::Loaded {
            panel: chart.clone(),
            grid_pos: PanelGridPos {
                grid_row: 0,
                grid_column: 0,
                grid_width: 6,
                grid_height: 2,
            },
            title_override: None,
        })
        .collect();

    let app_state = console.app_state.clone();
    let dashboard = console.window.update(|window, cx| {
        let shared_time_range = cx.new(|cx| TimeRangePanel::new("24h", Some(3), window, cx));
        cx.new(|cx| {
            DashboardDocument::new(
                Uuid::new_v4(),
                "Dashboard".to_string(),
                slots,
                shared_time_range,
                None,
                SavedChartRefreshPolicy::Off,
                false,
                app_state,
                cx,
            )
        })
    });

    let requested = dashboard.clone();
    console.window.update(|_, cx| {
        requested.update(cx, |dashboard, cx| {
            for slot in 0..panel_count {
                dashboard.request_reexec_for_slot(slot, cx);
            }
        });
    });

    // Rendering drains what the dashboard marked; a test without a frame does it
    // by hand, once per request the dashboard admitted.
    for _ in 0..panel_count {
        let admitted: Vec<Entity<ChartDocument>> = console.window.update(|_, cx| {
            charts
                .iter()
                .filter(|chart| chart.read(cx).has_pending_reexecute_for_test())
                .cloned()
                .collect()
        });
        for chart in admitted {
            let handle = chart.clone();
            console.window.update(|window, cx| {
                handle.update(cx, |chart, cx| {
                    chart.drain_pending_reexecute_for_test(window, cx)
                });
            });
            console.window.run_until_parked();
            console.window.update(|_, cx| {
                chart.update(cx, |chart, cx| chart.apply_pending_result_for_test(cx));
            });
        }
        console.window.run_until_parked();
    }

    let inflight = console
        .window
        .update(|_, cx| dashboard.read(cx).inflight_reexec_count_for_testing());
    assert_eq!(inflight, 0, "every panel handed its slot back");

    let requests = console.driver.stats().executed_requests;
    assert_eq!(requests.len(), 1, "only the panel on a known database ran");
    assert_eq!(requests[0].database.as_deref(), Some("analytics"));
    assert!(console.message(&charts[1]).contains("gone"));
    assert!(console.message(&charts[panel_count - 1]).contains("gone"));
}

/// A console with a session of its own holds its transaction there, where the
/// chart cannot see it, and the chart says so only while that is the case.
#[gpui::test]
fn the_transaction_note_appears_only_with_an_open_session_transaction(cx: &mut TestAppContext) {
    let mut console = console_on_sessions(cx);

    let opens = |console: &mut Fixture<'_>| {
        console.events.borrow_mut().clear();
        console.click_open_in_chart();
        console
            .events
            .borrow()
            .iter()
            .find_map(|event| match event {
                DocumentEvent::ChartThisQuery {
                    console_transaction_open,
                    ..
                } => Some(*console_transaction_open),
                _ => None,
            })
            .expect("the button asked for a chart")
    };

    assert!(!opens(&mut console), "nothing has run yet");

    console.run_on_session("select 1");
    assert!(!opens(&mut console), "no transaction is open");

    console.run_on_session("BEGIN");
    assert!(opens(&mut console), "BEGIN opened one");

    let chart = console.window.update(|window, cx| {
        let app_state = console.app_state.clone();
        cx.new(|cx| {
            let mut chart = ChartDocument::new(
                Some(console.profile_id),
                SECOND_STATEMENT.to_string(),
                app_state,
                window,
                cx,
            );
            chart.set_console_transaction_note(true);
            chart
        })
    });
    let note = console
        .window
        .update(|_, cx| chart.read(cx).transaction_note());
    assert!(note.is_some_and(|note| note.contains("uncommitted")));

    console.run_on_session("select 2");
    assert!(opens(&mut console), "still open");

    console.run_on_session("COMMIT");
    assert!(!opens(&mut console), "COMMIT ended it");

    console.run_on_session("BEGIN");
    console.run_on_session("ROLLBACK");
    assert!(!opens(&mut console), "ROLLBACK ended it");

    console.run_on_session("BEGIN");
    let document = console.document.clone();
    console.window.update(|_, cx| {
        document.update(cx, |document, cx| document.invalidate_execution_session(cx));
    });
    console.window.run_until_parked();
    assert!(
        !opens(&mut console),
        "the session went, and its transaction"
    );
}

/// Without a session the console runs on the shared connection the chart uses, so
/// a chart does see what it has not committed and the note would be false.
#[gpui::test]
fn the_transaction_note_stays_away_from_a_shared_connection(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    console.run_on_session("BEGIN");

    console.click_open_in_chart();
    let open = console.events.borrow().iter().any(|event| {
        matches!(
            event,
            DocumentEvent::ChartThisQuery {
                console_transaction_open: true,
                ..
            }
        )
    });
    assert!(!open);
}

/// Saving from the Chart view of a result keeps where the result ran, even once
/// the document has moved on. Without this such a chart stored nothing.
#[gpui::test]
fn a_chart_saved_from_a_result_keeps_where_it_ran(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    console.set_active_database("analytics");

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
    let profile_id = console.profile_id;
    let spec = ChartSpec::from_detection(1, vec![3], &api_call_log_result().columns, 10_000)
        .expect("a spec");
    let chart = console.window.update(|_, cx| {
        grid.read(cx).query_result_chart(
            "from the result".to_string(),
            profile_id,
            SECOND_STATEMENT.to_string(),
            spec,
            BindingSpec::default(),
            cx,
        )
    });

    assert_eq!(chart.context.database.as_deref(), Some("analytics"));
}

/// A reused result tab shows the latest run, so saving its chart keeps where that
/// run went. It kept the first run's database.
#[gpui::test]
fn a_reused_result_tab_saves_where_its_latest_run_went(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);

    let document = console.document.clone();
    let pick = |console: &mut Fixture<'_>, database: &str| {
        let document = document.clone();
        console.window.update(|_, cx| {
            document.update(cx, |document, _| {
                document.source.exec_ctx.database = Some(database.to_string());
            });
        });
    };
    let run = |console: &mut Fixture<'_>| {
        let document = document.clone();
        console.window.update(|window, cx| {
            document.update(cx, |document, cx| document.run_query(window, cx));
        });
        console.window.run_until_parked();
        console.window.update(|window, _| window.refresh());
        console.window.run_until_parked();
    };
    pick(&mut console, "first");
    run(&mut console);
    pick(&mut console, "second");
    run(&mut console);

    let grid = console
        .window
        .update(|_, cx| console.document.read(cx).active_result_grid())
        .expect("the run made a result grid");
    let profile_id = console.profile_id;
    let spec = ChartSpec::from_detection(1, vec![3], &api_call_log_result().columns, 10_000)
        .expect("a spec");
    let chart = console.window.update(|_, cx| {
        grid.read(cx).query_result_chart(
            "latest".to_string(),
            profile_id,
            SECOND_STATEMENT.to_string(),
            spec,
            BindingSpec::default(),
            cx,
        )
    });

    assert_eq!(chart.context.database.as_deref(), Some("second"));
}

/// A chart runs where it can leave nothing on the session a console uses, and a
/// console's own run stays on it.
#[gpui::test]
fn a_chart_runs_unattended_and_a_console_does_not(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    let saved = console.saved_with(database_context("analytics"));
    let chart = console.reopen(&saved);
    console.run_chart(&chart);

    let unattended = console.driver.stats().unattended_requests;
    assert_eq!(unattended.len(), 1);
    assert_eq!(unattended[0].database.as_deref(), Some("analytics"));

    console.run_on_session("select 1");
    assert_eq!(console.driver.stats().unattended_requests.len(), 1);
    assert_eq!(console.driver.stats().executed_requests.len(), 2);
}

/// What the chart keeps out: a driver's own metric dispatch stays where it was.
#[gpui::test]
fn a_metric_chart_does_not_run_unattended(cx: &mut TestAppContext) {
    let mut console = console_on(cx, DbKind::CloudWatchLogs, "");
    let app_state = console.app_state.clone();
    let profile_id = console.profile_id;
    let chart = console.window.update(|window, cx| {
        cx.new(|cx| {
            let mut chart = ChartDocument::new_with_source(
                Some(profile_id),
                "metric".to_string(),
                Box::new(InstanceMetricSource {
                    metric_id: "m".to_string(),
                }),
                app_state,
                window,
                cx,
            );
            chart.stage_window_for_test(1, 2);
            chart
        })
    });
    console.run_chart(&chart);

    assert!(console.driver.stats().unattended_requests.is_empty());
}

/// Every way an open can end is visible once and reports once: a cancelled open
/// has no coordinator report behind it, so the chart makes one.
#[gpui::test]
fn a_cancelled_open_is_named_reported_and_finishes_once(cx: &mut TestAppContext) {
    init_keyboard_runtime(cx);
    let driver = driver(DbKind::Postgres).with_per_database_connections();
    let mut console = console_for_driver(cx, driver, "");
    console.register_driver();
    let saved = console.saved_with(database_context("analytics"));

    let app_state = console.app_state.clone();
    let reports: Rc<RefCell<usize>> = Rc::default();
    let sink = reports.clone();
    console.window.update(|_, cx| {
        cx.set_global(dbflux_ui_base::AppStateGlobal {
            entity: app_state.clone(),
        });
        cx.subscribe(
            &app_state,
            move |_, _: &dbflux_ui_base::UserErrorReported, _| {
                *sink.borrow_mut() += 1;
            },
        )
        .detach();
    });

    let chart = console.reopen(&saved);
    let finishes = finishes_of(&mut console, &chart);
    console.request(&chart);
    assert_eq!(*finishes.borrow(), 0);

    let key = dbflux_ui_base::object_tree::ObjectTreeRequestKey::DatabaseSchema {
        profile_id: console.profile_id,
        database: "analytics".to_string(),
    };
    console.window.update(|_, cx| {
        app_state.update(cx, |state, cx| state.object_tree_cancel(&key, cx));
    });
    console.window.run_until_parked();

    assert_eq!(*finishes.borrow(), 1);
    assert_eq!(*reports.borrow(), 1);
    assert!(console.message(&chart).contains("analytics"));
    assert!(console.executed().is_empty());
}

/// The dashboard's timer asks every panel on every tick, so a database that
/// cannot open must not be tried again from there; its Refresh does try.
#[gpui::test]
fn the_dashboard_timer_does_not_retry_a_database_but_refresh_does(cx: &mut TestAppContext) {
    use crate::dashboard::{DashboardDocument, DashboardPanelSlot, PanelGridPos};
    use dbflux_components::common::time_range::view::TimeRangePanel;
    use dbflux_components::saved_chart::SavedChartRefreshPolicy;

    // No driver is registered yet, so opening fails.
    init_keyboard_runtime(cx);
    let driver = driver(DbKind::Postgres).with_per_database_connections();
    let mut console = console_for_driver(cx, driver, "");
    let saved = console.saved_with(database_context("analytics"));
    let chart = console.reopen(&saved);

    let app_state = console.app_state.clone();
    let panel = chart.clone();
    let dashboard = console.window.update(|window, cx| {
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
                        grid_width: 6,
                        grid_height: 2,
                    },
                    title_override: None,
                }],
                shared_time_range,
                None,
                SavedChartRefreshPolicy::Interval { every_secs: 30 },
                false,
                app_state,
                cx,
            )
        })
    });
    let timed = dashboard.clone();
    console.window.update(|_, cx| {
        timed.update(cx, |dashboard, cx| {
            dashboard.set_active_tab(true, cx);
            dashboard.update_refresh_timer(cx);
        });
    });

    let drain = |console: &mut Fixture<'_>| {
        let handle = chart.clone();
        console.window.update(|window, cx| {
            handle.update(cx, |chart, cx| {
                chart.drain_pending_reexecute_for_test(window, cx);
            });
        });
        console.window.run_until_parked();
    };
    let tick = |console: &mut Fixture<'_>| {
        console
            .window
            .executor()
            .advance_clock(Duration::from_secs(31));
        console.window.run_until_parked();
        let panel = chart.clone();
        let marked = console
            .window
            .update(|_, cx| panel.read(cx).has_pending_reexecute_for_test());
        assert!(marked, "the timer asked the panel");
    };

    tick(&mut console);
    drain(&mut console);
    assert!(console.message(&chart).contains("analytics"));

    console.register_driver();
    tick(&mut console);
    drain(&mut console);
    assert!(console.executed().is_empty(), "a tick does not try again");

    let refreshed = dashboard.clone();
    console.window.update(|_, cx| {
        refreshed.update(cx, |dashboard, cx| dashboard.refresh_all_loaded_panels(cx));
    });
    drain(&mut console);
    assert_eq!(console.executed(), [CALLS_QUERY], "Refresh does");
}

/// A chart saved just now keeps running where it just ran, even on a list that
/// has not caught up with a new database. Only a reopened chart is checked.
#[gpui::test]
fn saving_does_not_put_a_live_chart_under_the_list_check(cx: &mut TestAppContext) {
    let mut console = console_with(cx, SECOND_STATEMENT);
    console.list_databases(&["app"]);
    console.set_active_database("created_just_now");
    console.click_open_in_chart();
    let chart = console.open_the_chart();

    let saved = console.save(&chart, "new database");
    console.run_chart(&chart);
    assert_eq!(console.driver.stats().executed_requests.len(), 2);

    let reopened = console.reopen(&saved);
    console.run_chart(&reopened);
    assert_eq!(console.driver.stats().executed_requests.len(), 2, "refused");
    assert!(console.message(&reopened).contains("created_just_now"));
}

/// A chart without the flag shows no note, a saved one included.
#[gpui::test]
fn a_chart_without_the_flag_shows_no_note(cx: &mut TestAppContext) {
    let mut console = console_with(cx, "");
    let saved = console.saved_with(SavedChartContext::default());
    let chart = console.reopen(&saved);

    let note = console
        .window
        .update(|_, cx| chart.read(cx).transaction_note());
    assert_eq!(note, None);
}

fn console_on_sessions<'a>(cx: &'a mut TestAppContext) -> Fixture<'a> {
    init_keyboard_runtime(cx);

    let driver = driver(DbKind::DuckDB).with_execution_sessions();
    console_for_driver(cx, driver, SECOND_STATEMENT)
}

impl Fixture<'_> {
    /// Runs `sql` the way the editor runs a statement: through the document's
    /// session binding.
    fn run_on_session(&mut self, sql: &str) {
        let document = self.document.clone();
        let app_state = self.app_state.clone();
        let profile_id = self.profile_id;
        let request = dbflux_core::QueryRequest::new(sql.to_string());
        self.window.update(|_, cx| {
            let root = app_state
                .read(cx)
                .connections()
                .get(&profile_id)
                .expect("connected")
                .connection
                .clone();
            document
                .read(cx)
                .execution_session
                .execute(root, None, &request)
                .result
                .expect("the statement ran");
        });
    }
}
