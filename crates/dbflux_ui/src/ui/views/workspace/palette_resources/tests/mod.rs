use super::{
    PaletteFetchQueue, PaletteObjectCache, build_resource_items_for_connection,
    load_palette_objects, remember_database, write_palette_objects,
};
use crate::app::AppStateEntity;
use crate::ui::overlays::command_palette::{PaletteItem, PaletteSelection, ResourceItem};
use crate::ui::views::workspace::map_item_to_selection;
use dbflux_core::{
    CollectionInfo, ConnectedProfile, DatabaseConnection, DatabaseInfo, DbSchemaInfo,
    DocumentSchema, MeasurementInfo, RelationalSchema, SchemaLoadingStrategy, SchemaSnapshot,
    TableInfo, TimeSeriesSchema, ViewInfo,
};
use dbflux_storage::{IndexedObject, IndexedObjectKind, PaletteObjectIndexRepo};
use dbflux_ui_base::object_tree::ObjectTreeRequestKey;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};
use std::time::Duration;
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

/// Opens the palette and, when it asked for stored lists, lets them land
/// and lists again, as the workspace refills the open palette.
fn open_palette(
    state: &AppStateEntity,
    profile_id: Uuid,
    cache: &mut PaletteObjectCache,
) -> PaletteOpen {
    let connected = state.connections().get(&profile_id).expect("connected");
    let mut items = Vec::new();
    let mut to_fetch =
        build_resource_items_for_connection(state, profile_id, connected, cache, &mut items);
    if land_stored_lists(cache) {
        items.clear();
        to_fetch =
            build_resource_items_for_connection(state, profile_id, connected, cache, &mut items);
    }
    write_palette_objects(&state.palette_object_index_repo, cache.take_writes());

    PaletteOpen {
        targets: items.iter().map(target).collect(),
        to_fetch,
    }
}

/// Reads the stored lists `cache` asked for and merges them, as the
/// workspace does off the UI thread. Returns whether it asked for any.
fn land_stored_lists(cache: &mut PaletteObjectCache) -> bool {
    let repo = cache.repo();
    let loads = cache.take_loads();
    for &profile_id in &loads {
        assert!(cache.finish_load(profile_id, load_palette_objects(&repo, profile_id)));
    }

    !loads.is_empty()
}

fn new_cache(state: &AppStateEntity) -> PaletteObjectCache {
    PaletteObjectCache::new(Arc::clone(&state.palette_object_index_repo))
}

/// A cache that has read the stored lists of `profile_id`.
fn loaded_cache(state: &AppStateEntity, profile_id: Uuid) -> PaletteObjectCache {
    let mut cache = new_cache(state);
    cache.recall(profile_id, "");
    land_stored_lists(&mut cache);
    cache
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
    let mut cache = loaded_cache(&state, profile_id);
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
    let mut cache = loaded_cache(&state, profile_id);
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

#[test]
fn a_database_whose_background_load_failed_is_not_asked_for_again() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    state
        .connections_mut()
        .insert(profile_id, lazy_mysql(&["f3base", "f3trade"]));
    let mut cache = new_cache(&state);

    cache.mark_fetch_failed(profile_id, "f3base", None);
    assert_eq!(
        open_palette(&state, profile_id, &mut cache).to_fetch,
        vec!["f3trade"]
    );

    cache.mark_fetch_failed(profile_id, "f3trade", Some(7));
    assert_eq!(
        open_palette(&state, profile_id, &mut cache).to_fetch,
        vec!["f3trade"],
        "a failure from another connection session does not hold"
    );
}

#[test]
fn a_fetched_database_is_remembered_once_the_profile_was_read() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    let mut profile = lazy_mysql(&["f3base", "f3trade"]);
    profile.database_schemas.insert(
        "f3trade".to_string(),
        group("f3trade", vec![table("f3_sport", Some("f3trade"))], vec![]),
    );
    state.connections_mut().insert(profile_id, profile);
    let connected = state.connections().get(&profile_id).expect("connected");
    let mut cache = new_cache(&state);

    remember_database(&state, profile_id, connected, "f3trade", &mut cache);
    assert!(
        cache.take_writes().is_empty(),
        "a profile the palette never read is left for its next open"
    );

    open_palette(&state, profile_id, &mut cache);
    let mut profile = lazy_mysql(&["f3base", "f3trade"]);
    profile.database_schemas.insert(
        "f3trade".to_string(),
        group(
            "f3trade",
            vec![
                table("f3_sport", Some("f3trade")),
                table("f3_fix", Some("f3trade")),
            ],
            vec![],
        ),
    );
    state.connections_mut().insert(profile_id, profile);
    let connected = state.connections().get(&profile_id).expect("connected");

    remember_database(&state, profile_id, connected, "f3trade", &mut cache);
    write_palette_objects(&state.palette_object_index_repo, cache.take_writes());

    let stored = state
        .palette_object_index_repo
        .load_profile(&profile_id.to_string())
        .expect("load");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0]
            .objects
            .iter()
            .map(|object| object.name.as_str())
            .collect::<Vec<_>>(),
        vec!["f3_sport", "f3_fix"]
    );
}

#[test]
fn a_list_that_failed_to_save_is_written_again() {
    let state = app_state();
    let profile_id = Uuid::new_v4();
    let mut cache = loaded_cache(&state, profile_id);
    let users = vec![IndexedObject {
        schema: Some("app".to_string()),
        name: "users".to_string(),
        kind: IndexedObjectKind::Table,
    }];

    cache.remember(profile_id, "app", users.clone());
    assert_eq!(cache.take_writes().len(), 1);

    cache.forget_unsaved(profile_id, "app");
    cache.remember(profile_id, "app", users);
    assert_eq!(cache.take_writes().len(), 1);
}

#[test]
fn closing_drops_the_queued_fetches_but_not_those_in_flight() {
    let profile_id = Uuid::new_v4();
    let mut queue = PaletteFetchQueue::default();
    queue.enqueue(["a", "b", "c"].map(|database| schema_key(profile_id, database)));
    assert_eq!(queue.next(1), Some(schema_key(profile_id, "a")));

    queue.clear_queued();

    assert_eq!(queue.next(2), None);
    assert!(queue.settled(&schema_key(profile_id, "a")));
    queue.enqueue([schema_key(profile_id, "b")]);
    assert_eq!(
        queue.next(2),
        Some(schema_key(profile_id, "b")),
        "a cleared database can be queued again"
    );
}

#[test]
fn an_empty_database_list_forgets_nothing() {
    let state = app_state();
    let profile_id = Uuid::new_v4();
    let mut cache = loaded_cache(&state, profile_id);
    cache.remember(
        profile_id,
        "app",
        vec![IndexedObject {
            schema: Some("app".to_string()),
            name: "users".to_string(),
            kind: IndexedObjectKind::Table,
        }],
    );
    cache.take_writes();

    cache.forget_unlisted(profile_id, &HashSet::new());

    assert!(cache.take_writes().is_empty());
}

#[test]
fn a_write_that_fails_is_reported_back() {
    let runtime =
        dbflux_storage::bootstrap::StorageRuntime::in_memory().expect("in-memory storage");
    let connection = runtime.viz_connection().expect("connection");
    connection
        .lock()
        .expect("lock")
        .execute("DROP TABLE pal_object_index", [])
        .expect("drop the index table");
    let repo = dbflux_storage::PaletteObjectIndexRepo::new(connection);
    let profile_id = Uuid::new_v4();
    let state = app_state();
    let mut cache = loaded_cache(&state, profile_id);
    cache.remember(
        profile_id,
        "app",
        vec![IndexedObject {
            schema: Some("app".to_string()),
            name: "users".to_string(),
            kind: IndexedObjectKind::Table,
        }],
    );

    assert_eq!(
        write_palette_objects(&repo, cache.take_writes()),
        vec![(profile_id, "app".to_string())]
    );
}

#[test]
fn a_fetched_document_database_is_remembered_and_a_hidden_one_is_not() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    let mut profile = connected(
        SchemaLoadingStrategy::LazyPerDatabase,
        Some(SchemaSnapshot::document(DocumentSchema {
            databases: databases(&["shop", "logs"], None),
            ..Default::default()
        })),
    );
    let mut config = dbflux_core::DbConfig::default_mongodb();
    if let dbflux_core::DbConfig::MongoDB { database, .. } = &mut config {
        *database = Some("shop".to_string());
    }
    profile.profile.config = config;
    profile.profile.show_all_databases = false;
    for database in ["shop", "logs"] {
        profile.database_schemas.insert(
            database.to_string(),
            group(database, vec![table("events", Some(database))], vec![]),
        );
    }
    state.connections_mut().insert(profile_id, profile);
    let connected = state.connections().get(&profile_id).expect("connected");
    let mut cache = loaded_cache(&state, profile_id);

    remember_database(&state, profile_id, connected, "logs", &mut cache);
    assert!(cache.take_writes().is_empty(), "the profile hides 'logs'");

    remember_database(&state, profile_id, connected, "shop", &mut cache);
    write_palette_objects(&state.palette_object_index_repo, cache.take_writes());

    let stored = state
        .palette_object_index_repo
        .load_profile(&profile_id.to_string())
        .expect("load");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].database, "shop");
    assert_eq!(stored[0].objects[0].kind, IndexedObjectKind::Collection);
}

fn app_table(name: &str) -> IndexedObject {
    IndexedObject {
        schema: Some("app".to_string()),
        name: name.to_string(),
        kind: IndexedObjectKind::Table,
    }
}

#[test]
fn the_first_open_does_not_wait_for_the_index_and_fills_in_once_it_is_read() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    state
        .connections_mut()
        .insert(profile_id, lazy_mysql(&["f3base", "f3trade"]));

    let runtime =
        dbflux_storage::bootstrap::StorageRuntime::in_memory().expect("in-memory storage");
    let connection = runtime.viz_connection().expect("connection");
    let repo = Arc::new(PaletteObjectIndexRepo::new(Arc::clone(&connection)));
    repo.replace_database(
        &profile_id.to_string(),
        "f3trade",
        &[IndexedObject {
            schema: Some("f3trade".to_string()),
            name: "f3_sport".to_string(),
            kind: IndexedObjectKind::Table,
        }],
    )
    .expect("store a list");
    let mut cache = PaletteObjectCache::new(Arc::clone(&repo));

    // A background index write holds the repository's connection.
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let writer = std::thread::spawn({
        let connection = Arc::clone(&connection);
        move || {
            let _held = connection.lock().expect("lock");
            locked_tx.send(()).expect("report the lock");
            release_rx.recv_timeout(Duration::from_secs(10)).is_ok()
        }
    });
    locked_rx.recv().expect("the writer holds the connection");

    let connected = state.connections().get(&profile_id).expect("connected");
    let mut items = Vec::new();
    let to_fetch =
        build_resource_items_for_connection(&state, profile_id, connected, &mut cache, &mut items);
    release_tx.send(()).expect("release the index writer");
    assert!(
        writer.join().expect("writer thread"),
        "opening the palette waited for the index write"
    );

    assert!(items.is_empty(), "the stored list is not read yet");
    assert_eq!(to_fetch, vec!["f3base", "f3trade"]);
    assert_eq!(cache.take_loads(), vec![profile_id]);

    assert!(cache.finish_load(profile_id, load_palette_objects(&repo, profile_id)));
    let mut items = Vec::new();
    build_resource_items_for_connection(&state, profile_id, connected, &mut cache, &mut items);
    assert_eq!(
        items.iter().map(target).collect::<Vec<_>>(),
        vec!["table f3trade/f3trade/f3_sport"]
    );
    assert!(cache.take_loads().is_empty(), "each profile is read once");
}

#[test]
fn lists_seen_before_the_index_is_read_win_over_the_stored_ones() {
    let state = app_state();
    let profile_id = Uuid::new_v4();
    let repo = &state.palette_object_index_repo;
    for (database, name) in [
        ("app", "old"),
        ("emptied", "gone"),
        ("same", "kept"),
        ("untouched", "stored"),
    ] {
        repo.replace_database(&profile_id.to_string(), database, &[app_table(name)])
            .expect("store a list");
    }
    let mut cache = new_cache(&state);

    cache.remember(profile_id, "app", vec![app_table("new")]);
    cache.remember(profile_id, "emptied", Vec::new());
    cache.remember(profile_id, "same", vec![app_table("kept")]);
    cache.remember(profile_id, "never_stored", Vec::new());
    assert!(
        cache.take_writes().is_empty(),
        "nothing is written before the stored lists are known"
    );

    assert!(cache.finish_load(profile_id, load_palette_objects(repo, profile_id)));
    let mut written: Vec<(String, usize)> = cache
        .take_writes()
        .iter()
        .map(|write| (write.database.clone(), write.objects.len()))
        .collect();
    written.sort();
    assert_eq!(
        written,
        vec![("app".to_string(), 1), ("emptied".to_string(), 0)],
        "only the lists that changed are written"
    );
    assert_eq!(cache.recall(profile_id, "app"), vec![app_table("new")]);
    assert!(cache.recall(profile_id, "emptied").is_empty());
    assert_eq!(cache.recall(profile_id, "same"), vec![app_table("kept")]);
    assert_eq!(
        cache.recall(profile_id, "untouched"),
        vec![app_table("stored")]
    );
    assert!(
        !cache.finish_load(profile_id, None),
        "the stored lists are merged once"
    );
}

#[test]
fn a_failed_index_read_writes_the_lists_seen_meanwhile() {
    let state = app_state();
    let profile_id = Uuid::new_v4();
    let mut cache = new_cache(&state);

    cache.remember(profile_id, "app", vec![app_table("users")]);
    cache.remember(profile_id, "emptied", Vec::new());
    assert!(cache.finish_load(profile_id, None));

    assert_eq!(cache.take_writes().len(), 2);
    assert_eq!(cache.recall(profile_id, "app"), vec![app_table("users")]);
}

#[test]
fn a_database_closed_in_the_sidebar_keeps_its_stored_list_but_is_not_fetched() {
    let mut state = app_state();
    let profile_id = Uuid::new_v4();
    let mut profile = lazy_mysql(&["f3base", "f3trade"]);
    profile.database_schemas.insert(
        "f3trade".to_string(),
        group("f3trade", vec![table("f3_sport", Some("f3trade"))], vec![]),
    );
    state.connections_mut().insert(profile_id, profile);
    open_palette(&state, profile_id, &mut new_cache(&state));

    state
        .connections_mut()
        .insert(profile_id, lazy_mysql(&["f3base", "f3trade"]));
    state.object_tree_note_database_closed(profile_id, "f3trade");

    let reopened = open_palette(&state, profile_id, &mut new_cache(&state));
    assert_eq!(reopened.targets, vec!["table f3trade/f3trade/f3_sport"]);
    assert_eq!(reopened.to_fetch, vec!["f3base"]);
}
