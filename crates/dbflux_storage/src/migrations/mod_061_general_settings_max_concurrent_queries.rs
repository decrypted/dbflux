//! Migration 061: Add `max_concurrent_queries` and `max_queued_queries`
//! columns to `cfg_general_settings`.
//!
//! Persists the per-connection query limit (how many queries run at once on
//! one connection, default 8) and the length of the queue in front of it
//! (default 50).
//!
//! Keyed by name only: it checks for its own columns and does not depend on
//! any migration after 047.

use rusqlite::Transaction;

use crate::migrations::{Migration, MigrationError};

pub(crate) struct MigrationImpl;

const COLUMNS: [(&str, i64); 2] = [("max_concurrent_queries", 8), ("max_queued_queries", 50)];

impl Migration for MigrationImpl {
    fn name(&self) -> &str {
        "061_general_settings_max_concurrent_queries"
    }

    fn run(&self, tx: &Transaction) -> Result<(), MigrationError> {
        let table_exists: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cfg_general_settings'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .map_err(sqlite_err)?;

        if !table_exists {
            return Ok(());
        }

        for (column, default) in COLUMNS {
            let column_exists: bool = tx
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('cfg_general_settings') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0),
                )
                .map(|count| count > 0)
                .map_err(sqlite_err)?;

            if !column_exists {
                tx.execute_batch(&format!(
                    "ALTER TABLE cfg_general_settings ADD COLUMN {column} INTEGER NOT NULL DEFAULT {default};"
                ))
                .map_err(sqlite_err)?;
            }
        }

        Ok(())
    }
}

fn sqlite_err(source: rusqlite::Error) -> MigrationError {
    MigrationError::Sqlite {
        path: std::path::PathBuf::from("<unknown>"),
        source,
    }
}
