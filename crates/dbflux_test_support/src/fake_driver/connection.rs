use super::*;

pub(super) struct FakeConnection {
    kind: DbKind,
    state: Arc<FakeDriverState>,
    metadata: Arc<DriverMetadata>,
    active_database: RwLock<Option<String>>,
    current_region: Option<String>,
    secret_stores: Vec<FakeSecretStore>,
    session_factory: FakeSessionFactory,
}

/// Opens sessions on the same fake state as the connection it belongs to.
struct FakeSessionFactory {
    kind: DbKind,
    state: Arc<FakeDriverState>,
    metadata: Arc<DriverMetadata>,
}

impl ExecutionSessionFactory for FakeSessionFactory {
    fn open(&self) -> Result<Arc<dyn ExecutionSession>, DbError> {
        Ok(Arc::new(FakeSession {
            connection: Arc::new(FakeConnection {
                kind: self.kind,
                state: self.state.clone(),
                metadata: self.metadata.clone(),
                active_database: RwLock::new(None),
                current_region: None,
                secret_stores: Vec::new(),
                session_factory: FakeSessionFactory {
                    kind: self.kind,
                    state: self.state.clone(),
                    metadata: self.metadata.clone(),
                },
            }),
            closed: AtomicBool::new(false),
        }))
    }

    fn shutdown(&self) -> Result<(), DbError> {
        Ok(())
    }
}

struct FakeSession {
    connection: Arc<FakeConnection>,
    closed: AtomicBool,
}

impl ExecutionSession for FakeSession {
    fn connection(&self) -> Arc<dyn Connection> {
        self.connection.clone()
    }

    fn close(&self) -> Result<(), DbError> {
        self.closed.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn finish_operation(&self) -> Result<(), DbError> {
        self.close()
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

impl FakeConnection {
    pub(super) fn new(
        kind: DbKind,
        profile: &ConnectionProfile,
        state: Arc<FakeDriverState>,
        metadata: Arc<DriverMetadata>,
    ) -> Self {
        let secret_stores = match kind {
            DbKind::AwsSecrets => fake_secret_stores(&state),
            _ => Vec::new(),
        };

        let session_factory = FakeSessionFactory {
            kind,
            state: state.clone(),
            metadata: metadata.clone(),
        };

        let current_region = match &profile.config {
            DbConfig::AwsSecrets { region, .. } => Some(region.clone()),
            _ => None,
        };

        Self {
            kind,
            state,
            metadata,
            active_database: RwLock::new(active_database_from_profile(profile)),
            current_region,
            secret_stores,
            session_factory,
        }
    }

    fn execute_internal(&self, request: &QueryRequest) -> Result<QueryResult, Box<DbError>> {
        mutex_lock(&self.state.executed_requests).push(request.clone());

        let refuses_read_only = !self.metadata.enforces_read_only()
            || self.state.refuse_read_only.load(Ordering::Relaxed);
        if refuses_read_only {
            request
                .refuse_read_only_enforcement("Fake driver")
                .map_err(Box::new)?;
        }

        if let Some(database) = request.database.clone() {
            *rwlock_write(&self.active_database) = Some(database);
        }

        if let Some(outcome) = rwlock_read(&self.state.query_outcomes)
            .get(&request.sql)
            .cloned()
        {
            return outcome.to_result();
        }

        if let Some(outcome) = rwlock_read(&self.state.default_outcome).clone() {
            return outcome.to_result();
        }

        Ok(QueryResult::empty())
    }
}

impl Connection for FakeConnection {
    fn metadata(&self) -> &DriverMetadata {
        &self.metadata
    }

    fn ping(&self) -> Result<(), DbError> {
        if let Some(message) = rwlock_read(&self.state.ping_error).clone() {
            return Err(DbError::connection_failed(message));
        }

        Ok(())
    }

    fn close(&mut self) -> Result<(), DbError> {
        self.state.close_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn execute(&self, req: &QueryRequest) -> Result<QueryResult, DbError> {
        self.execute_internal(req).map_err(|error| *error)
    }

    fn execute_unattended(
        &self,
        req: &QueryRequest,
        _cancel: &dbflux_core::CancelToken,
    ) -> Result<QueryResult, DbError> {
        mutex_lock(&self.state.unattended_requests).push(req.clone());
        self.execute_internal(req).map_err(|error| *error)
    }

    fn unattended_runs_apart(&self) -> bool {
        self.state.unattended_apart.load(Ordering::Relaxed)
    }

    fn execute_with_handle(
        &self,
        req: &QueryRequest,
    ) -> Result<(QueryHandle, QueryResult), DbError> {
        let handle = QueryHandle::new();
        let result = self.execute_internal(req).map_err(|error| *error)?;
        Ok((handle, result))
    }

    fn explain(&self, request: &ExplainRequest) -> Result<QueryResult, DbError> {
        let Some(result) = rwlock_read(&self.state.explain_result).clone() else {
            return Err(DbError::NotSupported(
                "EXPLAIN not supported by this driver".to_string(),
            ));
        };

        mutex_lock(&self.state.explain_requests).push(request.clone());
        Ok(result)
    }

    fn plan_semantic_request(
        &self,
        request: &dbflux_core::SemanticRequest,
    ) -> Result<dbflux_core::SemanticPlan, DbError> {
        let prefix = *rwlock_read(&self.state.explain_prefix);

        match (request, prefix) {
            (dbflux_core::SemanticRequest::Explain(explain), Some(prefix)) => {
                Ok(dbflux_core::SemanticPlan::single_query(
                    dbflux_core::SemanticPlanKind::Query,
                    dbflux_core::PlannedQuery::new(
                        QueryLanguage::Sql,
                        format!("{prefix} {}", explain.query.as_deref().unwrap_or_default()),
                    ),
                ))
            }
            _ => Err(DbError::NotSupported(
                "Semantic planning not supported".to_string(),
            )),
        }
    }

    fn cancel(&self, handle: &QueryHandle) -> Result<(), DbError> {
        mutex_lock(&self.state.cancelled_handles).push(handle.clone());
        Ok(())
    }

    fn cancel_active(&self) -> Result<(), DbError> {
        self.state
            .cancel_active_calls
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn schema(&self) -> Result<SchemaSnapshot, DbError> {
        Ok(rwlock_read(&self.state.schema).clone())
    }

    fn list_databases(&self) -> Result<Vec<DatabaseInfo>, DbError> {
        Ok(rwlock_read(&self.state.schema).databases().to_vec())
    }

    fn set_active_database(&self, database: Option<&str>) -> Result<(), DbError> {
        *rwlock_write(&self.active_database) = database.map(std::string::ToString::to_string);
        Ok(())
    }

    fn active_database(&self) -> Option<String> {
        rwlock_read(&self.active_database).clone()
    }

    fn current_region(&self) -> Option<String> {
        self.current_region.clone()
    }

    fn available_regions(&self) -> Result<Vec<String>, DbError> {
        if !self.state.region_switch_enabled.load(Ordering::Relaxed) {
            return Err(DbError::NotSupported(
                "region listing not supported".to_string(),
            ));
        }

        self.state
            .available_regions_calls
            .fetch_add(1, Ordering::Relaxed);

        if let Some(message) = rwlock_read(&self.state.region_list_error).clone() {
            return Err(DbError::PermissionDenied(message.into()));
        }

        Ok(rwlock_read(&self.state.available_regions).clone())
    }

    fn kind(&self) -> DbKind {
        self.kind
    }

    fn execution_session_factory(&self) -> Option<&dyn ExecutionSessionFactory> {
        self.state
            .offers_execution_sessions
            .load(Ordering::Relaxed)
            .then_some(&self.session_factory as &dyn ExecutionSessionFactory)
    }

    fn schema_loading_strategy(&self) -> SchemaLoadingStrategy {
        match self.kind {
            DbKind::MySQL | DbKind::MariaDB | DbKind::SqlServer => {
                SchemaLoadingStrategy::LazyPerDatabase
            }
            DbKind::Postgres | DbKind::Redshift => SchemaLoadingStrategy::ConnectionPerDatabase,
            DbKind::SQLite | DbKind::MongoDB | DbKind::Redis => {
                SchemaLoadingStrategy::SingleDatabase
            }
            DbKind::DynamoDB | DbKind::CloudWatchLogs | DbKind::S3 | DbKind::AwsSecrets => {
                SchemaLoadingStrategy::SingleDatabase
            }
            DbKind::ClickHouse | DbKind::DuckDB | DbKind::InfluxDB => {
                SchemaLoadingStrategy::LazyPerDatabase
            }
            DbKind::Turso => SchemaLoadingStrategy::SingleDatabase,
        }
    }

    fn table_details(
        &self,
        _database: &str,
        _schema: Option<&str>,
        table: &str,
    ) -> Result<dbflux_core::TableInfo, DbError> {
        let call = self
            .state
            .table_details_calls
            .fetch_add(1, Ordering::Relaxed);

        match rwlock_read(&self.state.table_details).get(table) {
            Some(outcome) => fake_table_details_answer(outcome, call, table),
            None => Err(DbError::NotSupported(
                "table_details not supported".to_string(),
            )),
        }
    }

    /// Built from the connection's dialect, so a test can render a recorded
    /// [`CrudOp`] the same way and compare it with a preview.
    fn update_row_statement(&self, patch: &RowPatch) -> Option<Result<String, DbError>> {
        Some(
            dbflux_core::SqlQueryBuilder::new(self.dialect())
                .build_update(patch, false)
                .ok_or_else(|| DbError::query_failed("Failed to build UPDATE query")),
        )
    }

    fn insert_row_statement(&self, insert: &RowInsert) -> Option<Result<String, DbError>> {
        Some(
            dbflux_core::SqlQueryBuilder::new(self.dialect())
                .build_insert(insert, false)
                .ok_or_else(|| DbError::query_failed("Failed to build INSERT query")),
        )
    }

    fn delete_row_statement(&self, delete: &RowDelete) -> Option<Result<String, DbError>> {
        Some(
            dbflux_core::SqlQueryBuilder::new(self.dialect())
                .build_delete(delete, false)
                .ok_or_else(|| DbError::query_failed("Failed to build DELETE query")),
        )
    }

    fn update_row(&self, patch: &RowPatch) -> Result<CrudResult, DbError> {
        mutex_lock(&self.state.crud_ops).push(CrudOp::Update(patch.clone()));
        Ok(CrudResult::new(1, None))
    }

    fn insert_row(&self, insert: &RowInsert) -> Result<CrudResult, DbError> {
        mutex_lock(&self.state.crud_ops).push(CrudOp::Insert(insert.clone()));
        Ok(CrudResult::new(1, None))
    }

    fn delete_row(&self, delete: &RowDelete) -> Result<CrudResult, DbError> {
        if let dbflux_core::RecordIdentity::Composite { values, .. } = &delete.identity
            && let Some((_, message)) = rwlock_read(&self.state.delete_row_errors)
                .iter()
                .find(|(value, _)| values.contains(value))
        {
            return Err(DbError::query_failed(message.clone()));
        }

        mutex_lock(&self.state.crud_ops).push(CrudOp::Delete(delete.clone()));
        Ok(CrudResult::new(1, None))
    }

    /// The query modes of the real drivers, whose metadata language does not
    /// name what a query is written in.
    fn source_context_spec(&self) -> Option<dbflux_core::SourceContextSpec> {
        let (default_mode, modes) = match self.kind {
            DbKind::CloudWatchLogs => (
                "cwli",
                vec![
                    ("cwli", QueryLanguage::CloudWatchLogsInsightsQl),
                    ("ppl", QueryLanguage::OpenSearchPpl),
                    ("sql", QueryLanguage::OpenSearchSql),
                ],
            ),
            DbKind::InfluxDB => (
                "influxql",
                vec![
                    ("influxql", QueryLanguage::InfluxQuery),
                    ("flux", QueryLanguage::Flux),
                ],
            ),
            _ => return None,
        };

        Some(dbflux_core::SourceContextSpec {
            targets_label: "Targets".to_string(),
            targets_placeholder: String::new(),
            default_target: None,
            start_label: "Start".to_string(),
            end_label: "End".to_string(),
            query_mode_label: Some("Syntax".to_string()),
            query_modes: modes
                .into_iter()
                .map(|(value, query_language)| dbflux_core::SourceQueryMode {
                    value: value.to_string(),
                    label: value.to_string(),
                    query_language,
                })
                .collect(),
            default_query_mode: Some(default_mode.to_string()),
        })
    }

    fn language_service(&self) -> &dyn dbflux_core::LanguageService {
        let service: Option<&'static dyn LanguageService> =
            *rwlock_read(&self.state.language_service);
        if let Some(service) = service {
            return service;
        }

        match self.kind {
            DbKind::Redis => &REDIS_LANGUAGE_SERVICE,
            DbKind::CloudWatchLogs => &CLOUDWATCH_LANGUAGE_SERVICE,
            DbKind::InfluxDB => &INFLUX_LANGUAGE_SERVICE,
            DbKind::MongoDB => &MONGO_LANGUAGE_SERVICE,
            _ => &SQL_LANGUAGE_SERVICE,
        }
    }

    fn dialect(&self) -> &dyn SqlDialect {
        let dialect: Option<&'static dyn SqlDialect> = *rwlock_read(&self.state.dialect);
        dialect.unwrap_or(&DEFAULT_SQL_DIALECT)
    }

    fn query_generator(&self) -> Option<&dyn dbflux_core::QueryGenerator> {
        static GENERATOR: dbflux_core::SqlMutationGenerator =
            dbflux_core::SqlMutationGenerator::new(&DEFAULT_SQL_DIALECT);
        Some(&GENERATOR)
    }

    fn secret_stores(&self) -> Vec<&dyn SecretStoreSource> {
        self.secret_stores
            .iter()
            .map(|store| store as &dyn SecretStoreSource)
            .collect()
    }
}

fn fake_table_details_answer(
    outcome: &FakeTableDetails,
    call: usize,
    table: &str,
) -> Result<dbflux_core::TableInfo, DbError> {
    match outcome {
        FakeTableDetails::Info(info) => Ok((**info).clone()),
        FakeTableDetails::NotFound => Err(DbError::object_not_found(format!(
            "table {table} not found"
        ))),
        FakeTableDetails::Failure(message) => Err(DbError::connection_failed(message.clone())),
        FakeTableDetails::Sequence(answers) => match answers.get(call).or(answers.last()) {
            Some(answer) => fake_table_details_answer(answer, call, table),
            None => Err(DbError::NotSupported(
                "table_details not supported".to_string(),
            )),
        },
    }
}
