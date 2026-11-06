// Explicit imports, not `use super::*`: the parent glob together with
// `#[gpui::test]` sends the macro expansion into unbounded recursion.
use crate::ui::overlays::command_palette::{PaletteItem, PaletteSelection, ResourceItem};
use crate::ui::views::workspace::Workspace;
use dbflux_core::{
    DatabaseInfo, DbConfig, DbError, DbSchemaInfo, RelationalSchema, SchemaLoadingStrategy,
    SchemaSnapshot, TableInfo, WritePrivilege,
};
use dbflux_ui_base::{AppStateChanged, AppStateEntity};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

/// A server that loads each database on demand; each database `name` holds
/// one table, `<name>_table`.
struct LazyServer {
    metadata: dbflux_core::DriverMetadata,
    schema_calls: Mutex<Vec<String>>,
}

impl LazyServer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            metadata: dbflux_core::DriverMetadata {
                id: "stub-lazy".to_string(),
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
            schema_calls: Mutex::new(Vec::new()),
        })
    }

    fn schema_calls(&self) -> Vec<String> {
        self.schema_calls.lock().expect("schema calls").clone()
    }
}

impl dbflux_core::Connection for LazyServer {
    fn metadata(&self) -> &dbflux_core::DriverMetadata {
        &self.metadata
    }

    fn kind(&self) -> dbflux_core::DbKind {
        dbflux_core::DbKind::MySQL
    }

    fn schema_loading_strategy(&self) -> SchemaLoadingStrategy {
        SchemaLoadingStrategy::LazyPerDatabase
    }

    fn dialect(&self) -> &dyn dbflux_core::SqlDialect {
        &dbflux_core::DefaultSqlDialect
    }

    fn ping(&self) -> Result<(), DbError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }

    fn execute(
        &self,
        _request: &dbflux_core::QueryRequest,
    ) -> Result<dbflux_core::QueryResult, DbError> {
        Err(DbError::NotSupported("stub".to_string()))
    }

    fn cancel(&self, _handle: &dbflux_core::QueryHandle) -> Result<(), DbError> {
        Ok(())
    }

    fn schema(&self) -> Result<SchemaSnapshot, DbError> {
        Ok(SchemaSnapshot::default())
    }

    #[expect(
        clippy::unwrap_in_result,
        reason = "test double: a poisoned fixture mutex fails the test loudly"
    )]
    fn schema_for_database(&self, database: &str) -> Result<DbSchemaInfo, DbError> {
        self.schema_calls
            .lock()
            .expect("schema calls")
            .push(database.to_string());

        Ok(DbSchemaInfo {
            name: database.to_string(),
            tables: vec![TableInfo {
                name: format!("{database}_table"),
                schema: Some(database.to_string()),
                columns: None,
                indexes: None,
                foreign_keys: None,
                constraints: None,
                sample_fields: None,
                presentation: dbflux_core::CollectionPresentation::DataGrid,
                child_items: None,
                storage_hints: None,
                pseudo_columns: Box::default(),
            }],
            views: Vec::new(),
            custom_types: None,
        })
    }
}

struct Harness<'a> {
    workspace: Entity<Workspace>,
    app_state: Entity<AppStateEntity>,
    window: &'a mut VisualTestContext,
}

fn open_workspace(cx: &mut TestAppContext) -> Harness<'_> {
    cx.update(gpui_component::init);
    cx.update(dbflux_components::theme::init);
    cx.update(dbflux_ui_base::keymap::init_keymap);

    let app_state: Entity<AppStateEntity> = cx.update(|cx| {
        cx.new(|_| {
            let runtime =
                dbflux_storage::bootstrap::StorageRuntime::in_memory().expect("in-memory storage");
            AppStateEntity::new_with_storage_runtime(runtime).expect("test storage setup")
        })
    });

    let holder: Rc<RefCell<Option<Entity<Workspace>>>> = Rc::default();
    let (_, window) = cx.add_window_view({
        let holder = holder.clone();
        let app_state = app_state.clone();
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(app_state, window, cx));
            holder.replace(Some(workspace.clone()));
            gpui_component::Root::new(workspace, window, cx)
        }
    });
    let workspace = holder.borrow().clone().expect("workspace created");
    window.run_until_parked();

    Harness {
        workspace,
        app_state,
        window,
    }
}

fn snapshot(databases: &[&str]) -> SchemaSnapshot {
    SchemaSnapshot::relational(RelationalSchema {
        databases: databases
            .iter()
            .map(|name| DatabaseInfo {
                name: name.to_string(),
                is_current: false,
            })
            .collect(),
        ..Default::default()
    })
}

/// Connects a saved profile to a lazily loaded server listing `databases`.
fn connect(harness: &mut Harness<'_>, databases: &[&str]) -> (Uuid, Arc<LazyServer>) {
    let server = LazyServer::new();
    let profile = dbflux_core::ConnectionProfile::new("lazy", DbConfig::default_mysql());
    let profile_id = profile.id;
    let snapshot = snapshot(databases);

    harness.app_state.update(harness.window, |state, _| {
        state.add_profile_in_folder(profile.clone(), None);
        state.apply_connect_profile(
            profile,
            server.clone(),
            Some(snapshot),
            None,
            false,
            WritePrivilege::Unknown,
        );
    });

    (profile_id, server)
}

fn open_palette(harness: &mut Harness<'_>) {
    harness
        .workspace
        .update_in(harness.window, |workspace, window, cx| {
            workspace.toggle_command_palette(window, cx);
        });
}

/// Lets fetches, index reads and writes, and the coalesced refill land.
fn settle(harness: &mut Harness<'_>) {
    harness.window.run_until_parked();
    harness
        .window
        .executor()
        .advance_clock(Duration::from_millis(500));
    harness.window.run_until_parked();
}

fn listed_tables(harness: &mut Harness<'_>) -> Vec<String> {
    harness
        .workspace
        .read_with(harness.window, |workspace, cx| {
            workspace
                .command_palette
                .read(cx)
                .items()
                .iter()
                .filter_map(|item| match item {
                    PaletteItem::Resource(ResourceItem::Table { name, .. }) => Some(name.clone()),
                    _ => None,
                })
                .collect()
        })
}

#[gpui::test]
fn choosing_an_item_stops_the_background_walk(cx: &mut TestAppContext) {
    let mut harness = open_workspace(cx);
    let (_, server) = connect(&mut harness, &["a", "b", "c", "d", "e", "f"]);

    open_palette(&mut harness);
    let (in_flight, queued) = harness.workspace.read_with(harness.window, |workspace, _| {
        (
            workspace.palette_sync.fetches_in_flight(),
            workspace.palette_sync.fetches_queued(),
        )
    });
    assert_eq!((in_flight, queued), (2, 4));

    // Enter runs the highlighted item, which closes the palette without its
    // closed event.
    harness.window.simulate_keystrokes("enter");
    settle(&mut harness);

    assert_eq!(
        server.schema_calls().len(),
        2,
        "only the fetches already running settle"
    );
    let queued = harness.workspace.read_with(harness.window, |workspace, _| {
        workspace.palette_sync.fetches_queued()
    });
    assert_eq!(queued, 0);
}

#[gpui::test]
fn a_fetched_database_refills_the_open_palette_without_listing_everything_again(
    cx: &mut TestAppContext,
) {
    let mut harness = open_workspace(cx);
    let (_, server) = connect(&mut harness, &["alpha", "bravo"]);

    open_palette(&mut harness);
    assert!(listed_tables(&mut harness).is_empty());
    settle(&mut harness);

    assert_eq!(server.schema_calls().len(), 2);
    let mut tables = listed_tables(&mut harness);
    tables.sort();
    assert_eq!(tables, vec!["alpha_table", "bravo_table"]);
    let full_listings = harness.workspace.read_with(harness.window, |workspace, _| {
        workspace.palette_sync.full_listings
    });
    assert_eq!(
        full_listings, 1,
        "only the opening lists every connection; fetched databases are added on their own"
    );
}

#[gpui::test]
fn a_deleted_profile_loses_its_stored_objects(cx: &mut TestAppContext) {
    let mut harness = open_workspace(cx);
    let (profile_id, _) = connect(&mut harness, &["alpha"]);

    open_palette(&mut harness);
    settle(&mut harness);
    let stored = |harness: &mut Harness<'_>| {
        harness.app_state.read_with(harness.window, |state, _| {
            state
                .palette_object_index_repo
                .load_profile(&profile_id.to_string())
                .expect("load")
                .len()
        })
    };
    assert_eq!(stored(&mut harness), 1);

    harness.app_state.update(harness.window, |state, cx| {
        let index = state
            .profiles()
            .iter()
            .position(|profile| profile.id == profile_id)
            .expect("profile present");
        state.remove_profile(index);
        cx.emit(AppStateChanged);
    });
    settle(&mut harness);

    assert_eq!(stored(&mut harness), 0);
}

fn stored_databases(harness: &mut Harness<'_>, profile_id: Uuid) -> usize {
    harness.app_state.read_with(harness.window, |state, _| {
        state
            .palette_object_index_repo
            .load_profile(&profile_id.to_string())
            .expect("load")
            .len()
    })
}

#[gpui::test]
fn a_profile_pointed_elsewhere_while_connected_is_forgotten_again_on_reconnect(
    cx: &mut TestAppContext,
) {
    let mut harness = open_workspace(cx);
    let (profile_id, server) = connect(&mut harness, &["alpha"]);
    open_palette(&mut harness);
    settle(&mut harness);
    assert_eq!(stored_databases(&mut harness, profile_id), 1);

    let edited = harness.app_state.update(harness.window, |state, cx| {
        let mut edited = state
            .profiles()
            .iter()
            .find(|profile| profile.id == profile_id)
            .cloned()
            .expect("profile present");
        if let DbConfig::MySQL { host, .. } = &mut edited.config {
            *host = "other.example".to_string();
        }
        state.update_profile(edited.clone());
        cx.emit(AppStateChanged);
        edited
    });
    settle(&mut harness);
    assert_eq!(stored_databases(&mut harness, profile_id), 0);

    // Until it reconnects, the open connection still lists the old server.
    open_palette(&mut harness);
    open_palette(&mut harness);
    settle(&mut harness);
    assert_eq!(stored_databases(&mut harness, profile_id), 1);

    harness.app_state.update(harness.window, |state, cx| {
        state.apply_connect_profile(
            edited,
            server.clone(),
            Some(snapshot(&["alpha"])),
            None,
            false,
            WritePrivilege::Unknown,
        );
        cx.emit(AppStateChanged);
    });
    settle(&mut harness);

    assert_eq!(
        stored_databases(&mut harness, profile_id),
        0,
        "the reconnect reaches the new server, so the old lists go"
    );
}

/// A table chosen while a query tab holds the keyboard is chosen once,
/// opens in front of that tab and takes the keyboard from it, so the
/// palette shortcut works again right away instead of landing on the
/// hidden query tab.
#[gpui::test]
fn the_palette_opens_again_after_choosing_a_table_over_a_query_tab(cx: &mut TestAppContext) {
    let mut harness = open_workspace(cx);
    connect(&mut harness, &["alpha"]);
    harness
        .workspace
        .update_in(harness.window, |workspace, window, cx| {
            workspace.new_query_tab(window, cx);
        });
    settle(&mut harness);

    #[cfg(target_os = "macos")]
    let toggle = "cmd-shift-p";
    #[cfg(not(target_os = "macos"))]
    let toggle = "ctrl-shift-p";

    let palette = harness.workspace.read_with(harness.window, |workspace, _| {
        workspace.command_palette.clone()
    });
    let chosen = Rc::new(std::cell::Cell::new(0));
    let counter = chosen.clone();
    harness.window.update(|_, cx| {
        cx.subscribe(&palette, move |_, _: &PaletteSelection, _| {
            counter.set(counter.get() + 1);
        })
        .detach();
    });

    harness.window.simulate_keystrokes(toggle);
    settle(&mut harness);
    assert_eq!(listed_tables(&mut harness), vec!["alpha_table"]);
    harness.window.simulate_input("@alpha_table");
    harness.window.simulate_keystrokes("enter");
    settle(&mut harness);
    assert_eq!(chosen.get(), 1, "one Enter on an `@` table chooses it once");

    assert!(
        !palette.read_with(harness.window, |palette, _| palette.is_visible()),
        "choosing the table closes the palette"
    );

    harness.window.simulate_keystrokes(toggle);
    assert!(
        palette.read_with(harness.window, |palette, _| palette.is_visible()),
        "the shortcut opens the palette again over the table"
    );
}
