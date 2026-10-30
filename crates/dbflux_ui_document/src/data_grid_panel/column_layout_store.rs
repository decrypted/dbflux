//! Keeps the column layout of a table (order and hidden columns) across
//! reopening it and restarts.
//!
//! The layout is a view setting, so it lives in the UI state store keyed by
//! the table it belongs to, never in the query: the grid still reads every
//! column, and editing, copy and export still see them all.

use super::{DataGridPanel, DataSource};
use dbflux_components::components::data_table::ColumnLayoutSpec;
use gpui::{App, Context};

/// Prefix of the `st_ui_state` keys. The version lets a later layout format
/// use new keys instead of misreading old rows.
const KEY_PREFIX: &str = dbflux_storage::ui_state_keys::GRID_COLUMN_LAYOUT_PREFIX;

impl DataGridPanel {
    /// The store key of this grid's layout: profile, database, schema and
    /// table. `None` for sources that are not a table, whose layout lasts
    /// as long as the grid.
    pub(super) fn column_layout_key(&self) -> Option<String> {
        let DataSource::Table {
            profile_id,
            database,
            table,
            ..
        } = &self.source
        else {
            return None;
        };

        // A JSON array keeps names holding the separator apart.
        let parts = [
            profile_id.to_string(),
            database.clone().unwrap_or_default(),
            table.schema.clone().unwrap_or_default(),
            table.name.clone(),
        ];
        let parts = serde_json::to_string(&parts).ok()?;

        Some(format!("{KEY_PREFIX}:{parts}"))
    }

    /// Whether this grid offers hiding and moving columns. A
    /// document grid groups its columns by the nested field they come from,
    /// which a reorder would split, so it keeps the model order.
    pub(super) fn offers_column_layout(&self, cx: &App) -> bool {
        self.grid_table
            .table_state
            .as_ref()
            .is_some_and(|state| state.read(cx).document_presentation().is_none())
    }

    /// Lays the grid out as the store says, for a table that has a kept
    /// layout.
    pub(super) fn restore_column_layout(&mut self, cx: &mut Context<Self>) {
        if !self.offers_column_layout(cx) {
            return;
        }
        let (Some(key), Some(table_state)) = (
            self.column_layout_key(),
            self.grid_table.table_state.clone(),
        ) else {
            return;
        };

        if let Some(spec) = self.read_column_layout(&key, cx) {
            table_state.update(cx, |state, cx| state.set_column_layout_spec(&spec, cx));
        }
    }

    /// The layout the store keeps under `key`, `None` when it keeps none or
    /// cannot be read.
    fn read_column_layout(&self, key: &str, cx: &App) -> Option<ColumnLayoutSpec> {
        let stored = match self
            .app_state
            .read(cx)
            .storage_runtime()
            .ui_state()
            .get(key)
        {
            Ok(stored) => stored?,
            Err(error) => {
                log::warn!("Could not read the column layout of {key}: {error}");
                return None;
            }
        };

        match serde_json::from_str::<ColumnLayoutSpec>(&stored) {
            Ok(spec) => Some(spec),
            Err(error) => {
                log::warn!("Ignoring the unreadable column layout of {key}: {error}");
                None
            }
        }
    }

    /// Writes the grid's layout to the store after the user changed it. A
    /// layout back to the table's own order with every column shown removes
    /// the row, so the store only holds tables someone rearranged.
    ///
    /// While a column projection reads only some of the table's columns, the
    /// grid's layout covers just those, so it is laid over the kept one:
    /// the columns left out keep their place and visibility, and the row is
    /// never removed, since the grid cannot tell whether the whole table is
    /// back in its own order.
    pub(super) fn save_column_layout(&self, cx: &App) {
        let (Some(key), Some(table_state)) = (
            self.column_layout_key(),
            self.grid_table.table_state.as_ref(),
        ) else {
            return;
        };

        let state = table_state.read(cx);
        let identity = state.column_layout().is_identity();
        let spec = state.column_layout_spec();
        let spec = if self.table_projection_columns().is_empty() {
            (!identity).then_some(spec)
        } else {
            match self.read_column_layout(&key, cx) {
                Some(stored) => Some(spec.merged_over(&stored)),
                None => (!identity).then_some(spec),
            }
        };

        let store = self.app_state.read(cx).storage_runtime().ui_state();
        let result = match spec {
            None => store.delete(&key),
            Some(spec) => match serde_json::to_string(&spec) {
                Ok(json) => store.set(&key, &json),
                Err(error) => {
                    log::error!("Could not encode the column layout of {key}: {error}");
                    return;
                }
            },
        };

        if let Err(error) = result {
            log::error!("Could not save the column layout of {key}: {error}");
        }
    }
}
