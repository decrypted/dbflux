//! Migration 059: Add the `cfg_connection_profile_driver_params` child table.
//!
//! Holds the per-connection driver parameters (`key = value` pairs a driver
//! maps onto its client options) in the order the user listed them, which is
//! the order later duplicates override earlier ones. Existing profiles have no
//! rows, so they connect exactly as before, except where a driver's default
//! for an absent parameter is documented to change.
//!
//! Independent of migrations 048-058 so they can land in any order.

use rusqlite::Transaction;

use crate::migrations::{Migration, MigrationError};

pub(crate) struct MigrationImpl;

impl Migration for MigrationImpl {
    fn name(&self) -> &str {
        "059_connection_profile_driver_params"
    }

    fn run(&self, tx: &Transaction) -> Result<(), MigrationError> {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS cfg_connection_profile_driver_params (
                profile_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                param_key TEXT NOT NULL,
                param_value TEXT NOT NULL,
                PRIMARY KEY (profile_id, position),
                FOREIGN KEY (profile_id) REFERENCES cfg_connection_profiles(id) ON DELETE CASCADE
            );",
        )
        .map_err(sqlite_err)?;

        Ok(())
    }
}

fn sqlite_err(source: rusqlite::Error) -> MigrationError {
    MigrationError::Sqlite {
        path: std::path::PathBuf::from("<059_connection_profile_driver_params>"),
        source,
    }
}
