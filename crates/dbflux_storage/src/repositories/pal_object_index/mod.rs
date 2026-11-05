//! Repository for `pal_object_index`: the tables, views and collections the
//! command palette last saw in each database of a connection profile.
//!
//! A database's rows are always replaced together, so a list never mixes
//! objects from two fetches.

use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::error::StorageError;

#[cfg(test)]
mod tests;

const DB_PATH: &str = "dbflux.db";

/// What kind of object an index row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexedObjectKind {
    Table,
    View,
    Collection,
}

impl IndexedObjectKind {
    fn as_text(self) -> &'static str {
        match self {
            IndexedObjectKind::Table => "table",
            IndexedObjectKind::View => "view",
            IndexedObjectKind::Collection => "collection",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "table" => Some(IndexedObjectKind::Table),
            "view" => Some(IndexedObjectKind::View),
            "collection" => Some(IndexedObjectKind::Collection),
            _ => None,
        }
    }
}

/// One object of a database, addressed the way the palette opens it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexedObject {
    pub schema: Option<String>,
    pub name: String,
    pub kind: IndexedObjectKind,
}

/// The objects last seen in one database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedDatabase {
    pub database: String,
    pub objects: Vec<IndexedObject>,
}

/// Repository for the `pal_object_index` table.
#[derive(Clone)]
pub struct PaletteObjectIndexRepo {
    conn: Arc<Mutex<Connection>>,
}

impl PaletteObjectIndexRepo {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Every database indexed for `profile_id`, by database name, each with
    /// its objects in the order they were written.
    pub fn load_profile(&self, profile_id: &str) -> Result<Vec<IndexedDatabase>, StorageError> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let mut statement = conn
            .prepare(
                "SELECT database_name, schema_name, object_name, object_kind
                 FROM pal_object_index
                 WHERE profile_id = ?1
                 ORDER BY database_name ASC, rowid ASC",
            )
            .map_err(sqlite_err)?;

        let rows = statement
            .query_map(rusqlite::params![profile_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;

        let mut databases: Vec<IndexedDatabase> = Vec::new();

        for (database, schema, name, kind) in rows {
            let Some(kind) = IndexedObjectKind::parse(&kind) else {
                log::warn!("Skipping palette index row with unknown kind '{kind}'");
                continue;
            };

            let object = IndexedObject { schema, name, kind };

            match databases.last_mut() {
                Some(last) if last.database == database => last.objects.push(object),
                _ => databases.push(IndexedDatabase {
                    database,
                    objects: vec![object],
                }),
            }
        }

        Ok(databases)
    }

    /// Replaces the objects indexed for `database` of `profile_id` with
    /// `objects`; an empty list removes the database from the index.
    pub fn replace_database(
        &self,
        profile_id: &str,
        database: &str,
        objects: &[IndexedObject],
    ) -> Result<(), StorageError> {
        let mut conn = self.conn.lock().map_err(lock_err)?;
        let transaction = conn.transaction().map_err(sqlite_err)?;

        transaction
            .execute(
                "DELETE FROM pal_object_index WHERE profile_id = ?1 AND database_name = ?2",
                rusqlite::params![profile_id, database],
            )
            .map_err(sqlite_err)?;

        {
            let mut insert = transaction
                .prepare(
                    "INSERT INTO pal_object_index
                         (profile_id, database_name, schema_name, object_name, object_kind)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(sqlite_err)?;

            for object in objects {
                insert
                    .execute(rusqlite::params![
                        profile_id,
                        database,
                        object.schema,
                        object.name,
                        object.kind.as_text(),
                    ])
                    .map_err(sqlite_err)?;
            }
        }

        transaction.commit().map_err(sqlite_err)
    }

    /// Forgets the rows of every profile not in `profile_ids`, such as one
    /// deleted while a write for it was still queued.
    pub fn retain_profiles(&self, profile_ids: &[String]) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(lock_err)?;

        let indexed: Vec<String> = conn
            .prepare("SELECT DISTINCT profile_id FROM pal_object_index")
            .map_err(sqlite_err)?
            .query_map([], |row| row.get(0))
            .map_err(sqlite_err)?
            .collect::<Result<_, _>>()
            .map_err(sqlite_err)?;

        for profile_id in indexed {
            if !profile_ids.contains(&profile_id) {
                conn.execute(
                    "DELETE FROM pal_object_index WHERE profile_id = ?1",
                    rusqlite::params![profile_id],
                )
                .map_err(sqlite_err)?;
            }
        }

        Ok(())
    }

    /// Forgets every database indexed for `profile_id`.
    pub fn delete_profile(&self, profile_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(lock_err)?;

        conn.execute(
            "DELETE FROM pal_object_index WHERE profile_id = ?1",
            rusqlite::params![profile_id],
        )
        .map_err(sqlite_err)?;

        Ok(())
    }
}

fn sqlite_err(source: rusqlite::Error) -> StorageError {
    StorageError::Sqlite {
        path: DB_PATH.into(),
        source,
    }
}

fn lock_err<T>(error: std::sync::PoisonError<T>) -> StorageError {
    StorageError::Sqlite {
        path: DB_PATH.into(),
        source: rusqlite::Error::InvalidParameterName(error.to_string()),
    }
}
