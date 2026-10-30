use dbflux_storage::migrations::MigrationRegistry;
use rusqlite::{Connection, Result as SqlResult};

#[test]
#[expect(
    clippy::unwrap_in_result,
    reason = "fixture setup uses expect() so a failure names the setup step that broke, not just a propagated error"
)]
fn session_tab_view_state_migration_upgrades_pre_column_schema_preserving_tabs() -> SqlResult<()> {
    let connection = Connection::open_in_memory()?;
    let registry = MigrationRegistry::new();
    registry
        .run_all(&connection)
        .expect("migration chain must establish the fixture");

    connection
        .execute_batch("ALTER TABLE st_session_tabs DROP COLUMN view_state_json;")
        .expect("SQLite must support dropping the added column for the pre-050 fixture");
    connection.execute(
        "DELETE FROM sys_migrations WHERE name = '050_session_tab_view_state'",
        [],
    )?;
    connection.execute(
        "INSERT INTO st_sessions (id, name, kind) VALUES ('s1', 'workspace', 'workspace')",
        [],
    )?;
    connection.execute(
        "INSERT INTO st_session_tabs (id, session_id, tab_kind, title, position)
         VALUES ('t1', 's1', 'Scratch', 'Query 1', 0)",
        [],
    )?;

    registry
        .run_all(&connection)
        .expect("migration 050 must upgrade a schema with saved tabs");

    let (title, view_state): (String, Option<String>) = connection.query_row(
        "SELECT title, view_state_json FROM st_session_tabs WHERE id = 't1'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(title, "Query 1", "a tab saved before 050 must survive it");
    assert_eq!(view_state, None, "a tab saved before 050 has no view state");

    connection.execute(
        "UPDATE st_session_tabs SET view_state_json = '{\"filter\":\"id > 1\"}' WHERE id = 't1'",
        [],
    )?;

    registry
        .run_all(&connection)
        .expect("rerunning the registry must be idempotent");
    let applied: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sys_migrations WHERE name = '050_session_tab_view_state'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(applied, 1, "migration 050 must remain recorded once");

    let view_state: Option<String> = connection.query_row(
        "SELECT view_state_json FROM st_session_tabs WHERE id = 't1'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(view_state.as_deref(), Some("{\"filter\":\"id > 1\"}"));

    Ok(())
}
