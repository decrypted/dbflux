//! Migration 063: Store where a saved chart's query runs.
//!
//! A chart reopened on the connection's default database either fails on an
//! unqualified table name or silently reads a same-named table somewhere else,
//! and a CloudWatch Logs Insights chart has no log groups to read at all.
//!
//! Additive and nullable, so a chart saved before this migration reads as "no
//! stored context" and keeps running where it did, and an older build sharing
//! the data folder keeps working: it selects and inserts by column name.
//!
//! Keyed by name only: it checks for its own columns and does not depend on any
//! migration after 012, the last rebuild of `viz_saved_charts`.

use rusqlite::Transaction;

use crate::migrations::{Migration, MigrationError};

pub(crate) struct MigrationImpl;

const CONTEXT_COLUMNS: [&str; 4] = [
    "context_database",
    "context_schema",
    "context_container",
    "context_query_mode",
];

impl Migration for MigrationImpl {
    fn name(&self) -> &str {
        "063_viz_saved_chart_context"
    }

    fn run(&self, tx: &Transaction) -> Result<(), MigrationError> {
        let table_exists: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='viz_saved_charts'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count > 0)
            .map_err(sqlite_err)?;

        if !table_exists {
            return Ok(());
        }

        for column in CONTEXT_COLUMNS {
            let column_exists: bool = tx
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('viz_saved_charts') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0),
                )
                .map(|count| count > 0)
                .map_err(sqlite_err)?;

            if !column_exists {
                tx.execute_batch(&format!(
                    "ALTER TABLE viz_saved_charts ADD COLUMN {column} TEXT;"
                ))
                .map_err(sqlite_err)?;
            }
        }

        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS viz_saved_chart_context_targets (
                 chart_id     TEXT    NOT NULL
                     REFERENCES viz_saved_charts(id) ON DELETE CASCADE,
                 target_index INTEGER NOT NULL CHECK (target_index >= 0),
                 target       TEXT    NOT NULL,
                 PRIMARY KEY (chart_id, target_index)
             );",
        )
        .map_err(sqlite_err)?;

        Ok(())
    }
}

fn sqlite_err(source: rusqlite::Error) -> MigrationError {
    MigrationError::Sqlite {
        path: std::path::PathBuf::from("<unknown>"),
        source,
    }
}
