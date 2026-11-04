//! Removing a panel leaves the others where they are stored, on any layout. The stored
//! rows are read back through a fresh manager, as the next start reads them.

use super::*;

/// `(grid_row, grid_column, grid_width, grid_height)`
type Rect = (u32, u32, u32, u32);

const FULL_WIDTH: &[Rect] = &[(0, 0, 12, 4), (4, 0, 12, 4), (8, 0, 12, 4)];
const MIXED_WIDTH: &[Rect] = &[(0, 0, 6, 4), (0, 6, 6, 4), (4, 0, 4, 2), (4, 4, 8, 2)];
const MULTI_ROW: &[Rect] = &[(0, 0, 12, 2), (2, 0, 6, 6), (2, 6, 6, 3), (5, 6, 6, 3)];

fn dashboard_of(layout: &[Rect]) -> (DashboardManager, StorageRuntime, Uuid) {
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
    let drafts = layout
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

fn rects(mgr: &DashboardManager, dash_id: Uuid) -> Vec<Rect> {
    mgr.panels_for_dashboard(dash_id)
        .iter()
        .map(|p| (p.grid_row, p.grid_column, p.grid_width, p.grid_height))
        .collect()
}

fn stored_rects(rt: &StorageRuntime, dash_id: Uuid) -> Vec<Rect> {
    let conn = rt.viz_connection().expect("viz connection");
    let manager = DashboardManager::new(
        Arc::new(DashboardsRepository::new(Arc::clone(&conn))),
        Arc::new(DashboardPanelsRepository::new(conn)),
    );

    rects(&manager, dash_id)
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.3 && b.0 < a.0 + a.3 && a.1 < b.1 + b.2 && b.1 < a.1 + a.2
}

#[test]
fn removing_a_panel_leaves_the_others_where_they_are() {
    for layout in [FULL_WIDTH, MIXED_WIDTH, MULTI_ROW] {
        for removed in [0, layout.len() / 2, layout.len() - 1] {
            let (mut mgr, rt, dash_id) = dashboard_of(layout);
            let expected: Vec<Rect> = layout
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != removed)
                .map(|(_, rect)| *rect)
                .collect();

            mgr.remove_panel(dash_id, removed as u32).unwrap();

            let at = format!("{layout:?} without {removed}");
            assert_eq!(rects(&mgr, dash_id), expected, "cached, {at}");
            assert_eq!(stored_rects(&rt, dash_id), expected, "stored, {at}");

            let indices: Vec<u32> = mgr
                .panels_for_dashboard(dash_id)
                .iter()
                .map(|p| p.panel_index)
                .collect();
            let dense: Vec<u32> = (0..expected.len() as u32).collect();
            assert_eq!(indices, dense, "panel_index stays dense, {at}");
        }
    }
}

/// A panel added after a removal lands past everything left, the gap or not.
#[test]
fn a_panel_added_after_a_removal_lands_clear_of_the_rest() {
    for layout in [FULL_WIDTH, MIXED_WIDTH, MULTI_ROW] {
        for removed in [0, layout.len() / 2, layout.len() - 1] {
            let (mut mgr, _rt, dash_id) = dashboard_of(layout);
            mgr.remove_panel(dash_id, removed as u32).unwrap();
            let left = rects(&mgr, dash_id);

            mgr.append_panels(
                dash_id,
                vec![DashboardPanelDraft::Chart {
                    saved_chart_id: Uuid::new_v4(),
                    layout: None,
                }],
            )
            .unwrap();

            let all = rects(&mgr, dash_id);
            let added = *all.last().unwrap();
            assert!(
                left.iter().all(|rect| !overlap(*rect, added)),
                "{added:?} on {left:?}"
            );
        }
    }
}
