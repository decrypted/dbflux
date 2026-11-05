//! The tables, views, collections and keyspaces the command palette lists
//! for each connected profile.

use crate::app::AppStateEntity;
use crate::ui::overlays::command_palette::{PaletteItem, ResourceItem};
use dbflux_core::{
    ConnectedProfile, DataStructure, DatabaseLayout, ObjectFolder, SchemaLoadingStrategy,
    SchemaSnapshot,
};
use dbflux_storage::{IndexedObject, IndexedObjectKind, PaletteObjectIndexRepo};
use dbflux_ui_base::object_tree::{
    FolderObjects, NodeContent, ObjectTreeKey, ObjectTreeNode, ObjectTreeOutcome,
    ObjectTreeRequestKey, folder_objects, is_current_database, only_shown_database,
    project_profile_tree, shows_one_unnamed_database,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use uuid::Uuid;

mod sync;
#[cfg(test)]
mod tests;

pub(super) use sync::PaletteSync;

/// Database fetches the palette keeps in flight at once, so a server with
/// many databases is walked a few at a time instead of all at once.
pub(super) const PALETTE_FETCHES_IN_FLIGHT: usize = 2;

/// The objects of one database, when this session has listed them.
enum DatabaseListing {
    Loaded(Vec<PaletteItem>),
    Unloaded,
}

/// A change to the persisted index, applied off the UI thread.
pub(super) struct PaletteObjectWrite {
    profile_id: Uuid,
    database: String,
    objects: Vec<IndexedObject>,
}

/// Applies `writes` in order. A failure is logged: the index only speeds up
/// the next search, and the live lists stay correct without it.
pub(super) fn write_palette_objects(
    repo: &PaletteObjectIndexRepo,
    writes: Vec<PaletteObjectWrite>,
) {
    for write in writes {
        if let Err(error) = repo.replace_database(
            &write.profile_id.to_string(),
            &write.database,
            &write.objects,
        ) {
            log::warn!(
                "Could not save the palette's objects of '{}': {error}",
                write.database
            );
        }
    }
}

/// The objects the palette last saw in the databases of each profile, kept
/// across restarts so a database lazily loaded in an earlier session is
/// searchable before it is fetched again. Changes are queued as writes for
/// the caller to apply off the UI thread.
pub(super) struct PaletteObjectCache {
    repo: Arc<PaletteObjectIndexRepo>,
    profiles: HashMap<Uuid, HashMap<String, Vec<IndexedObject>>>,
    writes: Vec<PaletteObjectWrite>,
}

impl PaletteObjectCache {
    pub(super) fn new(repo: Arc<PaletteObjectIndexRepo>) -> Self {
        Self {
            repo,
            profiles: HashMap::new(),
            writes: Vec::new(),
        }
    }

    pub(super) fn repo(&self) -> Arc<PaletteObjectIndexRepo> {
        Arc::clone(&self.repo)
    }

    /// The writes queued since the last call, in the order they were made.
    pub(super) fn take_writes(&mut self) -> Vec<PaletteObjectWrite> {
        std::mem::take(&mut self.writes)
    }

    fn databases(&mut self, profile_id: Uuid) -> &mut HashMap<String, Vec<IndexedObject>> {
        let repo = &self.repo;

        self.profiles.entry(profile_id).or_insert_with(|| {
            match repo.load_profile(&profile_id.to_string()) {
                Ok(databases) => databases
                    .into_iter()
                    .map(|indexed| (indexed.database, indexed.objects))
                    .collect(),
                Err(error) => {
                    log::warn!("Could not read the palette's cached objects: {error}");
                    HashMap::new()
                }
            }
        })
    }

    fn recall(&mut self, profile_id: Uuid, database: &str) -> Vec<IndexedObject> {
        self.databases(profile_id)
            .get(database)
            .cloned()
            .unwrap_or_default()
    }

    fn remember(&mut self, profile_id: Uuid, database: &str, objects: Vec<IndexedObject>) {
        let databases = self.databases(profile_id);
        let unchanged = match databases.get(database) {
            Some(known) => *known == objects,
            None => objects.is_empty(),
        };
        if unchanged {
            return;
        }

        if objects.is_empty() {
            databases.remove(database);
        } else {
            databases.insert(database.to_string(), objects.clone());
        }

        self.writes.push(PaletteObjectWrite {
            profile_id,
            database: database.to_string(),
            objects,
        });
    }

    /// Forgets the databases of `profile_id` the server no longer lists.
    fn forget_unlisted(&mut self, profile_id: Uuid, listed: &HashSet<&str>) {
        let gone: Vec<String> = self
            .databases(profile_id)
            .keys()
            .filter(|database| !listed.contains(database.as_str()))
            .cloned()
            .collect();

        for database in gone {
            self.remember(profile_id, &database, Vec::new());
        }
    }
}

/// The databases the palette asks the coordinator for, a few at a time.
#[derive(Default)]
pub(super) struct PaletteFetchQueue {
    queued: VecDeque<ObjectTreeRequestKey>,
    in_flight: HashSet<ObjectTreeRequestKey>,
}

impl PaletteFetchQueue {
    /// Queues `keys` that are neither queued nor in flight already.
    pub(super) fn enqueue(&mut self, keys: impl IntoIterator<Item = ObjectTreeRequestKey>) {
        for key in keys {
            if !self.in_flight.contains(&key) && !self.queued.contains(&key) {
                self.queued.push_back(key);
            }
        }
    }

    /// The next queued request when fewer than `limit` are in flight; it
    /// counts as in flight until [`Self::settled`] or [`Self::finished`].
    pub(super) fn next(&mut self, limit: usize) -> Option<ObjectTreeRequestKey> {
        if self.in_flight.len() >= limit {
            return None;
        }

        let key = self.queued.pop_front()?;
        self.in_flight.insert(key.clone());
        Some(key)
    }

    /// Marks a request that ended without a settle event.
    pub(super) fn finished(&mut self, key: &ObjectTreeRequestKey) {
        self.in_flight.remove(key);
    }

    /// Whether `key` was one of the palette's requests in flight; it no
    /// longer is.
    pub(super) fn settled(&mut self, key: &ObjectTreeRequestKey) -> bool {
        self.in_flight.remove(key)
    }
}

/// Palette items for the objects of one connected profile, from the same
/// caches and database list the sidebar draws, addressed the way the
/// sidebar opens them. An object found by more than one path appears once.
///
/// A connection that loads each database on demand also lists the objects
/// `cache` remembers for a database not loaded yet, records the lists it
/// has, and returns the databases worth fetching now.
pub(super) fn build_resource_items_for_connection(
    state: &AppStateEntity,
    profile_id: Uuid,
    connected: &ConnectedProfile,
    cache: &mut PaletteObjectCache,
    items: &mut Vec<PaletteItem>,
) -> Vec<String> {
    let Some(snapshot) = &connected.schema else {
        return Vec::new();
    };

    let profile_name = connected.profile.name.as_str();
    let lazy =
        connected.connection.schema_loading_strategy() == SchemaLoadingStrategy::LazyPerDatabase;
    let mut resources = Vec::new();

    let listings = match snapshot.database_layout() {
        DatabaseLayout::Schemas => relational_listings(state, profile_id, connected, snapshot),
        DatabaseLayout::Folder(folder) => {
            // A lazily loaded folder lists only what each database's fetch
            // found, as the sidebar does, not the connect-time snapshot.
            if !lazy {
                build_resource_items_from_schema(
                    profile_id,
                    profile_name,
                    &snapshot.structure,
                    &mut resources,
                );
            }
            folder_listings(state, profile_id, connected, snapshot, folder, lazy)
        }
        DatabaseLayout::Keyspaces => {
            build_resource_items_from_schema(
                profile_id,
                profile_name,
                &snapshot.structure,
                &mut resources,
            );
            Vec::new()
        }
    };

    let shown = only_shown_database(
        &connected.profile,
        listings.iter().map(|(database, _)| database.as_str()),
    );
    if let Some(only) = &shown {
        resources.retain(|item| item_database(item).is_none_or(|database| database == only));
    }

    if lazy && state.get_database_list(profile_id).is_some() {
        let listed: HashSet<&str> = listings
            .iter()
            .map(|(database, _)| database.as_str())
            .collect();
        cache.forget_unlisted(profile_id, &listed);
    }

    let mut to_fetch = Vec::new();

    for (database, listing) in listings {
        if shown.as_ref().is_some_and(|only| *only != database) {
            continue;
        }

        match listing {
            DatabaseListing::Loaded(objects) => {
                if lazy {
                    cache.remember(
                        profile_id,
                        &database,
                        objects.iter().filter_map(indexed_object).collect(),
                    );
                }
                resources.extend(objects);
            }
            DatabaseListing::Unloaded if lazy => {
                resources.extend(
                    cache
                        .recall(profile_id, &database)
                        .iter()
                        .map(|object| cached_item(profile_id, profile_name, &database, object)),
                );

                if worth_fetching(state, profile_id, &database) {
                    to_fetch.push(database);
                }
            }
            DatabaseListing::Unloaded => {}
        }
    }

    let mut seen = HashSet::new();
    items.extend(
        resources
            .into_iter()
            .filter(|item| resource_identity(item).is_none_or(|identity| seen.insert(identity))),
    );

    to_fetch
}

fn relational_listings(
    state: &AppStateEntity,
    profile_id: Uuid,
    connected: &ConnectedProfile,
    snapshot: &SchemaSnapshot,
) -> Vec<(String, DatabaseListing)> {
    let Some(root) = project_profile_tree(state, profile_id) else {
        return Vec::new();
    };

    let target = RelationalTarget {
        profile_id,
        profile_name: connected.profile.name.as_str(),
        names_database: !shows_one_unnamed_database(
            state,
            profile_id,
            snapshot,
            connected.connection.schema_loading_strategy(),
            &root,
        ),
    };

    root.children
        .iter()
        .filter_map(|node| {
            let database = node.key.database()?;
            let listing = if node.state == NodeContent::Unloaded {
                DatabaseListing::Unloaded
            } else {
                let mut objects = Vec::new();
                push_projected_relational_items(&target, node, None, &mut objects);
                DatabaseListing::Loaded(objects)
            };
            Some((database.to_string(), listing))
        })
        .collect()
}

fn folder_listings(
    state: &AppStateEntity,
    profile_id: Uuid,
    connected: &ConnectedProfile,
    snapshot: &SchemaSnapshot,
    folder: ObjectFolder,
    lazy: bool,
) -> Vec<(String, DatabaseListing)> {
    let profile_name = connected.profile.name.as_str();

    project_profile_tree(state, profile_id)
        .iter()
        .flat_map(|root| root.children.iter())
        .filter_map(|node| node.key.database())
        .map(|database| {
            let listing = match folder_objects(
                connected,
                folder,
                snapshot,
                database,
                lazy,
                is_current_database(snapshot, database),
            ) {
                FolderObjects::Listed(objects) => DatabaseListing::Loaded(
                    objects
                        .iter()
                        .map(|object| {
                            collection_item(profile_id, profile_name, database, &object.name)
                        })
                        .collect(),
                ),
                FolderObjects::Unlisted => DatabaseListing::Loaded(Vec::new()),
                FolderObjects::Unloaded => DatabaseListing::Unloaded,
            };
            (database.to_string(), listing)
        })
        .collect()
}

/// The database a resource item lives in, if it names one.
fn item_database(item: &PaletteItem) -> Option<&str> {
    match item {
        PaletteItem::Resource(ResourceItem::Table { database, .. })
        | PaletteItem::Resource(ResourceItem::View { database, .. }) => database.as_deref(),
        PaletteItem::Resource(ResourceItem::Collection { database, .. })
        | PaletteItem::Resource(ResourceItem::KeyValueDb { database, .. }) => Some(database),
        _ => None,
    }
}

/// A database is fetched for the palette once per session: not while a
/// fetch for it runs, and not again after one failed.
fn worth_fetching(state: &AppStateEntity, profile_id: Uuid, database: &str) -> bool {
    let key = ObjectTreeRequestKey::DatabaseSchema {
        profile_id,
        database: database.to_string(),
    };

    !state.object_tree_is_pending(&key)
        && !matches!(
            state.object_tree_outcome(&key),
            Some(ObjectTreeOutcome::Failed(_))
        )
}

fn collection_item(
    profile_id: Uuid,
    profile_name: &str,
    database: &str,
    name: &str,
) -> PaletteItem {
    PaletteItem::Resource(ResourceItem::Collection {
        profile_id,
        profile_name: profile_name.to_string(),
        database: database.to_string(),
        name: name.to_string(),
    })
}

fn indexed_object(item: &PaletteItem) -> Option<IndexedObject> {
    let PaletteItem::Resource(resource) = item else {
        return None;
    };

    let (schema, name, kind) = match resource {
        ResourceItem::Table { schema, name, .. } => (schema, name, IndexedObjectKind::Table),
        ResourceItem::View { schema, name, .. } => (schema, name, IndexedObjectKind::View),
        ResourceItem::Collection { name, .. } => (&None, name, IndexedObjectKind::Collection),
        ResourceItem::KeyValueDb { .. } => return None,
    };

    Some(IndexedObject {
        schema: schema.clone(),
        name: name.clone(),
        kind,
    })
}

fn cached_item(
    profile_id: Uuid,
    profile_name: &str,
    database: &str,
    object: &IndexedObject,
) -> PaletteItem {
    match object.kind {
        IndexedObjectKind::Table => PaletteItem::Resource(ResourceItem::Table {
            profile_id,
            profile_name: profile_name.to_string(),
            database: Some(database.to_string()),
            schema: object.schema.clone(),
            name: object.name.clone(),
        }),
        IndexedObjectKind::View => PaletteItem::Resource(ResourceItem::View {
            profile_id,
            profile_name: profile_name.to_string(),
            database: Some(database.to_string()),
            schema: object.schema.clone(),
            name: object.name.clone(),
        }),
        IndexedObjectKind::Collection => {
            collection_item(profile_id, profile_name, database, &object.name)
        }
    }
}

/// Collections and keyspaces listed by a non-relational snapshot itself.
pub(super) fn build_resource_items_from_schema(
    profile_id: Uuid,
    profile_name: &str,
    structure: &DataStructure,
    items: &mut Vec<PaletteItem>,
) {
    match structure {
        DataStructure::Document(doc) => {
            let default_db = doc
                .current_database
                .clone()
                .unwrap_or_else(|| "default".to_string());
            for collection in &doc.collections {
                items.push(PaletteItem::Resource(ResourceItem::Collection {
                    profile_id,
                    profile_name: profile_name.to_string(),
                    database: collection
                        .database
                        .clone()
                        .unwrap_or_else(|| default_db.clone()),
                    name: collection.name.clone(),
                }));
            }
        }
        DataStructure::KeyValue(kv) => {
            for ks in &kv.keyspaces {
                items.push(PaletteItem::Resource(ResourceItem::KeyValueDb {
                    profile_id,
                    profile_name: profile_name.to_string(),
                    database: format!("db{}", ks.db_index),
                }));
            }
        }
        _ => {}
    }
}

struct RelationalTarget<'a> {
    profile_id: Uuid,
    profile_name: &'a str,
    /// False when the sidebar lists the objects without a database row, and
    /// so opens them without naming one.
    names_database: bool,
}

/// A table or view outside a schema group lives in the schema named after
/// its database, as the sidebar opens it; the unnamed database of a
/// single-database connection has neither.
fn push_projected_relational_items(
    target: &RelationalTarget<'_>,
    node: &ObjectTreeNode,
    group: Option<&str>,
    items: &mut Vec<PaletteItem>,
) {
    let named = |database: &str| (!database.is_empty()).then(|| database.to_string());
    let address = |database: &str, schema: &Option<String>| {
        let schema = schema
            .clone()
            .or_else(|| group.map(str::to_string))
            .or_else(|| named(database));
        let database = named(database).filter(|_| target.names_database);
        (database, schema)
    };

    match &node.key {
        ObjectTreeKey::Table {
            database,
            schema,
            table,
            ..
        } => {
            let (database, schema) = address(database, schema);
            items.push(PaletteItem::Resource(ResourceItem::Table {
                profile_id: target.profile_id,
                profile_name: target.profile_name.to_string(),
                database,
                schema,
                name: table.clone(),
            }));
        }
        ObjectTreeKey::View {
            database,
            schema,
            view,
            ..
        } => {
            let (database, schema) = address(database, schema);
            items.push(PaletteItem::Resource(ResourceItem::View {
                profile_id: target.profile_id,
                profile_name: target.profile_name.to_string(),
                database,
                schema,
                name: view.clone(),
            }));
        }
        ObjectTreeKey::Schema { schema, .. } => {
            for child in &node.children {
                push_projected_relational_items(target, child, Some(schema), items);
            }
        }
        ObjectTreeKey::Profile { .. } | ObjectTreeKey::Database { .. } => {
            for child in &node.children {
                push_projected_relational_items(target, child, None, items);
            }
        }
    }
}

#[derive(PartialEq, Eq, Hash)]
enum ResourceIdentity {
    Table(Option<String>, Option<String>, String),
    View(Option<String>, Option<String>, String),
    Collection(String, String),
    KeyValueDb(String),
}

fn resource_identity(item: &PaletteItem) -> Option<ResourceIdentity> {
    let PaletteItem::Resource(resource) = item else {
        return None;
    };

    Some(match resource {
        ResourceItem::Table {
            database,
            schema,
            name,
            ..
        } => ResourceIdentity::Table(database.clone(), schema.clone(), name.clone()),
        ResourceItem::View {
            database,
            schema,
            name,
            ..
        } => ResourceIdentity::View(database.clone(), schema.clone(), name.clone()),
        ResourceItem::Collection { database, name, .. } => {
            ResourceIdentity::Collection(database.clone(), name.clone())
        }
        ResourceItem::KeyValueDb { database, .. } => ResourceIdentity::KeyValueDb(database.clone()),
    })
}
