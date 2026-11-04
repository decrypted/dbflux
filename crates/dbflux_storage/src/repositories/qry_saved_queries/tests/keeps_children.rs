//! A save keeps a query's children, its creation time, and any other query.

use super::*;

fn rows_of(conn: &Arc<Mutex<Connection>>, table: &str, id: &str) -> Vec<String> {
    let conn = conn.lock().expect("lock connection");
    let mut statement = conn
        .prepare(&format!(
            "SELECT * FROM {table} WHERE saved_query_id = ?1 ORDER BY 2"
        ))
        .expect("prepare");
    let columns = statement.column_count();
    statement
        .query_map([id], |row| {
            let values: Vec<String> = (0..columns)
                .map(|index| format!("{:?}", row.get_ref(index)))
                .collect::<Vec<_>>();
            Ok(values.join("|"))
        })
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn spec_with_every_child() -> VisualQuerySpec {
    let mut spec = base_spec();
    spec.projection = Projection::Explicit(vec![ProjectedColumn {
        source_alias: "users".to_string(),
        column: "email".to_string(),
        alias: Some("user_email".to_string()),
    }]);
    spec.joins = vec![JoinStep {
        kind: JoinKind::Inner,
        from_alias: "users".to_string(),
        to_schema: Some("public".to_string()),
        to_table: "orders".to_string(),
        to_alias: "orders".to_string(),
        on: JoinOn::FkPath {
            from_column: "id".to_string(),
            to_column: "user_id".to_string(),
        },
    }];
    spec.sort = vec![SortEntry {
        source_alias: "users".to_string(),
        column: "email".to_string(),
        direction: VisualSortDirection::Desc,
    }];
    spec
}

/// The tables are read from the schema, so a new one that cascades from a saved
/// query fails here until a save writes it.
#[test]
fn saving_again_keeps_every_child_table() {
    let (conn, repo, profile_id) = setup("keeps_children");
    let spec = spec_with_every_child();
    let summary = repo
        .upsert_by_name(&profile_id, "Q", &spec)
        .expect("first save");

    let tables: Vec<String> = {
        let locked = conn.lock().expect("lock connection");
        let mut statement = locked
            .prepare(
                "SELECT m.name FROM sqlite_master m, pragma_foreign_key_list(m.name) f
                 WHERE m.type = 'table' AND f.\"table\" = 'qry_saved_queries'
                   AND f.on_delete = 'CASCADE'",
            )
            .expect("prepare");
        statement
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("tables")
    };
    assert_eq!(tables.len(), 3, "{tables:?}");

    let before: Vec<Vec<String>> = tables
        .iter()
        .map(|table| rows_of(&conn, table, &summary.id))
        .collect();
    for (table, rows) in tables.iter().zip(&before) {
        assert!(!rows.is_empty(), "{table} holds nothing to keep");
    }

    repo.upsert_by_name(&profile_id, "Q", &spec)
        .expect("save by name");
    let after_by_name: Vec<Vec<String>> = tables
        .iter()
        .map(|table| rows_of(&conn, table, &summary.id))
        .collect();
    assert_eq!(after_by_name, before, "save by name");

    repo.upsert_by_id(&summary.id, &profile_id, "Q", &spec)
        .expect("save by id");
    let after_by_id: Vec<Vec<String>> = tables
        .iter()
        .map(|table| rows_of(&conn, table, &summary.id))
        .collect();
    assert_eq!(after_by_id, before, "save by id");
}

#[test]
fn saving_again_keeps_the_created_time() {
    let (conn, repo, profile_id) = setup("keeps_created_at");
    let summary = repo
        .upsert_by_name(&profile_id, "Q", &base_spec())
        .expect("first save");
    conn.lock()
        .expect("lock connection")
        .execute(
            "UPDATE qry_saved_queries SET created_at = 7 WHERE id = ?1",
            [&summary.id],
        )
        .expect("set created_at");

    repo.upsert_by_id(&summary.id, &profile_id, "Q", &base_spec())
        .expect("save");

    let created_at: i64 = conn
        .lock()
        .expect("lock connection")
        .query_row(
            "SELECT created_at FROM qry_saved_queries WHERE id = ?1",
            [&summary.id],
            |row| row.get(0),
        )
        .expect("created_at");
    assert_eq!(created_at, 7);
}

/// Writing a name another query holds used to delete that query and its children.
#[test]
fn a_name_held_by_another_query_is_refused_and_that_query_stays() {
    let (conn, repo, profile_id) = setup("name_conflict");
    let held = repo
        .upsert_by_name(&profile_id, "Held", &spec_with_every_child())
        .expect("first query");
    let columns = rows_of(&conn, "qry_saved_query_columns", &held.id);
    assert!(!columns.is_empty());

    let other = Uuid::now_v7().to_string();
    let result = repo.upsert_by_id(&other, &profile_id, "Held", &base_spec());

    assert!(result.is_err(), "the name is taken");
    let kept = repo.get(&held.id).expect("get").expect("still stored");
    assert_eq!(kept, spec_with_every_child());
    assert_eq!(rows_of(&conn, "qry_saved_query_columns", &held.id), columns);
    assert!(repo.get(&other).expect("get").is_none());
}
