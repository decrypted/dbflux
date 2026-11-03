use dbflux_storage::migrations::MigrationRegistry;
use rusqlite::{Connection, Result as SqlResult};

const MIGRATION: &str = "063_viz_saved_chart_context";

/// The column list of builds from before migration 063, which select and insert
/// by name. They keep working on the migrated file.
const OLD_COLUMNS: &str = "id, name, profile_id, created_at, updated_at,
    chart_kind, legend_visible, decimation_threshold,
    track_source_indices, y_scale,
    x_axis_column_index, x_axis_label, x_axis_kind, x_axis_unit,
    binding_x, binding_group_by, binding_filter, binding_aggregation,
    source_kind, source_query,
    source_collection_database, source_collection_name,
    source_time_window_start_ms, source_time_window_end_ms,
    source_time_window_language,
    time_range_preset, refresh_policy_kind, refresh_policy_interval_secs";

fn insert_as_old_build(connection: &Connection, chart_id: &str) -> SqlResult<usize> {
    connection.execute(
        &format!(
            "INSERT OR REPLACE INTO viz_saved_charts ({OLD_COLUMNS})
             VALUES (?1, 'chart', 'profile', 1, 2,
                     'line', 1, 10000, 0, 'linear',
                     0, 'time', 'time', NULL,
                     0, NULL, NULL, 'none',
                     'query', 'select 1',
                     NULL, NULL, NULL, NULL, NULL,
                     NULL, 'off', NULL)"
        ),
        [chart_id],
    )
}

#[expect(
    clippy::expect_used,
    reason = "fixture setup uses expect() so a failure names the setup step that broke"
)]
fn migrated() -> Connection {
    let connection = Connection::open_in_memory().expect("in-memory database");
    MigrationRegistry::new()
        .run_all(&connection)
        .expect("migration chain must apply");
    connection
        .execute(
            "INSERT INTO cfg_connection_profiles (id, name) VALUES ('profile', 'profile')",
            [],
        )
        .expect("profile row");
    connection
}

fn context_of(connection: &Connection, chart_id: &str) -> SqlResult<[Option<String>; 4]> {
    connection.query_row(
        "SELECT context_database, context_schema, context_container, context_query_mode
         FROM viz_saved_charts WHERE id = ?1",
        [chart_id],
        |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?]),
    )
}

#[test]
fn context_migration_adds_nullable_columns_and_a_target_table() -> SqlResult<()> {
    let connection = migrated();

    insert_as_old_build(&connection, "chart")?;
    assert_eq!(context_of(&connection, "chart")?, [None, None, None, None]);

    connection.execute(
        "UPDATE viz_saved_charts SET context_database = 'analytics' WHERE id = 'chart'",
        [],
    )?;
    connection.execute(
        "INSERT INTO viz_saved_chart_context_targets (chart_id, target_index, target)
         VALUES ('chart', 0, '/aws/app')",
        [],
    )?;
    assert_eq!(
        context_of(&connection, "chart")?[0].as_deref(),
        Some("analytics")
    );

    Ok(())
}

#[test]
#[expect(
    clippy::unwrap_in_result,
    reason = "fixture setup uses expect() so a failure names the setup step that broke, not just a propagated error"
)]
fn context_migration_upgrades_a_pre_column_schema_preserving_charts() -> SqlResult<()> {
    let connection = migrated();
    let registry = MigrationRegistry::new();

    insert_as_old_build(&connection, "before")?;

    connection
        .execute_batch(
            "DROP TABLE viz_saved_chart_context_targets;
             ALTER TABLE viz_saved_charts DROP COLUMN context_database;
             ALTER TABLE viz_saved_charts DROP COLUMN context_schema;
             ALTER TABLE viz_saved_charts DROP COLUMN context_container;
             ALTER TABLE viz_saved_charts DROP COLUMN context_query_mode;",
        )
        .expect("SQLite must support dropping the added columns for the pre-063 fixture");
    connection.execute("DELETE FROM sys_migrations WHERE name = ?1", [MIGRATION])?;

    registry
        .run_all(&connection)
        .expect("migration 063 must upgrade populated charts");

    assert_eq!(
        context_of(&connection, "before")?,
        [None, None, None, None],
        "a chart from before keeps NULL and with it today's behaviour"
    );
    let source_query: String = connection.query_row(
        "SELECT source_query FROM viz_saved_charts WHERE id = 'before'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(source_query, "select 1");

    registry
        .run_all(&connection)
        .expect("rerunning the registry must be idempotent");
    let applied: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sys_migrations WHERE name = ?1",
        [MIGRATION],
        |row| row.get(0),
    )?;
    assert_eq!(applied, 1, "migration 063 must remain recorded once");

    Ok(())
}

/// An older build sharing the data folder reads and writes by name, so it never
/// sees the new columns. Saving a chart replaces the row, which loses the stored
/// context and brings that chart back to today's behaviour, and nothing breaks.
#[test]
fn an_older_build_keeps_working_on_the_migrated_file() -> SqlResult<()> {
    let connection = migrated();
    connection.pragma_update(None, "foreign_keys", "ON")?;

    insert_as_old_build(&connection, "chart")?;
    connection.execute(
        "UPDATE viz_saved_charts SET context_database = 'analytics' WHERE id = 'chart'",
        [],
    )?;
    connection.execute(
        "INSERT INTO viz_saved_chart_context_targets (chart_id, target_index, target)
         VALUES ('chart', 0, '/aws/app')",
        [],
    )?;

    let name: String = connection.query_row(
        &format!("SELECT {OLD_COLUMNS} FROM viz_saved_charts WHERE id = 'chart'"),
        [],
        |row| row.get(1),
    )?;
    assert_eq!(name, "chart");

    insert_as_old_build(&connection, "chart")?;

    assert_eq!(context_of(&connection, "chart")?, [None, None, None, None]);
    let targets: i64 = connection.query_row(
        "SELECT COUNT(*) FROM viz_saved_chart_context_targets WHERE chart_id = 'chart'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        targets, 0,
        "no targets stay behind for a row that lost its context"
    );

    Ok(())
}
