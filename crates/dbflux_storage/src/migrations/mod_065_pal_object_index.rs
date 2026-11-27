//! Migration 065: `pal_object_index`, the tables, views and collections the
//! command palette last saw in each database of a connection profile.
//!
//! The palette lists them for a database that is not loaded in this session
//! yet, so a search finds objects across restarts before the database is
//! fetched again. Each database's rows are replaced as a whole when a fresh
//! list arrives.

use rusqlite::Transaction;

use crate::migrations::{Migration, MigrationError};

pub(crate) struct MigrationImpl;

impl Migration for MigrationImpl {
    fn name(&self) -> &str {
        "065_pal_object_index"
    }

    fn run(&self, tx: &Transaction) -> Result<(), MigrationError> {
        tx.execute_batch(SCHEMA).map_err(sqlite_err)?;
        Ok(())
    }
}

fn sqlite_err(source: rusqlite::Error) -> MigrationError {
    MigrationError::Sqlite {
        path: std::path::PathBuf::from("<065_pal_object_index>"),
        source,
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS pal_object_index (
    profile_id    TEXT    NOT NULL,
    database_name TEXT    NOT NULL,
    schema_name   TEXT,
    object_name   TEXT    NOT NULL,
    object_kind   TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pal_object_index_database
    ON pal_object_index (profile_id, database_name);
"#;
