//! The tables, views, collections and keyspaces the command palette lists
//! for each connected profile.

use crate::app::AppStateEntity;
use crate::ui::overlays::command_palette::{PaletteItem, ResourceItem};
use dbflux_core::{
    ConnectedProfile, DataStructure, DatabaseLayout, ObjectFolder, SchemaLoadingStrategy,
    SchemaSnapshot,
};
use dbflux_storage::{IndexedDatabase, IndexedObject, IndexedObjectKind, PaletteObjectIndexRepo};
use dbflux_ui_base::object_tree::{
    FolderObjects, NodeContent, ObjectTreeKey, ObjectTreeNode, ObjectTreeRequestKey,
    folder_objects, is_current_database, only_shown_database, project_database_node,
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

/// Applies `writes` in order and returns the databases whose write failed,
/// so their cached copy can be dropped and written again later.
pub(super) fn write_palette_objects(
    repo: &PaletteObjectIndexRepo,
    writes: Vec<PaletteObjectWrite>,
) -> Vec<(Uuid, String)> {
    let mut failed = Vec::new();

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
            failed.push((write.profile_id, write.database));
        }
    }

    failed
}

/// Reads the stored lists of `profile_id`; run off the UI thread, since a
/// background write can hold the repository's connection. `None` when the
/// read failed.
pub(super) fn load_palette_objects(
    repo: &PaletteObjectIndexRepo,
    profile_id: Uuid,
) -> Option<Vec<IndexedDatabase>> {
    repo.load_profile(&profile_id.to_string())
        .inspect_err(|error| log::warn!("Could not read the palette's cached objects: {error}"))
        .ok()
}

/// The lists the palette knows for one profile.
struct ProfileLists {
    databases: HashMap<String, Vec<IndexedObject>>,
    /// Set until the stored lists are read: the databases listed meanwhile.
    /// Their lists are newer than the stored ones, so the stored copy does
    /// not replace them, and they are written once it is known whether
    /// they changed.
    loading: Option<HashSet<String>>,
}

/// The objects the palette last saw in the databases of each profile, kept
/// across restarts so a database lazily loaded in an earlier session is
/// searchable before it is fetched again. Neither reads nor writes touch
/// the store here: the stored lists of a profile are asked for through
/// [`Self::take_loads`] and merged by [`Self::finish_load`], and changes
/// are queued as writes, both for the caller to run off the UI thread.
pub(super) struct PaletteObjectCache {
    repo: Arc<PaletteObjectIndexRepo>,
    profiles: HashMap<Uuid, ProfileLists>,
    loads: Vec<Uuid>,
    writes: Vec<PaletteObjectWrite>,
    /// Databases whose background fetch failed, with the connection session
    /// it failed in; they are not asked for again until a new session.
    failed_fetches: HashMap<(Uuid, String), Option<u64>>,
}

impl PaletteObjectCache {
    pub(super) fn new(repo: Arc<PaletteObjectIndexRepo>) -> Self {
        Self {
            repo,
            profiles: HashMap::new(),
            loads: Vec::new(),
            writes: Vec::new(),
            failed_fetches: HashMap::new(),
        }
    }

    pub(super) fn repo(&self) -> Arc<PaletteObjectIndexRepo> {
        Arc::clone(&self.repo)
    }

    /// Whether this session has asked for the cached lists of `profile_id`.
    pub(super) fn has_profile(&self, profile_id: Uuid) -> bool {
        self.profiles.contains_key(&profile_id)
    }

    pub(super) fn mark_fetch_failed(
        &mut self,
        profile_id: Uuid,
        database: &str,
        session: Option<u64>,
    ) {
        self.failed_fetches
            .insert((profile_id, database.to_string()), session);
    }

    fn fetch_failed(&self, profile_id: Uuid, database: &str, session: Option<u64>) -> bool {
        self.failed_fetches
            .get(&(profile_id, database.to_string()))
            .is_some_and(|failed_in| *failed_in == session)
    }

    /// Drops the in-memory copy of a list whose write failed, so the next
    /// list for that database is written again.
    pub(super) fn forget_unsaved(&mut self, profile_id: Uuid, database: &str) {
        if let Some(lists) = self.profiles.get_mut(&profile_id) {
            lists.databases.remove(database);
        }
    }

    /// The writes queued since the last call, in the order they were made.
    pub(super) fn take_writes(&mut self) -> Vec<PaletteObjectWrite> {
        std::mem::take(&mut self.writes)
    }

    /// The profiles whose stored lists were asked for since the last call.
    pub(super) fn take_loads(&mut self) -> Vec<Uuid> {
        std::mem::take(&mut self.loads)
    }

    /// Merges the stored lists of `profile_id` once read (`None` when the
    /// read failed): a database listed meanwhile keeps its newer list and
    /// is written if it differs from the stored one. Returns whether the
    /// profile was waiting for them.
    pub(super) fn finish_load(
        &mut self,
        profile_id: Uuid,
        stored: Option<Vec<IndexedDatabase>>,
    ) -> bool {
        let Some(lists) = self.profiles.get_mut(&profile_id) else {
            return false;
        };
        let Some(listed_meanwhile) = lists.loading.take() else {
            return false;
        };

        let read = stored.is_some();
        let mut stored: HashMap<String, Vec<IndexedObject>> = stored
            .unwrap_or_default()
            .into_iter()
            .map(|indexed| (indexed.database, indexed.objects))
            .collect();

        for database in listed_meanwhile {
            let objects = lists.databases.get(&database).cloned().unwrap_or_default();
            let unchanged = read
                && stored
                    .remove(&database)
                    .map_or(objects.is_empty(), |known| known == objects);
            if !unchanged {
                self.writes.push(PaletteObjectWrite {
                    profile_id,
                    database,
                    objects,
                });
            }
        }

        lists.databases.extend(stored);
        true
    }

    /// The lists of `profile_id`; the first call per profile asks for its
    /// stored lists and starts out with none.
    fn lists(&mut self, profile_id: Uuid) -> &mut ProfileLists {
        self.profiles.entry(profile_id).or_insert_with(|| {
            self.loads.push(profile_id);
            ProfileLists {
                databases: HashMap::new(),
                loading: Some(HashSet::new()),
            }
        })
    }

    fn recall(&mut self, profile_id: Uuid, database: &str) -> Vec<IndexedObject> {
        self.lists(profile_id)
            .databases
            .get(database)
            .cloned()
            .unwrap_or_default()
    }

    fn remember(&mut self, profile_id: Uuid, database: &str, objects: Vec<IndexedObject>) {
        let lists = self.lists(profile_id);
        let unchanged = match lists.databases.get(database) {
            Some(known) => *known == objects,
            None => objects.is_empty() && lists.loading.is_none(),
        };
        if unchanged {
            return;
        }

        if objects.is_empty() {
            lists.databases.remove(database);
        } else {
            lists
                .databases
                .insert(database.to_string(), objects.clone());
        }

        // Written once the stored list is known.
        if let Some(listed_meanwhile) = &mut lists.loading {
            listed_meanwhile.insert(database.to_string());
            return;
        }

        self.writes.push(PaletteObjectWrite {
            profile_id,
            database: database.to_string(),
            objects,
        });
    }

    /// Forgets the databases of `profile_id` the server no longer lists.
    pub(super) fn forget_unlisted(&mut self, profile_id: Uuid, listed: &HashSet<&str>) {
        // An empty list says nothing about which databases are gone.
        if listed.is_empty() {
            return;
        }

        let gone: Vec<String> = self
            .lists(profile_id)
            .databases
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
    queued_keys: HashSet<ObjectTreeRequestKey>,
    in_flight: HashSet<ObjectTreeRequestKey>,
}

impl PaletteFetchQueue {
    /// Queues `keys` that are neither queued nor in flight already.
    pub(super) fn enqueue(&mut self, keys: impl IntoIterator<Item = ObjectTreeRequestKey>) {
        for key in keys {
            if !self.in_flight.contains(&key) && self.queued_keys.insert(key.clone()) {
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
        self.queued_keys.remove(&key);
        self.in_flight.insert(key.clone());
        Some(key)
    }

    /// Drops the requests not started yet; those in flight still settle.
    pub(super) fn clear_queued(&mut self) {
        self.queued.clear();
        self.queued_keys.clear();
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

                if worth_fetching(state, cache, profile_id, &database) {
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
            Some((database.to_string(), relational_listing(&target, node)))
        })
        .collect()
}

fn relational_listing(target: &RelationalTarget<'_>, node: &ObjectTreeNode) -> DatabaseListing {
    if node.state == NodeContent::Unloaded {
        return DatabaseListing::Unloaded;
    }

    let mut objects = Vec::new();
    push_projected_relational_items(target, node, None, &mut objects);
    DatabaseListing::Loaded(objects)
}

/// Records the list of one database of a lazily loaded profile after its
/// fetch lands, without rebuilding the rest of the profile. A profile whose
/// cached lists this session has not asked for yet is left for the next
/// palette open.
pub(super) fn remember_database(
    state: &AppStateEntity,
    profile_id: Uuid,
    connected: &ConnectedProfile,
    database: &str,
    cache: &mut PaletteObjectCache,
) {
    if !cache.has_profile(profile_id)
        || connected.connection.schema_loading_strategy() != SchemaLoadingStrategy::LazyPerDatabase
    {
        return;
    }

    let Some(snapshot) = &connected.schema else {
        return;
    };

    let listed: Vec<&str> = match state.get_database_list(profile_id) {
        Some(list) => list.iter().map(|entry| entry.name.as_str()).collect(),
        None => snapshot
            .databases()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect(),
    };
    if only_shown_database(&connected.profile, listed.into_iter())
        .is_some_and(|only| only != database)
    {
        return;
    }

    let profile_name = connected.profile.name.as_str();
    let listing = match snapshot.database_layout() {
        DatabaseLayout::Schemas => {
            let Some(node) = project_database_node(state, profile_id, database) else {
                return;
            };
            let target = RelationalTarget {
                profile_id,
                profile_name,
                names_database: true,
            };
            relational_listing(&target, &node)
        }
        DatabaseLayout::Folder(folder) => {
            let objects = folder_objects(
                connected,
                folder,
                snapshot,
                database,
                true,
                is_current_database(snapshot, database),
            );
            folder_listing(profile_id, profile_name, database, objects)
        }
        DatabaseLayout::Keyspaces => DatabaseListing::Unloaded,
    };

    if let DatabaseListing::Loaded(objects) = listing {
        cache.remember(
            profile_id,
            database,
            objects.iter().filter_map(indexed_object).collect(),
        );
    }
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
            let objects = folder_objects(
                connected,
                folder,
                snapshot,
                database,
                lazy,
                is_current_database(snapshot, database),
            );
            (
                database.to_string(),
                folder_listing(profile_id, profile_name, database, objects),
            )
        })
        .collect()
}

fn folder_listing(
    profile_id: Uuid,
    profile_name: &str,
    database: &str,
    objects: FolderObjects<'_>,
) -> DatabaseListing {
    match objects {
        FolderObjects::Listed(objects) => DatabaseListing::Loaded(
            objects
                .iter()
                .map(|object| collection_item(profile_id, profile_name, database, &object.name))
                .collect(),
        ),
        FolderObjects::Unlisted => DatabaseListing::Loaded(Vec::new()),
        FolderObjects::Unloaded => DatabaseListing::Unloaded,
    }
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

/// A database is fetched for the palette once per connection session: not
/// while a fetch for it runs, and not again after the palette's fetch failed
/// in this session.
fn worth_fetching(
    state: &AppStateEntity,
    cache: &PaletteObjectCache,
    profile_id: Uuid,
    database: &str,
) -> bool {
    !cache.fetch_failed(
        profile_id,
        database,
        state.profile_session_generation(profile_id),
    ) && !state.object_tree_is_pending(&ObjectTreeRequestKey::DatabaseSchema {
        profile_id,
        database: database.to_string(),
    })
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
