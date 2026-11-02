//! The Explain action of the code editor: show a statement's plan without
//! running the statement.

use super::*;
use dbflux_components::actions::RunCommand;
use dbflux_core::{QueryResultShape, Value};

/// Heads what a plan reports as its query. Plain English on purpose: it is data
/// in the history and the audit log, not interface text.
const EXPLAIN_MARKER: &str = "-- Explain (plan only)";

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
    ///
    /// A driver's plan may not be sendable as text at all (SQL Server's is a
    /// session setting around the statement), and `EXPLAIN ...` would mean
    /// something else per driver, bytecode on SQLite and a statement that runs
    /// on SQL Server. So this is the statement under a comment that says what
    /// happened to it. Picked from the history it loads into the editor, where
    /// Explain plans it again and Run asks what any run of it asks.
    pub(super) fn reported_query(self, statement: &str) -> String {
        match self {
            Self::DriverExplain if !statement.trim_start().starts_with(EXPLAIN_MARKER) => {
                format!("{EXPLAIN_MARKER}\n{}", statement.trim())
            }
            Self::DriverExplain | Self::Query | Self::WrittenExplain => statement.to_string(),
        }
    }

    pub(super) fn audit_success(self, rows: u64, rows_label: &str) -> String {
        if self.shows_plan() {
            format!("Explain finished, the statement did not run: {rows} plan rows")
        } else {
            format!("Query executed successfully: {rows} rows {rows_label}")
        }
    }

    pub(super) fn audit_failure(self, error: &str) -> String {
        if self.shows_plan() {
            format!("Explain failed, the statement did not run: {error}")
        } else {
            format!("Query failed: {error}")
        }
    }

    pub(super) fn audit_cancelled(self) -> &'static str {
        if self.shows_plan() {
            "Explain cancelled"
        } else {
            "Query cancelled"
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
mod tests;
