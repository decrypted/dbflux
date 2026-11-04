//! The 12-column grid, and moving a dashboard stored for a narrower one onto it.

use dbflux_storage::error::StorageError;
use dbflux_storage::repositories::viz_dashboard_panels::DashboardPanelDto;
use uuid::Uuid;

use super::{DashboardManager, DashboardPanel, panel_to_dto};

/// Grid column count for every dashboard.
pub const DASHBOARD_GRID_COLUMNS: u32 = 12;

/// Rescale a panel written against a `K`-column grid onto the canonical 12-column
/// grid. Returns the new `(grid_column, grid_width)`, which stays on the grid. `K = 0`
/// is treated as `K = 1` to avoid division-by-zero.
pub fn rescale_panel_to_12_cols(
    grid_column: u32,
    grid_width: u32,
    legacy_grid_columns: u32,
) -> (u32, u32) {
    let k = legacy_grid_columns.max(1);

    if k >= DASHBOARD_GRID_COLUMNS {
        return (grid_column.min(11), grid_width.clamp(1, 12));
    }

    let factor = DASHBOARD_GRID_COLUMNS / k;
    let new_column = grid_column.saturating_mul(factor).min(11);
    let new_width = grid_width
        .saturating_mul(factor)
        .clamp(1, DASHBOARD_GRID_COLUMNS - new_column);
    (new_column, new_width)
}

/// Rows of a `K`-column dashboard as they should be on the 12-column grid. A row wider
/// than `K` or starting past it can't have been placed on that grid: earlier versions
/// rescaled on every open, so the dashboard has been rescaled already and all of its rows
/// stay as they are. A row that only overruns the edge can come from `K` columns, where
/// a resize limits the width alone, so it scales. A layout rescaled once that still fits
/// inside `K`, possible at 4 or 6 columns, can't be told apart and scales once more:
/// that stays on the grid without overlaps, unlike leaving a real `K`-column layout narrow.
fn rows_on_12_columns(panels: &[DashboardPanel], legacy_grid_columns: u32) -> Vec<DashboardPanel> {
    let k = legacy_grid_columns.max(1);
    let already_rescaled = panels
        .iter()
        .any(|p| p.grid_width > k || p.grid_column >= k);

    panels
        .iter()
        .cloned()
        .map(|mut panel| {
            if !already_rescaled {
                (panel.grid_column, panel.grid_width) = rescale_panel_to_12_cols(
                    panel.grid_column,
                    panel.grid_width,
                    legacy_grid_columns,
                );
            }
            panel
        })
        .collect()
}

impl DashboardManager {
    /// Moves a dashboard stored for a grid under 12 columns onto the 12-column grid, once:
    /// its rows, hidden ones too, and its `grid_columns` are written in one transaction.
    /// Nothing to do for any other dashboard.
    ///
    /// When the write fails, storage stays as it was and the cache holds the rows on 12
    /// columns, so what draws and what reconciles against it agree. The dashboard's panels
    /// can't be written for this session: a move would store one row for the new grid
    /// under the old `grid_columns`, and the next open would rescale it again. The cached
    /// `grid_columns` stays old for the same reason. Opening it again changes nothing.
    pub fn move_to_12_columns(&mut self, dashboard_id: Uuid) -> Result<(), StorageError> {
        if self.move_failed.contains(&dashboard_id) {
            return Ok(());
        }

        let Some(legacy) = self
            .dashboards
            .iter()
            .find(|d| d.id == dashboard_id)
            .map(|d| d.grid_columns)
            .filter(|columns| *columns < DASHBOARD_GRID_COLUMNS)
        else {
            return Ok(());
        };

        let panels = rows_on_12_columns(self.panels_for_dashboard(dashboard_id), legacy);

        let result = self.ensure_panels_writable(dashboard_id).and_then(|()| {
            let dtos: Vec<DashboardPanelDto> = panels.iter().map(panel_to_dto).collect();
            self.panels_repo.replace_panels_and_grid_columns(
                dashboard_id,
                &dtos,
                i64::from(DASHBOARD_GRID_COLUMNS),
            )
        });

        self.panels.insert(dashboard_id, panels);

        match &result {
            Ok(()) => {
                if let Some(dashboard) = self.dashboards.iter_mut().find(|d| d.id == dashboard_id) {
                    dashboard.grid_columns = DASHBOARD_GRID_COLUMNS;
                }
            }
            Err(_) => {
                self.move_failed.insert(dashboard_id);
            }
        }

        result
    }
}
