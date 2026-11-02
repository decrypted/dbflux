#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dbflux_core::{
    ConnectionProfile, DbConfig, DbError, DbKind, ExplainFormat, QueryRequest, QueryResult,
    explain_plan_only, is_plan_only_explain, starts_with_explain,
};
use dbflux_test_support::FakeDriver;
use std::time::Duration;

fn connection(driver: &FakeDriver) -> std::sync::Arc<dyn dbflux_core::Connection> {
    let profile = ConnectionProfile::new(
        "explain",
        DbConfig::SQLite {
            path: ":memory:".into(),
            connection_id: None,
        },
    );
    driver.connect_arc(&profile).unwrap()
}

fn sent_sql(driver: &FakeDriver) -> Vec<String> {
    driver
        .stats()
        .executed_requests
        .iter()
        .map(|request| request.sql.clone())
        .collect()
}

fn plan_result() -> QueryResult {
    QueryResult::text("the plan".to_string(), Duration::ZERO)
}

#[test]
fn a_planner_is_asked_and_what_it_names_runs_with_the_statements_context() {
    let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN QUERY PLAN");
    let connection = connection(&driver);

    explain_plan_only(
        connection.as_ref(),
        &QueryRequest::new(" SELECT 1;\n").with_database(Some("db".to_string())),
    )
    .unwrap();

    let stats = driver.stats();
    assert_eq!(stats.executed_requests.len(), 1);
    assert_eq!(
        stats.executed_requests[0].sql,
        "EXPLAIN QUERY PLAN SELECT 1;"
    );
    assert_eq!(stats.executed_requests[0].database.as_deref(), Some("db"));
    assert!(stats.explain_requests.is_empty());
}

#[test]
fn a_planner_that_would_run_the_statement_is_refused_before_sending_it() {
    let driver = FakeDriver::new(DbKind::SQLite).with_explain_planner("EXPLAIN ANALYZE");
    let connection = connection(&driver);

    let result = explain_plan_only(connection.as_ref(), &QueryRequest::new("DELETE FROM t"));

    assert!(result.is_err());
    assert!(sent_sql(&driver).is_empty());
}

#[test]
fn without_a_planner_the_driver_explains_and_nothing_goes_through_execute() {
    let driver = FakeDriver::new(DbKind::SQLite).with_explain_result(plan_result());
    let connection = connection(&driver);

    let result = explain_plan_only(
        connection.as_ref(),
        &QueryRequest::new("DELETE FROM t").with_database(Some("db".to_string())),
    )
    .unwrap();

    assert_eq!(result.text_body.as_deref(), Some("the plan"));
    assert!(sent_sql(&driver).is_empty());

    let requests = driver.stats().explain_requests;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].query.as_deref(), Some("DELETE FROM t"));
    assert_eq!(requests[0].format, ExplainFormat::Readable);
    assert_eq!(requests[0].database.as_deref(), Some("db"));
}

/// DuckDB's `explain` prefixes the text and prepares it, which executes every
/// statement but the last, so this is what stands between it and a write.
#[test]
fn without_a_planner_a_statement_that_would_run_never_reaches_the_driver() {
    for statement in [
        "ANALYZE DELETE FROM t",
        "(ANALYZE) DELETE FROM t",
        "(FORMAT JSON, ANALYZE) SELECT 1",
        "SELECT 1; DELETE FROM t",
    ] {
        let driver = FakeDriver::new(DbKind::SQLite).with_explain_result(plan_result());
        let connection = connection(&driver);

        let result = explain_plan_only(connection.as_ref(), &QueryRequest::new(statement));

        assert!(result.is_err(), "{statement}");
        assert!(driver.stats().explain_requests.is_empty(), "{statement}");
        assert!(sent_sql(&driver).is_empty(), "{statement}");
    }
}

#[test]
fn a_driver_without_any_explain_says_so_and_nothing_is_sent() {
    let driver = FakeDriver::new(DbKind::SQLite);
    let connection = connection(&driver);

    let result = explain_plan_only(connection.as_ref(), &QueryRequest::new("SELECT 1"));

    assert!(matches!(result, Err(DbError::NotSupported(_))));
    assert!(sent_sql(&driver).is_empty());
    assert!(driver.stats().explain_requests.is_empty());
}

#[test]
fn only_an_explain_that_runs_nothing_is_plan_only() {
    for explain in [
        "EXPLAIN SELECT 1",
        "EXPLAIN DELETE FROM t",
        "EXPLAIN (FORMAT JSON) DELETE FROM t",
        "EXPLAIN FORMAT=JSON SELECT 1",
        "EXPLAIN QUERY PLAN DELETE FROM t",
        "EXPLAIN SELECT 1;",
        "/* a plan */ EXPLAIN SELECT ';' -- trailing",
    ] {
        assert!(is_plan_only_explain(explain), "{explain}");
    }

    for explain in [
        "EXPLAIN ANALYZE SELECT 1",
        "EXPLAIN ANALYSE SELECT 1",
        "EXPLAIN (ANALYZE) SELECT 1",
        "EXPLAIN (ANALYZE, BUFFERS) DELETE FROM t",
        "EXPLAIN (\"analyze\") DELETE FROM t",
        "EXPLAIN SELECT 1; DELETE FROM t",
        "EXPLAIN EXPLAIN ANALYZE DELETE FROM t",
    ] {
        assert!(!is_plan_only_explain(explain), "{explain}");
    }
}

#[test]
fn the_explain_keyword_is_found_behind_comments_and_only_as_a_word() {
    for sql in [
        "EXPLAIN SELECT 1",
        "explain select 1",
        "EXPLAIN(ANALYZE) SELECT 1",
        "  -- why\n/* and */ Explain SELECT 1",
    ] {
        assert!(starts_with_explain(sql), "{sql}");
    }

    for sql in ["SELECT 1", "EXPLAINED", "explain_me()", ""] {
        assert!(!starts_with_explain(sql), "{sql}");
    }
}
