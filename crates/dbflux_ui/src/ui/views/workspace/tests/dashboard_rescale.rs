//! A dashboard stored for a narrower grid, through what a user does: open it, close the
//! tab and open it again, with what was stored read back as the next start would.

use crate::ui::document::DocumentKey;
use crate::ui::views::workspace::Workspace;
use dbflux_components::SavedChartRefreshPolicy;
use dbflux_core::{ConnectionProfile, DbConfig};
use dbflux_storage::repositories::viz_dashboard_panels::DashboardPanelsRepository;
use dbflux_storage::repositories::viz_dashboards::DashboardsRepository;
use dbflux_ui_base::{AppStateEntity, DashboardManager, DashboardPanelDraft, DraftGridLayout};
use gpui::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext};
use std::sync::Arc;
use uuid::Uuid;

use super::new_workspace;

/// `(grid_row, grid_column, grid_width, grid_height)`
type Rect = (u32, u32, u32, u32);

fn layout(rect: Rect) -> Option<DraftGridLayout> {
    Some(DraftGridLayout {
        grid_row: rect.0,
        grid_column: rect.1,
        grid_width: rect.2,
        grid_height: rect.3,
    })
}

/// Stores `rows` under `grid_columns`, and reads storage back as the app does at start.
fn stored_dashboard(
    app_state: &Entity<AppStateEntity>,
    grid_columns: u32,
    rows: &[Rect],
    cx: &mut VisualTestContext,
) -> Uuid {
    let drafts: Vec<DashboardPanelDraft> = rows
        .iter()
        .map(|rect| DashboardPanelDraft::Chart {
            saved_chart_id: Uuid::new_v4(),
            layout: layout(*rect),
        })
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

        let conn = state
            .storage_runtime()
            .viz_connection()
            .expect("viz connection");
        conn.lock()
            .expect("connection")
            .execute_batch(&format!(
                "UPDATE viz_dashboards SET grid_columns = {grid_columns} WHERE id = '{id}'"
            ))
            .expect("older grid stored");
        state.dashboards = read_storage(state);
        id
    })
}

fn read_storage(state: &AppStateEntity) -> DashboardManager {
    let conn = state
        .storage_runtime()
        .viz_connection()
        .expect("viz connection");
    DashboardManager::new(
        Arc::new(DashboardsRepository::new(Arc::clone(&conn))),
        Arc::new(DashboardPanelsRepository::new(conn)),
    )
}

/// What storage holds, read fresh: the rows and `grid_columns`.
fn stored(
    app_state: &Entity<AppStateEntity>,
    dashboard_id: Uuid,
    cx: &mut VisualTestContext,
) -> (Vec<Rect>, u32) {
    app_state.update(cx, |state, _| {
        let manager = read_storage(state);
        let rows = manager
            .panels_for_dashboard(dashboard_id)
            .iter()
            .map(|p| (p.grid_row, p.grid_column, p.grid_width, p.grid_height))
            .collect();
        let columns = manager
            .dashboard_by_id(dashboard_id)
            .expect("the dashboard")
            .grid_columns;
        (rows, columns)
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

/// Opening three times draws the same, stores the same, and the stored rows are what
/// each grid's panels should be on 12 columns.
#[gpui::test]
fn an_older_dashboard_draws_the_same_on_every_open(cx: &mut TestAppContext) {
    let (workspace, app_state, window) = new_workspace(cx);

    for (grid_columns, rows, expected) in [
        (
            2,
            vec![(0, 0, 1, 4), (0, 1, 1, 4), (4, 0, 2, 2)],
            vec![(0, 0, 6, 4), (0, 6, 6, 4), (4, 0, 12, 2)],
        ),
        (
            3,
            vec![(0, 0, 1, 4), (0, 1, 2, 4)],
            vec![(0, 0, 4, 4), (0, 4, 8, 4)],
        ),
        (
            4,
            vec![(0, 0, 1, 4), (0, 3, 1, 4)],
            vec![(0, 0, 3, 4), (0, 9, 3, 4)],
        ),
        (
            6,
            vec![(0, 0, 3, 4), (0, 3, 3, 4)],
            vec![(0, 0, 6, 4), (0, 6, 6, 4)],
        ),
    ] {
        let dashboard_id = stored_dashboard(&app_state, grid_columns, &rows, window);

        open(&workspace, dashboard_id, window);
        let first = shown_slots(window);
        assert_eq!(first.len(), rows.len());
        assert_eq!(
            stored(&app_state, dashboard_id, window),
            (expected.clone(), 12),
            "{grid_columns} columns, stored after the first open"
        );

        for again in 0..2 {
            close(&workspace, dashboard_id, window);
            open(&workspace, dashboard_id, window);
            assert_eq!(
                shown_slots(window),
                first,
                "{grid_columns} columns, drawn on reopen {again}"
            );
            assert_eq!(
                stored(&app_state, dashboard_id, window),
                (expected.clone(), 12),
                "{grid_columns} columns, stored on reopen {again}"
            );
        }

        close(&workspace, dashboard_id, window);
    }
}

/// With the write refused the dashboard still draws on 12 columns, the same each time,
/// and storage stays as it was.
#[gpui::test]
fn an_older_dashboard_that_cannot_be_written_draws_the_same_and_stores_nothing(
    cx: &mut TestAppContext,
) {
    let (workspace, app_state, window) = new_workspace(cx);
    let rows = vec![(0, 0, 1, 4), (0, 1, 1, 4)];
    let dashboard_id = stored_dashboard(&app_state, 2, &rows, window);

    app_state.update(window, |state, _| {
        let conn = state
            .storage_runtime()
            .viz_connection()
            .expect("viz connection");
        conn.lock()
            .expect("connection")
            .execute_batch(
                "CREATE TRIGGER refuse_grid BEFORE UPDATE OF grid_columns ON viz_dashboards
                 BEGIN SELECT RAISE(ABORT, 'refused'); END;",
            )
            .expect("trigger");
    });

    open(&workspace, dashboard_id, window);
    let first = shown_slots(window);
    assert_eq!(first.len(), 2);
    assert_eq!(
        first[1].origin.x,
        first[0].origin.x + first[0].size.width,
        "side by side, half the grid each"
    );

    for _ in 0..2 {
        close(&workspace, dashboard_id, window);
        open(&workspace, dashboard_id, window);
        assert_eq!(shown_slots(window), first);
        assert_eq!(stored(&app_state, dashboard_id, window), (rows.clone(), 2));
    }
}
