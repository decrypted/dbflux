//! Save and rename keep what hangs off a chart, and the panels that show it.

use super::*;

/// Save and rename build the row from the cached chart, so what a chart can hold
/// has to survive that trip, for every kind of source. Each table has to hold rows
/// for the check to mean anything.
#[test]
fn test_save_and_rename_keep_every_child_table_of_each_source() {
    const TABLES: [&str; 6] = [
        "viz_saved_chart_series",
        "viz_saved_chart_binding_y",
        "viz_saved_chart_source_metric_series",
        "viz_saved_chart_source_metric_dimensions",
        "viz_saved_chart_source_instance_metric",
        "viz_saved_chart_context_targets",
    ];

    let rt = temp_storage();
    let conn = rt
        .viz_connection()
        .expect("viz connection should open in test");
    let profile_id = Uuid::new_v4();
    insert_test_profile(&conn, profile_id);
    let repo = Arc::new(SavedChartsRepository::new(Arc::clone(&conn)));

    let counts = |chart_id: Uuid| -> [i64; 6] {
        let locked = conn.lock().expect("lock connection");
        TABLES.map(|table| {
            locked
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE chart_id = ?1"),
                    [chart_id.to_string()],
                    |row| row.get(0),
                )
                .expect("count")
        })
    };

    let mut instance = sample_chart("instance", profile_id);
    instance.source = SavedChartSource::InstanceMetric {
        metric_id: "pg.cache_hit_ratio".to_string(),
    };
    let cases = [
        (sample_chart("query", profile_id), [1, 1, 0, 0, 0, 2]),
        (
            sample_metric_chart("metric", profile_id),
            [0, 0, 1, 2, 0, 2],
        ),
        (instance, [1, 1, 0, 0, 1, 2]),
    ];

    let mut manager = SavedChartManager::new(Arc::clone(&repo));
    for (mut chart, expected) in cases {
        let label = chart.name.clone();
        let id = chart.id;
        chart.context = stored_context();
        manager.upsert(chart.clone()).expect("first save");
        assert_eq!(counts(id), expected, "{label}: first save");

        let source = format!("{:?}", chart.source);
        let chart_spec = format!("{:?}", chart.chart_spec);

        manager.upsert(chart).expect("save again");
        assert_eq!(counts(id), expected, "{label}: save again");

        manager
            .rename_chart(id, format!("{label} renamed"))
            .expect("rename");
        assert_eq!(counts(id), expected, "{label}: rename");

        let fresh = SavedChartManager::new(Arc::clone(&repo));
        let stored = fresh.chart_by_id(id).expect("stored");
        assert_eq!(stored.name, format!("{label} renamed"));
        assert_eq!(format!("{:?}", stored.source), source, "{label}: source");
        assert_eq!(
            format!("{:?}", stored.chart_spec),
            chart_spec,
            "{label}: spec"
        );
        assert_eq!(stored.context, stored_context(), "{label}: context");
    }
}

/// Panels point at a chart without a foreign key, so a chart's write can't reach them.
#[test]
fn test_save_and_rename_keep_the_panels_that_show_the_chart() {
    use crate::dashboard_manager::{DashboardManager, DashboardPanelDraft};
    use dbflux_storage::repositories::viz_dashboard_panels::DashboardPanelsRepository;
    use dbflux_storage::repositories::viz_dashboards::DashboardsRepository;

    let (rt, repo, profile_id) = manager_for_profile();
    let conn = rt.viz_connection().expect("viz connection");
    let dashboards = || {
        DashboardManager::new(
            Arc::new(DashboardsRepository::new(Arc::clone(&conn))),
            Arc::new(DashboardPanelsRepository::new(Arc::clone(&conn))),
        )
    };

    let mut charts = SavedChartManager::new(repo);
    let chart = sample_chart("shown", profile_id);
    let id = chart.id;
    charts.upsert(chart.clone()).expect("first save");

    let mut manager = dashboards();
    let dashboard_id = manager
        .create_dashboard(
            "D".to_string(),
            None,
            profile_id,
            None,
            SavedChartRefreshPolicy::Off,
        )
        .expect("dashboard");
    manager
        .append_panels(
            dashboard_id,
            vec![DashboardPanelDraft::Chart {
                saved_chart_id: id,
                layout: None,
            }],
        )
        .expect("panel");

    charts.upsert(chart).expect("save again");
    charts
        .rename_chart(id, "renamed".to_string())
        .expect("rename");

    assert_eq!(dashboards().panels_for_dashboard(dashboard_id).len(), 1);
}
