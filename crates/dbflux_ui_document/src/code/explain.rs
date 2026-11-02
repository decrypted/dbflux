//! The Explain action of the code editor: show a statement's plan without
//! running the statement.

use super::*;
use dbflux_components::actions::RunCommand;
use dbflux_core::{QueryResultShape, Value};

/// What a run sends for its statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StatementRun {
    Query,
    /// The driver's own plan of the statement, which is not run.
    DriverExplain,
    /// A statement that already is a plan-only `EXPLAIN`, run as written.
    WrittenExplain,
}

impl StatementRun {
    pub(super) fn shows_plan(self) -> bool {
        self != Self::Query
    }

    /// What the run reports doing, for the task list, history and audit log.
    /// It never reads as a run of the statement it only planned, and picking
    /// it from the history plans again.
    pub(super) fn reported_query(self, statement: &str) -> String {
        match self {
            Self::DriverExplain => format!("EXPLAIN {}", statement.trim()),
            Self::Query | Self::WrittenExplain => statement.to_string(),
        }
    }
}

/// What Explain plans in the editor, picked as a run picks it.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ExplainTarget {
    Statement {
        text: String,
        /// Where the text starts in the buffer; `None` for a selection.
        origin: Option<usize>,
    },
    Empty,
    /// More than one statement: no single plan answers, and sending them
    /// would run all but the first on most engines.
    SeveralStatements(usize),
}

/// The selection when there is one, else the statement under the cursor.
///
/// A buffer the splitter leaves whole, such as one with a compound block, is
/// planned only if it counts as one statement, unlike a run, which confirms
/// and sends it all.
pub(super) fn explain_target(
    language: &QueryLanguage,
    buffer: &str,
    cursor: usize,
    selection: Option<&str>,
) -> ExplainTarget {
    if let Some(selection) = selection {
        return match language.statement_count(selection) {
            0 => ExplainTarget::Empty,
            1 => ExplainTarget::Statement {
                text: selection.to_string(),
                origin: None,
            },
            count => ExplainTarget::SeveralStatements(count),
        };
    }

    if let Some(range) = statements::cursor_statement(language, buffer, cursor)
        && let Some(text) = buffer.get(range.clone())
    {
        return ExplainTarget::Statement {
            text: text.to_string(),
            origin: Some(range.start),
        };
    }

    match language.statement_count(buffer) {
        0 => ExplainTarget::Empty,
        1 => ExplainTarget::Statement {
            text: buffer.to_string(),
            origin: Some(0),
        },
        count => ExplainTarget::SeveralStatements(count),
    }
}

/// How a statement reaches the database for its plan, or `None` when even
/// asking for the plan would run it (`EXPLAIN ANALYZE`, in either spelling).
pub(super) fn explain_route(statement: &str) -> Option<StatementRun> {
    if dbflux_core::starts_with_explain(statement) {
        return dbflux_core::is_plan_only_explain(statement)
            .then_some(StatementRun::WrittenExplain);
    }

    dbflux_core::is_plan_only_explain(&format!("EXPLAIN {statement}"))
        .then_some(StatementRun::DriverExplain)
}

/// A plan as the result view shows it. A plan of text lines, or with a cell
/// holding many lines, reads as text: the grid collapses a cell to one line
/// and sorts and filters rows whose order is the plan. A real table, such as
/// MySQL's or SQLite's, stays one.
pub(super) fn plan_for_display(result: &QueryResult) -> QueryResult {
    let is_table = result.shape == QueryResultShape::Table && !result.has_additional_results();
    let all_text = result
        .rows
        .iter()
        .flatten()
        .all(|cell| plan_text(cell).is_some());
    let has_multi_line_cell = result
        .rows
        .iter()
        .flatten()
        .filter_map(plan_text)
        .any(|text| text.contains('\n'));

    if !is_table
        || result.rows.is_empty()
        || !all_text
        || !(result.columns.len() == 1 || has_multi_line_cell)
    {
        return result.clone();
    }

    let body = result
        .rows
        .iter()
        .flatten()
        .filter_map(plan_text)
        .collect::<Vec<_>>()
        .join("\n");

    let mut plan = QueryResult::text(body, result.execution_time);
    plan.metadata_extra = result.metadata_extra.clone();
    plan
}

fn plan_text(cell: &Value) -> Option<&str> {
    match cell {
        Value::Text(text) | Value::Json(text) => Some(text),
        _ => None,
    }
}

/// The caption of a plan: "Explain", then the statement's own caption.
pub(super) fn plan_caption(statement_caption: Option<String>) -> String {
    let explain = dbflux_i18n::t!("document.code.result.explain_caption");

    match statement_caption {
        Some(caption) => format!("{explain} · {caption}"),
        None => explain,
    }
}

/// What the editor's right-click menu dispatches for Explain, routed like
/// the command palette's entry.
pub(super) fn explain_menu_action() -> RunCommand {
    RunCommand::new(Command::ExplainQuery.action_id())
}

impl CodeDocument {
    /// Whether this document offers Explain: SQL only, since the other query
    /// languages have no plan the drivers can ask for.
    pub(super) fn can_explain(&self) -> bool {
        !self.read_only
            && self.supports_connection_context()
            && *self.effective_language() == QueryLanguage::Sql
    }

    /// Shows the plan of the selection, or of the statement under the cursor,
    /// without running it. The result appears in the Results panel like a run.
    pub fn run_explain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || self.state == DocumentState::Executing {
            return;
        }

        let connection_runs_sql = self.connection_id.is_none_or(|conn_id| {
            self.app_state
                .read(cx)
                .connections()
                .get(&conn_id)
                .is_none_or(|connected| {
                    connected.connection.metadata().query_language == QueryLanguage::Sql
                })
        });

        if !self.can_explain() || !connection_runs_sql {
            self.explain_toast("document.code.execution.toast.explain_unavailable", cx);
            return;
        }

        let target = {
            let selection = self.selected_query(window, cx);
            let state = self.editor.input_state.read(cx);
            let buffer = state.value().to_string();
            let cursor = state.cursor();

            explain_target(
                self.effective_language(),
                &buffer,
                cursor,
                selection.as_deref(),
            )
        };

        let (statement, origin) = match target {
            ExplainTarget::Statement { text, origin } => (text, origin),
            ExplainTarget::Empty => {
                self.explain_toast("document.code.execution.toast.enter_query_to_explain", cx);
                return;
            }
            ExplainTarget::SeveralStatements(count) => {
                Toast::warning(dbflux_i18n::t!(
                    "document.code.execution.toast.explain_several_statements",
                    count = count
                ))
                .meta_right(now_hms())
                .push(cx);
                return;
            }
        };

        if statements::is_transaction_boundary(&statement) {
            self.explain_toast(
                "document.code.execution.toast.explain_transaction_control",
                cx,
            );
            return;
        }

        let Some(run) = explain_route(&statement) else {
            self.explain_toast("document.code.execution.toast.explain_would_execute", cx);
            return;
        };

        self.execution.load_all_rows = None;
        self.execution.query_origin = origin;
        self.execute_statement(statement, false, ReadOnlyEnforcement::None, run, window, cx);
    }

    fn explain_toast(&self, key: &'static str, cx: &mut Context<Self>) {
        Toast::warning(dbflux_i18n::t!(key))
            .meta_right(now_hms())
            .push(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExplainTarget, StatementRun, explain_menu_action, explain_route, explain_target,
        plan_caption, plan_for_display,
    };
    use dbflux_app::keymap::Command;
    use dbflux_core::{
        ColumnKind, ColumnMeta, QueryLanguage, QueryResult, QueryResultShape, Value,
    };
    use std::time::Duration;

    fn column(name: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.to_string(),
            type_name: "text".to_string(),
            kind: ColumnKind::Text,
            nullable: true,
            is_primary_key: false,
        }
    }

    fn table(columns: &[&str], rows: Vec<Vec<Value>>) -> QueryResult {
        QueryResult::table(
            columns.iter().map(|name| column(name)).collect(),
            rows,
            None,
            Duration::ZERO,
        )
    }

    fn text(value: &str) -> Value {
        Value::Text(value.to_string())
    }

    fn target(buffer: &str, cursor: usize, selection: Option<&str>) -> ExplainTarget {
        explain_target(&QueryLanguage::Sql, buffer, cursor, selection)
    }

    #[test]
    fn the_statement_under_the_cursor_is_the_target_of_a_buffer_of_several() {
        let buffer = "SELECT 1;\nDELETE FROM t;\nSELECT 3;";

        assert_eq!(
            target(buffer, 0, None),
            ExplainTarget::Statement {
                text: "SELECT 1".to_string(),
                origin: Some(0),
            }
        );
        assert_eq!(
            target(buffer, buffer.find("DELETE").unwrap(), None),
            ExplainTarget::Statement {
                text: "DELETE FROM t".to_string(),
                origin: buffer.find("DELETE"),
            }
        );
        assert_eq!(
            target(buffer, buffer.len(), None),
            ExplainTarget::Statement {
                text: "SELECT 3".to_string(),
                origin: buffer.find("SELECT 3"),
            }
        );
    }

    #[test]
    fn a_buffer_of_one_statement_is_the_target_whole() {
        assert_eq!(
            target("SELECT 1", 3, None),
            ExplainTarget::Statement {
                text: "SELECT 1".to_string(),
                origin: Some(0),
            }
        );
    }

    #[test]
    fn a_selection_wins_over_the_cursor_and_has_no_origin() {
        let buffer = "SELECT 1;\nSELECT 2;";

        assert_eq!(
            target(buffer, 0, Some("SELECT 2;")),
            ExplainTarget::Statement {
                text: "SELECT 2;".to_string(),
                origin: None,
            }
        );
    }

    #[test]
    fn a_selection_of_several_statements_is_refused_with_its_count() {
        assert_eq!(
            target("anything", 0, Some("SELECT 1; DELETE FROM t; SELECT 3")),
            ExplainTarget::SeveralStatements(3)
        );
    }

    #[test]
    fn a_trailing_comment_does_not_make_a_second_statement() {
        assert_eq!(
            target("anything", 0, Some("SELECT 1; -- one row")),
            ExplainTarget::Statement {
                text: "SELECT 1; -- one row".to_string(),
                origin: None,
            }
        );
    }

    #[test]
    fn a_buffer_with_a_procedure_body_is_refused_rather_than_sent_whole() {
        let buffer = "CREATE PROCEDURE p() BEGIN DELETE FROM t; SELECT 1; END";

        assert!(matches!(
            target(buffer, 0, None),
            ExplainTarget::SeveralStatements(_)
        ));
    }

    #[test]
    fn nothing_to_plan_is_empty() {
        assert_eq!(target("", 0, None), ExplainTarget::Empty);
        assert_eq!(target("  \n-- a note\n", 0, None), ExplainTarget::Empty);
        assert_eq!(target("x", 0, Some("-- a note")), ExplainTarget::Empty);
    }

    #[test]
    fn a_plain_statement_goes_to_the_driver_and_a_write_is_no_exception() {
        for statement in [
            "SELECT 1",
            "DELETE FROM t",
            "UPDATE t SET a = 1",
            "-- why\nWITH c AS (SELECT 1) SELECT * FROM c",
        ] {
            assert_eq!(
                explain_route(statement),
                Some(StatementRun::DriverExplain),
                "{statement}"
            );
        }
    }

    #[test]
    fn a_written_explain_keeps_its_form_and_is_never_prefixed_again() {
        for statement in [
            "EXPLAIN SELECT 1",
            "explain query plan SELECT 1",
            "/* mine */ EXPLAIN (COSTS OFF) SELECT 1",
            "EXPLAIN FORMAT=JSON DELETE FROM t",
        ] {
            assert_eq!(
                explain_route(statement),
                Some(StatementRun::WrittenExplain),
                "{statement}"
            );
        }
    }

    #[test]
    fn whatever_would_run_the_statement_has_no_route() {
        for statement in [
            "EXPLAIN ANALYZE SELECT 1",
            "EXPLAIN ANALYZE DELETE FROM t",
            "EXPLAIN (ANALYZE) DELETE FROM t",
            "ANALYZE DELETE FROM t",
            "(ANALYZE) DELETE FROM t",
            "(FORMAT JSON, ANALYZE) SELECT 1",
        ] {
            assert_eq!(explain_route(statement), None, "{statement}");
        }
    }

    #[test]
    fn a_plan_of_text_lines_keeps_its_lines_and_indentation() {
        let plan = plan_for_display(&table(
            &["QUERY PLAN"],
            vec![
                vec![text("Hash Join  (cost=1.00..2.00 rows=1 width=4)")],
                vec![text("  ->  Seq Scan on orders")],
            ],
        ));

        assert_eq!(plan.shape, QueryResultShape::Text);
        assert_eq!(
            plan.text_body.as_deref(),
            Some("Hash Join  (cost=1.00..2.00 rows=1 width=4)\n  ->  Seq Scan on orders")
        );
    }

    #[test]
    fn a_multi_line_cell_among_columns_reads_as_text() {
        let plan = plan_for_display(&table(
            &["explain_key", "explain_value"],
            vec![vec![text("physical_plan"), text("┌───┐\n│ X │\n└───┘")]],
        ));

        assert_eq!(plan.shape, QueryResultShape::Text);
        assert_eq!(
            plan.text_body.as_deref(),
            Some("physical_plan\n┌───┐\n│ X │\n└───┘")
        );
    }

    #[test]
    fn a_plan_that_is_a_table_stays_one() {
        let sqlite = table(
            &["id", "parent", "notused", "detail"],
            vec![vec![
                Value::Int(2),
                Value::Int(0),
                Value::Int(0),
                text("SCAN t"),
            ]],
        );
        assert_eq!(plan_for_display(&sqlite).shape, QueryResultShape::Table);

        let text_columns = table(
            &["select_type", "table"],
            vec![vec![text("SIMPLE"), text("t")]],
        );
        assert_eq!(
            plan_for_display(&text_columns).shape,
            QueryResultShape::Table
        );
    }

    #[test]
    fn an_empty_plan_stays_a_table() {
        let empty = table(&["QUERY PLAN"], Vec::new());
        assert_eq!(plan_for_display(&empty).shape, QueryResultShape::Table);
    }

    #[test]
    fn the_caption_says_it_is_a_plan_and_keeps_the_statements_caption() {
        assert_eq!(plan_caption(None), "Explain");
        assert_eq!(
            plan_caption(Some("Statement L2–2 · t".to_string())),
            "Explain · Statement L2–2 · t"
        );
    }

    #[test]
    fn what_a_run_reports_says_it_only_planned() {
        assert_eq!(
            StatementRun::DriverExplain.reported_query(" DELETE FROM t\n"),
            "EXPLAIN DELETE FROM t"
        );
        assert_eq!(
            StatementRun::WrittenExplain.reported_query("EXPLAIN SELECT 1"),
            "EXPLAIN SELECT 1"
        );
        assert_eq!(StatementRun::Query.reported_query("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn the_right_click_entry_runs_the_explain_command() {
        assert_eq!(
            dbflux_ui_base::keymap::run_command(&explain_menu_action()),
            Some(Command::ExplainQuery)
        );
    }
}

#[cfg(test)]
mod document_tests {
    // Explicit imports rather than the parent glob: combining `use super::*`
    // with `#[gpui::test]` sends the gpui_macros expansion into unbounded
    // recursion.
    use crate::code::CodeDocument;
    use crate::data_grid_panel::DataGridPanel;
    use crate::result_view::ResultViewMode;
    use dbflux_app::keymap::Command;
    use dbflux_components::controls::InputPosition;
    use dbflux_components::theme;
    use dbflux_core::{
        Connection, ConnectionProfile, DbConfig, DbDriver, DbKind, ExplainFormat, QueryLanguage,
        QueryRequest, QueryResult, QueryResultShape, Value, WritePrivilege,
    };
    use dbflux_driver_sqlite::SqliteDriver;
    use dbflux_storage::bootstrap::StorageRuntime;
    use dbflux_test_support::FakeDriver;
    use dbflux_ui_base::AppStateEntity;
    use dbflux_ui_base::modals::test_host::host_modal;
    use dbflux_ui_base::toast::{ToastGlobal, ToastHost};
    use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use std::ops::Range;
    use std::sync::Arc;
    use std::time::Duration;

    const SCRIPT: &str = "SELECT 1;\nDELETE FROM t WHERE id = 1;\nSELECT 3;";
    const FIRST_LINE: u32 = 0;
    const DELETE_LINE: u32 = 1;
    const LAST_LINE: u32 = 2;

    fn memory_profile() -> ConnectionProfile {
        ConnectionProfile::new(
            "explain",
            DbConfig::SQLite {
                path: ":memory:".into(),
                connection_id: None,
            },
        )
    }

    /// A SQL document on `connection` holding `content`, with its cursor on
    /// the zero-based `cursor_line`.
    fn open_on<'a>(
        cx: &'a mut TestAppContext,
        profile: ConnectionProfile,
        connection: Arc<dyn Connection>,
        language: QueryLanguage,
        content: &str,
        cursor_line: u32,
    ) -> (
        Entity<CodeDocument>,
        Entity<ToastHost>,
        Entity<AppStateEntity>,
        &'a mut VisualTestContext,
    ) {
        cx.update(theme::init);
        cx.update(dbflux_ui_base::keymap::init_keymap);
        let toasts = cx.update(|cx| {
            let host = cx.new(|_| ToastHost::new());
            cx.set_global(ToastGlobal { host: host.clone() });
            host
        });
        let app_state = cx.new(|_| {
            AppStateEntity::new_with_storage_runtime(
                StorageRuntime::in_memory().expect("in-memory storage"),
            )
            .expect("app state")
        });

        let profile_id = profile.id;
        cx.update(|cx| {
            app_state.update(cx, |app, _| {
                app.apply_connect_profile(
                    profile,
                    connection,
                    None,
                    None,
                    false,
                    WritePrivilege::Unknown,
                );
            });
        });

        let content = content.to_string();
        let state_for_document = app_state.clone();
        let (document, _outside, window) = host_modal(cx, move |window, cx| {
            let mut document =
                CodeDocument::new_with_language(state_for_document, None, language, window, cx);
            document.connection_id = Some(profile_id);
            document.set_content(&content, window, cx);
            document.editor.input_state.update(cx, |state, cx| {
                state.set_cursor_position(InputPosition::new(cursor_line, 0), window, cx);
            });
            document
        });
        window.run_until_parked();

        (document, toasts, app_state, window)
    }

    fn open_fake<'a>(
        cx: &'a mut TestAppContext,
        driver: &FakeDriver,
        content: &str,
        cursor_line: u32,
    ) -> (
        Entity<CodeDocument>,
        Entity<ToastHost>,
        Entity<AppStateEntity>,
        &'a mut VisualTestContext,
    ) {
        let profile = memory_profile();
        let connection = driver.connect_arc(&profile).expect("fake connection");
        open_on(
            cx,
            profile,
            connection,
            QueryLanguage::Sql,
            content,
            cursor_line,
        )
    }

    fn settle(window: &mut VisualTestContext) {
        for _ in 0..3 {
            window.run_until_parked();
            window.update(|window, _| window.refresh());
        }
        window.run_until_parked();
    }

    fn explain(window: &mut VisualTestContext, document: &Entity<CodeDocument>) {
        window.update(|window, cx| {
            document.update(cx, |document, cx| {
                document.dispatch_command(Command::ExplainQuery, window, cx);
            });
        });
        settle(window);
    }

    fn select(
        window: &mut VisualTestContext,
        document: &Entity<CodeDocument>,
        range: Range<usize>,
    ) {
        window.update(|_, cx| {
            document.update(cx, |document, cx| {
                document
                    .editor
                    .input_state
                    .update(cx, |state, cx| state.set_selected_range(range, cx));
            });
        });
    }

    fn sent(driver: &FakeDriver) -> Vec<String> {
        driver
            .stats()
            .executed_requests
            .iter()
            .map(|request| request.sql.clone())
            .collect()
    }

    fn last_toast(window: &mut VisualTestContext, toasts: &Entity<ToastHost>) -> Option<String> {
        window.update(|_, cx| toasts.read(cx).last_toast_title())
    }

    fn shown_grid(
        window: &mut VisualTestContext,
        document: &Entity<CodeDocument>,
    ) -> Entity<DataGridPanel> {
        window.update(|_, cx| {
            document
                .read(cx)
                .active_result_grid()
                .expect("a result tab")
        })
    }

    fn query_origin(
        window: &mut VisualTestContext,
        document: &Entity<CodeDocument>,
    ) -> Option<usize> {
        window.update(|_, cx| document.read(cx).execution.query_origin)
    }

    #[gpui::test]
    fn the_driver_planner_explains_only_the_statement_under_the_cursor(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
        let (document, _toasts, _state, window) = open_fake(cx, &driver, SCRIPT, LAST_LINE);

        explain(window, &document);

        assert_eq!(sent(&driver), vec!["EXPLAIN QUERY PLAN SELECT 3"]);
        assert_eq!(query_origin(window, &document), SCRIPT.find("SELECT 3"));
    }

    #[gpui::test]
    fn explaining_a_write_asks_nothing_and_sends_only_its_plan(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
        let (document, _toasts, _state, window) =
            open_fake(cx, &driver, "SELECT 1;\nDELETE FROM t;", DELETE_LINE);

        explain(window, &document);

        assert_eq!(sent(&driver), vec!["EXPLAIN QUERY PLAN DELETE FROM t"]);
        assert!(window.update(|_, cx| document.read(cx).pending.dangerous_query.is_none()));
        assert!(window.update(|_, cx| document.read(cx).pending.script_confirm.is_none()));
    }

    /// The whole-buffer bug on a real engine: the buffer used to go out as
    /// `EXPLAIN SELECT ...; DELETE ...;`, and the DELETE ran.
    #[gpui::test]
    fn a_real_sqlite_keeps_every_row_whatever_the_buffer_holds(cx: &mut TestAppContext) {
        let profile = memory_profile();
        let connection: Arc<dyn Connection> = Arc::from(
            SqliteDriver::new()
                .connect(&profile)
                .expect("in-memory SQLite"),
        );
        connection
            .execute(&QueryRequest::new(
                "CREATE TABLE t (id INTEGER PRIMARY KEY)",
            ))
            .expect("create");
        connection
            .execute(&QueryRequest::new(
                "INSERT INTO t (id) VALUES (1), (2), (3)",
            ))
            .expect("insert");
        let rows = {
            let connection = connection.clone();
            move || {
                connection
                    .execute(&QueryRequest::new("SELECT id FROM t"))
                    .expect("count")
                    .rows
                    .len()
            }
        };

        let content = "SELECT id FROM t;\nDELETE FROM t WHERE id = 1;\nDELETE FROM t;";
        let (document, toasts, _state, window) = open_on(
            cx,
            profile,
            connection,
            QueryLanguage::Sql,
            content,
            FIRST_LINE,
        );

        explain(window, &document);

        assert_eq!(rows(), 3, "only the first statement's plan was asked for");
        let plan = shown_grid(window, &document);
        let columns: Vec<String> = window.update(|_, cx| {
            let result = plan.read(cx).shown_result();
            result.columns.iter().map(|c| c.name.clone()).collect()
        });
        assert!(
            columns.iter().any(|name| name == "detail"),
            "a query plan, not bytecode: {columns:?}"
        );

        select(window, &document, 0..content.len());
        explain(window, &document);

        assert_eq!(rows(), 3, "a selection of several statements sends nothing");
        assert_eq!(
            last_toast(window, &toasts),
            Some(dbflux_i18n::t!(
                "document.code.execution.toast.explain_several_statements",
                count = 3
            ))
        );

        // The statement under the cursor may be the write itself.
        window.update(|window, cx| {
            document.update(cx, |document, cx| {
                document.editor.input_state.update(cx, |state, cx| {
                    state.set_cursor_position(InputPosition::new(2, 0), window, cx);
                });
            });
        });
        explain(window, &document);

        assert_eq!(rows(), 3, "a plan of the DELETE leaves the rows");
    }

    #[gpui::test]
    fn the_driver_explain_is_used_when_it_has_no_planner(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SqlServer)
            .with_explain_result(QueryResult::text("the plan".to_string(), Duration::ZERO));
        let (document, _toasts, _state, window) = open_fake(cx, &driver, SCRIPT, DELETE_LINE);

        explain(window, &document);

        assert!(sent(&driver).is_empty(), "no hard-coded prefix goes out");
        let requests = driver.stats().explain_requests;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].query.as_deref(),
            Some("DELETE FROM t WHERE id = 1")
        );
        assert_eq!(requests[0].format, ExplainFormat::Readable);
    }

    #[gpui::test]
    fn a_driver_without_explain_says_so_and_nothing_is_sent(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite);
        let (document, toasts, _state, window) = open_fake(cx, &driver, "SELECT 1", FIRST_LINE);

        explain(window, &document);

        assert!(sent(&driver).is_empty());
        assert!(driver.stats().explain_requests.is_empty());
        assert_eq!(
            last_toast(window, &toasts),
            Some(dbflux_i18n::t!(
                "document.code.execution.result_title.explain_failed"
            ))
        );
    }

    #[gpui::test]
    fn a_written_explain_runs_as_written_with_no_second_explain(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
        let (document, _toasts, _state, window) =
            open_fake(cx, &driver, "EXPLAIN SELECT 1", FIRST_LINE);

        explain(window, &document);

        assert_eq!(sent(&driver), vec!["EXPLAIN SELECT 1"]);
    }

    #[gpui::test]
    fn a_written_explain_analyze_is_refused_before_it_runs_the_statement(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
        let (document, toasts, _state, window) =
            open_fake(cx, &driver, "EXPLAIN ANALYZE DELETE FROM t", FIRST_LINE);

        explain(window, &document);

        assert!(sent(&driver).is_empty());
        assert_eq!(
            last_toast(window, &toasts),
            Some(dbflux_i18n::t!(
                "document.code.execution.toast.explain_would_execute"
            ))
        );
    }

    #[gpui::test]
    fn a_transaction_statement_under_the_cursor_is_not_explained(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
        let (document, toasts, _state, window) =
            open_fake(cx, &driver, "BEGIN;\nSELECT 1;\nCOMMIT;", FIRST_LINE);

        explain(window, &document);

        assert!(sent(&driver).is_empty());
        assert_eq!(
            last_toast(window, &toasts),
            Some(dbflux_i18n::t!(
                "document.code.execution.toast.explain_transaction_control"
            ))
        );
    }

    #[gpui::test]
    fn a_read_only_profile_plans_a_write_because_it_does_not_run(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
        let mut profile = memory_profile();
        profile.read_only_flag = true;
        let connection = driver.connect_arc(&profile).expect("fake connection");
        let (document, _toasts, _state, window) = open_on(
            cx,
            profile,
            connection,
            QueryLanguage::Sql,
            "DELETE FROM t",
            FIRST_LINE,
        );

        explain(window, &document);

        assert_eq!(sent(&driver), vec!["EXPLAIN QUERY PLAN DELETE FROM t"]);
    }

    #[gpui::test]
    fn a_plan_is_not_capped_by_the_editor_row_limit(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::Postgres).with_explain_planner("EXPLAIN");
        let (document, _toasts, _state, window) = open_fake(cx, &driver, "SELECT 1", FIRST_LINE);

        explain(window, &document);
        window.update(|window, cx| {
            document.update(cx, |document, cx| document.run_query(window, cx));
        });
        settle(window);

        let requests = driver.stats().executed_requests;
        assert_eq!(requests[0].sql, "EXPLAIN SELECT 1");
        assert_eq!(requests[0].limit, None);
        assert_eq!(requests[1].sql, "SELECT 1");
        assert!(
            requests[1].limit.is_some(),
            "a run on this driver is capped"
        );
    }

    #[gpui::test]
    fn the_result_tab_reads_as_a_plan_and_names_its_statement(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite)
            .with_explain_planner("EXPLAIN")
            .with_default_result(QueryResult::table(
                vec![dbflux_core::ColumnMeta {
                    name: "QUERY PLAN".to_string(),
                    type_name: "text".to_string(),
                    kind: dbflux_core::ColumnKind::Text,
                    nullable: true,
                    is_primary_key: false,
                }],
                vec![
                    vec![Value::Text("Hash Join".to_string())],
                    vec![Value::Text("  ->  Seq Scan on t".to_string())],
                ],
                None,
                Duration::ZERO,
            ));
        let (document, _toasts, state, window) = open_fake(cx, &driver, SCRIPT, DELETE_LINE);

        explain(window, &document);

        let grid = shown_grid(window, &document);
        let (caption, mode, shape, body, grid_query) = window.update(|_, cx| {
            let grid = grid.read(cx);
            (
                grid.result_caption().map(ToString::to_string),
                grid.result_view_mode(),
                grid.shown_result().shape.clone(),
                grid.shown_result().text_body.clone(),
                grid.result_query().map(str::to_string),
            )
        });

        assert_eq!(
            caption.as_deref(),
            Some("Explain · Statement L2–2 · t"),
            "the plan says what it is and which statement it answers"
        );
        assert_eq!(mode, ResultViewMode::Text);
        assert_eq!(shape, QueryResultShape::Text);
        assert_eq!(body.as_deref(), Some("Hash Join\n  ->  Seq Scan on t"));
        assert_eq!(
            grid_query.as_deref(),
            Some("EXPLAIN DELETE FROM t WHERE id = 1"),
            "the tab never holds the bare statement a rerun would execute"
        );

        let history = window.update(|_, cx| {
            state
                .read(cx)
                .history_entries()
                .first()
                .map(|entry| entry.sql.clone())
        });
        assert_eq!(
            history.as_deref(),
            Some("EXPLAIN DELETE FROM t WHERE id = 1"),
            "the history replays a plan, not the write"
        );
    }

    /// A window root that hands commands to the document, as the workspace does.
    struct MenuHost {
        document: Entity<CodeDocument>,
    }

    impl gpui::Render for MenuHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use dbflux_ui_base::keymap::{
                RunCommand, WORKSPACE_KEY_CONTEXT, root_key_context, run_command,
            };
            use gpui::{InteractiveElement as _, ParentElement as _, Styled as _};

            let context = self.document.read(cx).active_context(cx);

            gpui::div()
                .size_full()
                .key_context(root_key_context(WORKSPACE_KEY_CONTEXT, context, &[]))
                .on_action(cx.listener(|this, action: &RunCommand, window, cx| {
                    if let Some(command) = run_command(action) {
                        this.document.update(cx, |document, cx| {
                            document.dispatch_command(command, window, cx)
                        });
                    }
                }))
                .child(self.document.clone())
        }
    }

    #[gpui::test]
    fn the_right_click_menu_explains_the_statement_under_the_cursor(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, point, px};

        cx.update(gpui_component::init);
        cx.update(theme::init);
        cx.update(dbflux_ui_base::keymap::init_keymap);
        cx.update(|cx| {
            let host = cx.new(|_| ToastHost::new());
            cx.set_global(ToastGlobal { host });
        });
        let app_state = cx.new(|_| {
            AppStateEntity::new_with_storage_runtime(
                StorageRuntime::in_memory().expect("in-memory storage"),
            )
            .expect("app state")
        });

        let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
        let profile = memory_profile();
        let connection = driver.connect_arc(&profile).expect("fake connection");
        let profile_id = profile.id;
        cx.update(|cx| {
            app_state.update(cx, |app, _| {
                app.apply_connect_profile(
                    profile,
                    connection,
                    None,
                    None,
                    false,
                    WritePrivilege::Unknown,
                );
            });
        });

        let mut document = None;
        let (_root, window) = cx.add_window_view(|window, cx| {
            let code = cx.new(|cx| {
                let mut code = CodeDocument::new_with_language(
                    app_state.clone(),
                    None,
                    QueryLanguage::Sql,
                    window,
                    cx,
                );
                code.connection_id = Some(profile_id);
                code.set_content("SELECT 1", window, cx);
                code
            });
            document = Some(code.clone());
            let host = cx.new(|_| MenuHost { document: code });
            gpui_component::Root::new(host, window, cx)
        });
        let document = document.expect("the document was built");

        window.update(|window, cx| {
            document.update(cx, |document, cx| {
                document
                    .editor
                    .input_state
                    .update(cx, |state, cx| state.focus(window, cx));
            });
        });
        settle(window);

        let in_the_editor = point(px(500.0), px(300.0));
        window.simulate_mouse_down(in_the_editor, MouseButton::Right, Modifiers::default());
        window.simulate_mouse_up(in_the_editor, MouseButton::Right, Modifiers::default());
        settle(window);

        // The menu opens with nothing highlighted, so Up lands on its last
        // entry, which is the one added below the built-in items.
        window.simulate_keystrokes("up");
        settle(window);
        window.simulate_keystrokes("enter");
        settle(window);

        assert_eq!(sent(&driver), vec!["EXPLAIN QUERY PLAN SELECT 1"]);
    }

    /// The plan's text view must not stick to the tab: the next run is a table
    /// again, in the same tab.
    #[gpui::test]
    fn a_run_after_a_plan_goes_back_to_the_table_view(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite)
            .with_explain_planner("EXPLAIN")
            .with_default_result(QueryResult::table(
                vec![dbflux_core::ColumnMeta {
                    name: "line".to_string(),
                    type_name: "text".to_string(),
                    kind: dbflux_core::ColumnKind::Text,
                    nullable: true,
                    is_primary_key: false,
                }],
                vec![vec![Value::Text("one".to_string())]],
                None,
                Duration::ZERO,
            ));
        let (document, _toasts, _state, window) = open_fake(cx, &driver, "SELECT 1", FIRST_LINE);

        explain(window, &document);
        let grid = shown_grid(window, &document);
        assert_eq!(
            window.update(|_, cx| grid.read(cx).result_view_mode()),
            ResultViewMode::Text
        );

        window.update(|window, cx| {
            document.update(cx, |document, cx| document.run_query(window, cx));
        });
        settle(window);

        let grid = shown_grid(window, &document);
        let (mode, caption) = window.update(|_, cx| {
            let grid = grid.read(cx);
            (
                grid.result_view_mode(),
                grid.result_caption().map(ToString::to_string),
            )
        });
        assert_eq!(mode, ResultViewMode::Table);
        assert!(
            !caption.as_deref().unwrap_or_default().contains("Explain"),
            "{caption:?}"
        );
    }

    #[gpui::test]
    fn explain_is_offered_only_for_sql(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::MongoDB);
        let profile = memory_profile();
        let connection = driver.connect_arc(&profile).expect("fake connection");
        let (document, toasts, _state, window) = open_on(
            cx,
            profile,
            connection,
            QueryLanguage::MongoQuery,
            "db.users.find()",
            FIRST_LINE,
        );

        assert!(!window.update(|_, cx| document.read(cx).can_explain()));

        explain(window, &document);

        assert!(sent(&driver).is_empty());
        assert!(driver.stats().explain_requests.is_empty());
        assert_eq!(
            last_toast(window, &toasts),
            Some(dbflux_i18n::t!(
                "document.code.execution.toast.explain_unavailable"
            ))
        );
    }

    #[gpui::test]
    fn a_sql_buffer_on_a_connection_of_another_language_sends_nothing(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::MongoDB);
        let profile = memory_profile();
        let connection = driver.connect_arc(&profile).expect("fake connection");
        let (document, toasts, _state, window) = open_on(
            cx,
            profile,
            connection,
            QueryLanguage::Sql,
            "SELECT 1",
            FIRST_LINE,
        );

        explain(window, &document);

        assert!(sent(&driver).is_empty());
        assert_eq!(
            last_toast(window, &toasts),
            Some(dbflux_i18n::t!(
                "document.code.execution.toast.explain_unavailable"
            ))
        );
    }

    #[gpui::test]
    fn a_read_only_document_does_not_explain(cx: &mut TestAppContext) {
        let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
        let (document, _toasts, _state, window) = open_fake(cx, &driver, "SELECT 1", FIRST_LINE);
        window.update(|_, cx| document.update(cx, |document, _| document.read_only = true));

        assert!(!window.update(|_, cx| document.read(cx).can_explain()));
        explain(window, &document);

        assert!(sent(&driver).is_empty());
    }
}
