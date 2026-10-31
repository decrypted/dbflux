use std::sync::Arc;

use dbflux_core::{
    Connection, DbError, ExplainRequest, QueryLanguage, ReadOnlyEnforcement, SemanticRequest,
    TableRef, classify_query_for_governance,
};
use dbflux_policy::ExecutionClassification;
use rmcp::{
    ErrorData, handler::server::wrapper::Parameters, model::CallToolResult, schemars::JsonSchema,
    tool, tool_router,
};
use serde::Deserialize;

use crate::{
    helper::{IntoErrorData, *},
    server::DbFluxServer,
    state::ServerState,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PreviewMutationParams {
    #[schemars(description = "Connection ID")]
    pub connection_id: String,

    #[schemars(description = "SQL mutation query to preview (INSERT, UPDATE, DELETE, etc.)")]
    pub sql: String,

    #[schemars(description = "Optional database/schema name")]
    pub database: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)] // Used by explain_query tool via #[tool] macro
pub(crate) struct ExplainQueryParams {
    #[schemars(description = "Connection ID")]
    pub connection_id: String,

    #[schemars(description = "SQL query to explain (optional, requires table if not provided)")]
    pub sql: Option<String>,

    #[schemars(description = "Table name to explain (optional, requires sql if not provided)")]
    pub table: Option<String>,

    #[schemars(description = "Optional database/schema name")]
    pub database: Option<String>,
}

#[tool_router(router = query_router, vis = "pub")]
impl DbFluxServer {
    #[tool(description = "Explain query execution plan or table access strategy")]
    async fn explain_query(
        &self,
        Parameters(params): Parameters<ExplainQueryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.state.clone();
        let sql = params.sql.clone();
        let table = params.table.clone();
        let database = params.database.clone();
        let connection_id = params.connection_id.clone();

        self.governance
            .authorize_and_execute_audited(
                "explain_query",
                Some(&params.connection_id),
                ExecutionClassification::Read,
                move || async move {
                    use crate::governance::AuditDetails;

                    let audit_sql = sql.clone();
                    let result = Self::explain_query_impl(
                        state,
                        &connection_id,
                        sql.as_deref(),
                        table.as_deref(),
                        database.as_deref(),
                    )
                    .await
                    .map_err(|e| e.into_error_data())?;

                    Ok((
                        CallToolResult::success(vec![to_json_content(&result)?]),
                        AuditDetails { query: audit_sql },
                    ))
                },
            )
            .await
    }

    #[tool(description = "Preview the execution plan for a mutation query without executing it")]
    async fn preview_mutation(
        &self,
        Parameters(params): Parameters<PreviewMutationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = self.state.clone();
        let sql = params.sql.clone();
        let database = params.database.clone();
        let connection_id = params.connection_id.clone();

        self.governance
            .authorize_and_execute_audited(
                "preview_mutation",
                Some(&params.connection_id),
                ExecutionClassification::Read,
                move || async move {
                    use crate::governance::AuditDetails;

                    let audit_sql = sql.clone();
                    let result = Self::preview_mutation_impl(
                        state,
                        &connection_id,
                        &sql,
                        database.as_deref(),
                    )
                    .await
                    .map_err(|e| e.into_error_data())?;

                    Ok((
                        CallToolResult::success(vec![to_json_content(&result)?]),
                        AuditDetails {
                            query: Some(audit_sql),
                        },
                    ))
                },
            )
            .await
    }

    async fn explain_query_impl(
        state: ServerState,
        connection_id: &str,
        sql: Option<&str>,
        table: Option<&str>,
        database: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let connection =
            Self::query_connection_for_database(state, connection_id, database).await?;
        let request = Self::build_explain_request(&connection, sql, table)?;
        let result = Self::run_explain_request(connection, request, "Explain").await?;

        Ok(serialize_query_result(&result))
    }

    async fn preview_mutation_impl(
        state: ServerState,
        connection_id: &str,
        sql: &str,
        database: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let connection =
            Self::query_connection_for_database(state, connection_id, database).await?;
        let request = Self::build_explain_request(&connection, Some(sql), None)?;
        let result = Self::run_explain_request(connection, request, "Preview").await?;

        Ok(serialize_query_result(&result))
    }

    async fn query_connection_for_database(
        state: ServerState,
        connection_id: &str,
        database: Option<&str>,
    ) -> Result<Arc<dyn Connection>, String> {
        if let Some(target_db) = database {
            let current_db = Self::get_current_database(&state, connection_id).await?;

            if target_db != current_db.as_deref().unwrap_or("") {
                Self::connect_with_database(state, connection_id, target_db).await
            } else {
                Self::get_or_connect(state, connection_id).await
            }
        } else {
            Self::get_or_connect(state, connection_id).await
        }
    }

    fn build_explain_request(
        connection: &Arc<dyn Connection>,
        sql: Option<&str>,
        table: Option<&str>,
    ) -> Result<ExplainRequest, String> {
        if sql.is_none() && table.is_none() {
            return Err("Either 'sql' or 'table' parameter is required".to_string());
        }

        let table_ref = table
            .map(|table| Self::query_table_ref_for_connection(connection, table))
            .unwrap_or_else(|| TableRef::new("__dbflux_explain__"));

        let mut request = ExplainRequest::new(table_ref);

        if let Some(query) = sql {
            request = request.with_query(query);
        }

        Ok(request)
    }

    async fn run_explain_request(
        connection: Arc<dyn Connection>,
        request: ExplainRequest,
        error_prefix: &'static str,
    ) -> Result<dbflux_core::QueryResult, String> {
        tokio::task::spawn_blocking(move || {
            match connection.plan_semantic_request(&SemanticRequest::Explain(request.clone())) {
                Ok(plan) => {
                    let planned_query = plan.primary_query().cloned().ok_or_else(|| {
                        format!(
                            "{} planning error: driver returned no executable query",
                            error_prefix
                        )
                    })?;

                    let classification = classify_query_for_governance(
                        &planned_query.language,
                        &planned_query.text,
                        Some(connection.language_service()),
                    );
                    if !is_read_only_class(classification) {
                        return Err(format!(
                            "{} planning error: driver generated a non-read-only preview query",
                            error_prefix
                        ));
                    }

                    run_planned_explain(
                        connection.as_ref(),
                        planned_query.into_query_request(),
                        classification,
                    )
                    .map_err(|e| format!("{} error: {}", error_prefix, e))
                }
                Err(DbError::NotSupported(_)) => {
                    // Without a planner the driver builds and runs its own
                    // statement, so what it would run is classified first.
                    let classification =
                        fallback_explain_classification(connection.as_ref(), &request);
                    if !is_read_only_class(classification) {
                        return Err(format!(
                            "{} refused: the statement would execute or change data \
                             (EXPLAIN ANALYZE of a write, or more than one statement)",
                            error_prefix
                        ));
                    }

                    connection
                        .explain(&request)
                        .map_err(|e| format!("{} error: {}", error_prefix, e))
                }
                Err(e) => Err(format!("{} planning error: {}", error_prefix, e)),
            }
        })
        .await
        .map_err(|e| format!("Blocking task failed: {}", e))?
    }

    fn query_table_ref_for_connection(connection: &Arc<dyn Connection>, table: &str) -> TableRef {
        let default_schema = connection
            .metadata()
            .syntax
            .as_ref()
            .filter(|syntax| syntax.supports_schemas && !table.contains('.'))
            .and_then(|syntax| syntax.default_schema.clone());

        if let Some(schema) = default_schema {
            TableRef::with_schema(schema, table)
        } else {
            TableRef::from_qualified(table)
        }
    }
}

fn is_read_only_class(classification: ExecutionClassification) -> bool {
    matches!(
        classification,
        ExecutionClassification::Metadata | ExecutionClassification::Read
    )
}

/// The class of what a driver without a planner runs to explain `request`.
///
/// SQL drivers run `EXPLAIN <query>` (DuckDB, Turso, external drivers) or a
/// plan-only batch, so that prefixed text is classified: `ANALYZE DELETE ...`
/// becomes `EXPLAIN ANALYZE DELETE ...`, which executes the delete. For other
/// languages the driver's form is unknown, so the query itself must be a
/// read.
fn fallback_explain_classification(
    connection: &dyn Connection,
    request: &ExplainRequest,
) -> ExecutionClassification {
    let Some(query) = request.query.as_deref() else {
        // A table-only request explains a SELECT the driver builds itself.
        return ExecutionClassification::Metadata;
    };

    let language = &connection.metadata().query_language;
    let text = if *language == QueryLanguage::Sql {
        format!("EXPLAIN {query}")
    } else {
        query.to_string()
    };

    classify_query_for_governance(language, &text, Some(connection.language_service()))
}

/// Runs a planned explain. A plan classified `Read` executes its statement
/// (`EXPLAIN ANALYZE` of a read), so it runs with database-enforced read-only
/// where the driver offers it, which also stops writes hidden in functions.
/// A driver that cannot enforce it for this statement refuses before running
/// anything, and the plan then runs as before.
#[expect(
    clippy::result_large_err,
    reason = "returns the driver's own error unchanged, like `Connection::execute`"
)]
fn run_planned_explain(
    connection: &dyn Connection,
    mut request: dbflux_core::QueryRequest,
    classification: ExecutionClassification,
) -> Result<dbflux_core::QueryResult, DbError> {
    if classification != ExecutionClassification::Read
        || !connection.metadata().enforces_read_only()
    {
        return connection.execute(&request);
    }

    request.read_only = ReadOnlyEnforcement::Required;
    match connection.execute(&request) {
        Err(DbError::NotSupported(reason)) => {
            log::debug!(
                "explain: read-only enforcement refused ({reason}); running the plan without it"
            );
            request.read_only = ReadOnlyEnforcement::None;
            connection.execute(&request)
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use dbflux_core::{
        DangerousQueryKind, DbError, DbKind, DefaultSqlDialect, DriverCapabilities, DriverMetadata,
        ExplainRequest, LanguageService, MutationCapabilities, PlaceholderStyle, QueryCapabilities,
        QueryHandle, QueryLanguage, QueryResult, SchemaLoadingStrategy, SchemaSnapshot,
        SemanticPlan, SemanticPlanKind, SqlLanguageService, SyntaxInfo, TransactionCapabilities,
        ValidationResult,
    };
    use std::sync::LazyLock;

    static TEST_METADATA: LazyLock<DriverMetadata> = LazyLock::new(|| DriverMetadata {
        id: "test".into(),
        display_name: "Test".into(),
        description: "Test driver".into(),
        category: dbflux_core::DatabaseCategory::Relational,
        transfer_family: dbflux_core::TransferFamily::Sql,
        deployment_class: None,
        query_language: QueryLanguage::Sql,
        capabilities: DriverCapabilities::empty(),
        default_port: None,
        uri_scheme: "test".into(),
        icon: dbflux_core::Icon::Database,
        syntax: Some(SyntaxInfo {
            identifier_quote: '"',
            string_quote: '\'',
            placeholder_style: PlaceholderStyle::QuestionMark,
            supports_schemas: true,
            default_schema: Some("public".into()),
            case_sensitive_identifiers: true,
            misreads_unknown_quoted_identifiers: false,
        }),
        query: Some(QueryCapabilities::default()),
        mutation: Some(MutationCapabilities::default()),
        ddl: None,
        transactions: Some(TransactionCapabilities::default()),
        limits: None,
        ssl_modes: None,
        ssl_cert_fields: None,
        classification_override: None,
        default_chunk_size: None,
        supports_lock_timeout: false,
        editor_profile: None,
    });

    struct DestructiveOverrideLanguageService;

    impl LanguageService for DestructiveOverrideLanguageService {
        fn validate(&self, _query: &str) -> ValidationResult {
            ValidationResult::Valid
        }

        fn detect_dangerous(&self, _query: &str) -> Option<DangerousQueryKind> {
            None
        }

        fn classify_execution(
            &self,
            _query: &str,
        ) -> Option<dbflux_policy::ExecutionClassification> {
            Some(dbflux_policy::ExecutionClassification::Destructive)
        }
    }

    /// `TEST_METADATA` for a driver that enforces read-only execution.
    static ENFORCING_METADATA: LazyLock<DriverMetadata> = LazyLock::new(|| {
        let mut metadata = TEST_METADATA.clone();
        if let Some(transactions) = metadata.transactions.as_mut() {
            transactions.supports_read_only = true;
        }
        metadata
    });

    struct TestConnection {
        plan: Option<SemanticPlan>,
        planner_supported: bool,
        explain_result: QueryResult,
        execute_result: QueryResult,
        executed_queries: std::sync::Mutex<Vec<String>>,
        executed_read_only: std::sync::Mutex<Vec<bool>>,
        explain_queries: std::sync::Mutex<Vec<Option<String>>>,
        language_service_override: bool,
        enforces_read_only: bool,
    }

    impl TestConnection {
        fn new(plan: Option<SemanticPlan>, planner_supported: bool) -> Self {
            Self {
                plan,
                planner_supported,
                explain_result: QueryResult::empty(),
                execute_result: QueryResult::empty(),
                executed_queries: std::sync::Mutex::new(Vec::new()),
                executed_read_only: std::sync::Mutex::new(Vec::new()),
                explain_queries: std::sync::Mutex::new(Vec::new()),
                language_service_override: false,
                enforces_read_only: false,
            }
        }

        fn enforcing_read_only(mut self) -> Self {
            self.enforces_read_only = true;
            self
        }

        fn with_destructive_language_service(mut self) -> Self {
            self.language_service_override = true;
            self
        }
    }

    impl Connection for TestConnection {
        fn metadata(&self) -> &DriverMetadata {
            if self.enforces_read_only {
                &ENFORCING_METADATA
            } else {
                &TEST_METADATA
            }
        }

        fn ping(&self) -> Result<(), DbError> {
            Ok(())
        }

        fn close(&mut self) -> Result<(), DbError> {
            Ok(())
        }

        #[expect(
            clippy::unwrap_in_result,
            reason = "test connection double: mutex poisoning makes the fixture's recorded \
                state untrustworthy, so aborting the test is intentional"
        )]
        fn execute(&self, req: &dbflux_core::QueryRequest) -> Result<QueryResult, DbError> {
            self.executed_queries
                .lock()
                .expect("executed queries mutex poisoned")
                .push(req.sql.clone());
            self.executed_read_only
                .lock()
                .expect("executed read-only mutex poisoned")
                .push(req.read_only.is_required());
            Ok(self.execute_result.clone())
        }

        fn cancel(&self, _handle: &QueryHandle) -> Result<(), DbError> {
            Ok(())
        }

        fn schema(&self) -> Result<SchemaSnapshot, DbError> {
            Ok(SchemaSnapshot::default())
        }

        fn kind(&self) -> DbKind {
            DbKind::Postgres
        }

        fn schema_loading_strategy(&self) -> SchemaLoadingStrategy {
            SchemaLoadingStrategy::ConnectionPerDatabase
        }

        fn dialect(&self) -> &dyn dbflux_core::SqlDialect {
            &DefaultSqlDialect
        }

        fn language_service(&self) -> &dyn LanguageService {
            if self.language_service_override {
                &DestructiveOverrideLanguageService
            } else {
                &SqlLanguageService
            }
        }

        #[expect(
            clippy::unwrap_in_result,
            reason = "test connection double: mutex poisoning makes the fixture's recorded \
                state untrustworthy, so aborting the test is intentional"
        )]
        fn explain(&self, request: &ExplainRequest) -> Result<QueryResult, DbError> {
            self.explain_queries
                .lock()
                .expect("explain queries mutex poisoned")
                .push(request.query.clone());
            Ok(self.explain_result.clone())
        }

        fn plan_semantic_request(
            &self,
            _request: &SemanticRequest,
        ) -> Result<SemanticPlan, DbError> {
            if self.planner_supported {
                self.plan.clone().ok_or_else(|| {
                    DbError::NotSupported("planner returned no semantic plan".into())
                })
            } else {
                Err(DbError::NotSupported("no plan".into()))
            }
        }
    }

    #[test]
    fn build_explain_request_uses_default_schema_for_tables() {
        let connection: Arc<dyn Connection> = Arc::new(TestConnection::new(None, false));

        let request =
            DbFluxServer::build_explain_request(&connection, None, Some("users")).unwrap();

        assert_eq!(request.table.schema.as_deref(), Some("public"));
        assert_eq!(request.table.name, "users");
        assert!(request.query.is_none());
    }

    #[tokio::test]
    async fn run_explain_request_executes_driver_plan_when_available() {
        let connection = Arc::new(TestConnection::new(
            Some(SemanticPlan::single_query(
                SemanticPlanKind::Query,
                dbflux_core::PlannedQuery::new(QueryLanguage::Sql, "EXPLAIN SELECT 1"),
            )),
            true,
        ));

        DbFluxServer::run_explain_request(
            connection.clone(),
            ExplainRequest::new(TableRef::new("users")).with_query("SELECT 1"),
            "Explain",
        )
        .await
        .unwrap();

        assert_eq!(
            connection
                .executed_queries
                .lock()
                .expect("executed queries mutex poisoned")
                .as_slice(),
            ["EXPLAIN SELECT 1"]
        );
        assert!(
            connection
                .explain_queries
                .lock()
                .expect("explain queries mutex poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn run_explain_request_falls_back_to_legacy_explain_when_planning_is_unsupported() {
        let connection = Arc::new(TestConnection::new(None, false));
        let request = ExplainRequest::new(TableRef::new("users"))
            .with_query("UPDATE users SET active = true");

        DbFluxServer::run_explain_request(connection.clone(), request, "Preview")
            .await
            .unwrap();

        assert!(
            connection
                .executed_queries
                .lock()
                .expect("executed queries mutex poisoned")
                .is_empty()
        );
        assert_eq!(
            connection
                .explain_queries
                .lock()
                .expect("explain queries mutex poisoned")
                .as_slice(),
            [Some("UPDATE users SET active = true".to_string())]
        );
    }

    #[tokio::test]
    async fn run_explain_request_rejects_non_read_only_plans() {
        let connection = Arc::new(TestConnection::new(
            Some(SemanticPlan::single_query(
                SemanticPlanKind::Query,
                dbflux_core::PlannedQuery::new(
                    QueryLanguage::Sql,
                    "UPDATE users SET active = true",
                ),
            )),
            true,
        ));

        let error = DbFluxServer::run_explain_request(
            connection.clone(),
            ExplainRequest::new(TableRef::new("users")).with_query("SELECT 1"),
            "Preview",
        )
        .await
        .expect_err("non-read-only preview plans must be rejected");

        assert!(error.contains("non-read-only preview query"));
        assert!(
            connection
                .executed_queries
                .lock()
                .expect("executed queries mutex poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn run_explain_request_consults_the_connections_language_service() {
        // `OpenSearchPpl` falls back to `Read` (and executes) with no driver
        // service; a driver service overriding `classify_execution` to
        // `Destructive` must be consulted and reject the preview instead.
        let plan = Some(SemanticPlan::single_query(
            SemanticPlanKind::Query,
            dbflux_core::PlannedQuery::new(QueryLanguage::OpenSearchPpl, "source=logs | head 1"),
        ));

        let connection_without_override = Arc::new(TestConnection::new(plan.clone(), true));
        DbFluxServer::run_explain_request(
            connection_without_override.clone(),
            ExplainRequest::new(TableRef::new("logs")).with_query("source=logs | head 1"),
            "Preview",
        )
        .await
        .expect("fallback classification allows a read-only preview");

        let connection_with_override =
            Arc::new(TestConnection::new(plan, true).with_destructive_language_service());
        let error = DbFluxServer::run_explain_request(
            connection_with_override,
            ExplainRequest::new(TableRef::new("logs")).with_query("source=logs | head 1"),
            "Preview",
        )
        .await
        .expect_err("driver language service must be consulted and reject the preview");

        assert!(error.contains("non-read-only preview query"));
    }

    #[tokio::test]
    async fn fallback_explain_refuses_statements_the_explain_would_execute() {
        // Each is what a driver without a planner turns into `EXPLAIN <sql>`,
        // in the forms the supported dialects accept.
        for sql in [
            "ANALYZE DELETE FROM users",
            "ANALYZE UPDATE users SET active = false",
            "(ANALYZE) DELETE FROM users",
            "ANALYZE FORMAT=JSON DELETE FROM users",
            "ANALYZE COPY (SELECT 'x') TO '/tmp/out.txt'",
            "ANALYZE SELECT setval('users_id_seq', 1)",
            "SELECT 1; DELETE FROM users",
            "QUERY PLAN SELECT 1; DROP TABLE users",
            "EXPLAIN ANALYZE DELETE FROM users",
        ] {
            let connection = Arc::new(TestConnection::new(None, false));
            let error = DbFluxServer::run_explain_request(
                connection.clone(),
                ExplainRequest::new(TableRef::new("users")).with_query(sql),
                "Preview",
            )
            .await
            .expect_err("an explain that executes a write must be refused");

            assert!(error.contains("refused"), "{sql}: {error}");
            assert!(
                connection
                    .explain_queries
                    .lock()
                    .expect("explain queries mutex poisoned")
                    .is_empty(),
                "{sql} must not reach the driver"
            );
        }
    }

    #[tokio::test]
    async fn fallback_explain_still_previews_a_mutation_without_running_it() {
        for sql in [
            "DELETE FROM users WHERE id = 1",
            "ANALYZE SELECT * FROM users",
        ] {
            let connection = Arc::new(TestConnection::new(None, false));
            DbFluxServer::run_explain_request(
                connection.clone(),
                ExplainRequest::new(TableRef::new("users")).with_query(sql),
                "Preview",
            )
            .await
            .unwrap_or_else(|error| panic!("{sql} must be explained: {error}"));
        }
    }

    #[tokio::test]
    async fn planned_explain_that_executes_a_read_runs_read_only() {
        let analyze = Some(SemanticPlan::single_query(
            SemanticPlanKind::Query,
            dbflux_core::PlannedQuery::new(
                QueryLanguage::Sql,
                "EXPLAIN (ANALYZE, FORMAT JSON) SELECT * FROM users",
            ),
        ));
        let plan_only = Some(SemanticPlan::single_query(
            SemanticPlanKind::Query,
            dbflux_core::PlannedQuery::new(QueryLanguage::Sql, "EXPLAIN SELECT * FROM users"),
        ));

        for (plan, expected) in [(analyze, true), (plan_only, false)] {
            let connection = Arc::new(TestConnection::new(plan, true).enforcing_read_only());
            DbFluxServer::run_explain_request(
                connection.clone(),
                ExplainRequest::new(TableRef::new("users")),
                "Explain",
            )
            .await
            .expect("explain runs");

            assert_eq!(
                connection
                    .executed_read_only
                    .lock()
                    .expect("executed read-only mutex poisoned")
                    .as_slice(),
                [expected]
            );
        }
    }

    #[tokio::test]
    async fn duckdb_explain_analyze_of_a_delete_deletes_nothing() {
        use dbflux_core::{ConnectionProfile, DbConfig, DbDriver, QueryRequest, Value};

        let profile = ConnectionProfile::new(
            "explain-duck",
            DbConfig::DuckDB {
                path: std::path::PathBuf::new(),
                ducklake_catalog: None,
                ducklake_data_path: None,
                init_sql: None,
            },
        );
        let connection: Arc<dyn Connection> = Arc::from(
            dbflux_driver_duckdb::DuckDbDriver::sandboxed()
                .connect(&profile)
                .expect("in-memory DuckDB"),
        );
        connection
            .execute(&QueryRequest::new(
                "CREATE TABLE t AS SELECT * FROM range(3) r(id)",
            ))
            .expect("seed table");
        let count = |connection: &Arc<dyn Connection>| {
            connection
                .execute(&QueryRequest::new("SELECT count(*) FROM t"))
                .expect("count")
                .rows
                .remove(0)
                .remove(0)
        };

        for (sql, prefix) in [
            ("ANALYZE DELETE FROM t", "Explain"),
            ("ANALYZE DELETE FROM t", "Preview"),
            ("ANALYZE UPDATE t SET id = id + 10", "Preview"),
        ] {
            DbFluxServer::run_explain_request(
                connection.clone(),
                ExplainRequest::new(TableRef::new("t")).with_query(sql),
                prefix,
            )
            .await
            .expect_err("EXPLAIN ANALYZE of a write must be refused");
        }
        assert_eq!(count(&connection), Value::Int(3));
        assert_eq!(
            connection
                .execute(&QueryRequest::new("SELECT max(id) FROM t"))
                .expect("max")
                .rows
                .remove(0)
                .remove(0),
            Value::Int(2)
        );

        DbFluxServer::run_explain_request(
            connection.clone(),
            ExplainRequest::new(TableRef::new("t")).with_query("DELETE FROM t"),
            "Preview",
        )
        .await
        .expect("a plain EXPLAIN previews the delete");
        assert_eq!(count(&connection), Value::Int(3));
    }
}
