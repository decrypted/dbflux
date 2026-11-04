//! A dashboard's layout through what a user does: open it, remove a panel by clicking it
//! and pressing `x`, close the tab and open it again. The panels stay where they were.

use crate::ui::document::DocumentKey;
use crate::ui::views::workspace::Workspace;
use dbflux_components::SavedChartRefreshPolicy;
use dbflux_core::{ConnectionProfile, DbConfig};
use dbflux_ui_base::{AppStateEntity, DashboardPanelDraft, DraftGridLayout};
use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};
use uuid::Uuid;

use super::new_workspace;

/// `(grid_row, grid_column, grid_width, grid_height)`
type Rect = (u32, u32, u32, u32);

const FULL_WIDTH: &[Rect] = &[(0, 0, 12, 4), (4, 0, 12, 4), (8, 0, 12, 4)];
const MIXED_WIDTH: &[Rect] = &[(0, 0, 6, 4), (0, 6, 6, 4), (4, 0, 4, 2), (4, 4, 8, 2)];
const MULTI_ROW: &[Rect] = &[(0, 0, 12, 2), (2, 0, 6, 6), (2, 6, 6, 3), (5, 6, 6, 3)];

fn stored_dashboard(
    app_state: &Entity<AppStateEntity>,
    layout: &[Rect],
    cx: &mut VisualTestContext,
) -> Uuid {
    let drafts: Vec<DashboardPanelDraft> = layout
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

    app_state.update(cx, |state, _| {
        let profile = ConnectionProfile::new(
            "test",
            DbConfig::SQLite {
                path: std::path::PathBuf::from(":memory:"),
                connection_id: None,
            },
        );
        let profile_id = profile.id;
        state.add_profile_in_folder(profile, None);

        let id = state
            .dashboards
            .create_dashboard(
                "Layout".to_string(),
                None,
                profile_id,
                None,
                SavedChartRefreshPolicy::Off,
            )
            .expect("dashboard created");
        state
            .dashboards
            .append_panels(id, drafts)
            .expect("panels stored");
        id
    })
}

fn open(workspace: &Entity<Workspace>, dashboard_id: Uuid, window: &mut VisualTestContext) {
    let handle = workspace.clone();
    window.update(|window, cx| {
        handle.update(cx, |workspace, cx| {
            workspace.open_dashboard(dashboard_id, window, cx);
        });
    });
    window.run_until_parked();
}

fn close(workspace: &Entity<Workspace>, dashboard_id: Uuid, window: &mut VisualTestContext) {
    let handle = workspace.clone();
    window.update(|window, cx| {
        handle.update(cx, |workspace, cx| {
            let id = workspace
                .tab_manager
                .read(cx)
                .find_by_key(&DocumentKey::Dashboard { dashboard_id }, cx)
                .expect("the dashboard's tab");
            workspace.close_tab(id, window, cx);
        });
    });
    window.run_until_parked();
}

fn shown_slots(window: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
    window.update(|window, _| window.refresh());
    window.run_until_parked();

    (0..)
        .map_while(|index| window.debug_bounds(format!("panel-slot-{index}").leak()))
        .collect()
}

#[gpui::test]
fn removing_a_panel_and_reopening_moves_nothing(cx: &mut TestAppContext) {
    let (workspace, app_state, window) = new_workspace(cx);

    for layout in [FULL_WIDTH, MIXED_WIDTH, MULTI_ROW] {
        for removed in [0, layout.len() / 2, layout.len() - 1] {
            let at = format!("{layout:?} without {removed}");
            let dashboard_id = stored_dashboard(&app_state, layout, window);

            open(&workspace, dashboard_id, window);
            let before = shown_slots(window);
            assert_eq!(before.len(), layout.len(), "{at}");

            // `alt-l` switches the dashboard to Edit mode, where a click selects a panel.
            window.simulate_keystrokes("alt-l");
            window.run_until_parked();
            let chosen = before[removed];
            window.simulate_click(chosen.center(), Modifiers::default());
            window.simulate_keystrokes("x");
            window.run_until_parked();

            let mut expected = before.clone();
            expected.remove(removed);
            assert_eq!(shown_slots(window), expected, "on screen, {at}");

            close(&workspace, dashboard_id, window);
            open(&workspace, dashboard_id, window);
            assert_eq!(shown_slots(window), expected, "after reopening, {at}");

            close(&workspace, dashboard_id, window);
        }
    }
}
