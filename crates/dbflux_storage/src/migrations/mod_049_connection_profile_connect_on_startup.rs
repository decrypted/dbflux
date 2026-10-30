//! Migration 049: Add `connect_on_startup` column to `cfg_connection_profiles`.
//!
//! Stores whether the app connects the profile when it starts (`1`). Existing
//! profiles keep waiting for a manual connect.
//!
//! Independent of migration 048 so the two can land in either order.

use rusqlite::Transaction;

use crate::migrations::{Migration, MigrationError};

pub(crate) struct MigrationImpl;

impl Migration for MigrationImpl {
    fn name(&self) -> &str {
        "049_connection_profile_connect_on_startup"
    }

    fn run(&self, tx: &Transaction) -> Result<(), MigrationError> {
        let table_exists: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='cfg_connection_profiles'",
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
                "SELECT COUNT(*) FROM pragma_table_info('cfg_connection_profiles') WHERE name = 'connect_on_startup'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .map_err(sqlite_err)?;

        if !column_exists {
            tx.execute_batch("ALTER TABLE cfg_connection_profiles ADD COLUMN connect_on_startup INTEGER NOT NULL DEFAULT 0;")
                .map_err(sqlite_err)?;
        }

        Ok(())
    }
}

fn sqlite_err(source: rusqlite::Error) -> MigrationError {
    MigrationError::Sqlite {
        path: std::path::PathBuf::from("<049_connection_profile_connect_on_startup>"),
        source,
    }
}
