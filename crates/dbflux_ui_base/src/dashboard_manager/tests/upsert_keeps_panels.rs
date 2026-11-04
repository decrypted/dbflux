//! Every write of a dashboard row keeps its stored panels. The rows are read back through
//! a fresh manager, as the next start reads them, because the cache keeps drawing panels
//! that are gone from storage.

use super::*;

const LAYOUT: &[(u32, u32, u32, u32)] = &[(0, 0, 6, 4), (0, 6, 6, 4), (4, 0, 12, 2)];

fn dashboard_with_panels() -> (DashboardManager, StorageRuntime, Uuid) {
    let (mut mgr, rt) = make_manager_with_rt();
    let profile_id = insert_profile(&rt);
    let dash_id = mgr
        .create_dashboard(
            "D".to_string(),
            None,
            profile_id,
            None,
            SavedChartRefreshPolicy::Off,
        )
        .unwrap();
    let drafts = LAYOUT
        .iter()
        .map(
            |&(grid_row, grid_column, grid_width, grid_height)| DashboardPanelDraft::Chart {
                saved_chart_id: Uuid::new_v4(),
                layout: Some(DraftGridLayout {
                    grid_row,
                    grid_column,
                    grid_width,
                    grid_height,
                }),
            },
        )
        .collect();
    mgr.append_panels(dash_id, drafts).unwrap();

    (mgr, rt, dash_id)
}

fn fresh_manager(rt: &StorageRuntime) -> DashboardManager {
    let conn = rt.viz_connection().expect("viz connection");
    DashboardManager::new(
        Arc::new(DashboardsRepository::new(Arc::clone(&conn))),
        Arc::new(DashboardPanelsRepository::new(conn)),
    )
}

fn stored_layout(rt: &StorageRuntime, dash_id: Uuid) -> Vec<(u32, u32, u32, u32)> {
    fresh_manager(rt)
        .panels_for_dashboard(dash_id)
        .iter()
        .map(|p| (p.grid_row, p.grid_column, p.grid_width, p.grid_height))
        .collect()
}

#[test]
fn renaming_keeps_the_panels() {
    let (mut mgr, rt, dash_id) = dashboard_with_panels();

    mgr.rename_dashboard(dash_id, "Renamed".to_string())
        .unwrap();

    assert_eq!(stored_layout(&rt, dash_id), LAYOUT);
    assert_eq!(
        fresh_manager(&rt).dashboard_by_id(dash_id).unwrap().name,
        "Renamed"
    );
}

#[test]
fn setting_the_shared_time_range_keeps_the_panels() {
    let (mut mgr, rt, dash_id) = dashboard_with_panels();

    mgr.update_shared_time_range(dash_id, Some(TimeRangePreset::Last6Hours))
        .unwrap();

    assert_eq!(stored_layout(&rt, dash_id), LAYOUT);
    assert_eq!(
        fresh_manager(&rt)
            .dashboard_by_id(dash_id)
            .unwrap()
            .shared_time_range_preset,
        Some(TimeRangePreset::Last6Hours)
    );
}

#[test]
fn setting_the_shared_refresh_policy_keeps_the_panels() {
    let (mut mgr, rt, dash_id) = dashboard_with_panels();
    let policy = SavedChartRefreshPolicy::Interval { every_secs: 30 };

    mgr.update_shared_refresh_policy(dash_id, policy).unwrap();

    assert_eq!(stored_layout(&rt, dash_id), LAYOUT);
    assert_eq!(
        fresh_manager(&rt)
            .dashboard_by_id(dash_id)
            .unwrap()
            .shared_refresh_policy,
        policy
    );
}

#[test]
fn upserting_an_existing_dashboard_keeps_the_panels() {
    let (mut mgr, rt, dash_id) = dashboard_with_panels();
    let mut dashboard = mgr.dashboard_by_id(dash_id).unwrap().clone();
    dashboard.description = Some("described".to_string());

    assert!(mgr.upsert_dashboard(dashboard).unwrap());

    assert_eq!(stored_layout(&rt, dash_id), LAYOUT);
    assert_eq!(
        fresh_manager(&rt)
            .dashboard_by_id(dash_id)
            .unwrap()
            .description
            .as_deref(),
        Some("described")
    );
}

#[test]
fn creating_and_duplicating_keep_the_panels_of_the_source() {
    let (mut mgr, rt, dash_id) = dashboard_with_panels();
    let profile_id = insert_profile(&rt);

    let created = mgr
        .create_dashboard(
            "Other".to_string(),
            None,
            profile_id,
            None,
            SavedChartRefreshPolicy::Off,
        )
        .unwrap();
    let copy = mgr.duplicate_dashboard(dash_id).unwrap();

    assert_eq!(stored_layout(&rt, dash_id), LAYOUT);
    assert_eq!(stored_layout(&rt, copy), LAYOUT);
    assert!(stored_layout(&rt, created).is_empty());
}
