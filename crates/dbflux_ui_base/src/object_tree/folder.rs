//! Which cache lists the objects of a database whose objects sit in one
//! folder, shared by the sidebar and the command palette.

use dbflux_core::{ConnectedProfile, ObjectFolder, SchemaSnapshot, TableInfo};
use std::borrow::Cow;

/// Where the objects of one database stand in the caches.
pub enum FolderObjects<'a> {
    /// Known objects, possibly none.
    Listed(Cow<'a, [TableInfo]>),
    /// The database has its own connection, whose snapshot lists nothing for
    /// this folder.
    Unlisted,
    /// Nothing describes the database yet.
    Unloaded,
}

/// Picks the cache that describes the objects of `database`: the lazily
/// fetched schema, then the snapshot of the database's own connection, then
/// the connect-time snapshot for the current database.
pub fn folder_objects<'a>(
    connected: &'a ConnectedProfile,
    folder: ObjectFolder,
    snapshot: &SchemaSnapshot,
    database: &str,
    uses_lazy_loading: bool,
    is_current: bool,
) -> FolderObjects<'a> {
    if uses_lazy_loading {
        if let Some(loaded) = connected.database_schemas.get(database) {
            return FolderObjects::Listed(Cow::Borrowed(&loaded.tables));
        }

        if is_current && folder.snapshot_lists_current_objects() {
            return FolderObjects::Listed(Cow::Owned(snapshot.folder_objects(database)));
        }

        return FolderObjects::Unloaded;
    }

    if let Some(slot) = connected.database_connections.get(database) {
        return match slot.schema.as_ref() {
            Some(slot_snapshot) if folder.database_connection_lists_objects() => {
                FolderObjects::Listed(Cow::Owned(slot_snapshot.folder_objects(database)))
            }
            _ => FolderObjects::Unlisted,
        };
    }

    if is_current {
        return FolderObjects::Listed(Cow::Owned(snapshot.folder_objects(database)));
    }

    FolderObjects::Unloaded
}

/// Whether the snapshot names `database` as the one the connection opened.
pub fn is_current_database(snapshot: &SchemaSnapshot, database: &str) -> bool {
    snapshot
        .databases()
        .iter()
        .any(|listed| listed.name == database && listed.is_current)
}
