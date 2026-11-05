use super::{
    PaletteFetchQueue, PaletteObjectCache, build_resource_items_for_connection,
    write_palette_objects,
};
use crate::app::AppStateEntity;
use crate::ui::overlays::command_palette::{PaletteItem, PaletteSelection, ResourceItem};
use crate::ui::views::workspace::map_item_to_selection;
use dbflux_core::{
    CollectionInfo, ConnectedProfile, DatabaseConnection, DatabaseInfo, DbSchemaInfo,
    DocumentSchema, MeasurementInfo, RelationalSchema, SchemaLoadingStrategy, SchemaSnapshot,
    TableInfo, TimeSeriesSchema, ViewInfo,
};
use dbflux_storage::{IndexedObject, IndexedObjectKind};
use dbflux_ui_base::object_tree::ObjectTreeRequestKey;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// A connection that only reports how its driver loads schemas.
struct StrategyConnection {
    metadata: dbflux_core::DriverMetadata,
    strategy: SchemaLoadingStrategy,
}

impl StrategyConnection {
    fn new(strategy: SchemaLoadingStrategy) -> Arc<Self> {
        Arc::new(Self {
            metadata: dbflux_core::DriverMetadata {
                id: "stub-strategy".to_string(),
                display_name: "Stub".to_string(),
                description: "test stub".to_string(),
                category: dbflux_core::DatabaseCategory::Relational,
                transfer_family: dbflux_core::TransferFamily::Incompatible,
                deployment_class: None,
                query_language: dbflux_core::QueryLanguage::Sql,
                capabilities: dbflux_core::DriverCapabilities::empty(),
                default_port: None,
                uri_scheme: "stub".to_string(),
                icon: dbflux_core::Icon::Database,
                syntax: None,
                query: None,
                mutation: None,
                ddl: None,
                transactions: None,
                limits: None,
                ssl_modes: None,
                ssl_cert_fields: None,
                classification_override: None,
                default_chunk_size: None,
                supports_lock_timeout: false,
                editor_profile: None,
            },
            strategy,
        })
    }
}

impl dbflux_core::Connection for StrategyConnection {
    fn metadata(&self) -> &dbflux_core::DriverMetadata {
        &self.metadata
    }

    fn kind(&self) -> dbflux_core::DbKind {
        dbflux_core::DbKind::SQLite
    }

    fn schema_loading_strategy(&self) -> SchemaLoadingStrategy {
        self.strategy
    }

    fn dialect(&self) -> &dyn dbflux_core::SqlDialect {
        &dbflux_core::DefaultSqlDialect
    }

    fn ping(&self) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn execute(
        &self,
        _request: &dbflux_core::QueryRequest,
    ) -> Result<dbflux_core::QueryResult, dbflux_core::DbError> {
        Err(dbflux_core::DbError::NotSupported("stub".to_string()))
    }

    fn cancel(&self, _handle: &dbflux_core::QueryHandle) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn schema(&self) -> Result<SchemaSnapshot, dbflux_core::DbError> {
        Ok(SchemaSnapshot::default())
    }
}

fn connected(
    strategy: SchemaLoadingStrategy,
    snapshot: Option<SchemaSnapshot>,
) -> ConnectedProfile {
    ConnectedProfile {
        profile: dbflux_core::ConnectionProfile::new(
            "stub",
            dbflux_core::DbConfig::SQLite {
                path: std::path::PathBuf::from(":memory:"),
                connection_id: None,
            },
        ),
        connection: StrategyConnection::new(strategy),
        schema: snapshot,
        mutation_policy: dbflux_core::MutationPolicy::default(),
        read_only_reason: None,
        database_schemas: HashMap::new(),
        table_details: HashMap::new(),
        collection_children: HashMap::new(),
        schema_types: HashMap::new(),
        schema_columns: HashMap::new(),
        schema_indexes: HashMap::new(),
        schema_foreign_keys: HashMap::new(),
        schema_routines: HashMap::new(),
        dependents_cache: HashMap::new(),
        active_database: None,
        redis_key_cache: Default::default(),
        database_connections: HashMap::new(),
        proxy_tunnel: None,
    }
}

fn database_slot(strategy: SchemaLoadingStrategy, snapshot: SchemaSnapshot) -> DatabaseConnection {
    DatabaseConnection {
        connection: StrategyConnection::new(strategy),
        schema: Some(snapshot),
    }
}

fn table(name: &str, schema: Option<&str>) -> TableInfo {
    TableInfo {
        name: name.to_string(),
        schema: schema.map(str::to_string),
        columns: None,
        indexes: None,
        foreign_keys: None,
        constraints: None,
        sample_fields: None,
        presentation: dbflux_core::CollectionPresentation::DataGrid,
        child_items: None,
        storage_hints: None,
        pseudo_columns: Box::default(),
    }
}

fn view(name: &str, schema: Option<&str>) -> ViewInfo {
    ViewInfo {
        name: name.to_string(),
        schema: schema.map(str::to_string),
    }
}

fn group(name: &str, tables: Vec<TableInfo>, views: Vec<ViewInfo>) -> DbSchemaInfo {
    DbSchemaInfo {
        name: name.to_string(),
        tables,
        views,
        custom_types: None,
    }
}

fn databases(names: &[&str], current: Option<&str>) -> Vec<DatabaseInfo> {
    names
        .iter()
        .map(|name| DatabaseInfo {
            name: name.to_string(),
            is_current: Some(*name) == current,
        })
        .collect()
}

fn collection(name: &str) -> CollectionInfo {
    CollectionInfo {
        name: name.to_string(),
        database: None,
        document_count: None,
        avg_document_size: None,
        sample_fields: None,
        indexes: None,
        validator: None,
        is_capped: false,
        presentation: dbflux_core::CollectionPresentation::DataGrid,
        child_items: None,
    }
}

fn measurement(name: &str) -> MeasurementInfo {
    MeasurementInfo {
        name: name.to_string(),
        tags: vec![],
        fields: vec![],
    }
}

fn app_state() -> AppStateEntity {
    AppStateEntity::new_with_storage_runtime(
        dbflux_storage::bootstrap::StorageRuntime::in_memory().expect("in-memory storage"),
    )
    .expect("test app state")
}

fn target(item: &PaletteItem) -> String {
    let kind = match item {
        PaletteItem::Resource(ResourceItem::Table { .. }) => "table",
        PaletteItem::Resource(ResourceItem::View { .. }) => "view",
        PaletteItem::Resource(ResourceItem::Collection { .. }) => "collection",
        PaletteItem::Resource(ResourceItem::KeyValueDb { .. }) => "keyspace",
        _ => "other",
    };
    match map_item_to_selection(item) {
        Some(PaletteSelection::OpenTable {
            table, database, ..
        }) => format!(
            "{kind} {}/{}/{}",
            database.unwrap_or_default(),
            table.schema.unwrap_or_default(),
            table.name
        ),
        Some(PaletteSelection::OpenCollection { collection, .. }) => {
            format!("{kind} {}/{}", collection.database, collection.name)
        }
        Some(PaletteSelection::OpenKeyValue { database, .. }) => format!("{kind} {database}"),
        _ => format!("{kind} ?"),
    }
}

/// One palette open over one connected profile: what each item opens, in
/// palette order, and the databases it asks to fetch.
struct PaletteOpen {
    targets: Vec<String>,
    to_fetch: Vec<String>,
}

fn open_palette(
    state: &AppStateEntity,
    profile_id: Uuid,
    cache: &mut PaletteObjectCache,
) -> PaletteOpen {
    let mut items = Vec::new();
    let connected = state.connections().get(&profile_id).expect("connected");
    let to_fetch =
        build_resource_items_for_connection(state, profile_id, connected, cache, &mut items);
    write_palette_objects(&state.palette_object_index_repo, cache.take_writes());

    PaletteOpen {
        targets: items.iter().map(target).collect(),
        to_fetch,
    }
}

fn new_cache(state: &AppStateEntity) -> PaletteObjectCache {
    PaletteObjectCache::new(Arc::clone(&state.palette_object_index_repo))
}

/// What each palette item opens for `connected`, with an empty cache.
fn palette_targets(connected: ConnectedProfile) -> Vec<String> {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    state.connections_mut().insert(profile_id, connected);
    let mut cache = new_cache(&state);

    open_palette(&state, profile_id, &mut cache).targets
}

fn lazy_mysql(databases_listed: &[&str]) -> ConnectedProfile {
    connected(
        SchemaLoadingStrategy::LazyPerDatabase,
        Some(SchemaSnapshot::relational(RelationalSchema {
            databases: databases(databases_listed, None),
            ..Default::default()
        })),
    )
}

#[test]
fn remembers_a_fetched_database_across_restarts_and_asks_for_the_rest() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    let mut profile = lazy_mysql(&["f3base", "f3trade"]);
    profile.database_schemas.insert(
        "f3trade".to_string(),
        group("f3trade", vec![table("f3_sport", Some("f3trade"))], vec![]),
    );
    state.connections_mut().insert(profile_id, profile);

    let first = open_palette(&state, profile_id, &mut new_cache(&state));
    assert_eq!(first.targets, vec!["table f3trade/f3trade/f3_sport"]);
    assert_eq!(first.to_fetch, vec!["f3base"]);

    state
        .connections_mut()
        .insert(profile_id, lazy_mysql(&["f3base", "f3trade"]));

    let after_restart = open_palette(&state, profile_id, &mut new_cache(&state));
    assert_eq!(
        after_restart.targets,
        vec!["table f3trade/f3trade/f3_sport"]
    );
    assert_eq!(after_restart.to_fetch, vec!["f3base", "f3trade"]);
}

#[test]
fn a_fresh_list_replaces_the_remembered_one() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    let mut cache = new_cache(&state);

    let mut profile = lazy_mysql(&["f3trade"]);
    profile.database_schemas.insert(
        "f3trade".to_string(),
        group("f3trade", vec![table("dropped", Some("f3trade"))], vec![]),
    );
    state.connections_mut().insert(profile_id, profile);
    open_palette(&state, profile_id, &mut cache);

    let mut profile = lazy_mysql(&["f3trade"]);
    profile.database_schemas.insert(
        "f3trade".to_string(),
        group(
            "f3trade",
            vec![table("f3_sport", Some("f3trade"))],
            vec![view("sport_summary", Some("f3trade"))],
        ),
    );
    state.connections_mut().insert(profile_id, profile);
    open_palette(&state, profile_id, &mut cache);

    state
        .connections_mut()
        .insert(profile_id, lazy_mysql(&["f3trade"]));

    assert_eq!(
        open_palette(&state, profile_id, &mut new_cache(&state)).targets,
        vec![
            "table f3trade/f3trade/f3_sport",
            "view f3trade/f3trade/sport_summary",
        ]
    );
}

#[test]
fn remembers_collections_of_a_document_database() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    let snapshot = || {
        Some(SchemaSnapshot::document(DocumentSchema {
            databases: databases(&["logs"], None),
            ..Default::default()
        }))
    };

    let mut profile = connected(SchemaLoadingStrategy::LazyPerDatabase, snapshot());
    profile.database_schemas.insert(
        "logs".to_string(),
        group("logs", vec![table("events", Some("logs"))], vec![]),
    );
    state.connections_mut().insert(profile_id, profile);
    open_palette(&state, profile_id, &mut new_cache(&state));

    state.connections_mut().insert(
        profile_id,
        connected(SchemaLoadingStrategy::LazyPerDatabase, snapshot()),
    );

    let reopened = open_palette(&state, profile_id, &mut new_cache(&state));
    assert_eq!(reopened.targets, vec!["collection logs/events"]);
    assert_eq!(reopened.to_fetch, vec!["logs"]);
}

#[test]
fn lists_and_fetches_only_the_configured_database_when_not_showing_all() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    let mut profile = lazy_mysql(&["f3base", "f3trade"]);
    let mut config = dbflux_core::DbConfig::default_mysql();
    if let dbflux_core::DbConfig::MySQL { database, .. } = &mut config {
        *database = Some("f3trade".to_string());
    }
    profile.profile.config = config;
    profile.profile.show_all_databases = false;
    profile.database_schemas.insert(
        "f3base".to_string(),
        group("f3base", vec![table("f3e_sport", Some("f3base"))], vec![]),
    );
    state.connections_mut().insert(profile_id, profile);

    let opened = open_palette(&state, profile_id, &mut new_cache(&state));

    assert!(opened.targets.is_empty());
    assert_eq!(opened.to_fetch, vec!["f3trade"]);
}

#[test]
fn neither_remembers_nor_fetches_for_a_connection_per_database() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    let mut profile = connected(
        SchemaLoadingStrategy::ConnectionPerDatabase,
        Some(SchemaSnapshot::relational(RelationalSchema {
            databases: databases(&["app", "analytics"], Some("app")),
            current_database: Some("app".to_string()),
            schemas: vec![group(
                "public",
                vec![table("users", Some("public"))],
                vec![],
            )],
            ..Default::default()
        })),
    );
    profile.database_schemas.clear();
    state.connections_mut().insert(profile_id, profile);

    let opened = open_palette(&state, profile_id, &mut new_cache(&state));

    assert_eq!(opened.targets, vec!["table app/public/users"]);
    assert!(opened.to_fetch.is_empty());
    assert!(
        state
            .palette_object_index_repo
            .load_profile(&profile_id.to_string())
            .expect("load")
            .is_empty()
    );
}

#[test]
fn lists_the_tables_of_databases_fetched_on_demand() {
    let mut profile = connected(
        SchemaLoadingStrategy::LazyPerDatabase,
        Some(SchemaSnapshot::relational(RelationalSchema {
            databases: databases(&["f3base", "f3trade", "sales"], None),
            ..Default::default()
        })),
    );
    profile.database_schemas.insert(
        "f3trade".to_string(),
        group(
            "f3trade",
            vec![
                table("f3_sport", Some("f3trade")),
                table("margin_rule", None),
            ],
            vec![view("sport_summary", Some("f3trade"))],
        ),
    );
    profile.database_schemas.insert(
        "sales".to_string(),
        group("sales", vec![table("orders", Some("dbo"))], vec![]),
    );

    assert_eq!(
        palette_targets(profile),
        vec![
            "table f3trade/f3trade/f3_sport",
            "view f3trade/f3trade/sport_summary",
            "table f3trade/f3trade/margin_rule",
            "table sales/dbo/orders",
        ]
    );
}

#[test]
fn skips_a_fetched_database_the_connection_no_longer_lists() {
    let mut profile = connected(
        SchemaLoadingStrategy::LazyPerDatabase,
        Some(SchemaSnapshot::relational(RelationalSchema {
            databases: databases(&["live"], None),
            ..Default::default()
        })),
    );
    profile.database_schemas.insert(
        "dropped".to_string(),
        group("dropped", vec![table("ghost", Some("dropped"))], vec![]),
    );
    profile.database_schemas.insert(
        "live".to_string(),
        group("live", vec![table("users", Some("live"))], vec![]),
    );

    assert_eq!(palette_targets(profile), vec!["table live/live/users"]);
}

#[test]
fn reads_each_database_from_its_own_connection_before_its_fetched_schema() {
    let mut profile = connected(
        SchemaLoadingStrategy::ConnectionPerDatabase,
        Some(SchemaSnapshot::relational(RelationalSchema {
            databases: databases(&["app", "analytics"], Some("app")),
            current_database: Some("app".to_string()),
            schemas: vec![group(
                "public",
                vec![table("users", Some("public"))],
                vec![],
            )],
            ..Default::default()
        })),
    );
    profile.database_connections.insert(
        "analytics".to_string(),
        database_slot(
            SchemaLoadingStrategy::ConnectionPerDatabase,
            SchemaSnapshot::relational(RelationalSchema {
                current_database: Some("analytics".to_string()),
                schemas: vec![group(
                    "public",
                    vec![table("events", Some("public"))],
                    vec![view("daily", Some("public"))],
                )],
                ..Default::default()
            }),
        ),
    );
    profile.database_schemas.insert(
        "app".to_string(),
        group("app", vec![table("stale", Some("public"))], vec![]),
    );

    assert_eq!(
        palette_targets(profile),
        vec![
            "table app/public/users",
            "table analytics/public/events",
            "view analytics/public/daily",
        ]
    );
}

#[test]
fn opens_a_single_database_connection_without_naming_its_database() {
    let profile = connected(
        SchemaLoadingStrategy::SingleDatabase,
        Some(SchemaSnapshot::relational(RelationalSchema {
            current_database: Some("main".to_string()),
            schemas: vec![group(
                "main",
                vec![table("users", Some("main"))],
                vec![view("active_users", Some("main"))],
            )],
            ..Default::default()
        })),
    );

    assert_eq!(
        palette_targets(profile),
        vec!["table /main/users", "view /main/active_users"]
    );
}

#[test]
fn keeps_schema_less_objects_of_an_unnamed_database_unqualified() {
    let profile = connected(
        SchemaLoadingStrategy::SingleDatabase,
        Some(SchemaSnapshot::relational(RelationalSchema {
            tables: vec![table("users", None)],
            views: vec![view("active_users", None)],
            ..Default::default()
        })),
    );

    assert_eq!(
        palette_targets(profile),
        vec!["table //users", "view //active_users"]
    );
}

#[test]
fn lists_a_collection_of_the_current_database_once() {
    let profile = connected(
        SchemaLoadingStrategy::SingleDatabase,
        Some(SchemaSnapshot::document(DocumentSchema {
            databases: databases(&["default"], Some("default")),
            current_database: Some("default".to_string()),
            collections: vec![collection("/aws/lambda/api")],
        })),
    );

    assert_eq!(
        palette_targets(profile),
        vec!["collection default//aws/lambda/api"]
    );
}

#[test]
fn opens_document_databases_fetched_on_demand_as_collections() {
    let mut profile = connected(
        SchemaLoadingStrategy::LazyPerDatabase,
        Some(SchemaSnapshot::document(DocumentSchema {
            databases: databases(&["shop", "logs"], Some("shop")),
            current_database: Some("shop".to_string()),
            collections: vec![collection("orders")],
        })),
    );
    profile.database_schemas.insert(
        "logs".to_string(),
        group("logs", vec![table("events", Some("logs"))], vec![]),
    );
    profile.database_schemas.insert(
        "shop".to_string(),
        group("shop", vec![table("invoices", Some("shop"))], vec![]),
    );

    assert_eq!(
        palette_targets(profile),
        vec!["collection shop/invoices", "collection logs/events"],
        "a lazily loaded folder lists what each fetch found, not the snapshot"
    );
}

#[test]
fn prefers_fetched_measurements_over_the_connect_time_snapshot() {
    let mut profile = connected(
        SchemaLoadingStrategy::LazyPerDatabase,
        Some(SchemaSnapshot::time_series(TimeSeriesSchema {
            databases: databases(&["metrics", "ops", "archive"], Some("metrics")),
            current_database: Some("metrics".to_string()),
            measurements: vec![measurement("cpu"), measurement("dropped")],
            ..Default::default()
        })),
    );
    profile.database_schemas.insert(
        "metrics".to_string(),
        group("metrics", vec![table("cpu", Some("metrics"))], vec![]),
    );
    profile.database_schemas.insert(
        "ops".to_string(),
        group("ops", vec![table("disk", Some("ops"))], vec![]),
    );

    assert_eq!(
        palette_targets(profile),
        vec!["collection metrics/cpu", "collection ops/disk"]
    );
}

#[test]
fn lists_measurements_from_a_database_connection() {
    let mut profile = connected(
        SchemaLoadingStrategy::ConnectionPerDatabase,
        Some(SchemaSnapshot::time_series(TimeSeriesSchema {
            databases: databases(&["metrics", "ops"], Some("metrics")),
            current_database: Some("metrics".to_string()),
            measurements: vec![measurement("cpu")],
            ..Default::default()
        })),
    );
    profile.database_connections.insert(
        "ops".to_string(),
        database_slot(
            SchemaLoadingStrategy::ConnectionPerDatabase,
            SchemaSnapshot::time_series(TimeSeriesSchema {
                current_database: Some("ops".to_string()),
                measurements: vec![measurement("disk")],
                ..Default::default()
            }),
        ),
    );

    assert_eq!(
        palette_targets(profile),
        vec!["collection metrics/cpu", "collection ops/disk"]
    );
}

#[test]
fn lists_nothing_before_the_connect_time_snapshot() {
    let mut profile = connected(SchemaLoadingStrategy::LazyPerDatabase, None);
    profile.database_schemas.insert(
        "f3trade".to_string(),
        group("f3trade", vec![table("f3_sport", Some("f3trade"))], vec![]),
    );

    assert!(palette_targets(profile).is_empty());
}

#[test]
fn shows_only_the_configured_database_of_a_document_snapshot() {
    let mut profile = connected(
        SchemaLoadingStrategy::SingleDatabase,
        Some(SchemaSnapshot::document(DocumentSchema {
            databases: databases(&["shop", "logs"], Some("shop")),
            current_database: Some("shop".to_string()),
            collections: vec![
                collection("orders"),
                CollectionInfo {
                    database: Some("logs".to_string()),
                    ..collection("events")
                },
            ],
        })),
    );
    let mut config = dbflux_core::DbConfig::default_mongodb();
    if let dbflux_core::DbConfig::MongoDB { database, .. } = &mut config {
        *database = Some("shop".to_string());
    }
    profile.profile.config = config;
    profile.profile.show_all_databases = false;

    assert_eq!(palette_targets(profile), vec!["collection shop/orders"]);
}

#[test]
fn forgets_databases_the_server_no_longer_lists() {
    let state = app_state();
    let profile_id = Uuid::new_v4();
    let mut cache = new_cache(&state);
    let users = IndexedObject {
        schema: Some("app".to_string()),
        name: "users".to_string(),
        kind: IndexedObjectKind::Table,
    };
    cache.remember(profile_id, "app", vec![users.clone()]);
    cache.remember(profile_id, "dropped", vec![users.clone()]);
    write_palette_objects(&state.palette_object_index_repo, cache.take_writes());

    cache.forget_unlisted(profile_id, &["app"].into_iter().collect());
    write_palette_objects(&state.palette_object_index_repo, cache.take_writes());

    let stored = state
        .palette_object_index_repo
        .load_profile(&profile_id.to_string())
        .expect("load");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].database, "app");
}

#[test]
fn an_unchanged_list_queues_no_write() {
    let state = app_state();
    let profile_id = Uuid::new_v4();
    let mut cache = new_cache(&state);
    let users = vec![IndexedObject {
        schema: Some("app".to_string()),
        name: "users".to_string(),
        kind: IndexedObjectKind::Table,
    }];

    cache.remember(profile_id, "app", users.clone());
    assert_eq!(cache.take_writes().len(), 1);

    cache.remember(profile_id, "app", users);
    cache.remember(profile_id, "never_seen", Vec::new());
    assert!(cache.take_writes().is_empty());
}

fn schema_key(profile_id: Uuid, database: &str) -> ObjectTreeRequestKey {
    ObjectTreeRequestKey::DatabaseSchema {
        profile_id,
        database: database.to_string(),
    }
}

#[test]
fn the_fetch_queue_keeps_a_few_requests_in_flight() {
    let profile_id = Uuid::new_v4();
    let mut queue = PaletteFetchQueue::default();
    queue.enqueue(["a", "b", "c"].map(|database| schema_key(profile_id, database)));

    assert_eq!(queue.next(2), Some(schema_key(profile_id, "a")));
    assert_eq!(queue.next(2), Some(schema_key(profile_id, "b")));
    assert_eq!(queue.next(2), None, "two are in flight");

    assert!(queue.settled(&schema_key(profile_id, "a")));
    assert!(
        !queue.settled(&schema_key(profile_id, "z")),
        "a request the palette did not make is not its own"
    );
    assert_eq!(queue.next(2), Some(schema_key(profile_id, "c")));
    assert_eq!(queue.next(2), None);
}

#[test]
fn the_fetch_queue_ignores_requests_it_already_holds() {
    let profile_id = Uuid::new_v4();
    let mut queue = PaletteFetchQueue::default();
    queue.enqueue([schema_key(profile_id, "a"), schema_key(profile_id, "b")]);
    assert_eq!(queue.next(1), Some(schema_key(profile_id, "a")));

    queue.enqueue([schema_key(profile_id, "a"), schema_key(profile_id, "b")]);
    queue.finished(&schema_key(profile_id, "a"));

    assert_eq!(queue.next(1), Some(schema_key(profile_id, "b")));
    assert_eq!(queue.next(1), None, "each database is queued once");
}
