//! Migration 058: Add `grid_header_click_sorts` column to `cfg_general_settings`.
//!
//! Persists the opt-in to the data grid's earlier header click, which sorts
//! the column instead of selecting it. The column defaults to 0, so a click
//! on a header selects the column unless the user turns this on.
//!
//! Keyed by name only: it checks for its own column and does not depend on
//! any migration after 047.

use rusqlite::Transaction;

use crate::migrations::{Migration, MigrationError};

pub(crate) struct MigrationImpl;

impl Migration for MigrationImpl {
    fn name(&self) -> &str {
        "058_general_settings_grid_header_click_sorts"
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

        let column_exists: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('cfg_general_settings') WHERE name = 'grid_header_click_sorts'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .map_err(sqlite_err)?;

        if !column_exists {
            tx.execute_batch(
                "ALTER TABLE cfg_general_settings ADD COLUMN grid_header_click_sorts INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(sqlite_err)?;
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
