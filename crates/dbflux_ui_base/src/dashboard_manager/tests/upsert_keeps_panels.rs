//! Every write of a dashboard row keeps its stored panels. The rows are read back through
//! a fresh manager, as the next start reads them, because the cache keeps drawing panels
//! that are gone from storage.

use super::*;

const LAYOUT: &[(u32, u32, u32, u32)] = &[(0, 0, 6, 4), (0, 6, 6, 4), (4, 0, 12, 2)];

fn dashboard_with_panels() -> (DashboardManager, StorageRuntime, Uuid) {
    let (mut manager, runtime) = make_manager_with_rt();
    let profile_id = insert_profile(&runtime);
    let dashboard_id = manager
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
    manager.append_panels(dashboard_id, drafts).unwrap();

    (manager, runtime, dashboard_id)
}

fn fresh_manager(runtime: &StorageRuntime) -> DashboardManager {
    let conn = runtime.viz_connection().expect("viz connection");
    DashboardManager::new(
        Arc::new(DashboardsRepository::new(Arc::clone(&conn))),
        Arc::new(DashboardPanelsRepository::new(conn)),
    )
}

fn stored_layout(runtime: &StorageRuntime, dashboard_id: Uuid) -> Vec<(u32, u32, u32, u32)> {
    fresh_manager(runtime)
        .panels_for_dashboard(dashboard_id)
        .iter()
        .map(|panel| {
            (
                panel.grid_row,
                panel.grid_column,
                panel.grid_width,
                panel.grid_height,
            )
        })
        .collect()
}

#[test]
fn renaming_keeps_the_panels() {
    let (mut manager, runtime, dashboard_id) = dashboard_with_panels();

    manager
        .rename_dashboard(dashboard_id, "Renamed".to_string())
        .unwrap();

    assert_eq!(stored_layout(&runtime, dashboard_id), LAYOUT);
    assert_eq!(
        fresh_manager(&runtime)
            .dashboard_by_id(dashboard_id)
            .unwrap()
            .name,
        "Renamed"
    );
}

#[test]
fn setting_the_shared_time_range_keeps_the_panels() {
    let (mut manager, runtime, dashboard_id) = dashboard_with_panels();

    manager
        .update_shared_time_range(dashboard_id, Some(TimeRangePreset::Last6Hours))
        .unwrap();

    assert_eq!(stored_layout(&runtime, dashboard_id), LAYOUT);
    assert_eq!(
        fresh_manager(&runtime)
            .dashboard_by_id(dashboard_id)
            .unwrap()
            .shared_time_range_preset,
        Some(TimeRangePreset::Last6Hours)
    );
}

#[test]
fn setting_the_shared_refresh_policy_keeps_the_panels() {
    let (mut manager, runtime, dashboard_id) = dashboard_with_panels();
    let policy = SavedChartRefreshPolicy::Interval { every_secs: 30 };

    manager
        .update_shared_refresh_policy(dashboard_id, policy)
        .unwrap();

    assert_eq!(stored_layout(&runtime, dashboard_id), LAYOUT);
    assert_eq!(
        fresh_manager(&runtime)
            .dashboard_by_id(dashboard_id)
            .unwrap()
            .shared_refresh_policy,
        policy
    );
}

#[test]
fn upserting_an_existing_dashboard_keeps_the_panels() {
    let (mut manager, runtime, dashboard_id) = dashboard_with_panels();
    let mut dashboard = manager.dashboard_by_id(dashboard_id).unwrap().clone();
    dashboard.description = Some("described".to_string());

    assert!(manager.upsert_dashboard(dashboard).unwrap());

    assert_eq!(stored_layout(&runtime, dashboard_id), LAYOUT);
    assert_eq!(
        fresh_manager(&runtime)
            .dashboard_by_id(dashboard_id)
            .unwrap()
            .description
            .as_deref(),
        Some("described")
    );
}

#[test]
fn creating_and_duplicating_keep_the_panels_of_the_source() {
    let (mut manager, runtime, dashboard_id) = dashboard_with_panels();
    let profile_id = insert_profile(&runtime);

    let created = manager
        .create_dashboard(
            "Other".to_string(),
            None,
            profile_id,
            None,
            SavedChartRefreshPolicy::Off,
        )
        .unwrap();
    let copy = manager.duplicate_dashboard(dashboard_id).unwrap();

    assert_eq!(stored_layout(&runtime, dashboard_id), LAYOUT);
    assert_eq!(stored_layout(&runtime, copy), LAYOUT);
    assert!(stored_layout(&runtime, created).is_empty());
}
