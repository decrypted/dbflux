//! Asking a connection for a query plan without running the query.

use dbflux_policy::ExecutionClassification;

use super::language_service::strip_leading_comments;
use super::safety::classify_sql_execution;
use super::semantic::SemanticRequest;
use super::table_browser::{ExplainFormat, ExplainRequest};
use super::types::{QueryRequest, QueryResult};
use crate::{Connection, DbError, QueryLanguage};

/// Whether the SQL text `explain` only plans: one statement, without an
/// `ANALYZE` that would run what it explains.
///
/// Stricter than the governance read check, which lets `EXPLAIN ANALYZE` of a
/// read pass: a plan-only explain must not run anything at all.
pub fn is_plan_only_explain(explain: &str) -> bool {
    classify_sql_execution(explain) == ExecutionClassification::Metadata
}

/// Whether `sql` opens with the `EXPLAIN` keyword, after any comments.
pub fn starts_with_explain(sql: &str) -> bool {
    let statement = strip_leading_comments(sql);
    let keyword_end = statement
        .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .unwrap_or(statement.len());

    statement
        .get(..keyword_end)
        .is_some_and(|keyword| keyword.eq_ignore_ascii_case("EXPLAIN"))
}

/// The plan of the SQL statement in `statement.sql`, in the form a person
/// reads, without running the statement. `statement` otherwise carries the
/// database and execution context the statement would have run with.
///
/// A driver with a planner names the text it wants to run, which is checked
/// and run like any request. A driver without one plans the statement itself,
/// so what it would be asked is checked first: a statement such as
/// `ANALYZE DELETE ...` would become `EXPLAIN ANALYZE DELETE ...`, and several
/// statements in one text can run on engines that execute all but the last
/// while preparing.
///
/// A driver with neither answers `DbError::NotSupported`, and nothing is sent.
#[expect(
    clippy::result_large_err,
    reason = "returns the driver's own error unchanged, like `Connection::execute`"
)]
pub fn explain_plan_only(
    connection: &dyn Connection,
    statement: &QueryRequest,
) -> Result<QueryResult, DbError> {
    let query = statement.sql.trim();
    let request = ExplainRequest::for_query(query)
        .with_format(ExplainFormat::Readable)
        .with_database(statement.database.clone());

    match connection.plan_semantic_request(&SemanticRequest::Explain(request.clone())) {
        Ok(plan) => {
            let Some(planned) = plan.primary_query() else {
                return Err(DbError::NotSupported(
                    "the driver planned no statement for this explain".to_string(),
                ));
            };

            if planned.language != QueryLanguage::Sql {
                return Err(DbError::NotSupported(
                    "the driver does not plan this explain in SQL".to_string(),
                ));
            }

            refuse_unless_plan_only(&planned.text)?;

            let mut plan_request = statement.clone();
            plan_request.sql = planned.text.clone();
            if planned.target_database.is_some() {
                plan_request.database = planned.target_database.clone();
            }

            connection.execute(&plan_request)
        }
        Err(DbError::NotSupported(_)) => {
            refuse_unless_plan_only(&format!("EXPLAIN {query}"))?;
            connection.explain(&request)
        }
        Err(error) => Err(error),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "shares `explain_plan_only`'s error type"
)]
fn refuse_unless_plan_only(explain: &str) -> Result<(), DbError> {
    if is_plan_only_explain(explain) {
        return Ok(());
    }

    Err(DbError::query_failed(
        "this explain would run the statement or hold more than one, so it was refused \
         before reaching the database",
    ))
}
