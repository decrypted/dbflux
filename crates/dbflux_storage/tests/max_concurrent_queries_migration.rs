use dbflux_storage::migrations::MigrationRegistry;
use rusqlite::{Connection, Result as SqlResult};

#[test]
#[expect(
    clippy::unwrap_in_result,
    reason = "fixture setup uses expect() so a failure names the setup step that broke, not just a propagated error"
)]
fn query_limit_columns_default_to_eight_running_and_fifty_queued() -> SqlResult<()> {
    let connection = Connection::open_in_memory()?;
    MigrationRegistry::new()
        .run_all(&connection)
        .expect("existing migration chain must apply");

    let (running, queued): (i64, i64) = connection.query_row(
        "SELECT max_concurrent_queries, max_queued_queries FROM cfg_general_settings WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(running, 8);
    assert_eq!(queued, 50);

    Ok(())
}

#[test]
#[expect(
    clippy::unwrap_in_result,
    reason = "fixture setup uses expect() so a failure names the setup step that broke, not just a propagated error"
)]
fn query_limit_migration_upgrades_pre_column_schema_preserving_settings() -> SqlResult<()> {
    let connection = Connection::open_in_memory()?;
    let registry = MigrationRegistry::new();
    registry
        .run_all(&connection)
        .expect("migration chain must establish the fixture");

    connection
        .execute_batch(
            "ALTER TABLE cfg_general_settings DROP COLUMN max_concurrent_queries;
             ALTER TABLE cfg_general_settings DROP COLUMN max_queued_queries;",
        )
        .expect("SQLite must support dropping the added columns for the pre-061 fixture");
    connection.execute(
        "DELETE FROM sys_migrations WHERE name = '061_general_settings_max_concurrent_queries'",
        [],
    )?;
    connection.execute(
        "UPDATE cfg_general_settings SET theme = 'light', max_concurrent_background_tasks = 3 WHERE id = 1",
        [],
    )?;

    registry
        .run_all(&connection)
        .expect("migration 061 must upgrade populated settings");
    let (theme, background, running, queued): (String, i64, i64, i64) = connection.query_row(
        "SELECT theme, max_concurrent_background_tasks, max_concurrent_queries, max_queued_queries
         FROM cfg_general_settings WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(theme, "light");
    assert_eq!(background, 3);
    assert_eq!(running, 8);
    assert_eq!(queued, 50);

    registry
        .run_all(&connection)
        .expect("rerunning the registry must be idempotent");
    let applied: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sys_migrations WHERE name = '061_general_settings_max_concurrent_queries'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(applied, 1, "migration 061 must remain recorded once");

    Ok(())
}
