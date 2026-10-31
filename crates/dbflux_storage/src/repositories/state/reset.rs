//! Clearing categories of stored state from Settings → Reset.
//!
//! Each category maps to the exact keys and tables it owns, so a reset can
//! never reach connection profiles, auth/proxy/SSH tunnel profiles, driver
//! parameters, hooks, RPC services, MCP governance, saved queries or saved
//! audit filters. Keyring secrets live outside this database entirely.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::ui_state::keys;
use crate::bootstrap::OwnedConnection;
use crate::error::StorageError;

/// One kind of stored state that Settings → Reset clears as a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResetCategory {
    /// Remembered window and dialog sizes, and the results panel position.
    WindowLayout,
    /// Hidden, moved, pinned and resized grid columns, per table.
    ColumnLayouts,
    /// WHERE filters of table grids and queries of document collections.
    FilterHistory,
    /// The choices of the data grid's export menu.
    ExportOptions,
    /// The tabs restored at startup and the connections reopened with them.
    Session,
    /// The SQL editor's query history, starred entries excepted.
    QueryHistory,
    /// Files listed as recently opened.
    RecentFiles,
    /// Schema snapshots captured on connect, used by the schema diff.
    SchemaSnapshots,
    /// Settings → General, back to its defaults.
    GeneralSettings,
    /// Every audit event. Kept out of [`ResetCategory::BULK`].
    AuditLog,
}

impl ResetCategory {
    /// Every category, in the order Settings lists them.
    pub const ALL: [ResetCategory; 10] = [
        ResetCategory::WindowLayout,
        ResetCategory::ColumnLayouts,
        ResetCategory::FilterHistory,
        ResetCategory::ExportOptions,
        ResetCategory::Session,
        ResetCategory::QueryHistory,
        ResetCategory::RecentFiles,
        ResetCategory::SchemaSnapshots,
        ResetCategory::GeneralSettings,
        ResetCategory::AuditLog,
    ];

    /// What "Reset everything" clears. The audit log is a record of what
    /// happened, so it is only ever cleared on its own.
    pub const BULK: [ResetCategory; 9] = [
        ResetCategory::WindowLayout,
        ResetCategory::ColumnLayouts,
        ResetCategory::FilterHistory,
        ResetCategory::ExportOptions,
        ResetCategory::Session,
        ResetCategory::QueryHistory,
        ResetCategory::RecentFiles,
        ResetCategory::SchemaSnapshots,
        ResetCategory::GeneralSettings,
    ];

    /// A stable identifier, for element ids and audit records.
    pub fn id(self) -> &'static str {
        match self {
            ResetCategory::WindowLayout => "window-layout",
            ResetCategory::ColumnLayouts => "column-layouts",
            ResetCategory::FilterHistory => "filter-history",
            ResetCategory::ExportOptions => "export-options",
            ResetCategory::Session => "session",
            ResetCategory::QueryHistory => "query-history",
            ResetCategory::RecentFiles => "recent-files",
            ResetCategory::SchemaSnapshots => "schema-snapshots",
            ResetCategory::GeneralSettings => "general-settings",
            ResetCategory::AuditLog => "audit-log",
        }
    }
}

/// `st_ui_state` keys owned by a category: exact keys and key prefixes. A
/// prefix owns the keys `<prefix>:<anything>`.
fn ui_state_targets(category: ResetCategory) -> (&'static [&'static str], &'static [&'static str]) {
    match category {
        ResetCategory::WindowLayout => (
            &[
                keys::MAIN_WINDOW_GEOMETRY,
                keys::CELL_EDITOR_GEOMETRY,
                keys::QUERY_RESULTS_POSITION,
            ],
            &[],
        ),
        ResetCategory::ColumnLayouts => (&[], &[keys::GRID_COLUMN_LAYOUT_PREFIX]),
        ResetCategory::FilterHistory => (
            &[],
            &[
                keys::GRID_FILTER_HISTORY_PREFIX,
                keys::COLLECTION_QUERY_HISTORY_PREFIX,
            ],
        ),
        ResetCategory::ExportOptions => (&[keys::GRID_EXPORT_OPTIONS], &[]),
        ResetCategory::Session => (&[keys::LAST_CONNECTED_PROFILES], &[]),
        _ => (&[], &[]),
    }
}

/// What a category holds right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResetStats {
    /// Keys or rows the reset would remove.
    pub count: u64,
    /// Oldest and newest entry (RFC 3339, UTC) where entries carry a time.
    pub range: Option<(String, String)>,
}

fn sqlite_err(source: rusqlite::Error) -> StorageError {
    StorageError::Sqlite {
        path: "dbflux.db".into(),
        source,
    }
}

/// Counts and clears [`ResetCategory`] state in `dbflux.db`.
pub struct StateResetRepository {
    conn: OwnedConnection,
}

impl StateResetRepository {
    pub fn new(conn: OwnedConnection) -> Self {
        Self { conn }
    }

    fn conn(&self) -> &Connection {
        &self.conn
    }

    /// What `category` holds.
    pub fn stats(&self, category: ResetCategory) -> Result<ResetStats, StorageError> {
        let conn = self.conn();

        let count = match category {
            ResetCategory::WindowLayout
            | ResetCategory::ColumnLayouts
            | ResetCategory::FilterHistory
            | ResetCategory::ExportOptions => count_ui_state(conn, category)?,
            ResetCategory::Session => {
                count_rows(conn, "SELECT COUNT(*) FROM st_session_tabs")?
                    + count_ui_state(conn, category)?
            }
            ResetCategory::QueryHistory => count_rows(
                conn,
                "SELECT COUNT(*) FROM st_query_history WHERE is_favorite = 0",
            )?,
            ResetCategory::RecentFiles => count_rows(conn, "SELECT COUNT(*) FROM st_recent_items")?,
            ResetCategory::SchemaSnapshots => {
                count_rows(conn, "SELECT COUNT(*) FROM sch_schema_snapshots")?
            }
            ResetCategory::GeneralSettings => {
                count_rows(conn, "SELECT COUNT(*) FROM cfg_general_settings")?
            }
            ResetCategory::AuditLog => count_rows(conn, "SELECT COUNT(*) FROM aud_audit_events")?,
        };

        let range = match category {
            ResetCategory::AuditLog if count > 0 => audit_range(conn)?,
            _ => None,
        };

        Ok(ResetStats { count, range })
    }

    /// Clears `category` in one transaction and returns how many keys or
    /// rows went.
    pub fn reset(&self, category: ResetCategory) -> Result<u64, StorageError> {
        self.reset_all(&[category])
    }

    /// Clears every category in `categories` in one transaction: either all
    /// of them are cleared or none is.
    pub fn reset_all(&self, categories: &[ResetCategory]) -> Result<u64, StorageError> {
        let tx = self.conn().unchecked_transaction().map_err(sqlite_err)?;

        let mut removed = 0;
        for category in categories {
            removed += clear(&tx, *category)?;
        }

        tx.commit().map_err(sqlite_err)?;

        log::info!("Reset stored state {categories:?}: {removed} entries removed");
        Ok(removed)
    }
}

fn count_rows(conn: &Connection, sql: &str) -> Result<u64, StorageError> {
    let count: i64 = conn
        .query_row(sql, [], |row| row.get(0))
        .map_err(sqlite_err)?;
    Ok(u64::try_from(count).unwrap_or_default())
}

/// `LIKE` would treat the `_` in the prefixes as a wildcard, so prefixes
/// match by comparing the leading characters instead.
const PREFIX_MATCH: &str = "substr(key, 1, length(?1) + 1) = ?1 || ':'";

fn count_ui_state(conn: &Connection, category: ResetCategory) -> Result<u64, StorageError> {
    let (exact, prefixes) = ui_state_targets(category);
    let mut count = 0;

    for key in exact {
        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM st_ui_state WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .map_err(sqlite_err)?;
        count += u64::try_from(found).unwrap_or_default();
    }

    for prefix in prefixes {
        let found: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM st_ui_state WHERE {PREFIX_MATCH}"),
                [prefix],
                |row| row.get(0),
            )
            .map_err(sqlite_err)?;
        count += u64::try_from(found).unwrap_or_default();
    }

    Ok(count)
}

fn clear_ui_state(tx: &Transaction<'_>, category: ResetCategory) -> Result<u64, StorageError> {
    let (exact, prefixes) = ui_state_targets(category);
    let mut removed = 0;

    for key in exact {
        removed += tx
            .execute("DELETE FROM st_ui_state WHERE key = ?1", params![key])
            .map_err(sqlite_err)? as u64;
    }

    for prefix in prefixes {
        removed += tx
            .execute(
                &format!("DELETE FROM st_ui_state WHERE {PREFIX_MATCH}"),
                params![prefix],
            )
            .map_err(sqlite_err)? as u64;
    }

    Ok(removed)
}

fn clear(tx: &Transaction<'_>, category: ResetCategory) -> Result<u64, StorageError> {
    let execute = |sql: &str| -> Result<u64, StorageError> {
        tx.execute(sql, []).map(|n| n as u64).map_err(sqlite_err)
    };

    match category {
        ResetCategory::WindowLayout
        | ResetCategory::ColumnLayouts
        | ResetCategory::FilterHistory
        | ResetCategory::ExportOptions => clear_ui_state(tx, category),
        ResetCategory::Session => {
            // The session rows stay: restoring an empty workspace session
            // removes the scratch files its old tabs left behind, which a
            // missing session would never get to.
            let tabs = execute("DELETE FROM st_session_tabs")?;
            execute("UPDATE st_sessions SET active_index = NULL")?;
            Ok(tabs + clear_ui_state(tx, category)?)
        }
        ResetCategory::QueryHistory => {
            execute("DELETE FROM st_query_history WHERE is_favorite = 0")
        }
        ResetCategory::RecentFiles => execute("DELETE FROM st_recent_items"),
        ResetCategory::SchemaSnapshots => {
            // Explicit child delete: an older database may lack the cascade.
            execute("DELETE FROM sch_snapshot_tables")?;
            execute("DELETE FROM sch_schema_snapshots")
        }
        ResetCategory::GeneralSettings => {
            // Without the row the loader falls back to the defaults.
            execute("DELETE FROM cfg_general_settings")
        }
        ResetCategory::AuditLog => {
            // Explicit child deletes: the audit store rebuilds its events
            // table without the cascade on some databases.
            execute("DELETE FROM aud_audit_event_attributes")?;
            execute("DELETE FROM aud_audit_event_entities")?;
            execute("DELETE FROM aud_audit_events")
        }
    }
}

fn audit_range(conn: &Connection) -> Result<Option<(String, String)>, StorageError> {
    let bounds: Option<(Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT MIN(created_at_epoch_ms), MAX(created_at_epoch_ms) FROM aud_audit_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sqlite_err)?;

    let Some((Some(oldest), Some(newest))) = bounds else {
        return Ok(None);
    };

    let to_rfc3339 = |ms: i64| {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
            .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    };

    Ok(to_rfc3339(oldest).zip(to_rfc3339(newest)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::StorageRuntime;

    /// Every key Settings → Reset knows of, plus keys no category owns.
    const SEEDED_UI_KEYS: &[&str] = &[
        "main_window_geometry",
        "cell_editor_geometry",
        "query_results_position",
        "grid_column_layout.v1:[\"p\",\"db\",\"public\",\"users\"]",
        "grid_column_layout.v1:[\"p\",\"db\",\"public\",\"orders\"]",
        "grid_filter_history.v1:[\"p\",\"db\",\"public\",\"users\"]",
        "collection_query_history.v1:[\"p\",\"db\",\"events\"]",
        "grid_export_options.v1",
        "last_connected_profiles",
        // Not owned by any category: must survive every reset.
        "grid_column_layout.v1_unrelated",
        "gridXcolumn_layout.v1:wildcard",
        "some_future_key",
    ];

    const UNOWNED_UI_KEYS: &[&str] = &[
        "grid_column_layout.v1_unrelated",
        "gridXcolumn_layout.v1:wildcard",
        "some_future_key",
    ];

    const PROFILE_ID: &str = "11111111-1111-1111-1111-111111111111";

    fn seed(runtime: &StorageRuntime) {
        let conn = runtime.dbflux_db();

        for key in SEEDED_UI_KEYS {
            runtime.ui_state().set(key, "{}").expect("seed ui state");
        }

        // Connection-side data no reset may touch.
        conn.execute_batch(&format!(
            "INSERT INTO cfg_connection_profiles (id, name, driver_id, created_at, updated_at)
                 VALUES ('{PROFILE_ID}', 'prod', 'builtin:postgres', datetime('now'), datetime('now'));
             INSERT INTO cfg_ssh_tunnel_profiles (id, name, host, port, user, auth_method)
                 VALUES ('ssh-1', 'bastion', 'bastion.example.com', 22, 'deploy', 'private_key');
             INSERT INTO cfg_proxy_profiles (id, name, kind, host, port)
                 VALUES ('proxy-1', 'corp', 'socks5', 'proxy.example.com', 1080);
             INSERT INTO st_saved_queries (id, name, sql) VALUES ('saved-1', 'keep me', 'SELECT 1');
             INSERT INTO aud_saved_filters (name, filter_json) VALUES ('mine', '{{}}');
             INSERT INTO cfg_connection_profile_driver_params (profile_id, position, param_key, param_value)
                 VALUES ('{PROFILE_ID}', 0, 'compress', 'true');"
        ))
        .expect("seed connection data");

        conn.execute_batch(&format!(
            "INSERT INTO st_sessions (id, name, kind, active_index) VALUES ('s1', 'workspace', 'workspace', 1);
             INSERT INTO st_session_tabs (id, session_id, tab_kind, title, position)
                 VALUES ('t1', 's1', 'Scratch', 'one', 0), ('t2', 's1', 'Scratch', 'two', 1);
             INSERT INTO st_query_history (id, query_text, is_favorite)
                 VALUES ('h1', 'SELECT 1', 0), ('h2', 'SELECT 2', 0), ('h3', 'SELECT 3', 1);
             INSERT INTO st_recent_items (id, kind, path, title) VALUES ('r1', 'file', '/tmp/a.sql', 'a.sql');
             INSERT INTO sch_schema_snapshots (id, profile_id, database, captured_at, fingerprint, depth)
                 VALUES ('snap-1', '{PROFILE_ID}', 'db', 1000, 'abc', 'tables');
             INSERT INTO sch_snapshot_tables (snapshot_id, schema_name, name, detail_json)
                 VALUES ('snap-1', 'public', 'users', '{{}}');
             INSERT OR REPLACE INTO cfg_general_settings (id, theme) VALUES (1, 'light');
             INSERT INTO aud_audit_events (actor_id, tool_id, decision, created_at_epoch_ms)
                 VALUES ('user', 'query', 'allow', 1700000000000),
                        ('user', 'query', 'allow', 1760000000000);"
        ))
        .expect("seed state rows");
    }

    fn count(runtime: &StorageRuntime, sql: &str) -> i64 {
        runtime
            .dbflux_db()
            .query_row(sql, [], |row| row.get(0))
            .expect("count")
    }

    fn ui_keys(runtime: &StorageRuntime) -> Vec<String> {
        runtime
            .ui_state()
            .all()
            .expect("ui state")
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    /// Rows of every table some category owns, plus the connection-side
    /// rows no category owns.
    fn snapshot(runtime: &StorageRuntime) -> Vec<(&'static str, i64)> {
        [
            "SELECT COUNT(*) FROM st_session_tabs",
            "SELECT COUNT(*) FROM st_sessions",
            "SELECT COUNT(*) FROM st_query_history WHERE is_favorite = 0",
            "SELECT COUNT(*) FROM st_query_history WHERE is_favorite = 1",
            "SELECT COUNT(*) FROM st_recent_items",
            "SELECT COUNT(*) FROM sch_schema_snapshots",
            "SELECT COUNT(*) FROM sch_snapshot_tables",
            "SELECT COUNT(*) FROM cfg_general_settings",
            "SELECT COUNT(*) FROM aud_audit_events",
            "SELECT COUNT(*) FROM cfg_connection_profiles",
            "SELECT COUNT(*) FROM cfg_ssh_tunnel_profiles",
            "SELECT COUNT(*) FROM cfg_proxy_profiles",
            "SELECT COUNT(*) FROM st_saved_queries",
            "SELECT COUNT(*) FROM aud_saved_filters",
            "SELECT COUNT(*) FROM cfg_connection_profile_driver_params",
        ]
        .into_iter()
        .map(|sql| (sql, count(runtime, sql)))
        .collect()
    }

    /// The tables a category empties, as `snapshot` names them.
    fn owned_tables(category: ResetCategory) -> &'static [&'static str] {
        match category {
            ResetCategory::Session => &["SELECT COUNT(*) FROM st_session_tabs"],
            ResetCategory::QueryHistory => {
                &["SELECT COUNT(*) FROM st_query_history WHERE is_favorite = 0"]
            }
            ResetCategory::RecentFiles => &["SELECT COUNT(*) FROM st_recent_items"],
            ResetCategory::SchemaSnapshots => &[
                "SELECT COUNT(*) FROM sch_schema_snapshots",
                "SELECT COUNT(*) FROM sch_snapshot_tables",
            ],
            ResetCategory::GeneralSettings => &["SELECT COUNT(*) FROM cfg_general_settings"],
            ResetCategory::AuditLog => &["SELECT COUNT(*) FROM aud_audit_events"],
            _ => &[],
        }
    }

    fn owned_keys(category: ResetCategory) -> Vec<&'static str> {
        SEEDED_UI_KEYS
            .iter()
            .copied()
            .filter(|key| {
                let (exact, prefixes) = ui_state_targets(category);
                exact.contains(key)
                    || prefixes
                        .iter()
                        .any(|prefix| key.starts_with(&format!("{prefix}:")))
            })
            .collect()
    }

    #[test]
    fn each_category_removes_exactly_its_own_state() {
        for category in ResetCategory::ALL {
            let runtime = StorageRuntime::in_memory().expect("runtime");
            seed(&runtime);
            let before = snapshot(&runtime);
            let repo = runtime.state_reset();

            let stats = repo.stats(category).expect("stats");
            assert!(stats.count > 0, "{category:?} has seeded state");

            let removed = repo.reset(category).expect("reset");
            assert_eq!(removed, stats.count, "{category:?} removes what it counted");
            assert_eq!(repo.stats(category).expect("stats").count, 0);

            let after = snapshot(&runtime);
            for ((sql, was), (_, now)) in before.iter().zip(after.iter()) {
                if owned_tables(category).contains(sql) {
                    assert_eq!(*now, 0, "{category:?} empties `{sql}`");
                } else {
                    assert_eq!(was, now, "{category:?} leaves `{sql}` alone");
                }
            }

            let owned = owned_keys(category);
            let remaining = ui_keys(&runtime);
            for key in SEEDED_UI_KEYS {
                assert_eq!(
                    remaining.iter().any(|k| k == key),
                    !owned.contains(key),
                    "{category:?} and ui key {key}"
                );
            }
        }
    }

    #[test]
    fn the_bulk_reset_keeps_audit_events_and_connections() {
        let runtime = StorageRuntime::in_memory().expect("runtime");
        seed(&runtime);
        let repo = runtime.state_reset();

        repo.reset_all(&ResetCategory::BULK).expect("bulk reset");

        for category in ResetCategory::BULK {
            assert_eq!(
                repo.stats(category).expect("stats").count,
                0,
                "{category:?}"
            );
        }
        assert_eq!(count(&runtime, "SELECT COUNT(*) FROM aud_audit_events"), 2);
        assert_eq!(
            count(&runtime, "SELECT COUNT(*) FROM cfg_connection_profiles"),
            1
        );
        assert_eq!(
            count(&runtime, "SELECT COUNT(*) FROM cfg_ssh_tunnel_profiles"),
            1
        );
        assert_eq!(
            count(&runtime, "SELECT COUNT(*) FROM cfg_proxy_profiles"),
            1
        );
        assert_eq!(count(&runtime, "SELECT COUNT(*) FROM st_saved_queries"), 1);
        assert_eq!(
            count(
                &runtime,
                "SELECT COUNT(*) FROM st_query_history WHERE is_favorite = 1"
            ),
            1,
            "starred history entries stay"
        );

        let remaining = ui_keys(&runtime);
        for key in UNOWNED_UI_KEYS {
            assert!(remaining.iter().any(|k| k == key), "{key} survives");
        }
        assert_eq!(remaining.len(), UNOWNED_UI_KEYS.len());
    }

    #[test]
    fn the_audit_count_comes_with_its_date_range() {
        let runtime = StorageRuntime::in_memory().expect("runtime");
        seed(&runtime);

        let stats = runtime
            .state_reset()
            .stats(ResetCategory::AuditLog)
            .expect("stats");

        assert_eq!(stats.count, 2);
        assert_eq!(
            stats.range,
            Some((
                "2023-11-14T22:13:20Z".to_string(),
                "2025-10-09T08:53:20Z".to_string()
            ))
        );
    }

    /// A fresh database holds nothing to reset but the general settings
    /// row the migrations create with the defaults.
    #[test]
    fn a_fresh_database_resets_to_nothing_but_the_settings_row() {
        let runtime = StorageRuntime::in_memory().expect("runtime");
        let repo = runtime.state_reset();

        for category in ResetCategory::ALL {
            let expected = u64::from(category == ResetCategory::GeneralSettings);
            assert_eq!(
                repo.stats(category).expect("stats").count,
                expected,
                "{category:?}"
            );
        }
        assert_eq!(repo.reset_all(&ResetCategory::ALL).expect("reset"), 1);
        assert_eq!(repo.reset_all(&ResetCategory::ALL).expect("reset"), 0);
    }

    #[test]
    fn the_bulk_set_is_every_category_but_the_audit_log() {
        let bulk: Vec<_> = ResetCategory::ALL
            .into_iter()
            .filter(|category| *category != ResetCategory::AuditLog)
            .collect();

        assert_eq!(bulk, ResetCategory::BULK.to_vec());
    }
}
