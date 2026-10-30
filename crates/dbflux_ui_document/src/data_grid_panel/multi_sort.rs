//! Sorting by several columns: Alt+click on a header's sort arrow and the
//! header menu's "Add to sort" add a key after the ones already there. A table
//! sends every key to the query's ORDER BY; other results sort in memory.

use super::{DataGridPanel, DataSource};
use dbflux_components::components::data_table::SortState as TableSortState;
use dbflux_core::{OrderByColumn, OrderByMode, SortDirection};
use gpui::{App, Context};
use uuid::Uuid;

impl DataGridPanel {
    /// The header keys for an ORDER BY, by the result columns it names.
    pub(super) fn sort_keys_for_order_by(&self, order_by: &[OrderByColumn]) -> Vec<TableSortState> {
        order_by
            .iter()
            .filter_map(|column| {
                self.result
                    .columns
                    .iter()
                    .position(|meta| meta.name == column.column.name)
                    .map(|column_ix| TableSortState::new(column_ix, column.direction))
            })
            .collect()
    }

    /// The keys of the in-memory sort, first to last.
    pub(super) fn local_sort_keys(&self) -> Vec<TableSortState> {
        let Some(first) = self.grid_table.local_sort_state else {
            return Vec::new();
        };

        std::iter::once(first)
            .chain(self.grid_table.local_sort_then.iter().copied())
            .map(|key| TableSortState::new(key.column_ix, key.direction))
            .collect()
    }

    /// The keys the displayed rows are sorted by.
    fn displayed_sort_keys(&self) -> Vec<TableSortState> {
        match &self.source {
            DataSource::Table { order_by, .. } => self.sort_keys_for_order_by(order_by),
            DataSource::Collection { .. } | DataSource::QueryResult { .. } => {
                self.local_sort_keys()
            }
        }
    }

    /// Puts the header arrows on `keys` without asking for a sort: the table
    /// moves them before the panel sorts, so a refused or adjusted request
    /// sets them to what the rows really follow.
    pub(super) fn show_sort_keys(&mut self, keys: &[TableSortState], cx: &mut Context<Self>) {
        let Some(table_state) = self.grid_table.table_state.clone() else {
            return;
        };

        table_state.update(cx, |state, cx| {
            state.set_sort_keys_without_emit(keys);
            cx.notify();
        });
    }

    /// `rebuild_table` takes the first key only; this adds the rest of an
    /// in-memory sort to the headers.
    pub(super) fn show_local_secondary_sort_keys(&mut self, cx: &mut Context<Self>) {
        if self.grid_table.local_sort_then.is_empty() {
            return;
        }

        let keys = self.local_sort_keys();
        self.show_sort_keys(&keys, cx);
    }

    /// `rebuild_table` takes the first key only; this adds the rest of the
    /// table's ORDER BY to the headers.
    pub(super) fn show_loaded_secondary_sort_keys(&mut self, cx: &mut Context<Self>) {
        let DataSource::Table { order_by, .. } = &self.source else {
            return;
        };

        if order_by.len() < 2 {
            return;
        }

        let keys = self.sort_keys_for_order_by(order_by);
        self.show_sort_keys(&keys, cx);
    }

    /// The ORDER BY for `keys` on a table. A key added to the table's default
    /// order replaces it: the default is the primary key, after which no other
    /// column changes the order, so keeping it would make the new key a no-op.
    /// Keys past what the driver can order by are dropped, and the headers
    /// then show the keys that were kept.
    pub(super) fn order_by_for_sort_keys(
        &mut self,
        keys: &[TableSortState],
        cx: &mut Context<Self>,
    ) -> Vec<OrderByColumn> {
        let requested: Vec<OrderByColumn> = keys
            .iter()
            .filter_map(|key| {
                self.result
                    .columns
                    .get(key.column_ix)
                    .map(|meta| OrderByColumn::from_name(&meta.name, key.direction))
            })
            .collect();

        let DataSource::Table {
            profile_id,
            database,
            table,
            order_by,
            ..
        } = &self.source
        else {
            return requested;
        };

        let default_order = Self::get_primary_key_columns(
            &self.app_state,
            *profile_id,
            database.as_deref(),
            table,
            cx,
        );
        let limit = self.order_by_column_limit(*profile_id, cx);
        let adjusted = truncate_order_by(
            drop_default_order_prefix(order_by, &default_order, requested),
            limit,
        );

        if adjusted.len() != keys.len() {
            let shown = self.sort_keys_for_order_by(&adjusted);
            self.show_sort_keys(&shown, cx);
        }

        adjusted
    }

    /// How many ORDER BY columns the connection's driver takes, `None` for
    /// no limit. A driver that orders only by its sort key takes one.
    fn order_by_column_limit(&self, profile_id: Uuid, cx: &App) -> Option<usize> {
        let state = self.app_state.read(cx);
        let connected = state.connections().get(&profile_id)?;
        let query = connected.connection.metadata().query.as_ref()?;

        match query.order_by_mode {
            OrderByMode::AnyColumns => {
                (query.max_order_by_columns > 0).then_some(query.max_order_by_columns as usize)
            }
            OrderByMode::SortKeyOnly | OrderByMode::None => Some(1),
        }
    }

    /// The header menu's "Add to sort": the column becomes the next key, or
    /// takes `direction` where it already is one.
    pub(super) fn handle_add_sort_key(
        &mut self,
        col_ix: usize,
        direction: SortDirection,
        cx: &mut Context<Self>,
    ) {
        let keys = with_sort_key(&self.displayed_sort_keys(), col_ix, direction);
        self.handle_sort_keys_request(&keys, cx);
    }
}

/// `keys` with `col_ix` sorted by `direction`: in its place when it is a key
/// already, at the end otherwise.
fn with_sort_key(
    keys: &[TableSortState],
    col_ix: usize,
    direction: SortDirection,
) -> Vec<TableSortState> {
    let mut next = keys.to_vec();

    match next.iter_mut().find(|key| key.column_ix == col_ix) {
        Some(key) => key.direction = direction,
        None => next.push(TableSortState::new(col_ix, direction)),
    }

    next
}

fn same_order(a: &[OrderByColumn], b: &[OrderByColumn]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.column.name == b.column.name && a.direction == b.direction)
}

/// Drops the default order from the front of `requested` when the rows are
/// in the default order now and keys were added after it.
fn drop_default_order_prefix(
    current: &[OrderByColumn],
    default_order: &[OrderByColumn],
    requested: Vec<OrderByColumn>,
) -> Vec<OrderByColumn> {
    let extends_default = !default_order.is_empty()
        && requested.len() > default_order.len()
        && same_order(current, default_order)
        && same_order(
            requested.get(..default_order.len()).unwrap_or_default(),
            default_order,
        );

    if extends_default {
        requested.into_iter().skip(default_order.len()).collect()
    } else {
        requested
    }
}

/// `order` cut to the first `limit` columns.
fn truncate_order_by(mut order: Vec<OrderByColumn>, limit: Option<usize>) -> Vec<OrderByColumn> {
    if let Some(limit) = limit {
        order.truncate(limit.max(1));
    }

    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_order_by_keeps_what_the_driver_orders_by() {
        let order = vec![asc("city"), desc("name"), asc("id")];

        assert_eq!(truncate_order_by(order.clone(), None), order);
        assert_eq!(
            truncate_order_by(order.clone(), Some(2)),
            vec![asc("city"), desc("name")]
        );
        assert_eq!(truncate_order_by(order, Some(1)), vec![asc("city")]);
    }

    fn asc(name: &str) -> OrderByColumn {
        OrderByColumn::from_name(name, SortDirection::Ascending)
    }

    fn desc(name: &str) -> OrderByColumn {
        OrderByColumn::from_name(name, SortDirection::Descending)
    }

    #[test]
    fn a_key_added_to_the_default_order_replaces_it() {
        let default_order = vec![OrderByColumn::asc("id")];

        let order = drop_default_order_prefix(
            &default_order,
            &default_order,
            vec![asc("id"), desc("name")],
        );

        assert_eq!(order, vec![desc("name")]);
    }

    #[test]
    fn a_key_added_to_a_chosen_sort_follows_it() {
        let default_order = vec![OrderByColumn::asc("id")];

        let order =
            drop_default_order_prefix(&[desc("id")], &default_order, vec![desc("id"), asc("name")]);
        assert_eq!(order, vec![desc("id"), asc("name")]);

        let order = drop_default_order_prefix(
            &[asc("name")],
            &default_order,
            vec![asc("name"), asc("city")],
        );
        assert_eq!(order, vec![asc("name"), asc("city")]);
    }

    #[test]
    fn with_sort_key_appends_or_sets_the_direction_in_place() {
        let keys = [TableSortState::ascending(2), TableSortState::ascending(0)];

        assert_eq!(
            with_sort_key(&keys, 5, SortDirection::Descending),
            vec![
                TableSortState::ascending(2),
                TableSortState::ascending(0),
                TableSortState::descending(5),
            ]
        );
        assert_eq!(
            with_sort_key(&keys, 2, SortDirection::Descending),
            vec![TableSortState::descending(2), TableSortState::ascending(0)]
        );
    }
}
