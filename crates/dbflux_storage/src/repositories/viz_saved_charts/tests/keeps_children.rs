//! A save writes back everything that cascades from a chart.

use super::*;

fn tables_cascading_from(conn: &Connection, parent: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut parents = vec![parent.to_string()];
    while let Some(parent) = parents.pop() {
        let mut statement = conn
            .prepare(
                "SELECT DISTINCT m.name FROM sqlite_master m, pragma_foreign_key_list(m.name) f
                 WHERE m.type = 'table' AND f.\"table\" = ?1 AND f.on_delete = 'CASCADE'",
            )
            .expect("prepare");
        let children: Vec<String> = statement
            .query_map([&parent], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("tables");
        for child in children {
            if !found.contains(&child) {
                found.push(child.clone());
                parents.push(child);
            }
        }
    }
    found.sort();
    found
}

fn rows_of(conn: &Connection, table: &str, chart_id: &str) -> Vec<String> {
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {table} WHERE chart_id = ?1"))
        .expect("prepare");
    let columns = statement.column_count();
    let mut rows: Vec<String> = statement
        .query_map([chart_id], |row| {
            let values: Vec<String> = (0..columns)
                .map(|index| format!("{:?}", row.get_ref(index)))
                .collect::<Vec<_>>();
            Ok(values.join("|"))
        })
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    rows.sort();
    rows
}

/// The tables are read from the schema, so a new one that cascades from a chart
/// fails here until `upsert` writes it.
#[test]
fn test_saving_a_chart_again_keeps_every_child_table() {
    let (conn, repo, profile_id) = setup("keeps_children");
    let id = Uuid::new_v4();
    let chart_id = id.to_string();

    let mut dto = make_chart(id, profile_id, "query");
    dto.series = vec![SeriesDto {
        chart_id: chart_id.clone(),
        series_index: 0,
        column_index: 1,
        label: "value".to_string(),
        color_slot: 2,
    }];
    dto.binding_y = vec![BindingYDto {
        chart_id: chart_id.clone(),
        slot_index: 0,
        column_index: 1,
    }];
    dto.metric_series = vec![MetricSeriesDto {
        chart_id: chart_id.clone(),
        series_index: 0,
        namespace: "AWS/EC2".to_string(),
        metric_name: "CPUUtilization".to_string(),
        period_seconds: 300,
        statistic: "Average".to_string(),
        region: Some("us-east-1".to_string()),
        label: None,
    }];
    dto.metric_dimensions = vec![MetricDimensionDto {
        chart_id: chart_id.clone(),
        series_index: 0,
        dim_index: 0,
        dim_key: "InstanceId".to_string(),
        dim_value: "i-1".to_string(),
    }];
    dto.instance_metric_id = Some("pg.cache_hit_ratio".to_string());
    dto.context_targets = vec!["/aws/a".to_string(), "/aws/b".to_string()];
    repo.upsert(&dto).expect("first save");

    let locked = conn.lock().expect("lock connection");
    let tables = tables_cascading_from(&locked, "viz_saved_charts");
    assert_eq!(tables.len(), 6, "{tables:?}");
    let before: Vec<Vec<String>> = tables
        .iter()
        .map(|table| rows_of(&locked, table, &chart_id))
        .collect();
    drop(locked);
    for (table, rows) in tables.iter().zip(&before) {
        assert!(!rows.is_empty(), "{table} holds nothing to keep");
    }

    let stored = repo.get_full_chart(id).expect("get").expect("stored");
    repo.upsert(&stored).expect("save again");

    let locked = conn.lock().expect("lock connection");
    let after: Vec<Vec<String>> = tables
        .iter()
        .map(|table| rows_of(&locked, table, &chart_id))
        .collect();
    assert_eq!(after, before);
}
