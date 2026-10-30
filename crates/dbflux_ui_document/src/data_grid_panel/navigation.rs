use super::{
    DataGridEvent, DataGridPanel, DataSource, EditState, GridFocusMode, LocalSortState,
    PendingRequery, ReferencedRowTarget, ResultViewMode, TableReload, ToolbarFocus, row_inspector,
};
use dbflux_app::keymap::Command;
use dbflux_components::components::data_table::{Direction, Edge, SortState as TableSortState};
use dbflux_core::{Pagination, SortDirection, TableRef};
use gpui::*;
use std::cmp::Ordering;

impl DataGridPanel {
    // === Sorting ===

    /// Refuses a server-side sort, which re-queries the rows, while unsaved
    /// edits exist. The table has already moved its header arrow to the
    /// requested sort, so a refused request puts back the sort the displayed
    /// rows were loaded with.
    fn server_sort_blocked(&mut self, cx: &mut Context<Self>) -> bool {
        let DataSource::Table { order_by, .. } = &self.source else {
            return false;
        };

        if !self.reload_blocked_by_pending_edits(cx) {
            return false;
        }

        let loaded_keys = self.sort_keys_for_order_by(order_by);
        self.show_sort_keys(&loaded_keys, cx);
        true
    }

    /// Refuses an in-memory sort that would lose unsaved edits. The sort
    /// carries edits over by primary key, so it is refused only when the
    /// result has none and the edits are addressed by row position, which the
    /// sort reorders. The header arrow goes back to the current order.
    fn local_sort_blocked(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.pending_edits_lack_row_identity(cx) || !self.reload_blocked_by_pending_edits(cx) {
            return false;
        }

        let current_keys = self.local_sort_keys();
        self.show_sort_keys(&current_keys, cx);
        true
    }

    pub(super) fn handle_sort_request(
        &mut self,
        col_ix: usize,
        direction: SortDirection,
        cx: &mut Context<Self>,
    ) {
        self.handle_sort_keys_request(&[TableSortState::new(col_ix, direction)], cx);
    }

    /// Sorts by `keys`, first to last: in the query for a table, in memory
    /// otherwise. No keys returns to the default order.
    pub(super) fn handle_sort_keys_request(
        &mut self,
        keys: &[TableSortState],
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() {
            self.handle_sort_clear(cx);
            return;
        }

        if self.server_sort_blocked(cx) {
            return;
        }

        // Neighbor rows are not a page of the table: ordering them by a
        // column would read the table instead.
        if self.nearest_neighbors.is_some() {
            return;
        }

        // Extract values before mutating self.source
        let table_info = match &self.source {
            DataSource::Table {
                profile_id,
                database,
                table,
                pagination,
                total_rows,
                ..
            } => Some((
                *profile_id,
                database.clone(),
                table.clone(),
                pagination.reset_offset(),
                *total_rows,
            )),
            DataSource::Collection { .. } => None,
            DataSource::QueryResult { .. } => None,
        };

        if let Some((profile_id, database, table, new_pagination, total_rows)) = table_info {
            // Server-side sort: update source and queue re-query
            let new_order_by = self.order_by_for_sort_keys(keys, cx);

            let filter_value = self.filter_bar.filter_input.read(cx).value();
            let filter = if filter_value.trim().is_empty() {
                None
            } else {
                Some(filter_value.to_string())
            };

            // Update source immediately for UI consistency
            self.source = DataSource::Table {
                profile_id,
                database: database.clone(),
                table: table.clone(),
                pagination: new_pagination.clone(),
                order_by: new_order_by.clone(),
                total_rows,
            };

            // Queue re-query
            self.pending.requery = Some(PendingRequery {
                profile_id,
                database,
                table,
                pagination: new_pagination,
                order_by: new_order_by,
                filter,
                total_rows,
            });

            cx.notify();
        } else if !self.local_sort_blocked(cx) {
            // Client-side sort: sort in memory
            let local_keys: Vec<LocalSortState> = keys
                .iter()
                .map(|key| LocalSortState {
                    column_ix: key.column_ix,
                    direction: key.direction,
                })
                .collect();
            self.apply_local_sort_keys(&local_keys, cx);
        }
    }

    /// A header click cycled the sort off. While the rows are in the table's
    /// default order, clearing would read that same order again, so the
    /// click sorts the column the other way instead: the default is where
    /// the cycle starts. A table opened newest first (primary key
    /// descending) then reaches ascending on the next click.
    pub(super) fn handle_header_sort_off(&mut self, cx: &mut Context<Self>) {
        match self.default_order_flipped(cx) {
            Some((col_ix, direction)) => self.handle_sort_request(col_ix, direction, cx),
            None => self.handle_sort_clear(cx),
        }
    }

    /// The first default-order column the other way round, when the table's
    /// rows are in its default order now.
    fn default_order_flipped(&self, cx: &Context<Self>) -> Option<(usize, SortDirection)> {
        let DataSource::Table {
            profile_id,
            database,
            table,
            order_by,
            ..
        } = &self.source
        else {
            return None;
        };

        let default_order = Self::get_primary_key_columns(
            &self.app_state,
            *profile_id,
            database.as_deref(),
            table,
            cx,
        );
        let in_default_order = !default_order.is_empty()
            && order_by.len() == default_order.len()
            && order_by
                .iter()
                .zip(&default_order)
                .all(|(loaded, default)| {
                    loaded.column.name == default.column.name
                        && loaded.direction == default.direction
                });
        if !in_default_order {
            return None;
        }

        let first = default_order.first()?;
        let col_ix = self
            .result
            .columns
            .iter()
            .position(|column| column.name == first.column.name)?;
        let flipped = match first.direction {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        };

        Some((col_ix, flipped))
    }

    // current is the enumerate() index over original_order, which
    // apply_local_sort builds and apply_result_search filters in step with
    // result.rows. The order and the sort state are reset wherever the rows
    // are replaced, including the document-grid rebuild paths, so a refresh returning fewer rows can no longer leave
    // current past rows.len(); only the restore path was exposed to that. The
    // original value is only a sort key and can exceed rows.len() after a
    // search.
    #[expect(
        clippy::indexing_slicing,
        reason = "current indexes original_order, which apply_local_sort builds and apply_result_search filters in step with result.rows; the order and sort state are reset wherever the rows are replaced"
    )]
    pub(super) fn handle_sort_clear(&mut self, cx: &mut Context<Self>) {
        if self.server_sort_blocked(cx) {
            return;
        }

        // Extract values before mutating self.source
        let table_info = match &self.source {
            DataSource::Table {
                profile_id,
                database,
                table,
                pagination,
                total_rows,
                ..
            } => {
                let pk_order = Self::get_primary_key_columns(
                    &self.app_state,
                    *profile_id,
                    database.as_deref(),
                    table,
                    cx,
                );
                Some((
                    *profile_id,
                    database.clone(),
                    table.clone(),
                    pagination.reset_offset(),
                    *total_rows,
                    pk_order,
                ))
            }
            DataSource::Collection { .. } => None,
            DataSource::QueryResult { .. } => None,
        };

        if let Some((profile_id, database, table, new_pagination, total_rows, pk_order)) =
            table_info
        {
            let filter_value = self.filter_bar.filter_input.read(cx).value();
            let filter = if filter_value.trim().is_empty() {
                None
            } else {
                Some(filter_value.to_string())
            };

            self.default_order_by = pk_order.clone();
            self.source = DataSource::Table {
                profile_id,
                database: database.clone(),
                table: table.clone(),
                pagination: new_pagination.clone(),
                order_by: pk_order.clone(),
                total_rows,
            };

            self.pending.requery = Some(PendingRequery {
                profile_id,
                database,
                table,
                pagination: new_pagination,
                order_by: pk_order,
                filter,
                total_rows,
            });

            cx.notify();
        } else {
            if self.local_sort_blocked(cx) {
                return;
            }

            // Restore original row order
            if let Some(original_order) = self.grid_table.original_row_order.take() {
                let mut restore_indices: Vec<(usize, usize)> = original_order
                    .iter()
                    .enumerate()
                    .map(|(current, &original)| (original, current))
                    .collect();
                restore_indices.sort_by_key(|(orig, _)| *orig);

                let rows = std::mem::take(&mut self.result.rows);
                self.result.rows = restore_indices
                    .into_iter()
                    .map(|(_, current)| rows[current].clone())
                    .collect();
            }

            self.grid_table.local_sort_state = None;
            self.pending.rebuild = true;
            self.pending.rebuild_keeps_edits = true;
            cx.notify();
        }
    }

    /// One-key form of `apply_local_sort_keys`, for tests.
    #[cfg(test)]
    pub(super) fn apply_local_sort(
        &mut self,
        col_ix: usize,
        direction: SortDirection,
        cx: &mut Context<Self>,
    ) {
        self.apply_local_sort_keys(
            &[LocalSortState {
                column_ix: col_ix,
                direction,
            }],
            cx,
        );
    }

    /// Sorts the rows in memory by `keys`, first to last; a later key only
    /// orders the rows the earlier ones leave tied.
    pub(super) fn apply_local_sort_keys(
        &mut self,
        keys: &[LocalSortState],
        cx: &mut Context<Self>,
    ) {
        let Some((&first, then)) = keys.split_first() else {
            return;
        };

        // Save original order if this is the first sort
        if self.grid_table.original_row_order.is_none() {
            self.grid_table.original_row_order = Some((0..self.result.rows.len()).collect());
        }

        // Sort using indices for tracking
        let mut indices: Vec<usize> = (0..self.result.rows.len()).collect();
        // a and b come from indices, a permutation of 0..rows.len(), so both
        // row indexings stay in bounds.
        #[expect(
            clippy::indexing_slicing,
            reason = "a and b come from indices, a permutation of 0..result.rows.len()"
        )]
        indices.sort_by(|&a, &b| {
            keys.iter()
                .map(|key| {
                    let val_a = self.result.rows[a].get(key.column_ix);
                    let val_b = self.result.rows[b].get(key.column_ix);

                    let cmp = match (val_a, val_b) {
                        (Some(a), Some(b)) => a.cmp(b),
                        (None, Some(_)) => Ordering::Greater,
                        (Some(_), None) => Ordering::Less,
                        (None, None) => Ordering::Equal,
                    };

                    match key.direction {
                        SortDirection::Ascending => cmp,
                        SortDirection::Descending => cmp.reverse(),
                    }
                })
                .find(|cmp| cmp.is_ne())
                .unwrap_or(Ordering::Equal)
        });

        // Reorder rows according to sorted indices. i comes from indices, a
        // permutation of 0..rows.len().
        #[expect(
            clippy::indexing_slicing,
            reason = "i comes from indices, a permutation of 0..result.rows.len()"
        )]
        let sorted_rows: Vec<_> = indices
            .iter()
            .map(|&i| self.result.rows[i].clone())
            .collect();
        self.result.rows = sorted_rows;

        // Update original_row_order to map new order -> original.
        // orig[i] indexes original_row_order, which apply_local_sort builds in
        // step with result.rows. The order and the sort state are reset
        // wherever the rows are replaced, including the document-grid rebuild
        // paths, so orig always indexes an order built in step with the
        // current rows; only a refresh returning more rows could have overrun
        // orig here, while fewer rows affected the restore path.
        #[expect(
            clippy::indexing_slicing,
            reason = "orig indexes original_row_order, which apply_local_sort builds in step with result.rows; the order and sort state are reset wherever the rows are replaced"
        )]
        if let Some(ref mut orig) = self.grid_table.original_row_order {
            *orig = indices.iter().map(|&i| orig[i]).collect();
        }

        self.grid_table.local_sort_state = Some(first);
        self.grid_table.local_sort_then = then.to_vec();
        // The rows only move, so the edits staged on them follow by primary key.
        self.pending.rebuild = true;
        self.pending.rebuild_keeps_edits = true;
        cx.notify();
    }

    // === Pagination ===

    /// Whether a page change must not run: a static result has no pages, and
    /// unsaved edits are refused like a refresh, since the reload would drop
    /// them.
    fn page_change_blocked(&self, cx: &mut Context<Self>) -> bool {
        matches!(self.source, DataSource::QueryResult { .. })
            || self.reload_blocked_by_pending_edits(cx)
    }

    pub fn go_to_next_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page_change_blocked(cx) {
            return;
        }

        match &self.source {
            DataSource::Table {
                profile_id,
                database,
                table,
                pagination,
                order_by,
                total_rows,
            } => {
                // Another page is a different row set: the cursor belongs to
                // the page being left. Marked only where a request is issued,
                // so an arm that asks for nothing cannot leave the mark behind.
                self.grid_table.reload = TableReload::ResetRows;
                self.run_table_query(
                    *profile_id,
                    database.clone(),
                    table.clone(),
                    pagination.next_page(),
                    order_by.clone(),
                    *total_rows,
                    window,
                    cx,
                );
            }
            DataSource::Collection {
                profile_id,
                collection,
                pagination,
                total_docs,
            } => {
                self.grid_table.reload = TableReload::ResetRows;
                self.run_collection_query(
                    *profile_id,
                    collection.clone(),
                    pagination.next_page(),
                    *total_docs,
                    window,
                    cx,
                );
            }
            DataSource::QueryResult { .. } => {}
        }
    }

    pub fn go_to_prev_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prev) = self.source.pagination().and_then(|p| p.prev_page()) else {
            return;
        };

        if self.page_change_blocked(cx) {
            return;
        }

        match &self.source {
            DataSource::Table {
                profile_id,
                database,
                table,
                order_by,
                total_rows,
                ..
            } => {
                self.grid_table.reload = TableReload::ResetRows;
                self.run_table_query(
                    *profile_id,
                    database.clone(),
                    table.clone(),
                    prev,
                    order_by.clone(),
                    *total_rows,
                    window,
                    cx,
                );
            }
            DataSource::Collection {
                profile_id,
                collection,
                total_docs,
                ..
            } => {
                self.grid_table.reload = TableReload::ResetRows;
                self.run_collection_query(
                    *profile_id,
                    collection.clone(),
                    prev,
                    *total_docs,
                    window,
                    cx,
                );
            }
            DataSource::QueryResult { .. } => {}
        }
    }

    pub(super) fn can_go_prev(&self) -> bool {
        self.source
            .pagination()
            .map(|p| !p.is_first_page())
            .unwrap_or(false)
    }

    pub(super) fn can_go_next(&self) -> bool {
        let Some(pagination) = self.source.pagination() else {
            return false;
        };

        if let Some(total) = self.paged_total() {
            let next_offset = pagination.offset() + pagination.limit() as u64;
            return next_offset < total;
        }

        self.result.row_count() >= pagination.limit() as usize
    }

    /// Rows the pages cover: a collection's page offsets count from the
    /// builder's skip, so the skipped documents are left out.
    fn paged_total(&self) -> Option<u64> {
        let total = self.source.total_rows()?;

        match self.source {
            DataSource::Collection { .. } => {
                Some(total.saturating_sub(self.collection.applied_skip))
            }
            _ => Some(total),
        }
    }

    pub(super) fn total_pages(&self) -> Option<u64> {
        let pagination = self.source.pagination()?;
        let total = self.paged_total()?;
        let limit = pagination.limit() as u64;
        if limit == 0 {
            return Some(1);
        }
        Some(total.div_ceil(limit))
    }

    // === Navigation ===

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.result.rows.is_empty() {
            return;
        }
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                state.move_active(Direction::Down, false, cx);
            });
        }
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        if self.result.rows.is_empty() {
            return;
        }
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                state.move_active(Direction::Up, false, cx);
            });
        }
    }

    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        if self.result.rows.is_empty() {
            return;
        }
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                state.move_to_edge(Edge::Home, false, cx);
            });
        }
    }

    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        if self.result.rows.is_empty() {
            return;
        }
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                state.move_to_edge(Edge::End, false, cx);
            });
        }
    }

    pub fn column_left(&mut self, cx: &mut Context<Self>) {
        if self.result.columns.is_empty() {
            return;
        }
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                state.move_active(Direction::Left, false, cx);
            });
        }
    }

    pub fn column_right(&mut self, cx: &mut Context<Self>) {
        if self.result.columns.is_empty() {
            return;
        }
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                state.move_active(Direction::Right, false, cx);
            });
        }
    }

    // === Focus Management ===

    #[allow(dead_code)]
    pub(super) fn focus_mode(&self) -> GridFocusMode {
        self.focus.focus_mode
    }

    pub fn focus_toolbar(&mut self, cx: &mut Context<Self>) {
        if !self.source.is_table() {
            return;
        }
        self.focus.focus_mode = GridFocusMode::Toolbar;
        self.focus.toolbar_focus = ToolbarFocus::Filter;
        self.focus.edit_state = EditState::Navigating;
        cx.notify();
    }

    pub fn focus_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus_mode = GridFocusMode::Table;
        // `EditState` tracks the toolbar inputs, not the cell editor, so it
        // goes back to navigating even while a cell editor keeps the keyboard.
        self.focus.edit_state = EditState::Navigating;
        if !self.focus_open_cell_editor(window, cx) {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    /// Hand the keyboard to the grid's open text cell editor; false when none
    /// is open, an enum dropdown included. Clicking a cell asks the host for
    /// focus, and that request arrives after a double-click has already
    /// opened the editor.
    pub(crate) fn focus_open_cell_editor(&self, window: &mut Window, cx: &mut App) -> bool {
        self.grid_table
            .table_state
            .as_ref()
            .is_some_and(|state| state.update(cx, |state, cx| state.focus_open_editor(window, cx)))
    }

    #[cfg(test)]
    pub(crate) fn table_state_for_test(
        &self,
    ) -> Option<Entity<dbflux_components::components::data_table::DataTableState>> {
        self.grid_table.table_state.clone()
    }

    pub fn toolbar_left(&mut self, cx: &mut Context<Self>) {
        if self.focus.focus_mode != GridFocusMode::Toolbar {
            return;
        }
        self.focus.toolbar_focus = self.focus.toolbar_focus.left();
        cx.notify();
    }

    pub fn toolbar_right(&mut self, cx: &mut Context<Self>) {
        if self.focus.focus_mode != GridFocusMode::Toolbar {
            return;
        }
        self.focus.toolbar_focus = self.focus.toolbar_focus.right();
        cx.notify();
    }

    pub fn toolbar_execute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus.focus_mode != GridFocusMode::Toolbar {
            return;
        }

        match self.focus.toolbar_focus {
            ToolbarFocus::Filter => {
                self.focus.edit_state = EditState::Editing;
                self.filter_bar.filter_input.update(cx, |input, cx| {
                    input.focus(window, cx);
                });
                cx.notify();
            }
            ToolbarFocus::Limit => {
                self.focus.edit_state = EditState::Editing;
                self.filter_bar.limit_input.update(cx, |input, cx| {
                    input.focus(window, cx);
                });
                cx.notify();
            }
            ToolbarFocus::Refresh => {
                self.request_refresh(window, cx);
                self.focus_table(window, cx);
            }
        }
    }

    pub fn exit_edit_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus.switching_input {
            self.focus.switching_input = false;
            return;
        }

        if self.focus.edit_state == EditState::Editing {
            self.focus.edit_state = EditState::Navigating;
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// Runs `cmd` as a chart key (see `chart::keyboard`) while the result
    /// shows only its chart, or while an axis picker is open. In the table
    /// half of Table + Chart the keys stay with the table.
    fn dispatch_chart_key(&mut self, cmd: Command, cx: &mut Context<Self>) -> bool {
        let Some(shell) = self.chart.chart_shell.clone() else {
            return false;
        };

        let picker_open = shell.read(cx).axis_open_pill.is_some();
        if !picker_open && self.result_view_mode() != ResultViewMode::Chart {
            return false;
        }

        let columns = self.result.columns.clone();
        let handled = shell
            .update(cx, |shell, cx| shell.keyboard_command(cmd, &columns, cx))
            .handled();

        if handled {
            cx.notify();
        }

        handled
    }

    // === Command Dispatch ===

    pub fn dispatch_command(
        &mut self,
        cmd: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Handle delete confirmation modal
        if self.pending_delete_confirm.is_some() {
            match cmd {
                Command::Cancel => {
                    self.cancel_delete(window, cx);
                    return true;
                }
                Command::Execute => {
                    self.confirm_delete(window, cx);
                    return true;
                }
                _ => return true, // Block other commands while modal is open
            }
        }

        // The review owns the keys while it is open: Enter saves, Escape
        // cancels, and nothing reaches the grid behind it.
        if self.change_review.is_some() {
            match cmd {
                Command::Cancel => self.cancel_change_review(window, cx),
                Command::Execute => self.confirm_change_review(window, cx),
                _ => {}
            }
            return true;
        }

        if cmd == Command::ReviewChanges {
            return self.open_change_review(window, cx);
        }

        // Handle context menu commands when menu is open
        if self.context_menu.is_some() {
            return self.dispatch_menu_command(cmd, window, cx);
        }

        if self.chrome.export_menu_open {
            return self.dispatch_export_menu_command(cmd, window, cx);
        }

        if self.collection.history_open {
            return self.dispatch_history_menu_command(cmd, window, cx);
        }

        // Alt+L / Alt+H move between a collection's Documents, Schema and
        // Aggregate views, from any of them.
        if matches!(cmd, Command::NextResultTab | Command::PrevResultTab)
            && self.step_collection_tab(cmd == Command::NextResultTab, cx)
        {
            return true;
        }

        // The same keys move between a table's Data, Columns and Partitions
        // views when its connection offers them.
        if matches!(cmd, Command::NextResultTab | Command::PrevResultTab)
            && self.step_table_tab(cmd == Command::NextResultTab, window, cx)
        {
            if !self.focus_table_view(window, cx) {
                self.focus_table(window, cx);
            }
            return true;
        }

        if let Some(handled) = self.dispatch_table_view_command(cmd, window, cx) {
            return handled;
        }

        if let Some(handled) = self.dispatch_side_island_command(cmd, window, cx) {
            return handled;
        }

        // The Aggregate view has its own editor and result views; commands
        // meant for the documents grid must not reach the hidden grid.
        if self.collection.tab == super::documents::CollectionTab::Aggregate {
            return self.dispatch_aggregate_command(cmd, window, cx);
        }

        if self.dispatch_chart_key(cmd, cx) {
            return true;
        }

        // A modified value panel owns "save": while its editor holds the
        // keyboard the panel reports `ContextId::TextInput`, where Cmd+S
        // resolves to SaveQuery. Saving the script instead would leave what
        // the user just typed uncommitted.
        if matches!(cmd, Command::SaveQuery)
            && let Some(panel) = self.value_panel_pending_save(cx)
        {
            panel.update(cx, |panel, cx| panel.save(cx));
            return true;
        }

        // Handle toolbar mode commands
        if self.focus.focus_mode == GridFocusMode::Toolbar {
            match cmd {
                Command::Cancel | Command::FocusUp => {
                    self.focus_table(window, cx);
                    return true;
                }
                Command::FocusLeft | Command::ColumnLeft => {
                    self.toolbar_left(cx);
                    return true;
                }
                Command::ColumnRight => {
                    self.toolbar_right(cx);
                    return true;
                }
                Command::Execute => {
                    self.toolbar_execute(window, cx);
                    return true;
                }
                _ => {}
            }
        }

        // When an enum dropdown is open, route navigation to the dropdown
        let editing_enum = self
            .grid_table
            .table_state
            .as_ref()
            .map(|ts| ts.read(cx).is_editing_enum())
            .unwrap_or(false);

        if editing_enum && let Some(table_state) = &self.grid_table.table_state {
            match cmd {
                Command::SelectNext | Command::FocusDown => {
                    table_state.update(cx, |state, cx| state.enum_dropdown_next(cx));
                    return true;
                }
                Command::SelectPrev | Command::FocusUp => {
                    table_state.update(cx, |state, cx| state.enum_dropdown_prev(cx));
                    return true;
                }
                Command::Execute => {
                    table_state.update(cx, |state, cx| state.enum_dropdown_accept(cx));
                    return true;
                }
                Command::Cancel => {
                    table_state.update(cx, |state, cx| state.enum_dropdown_cancel(cx));
                    return true;
                }
                _ => {}
            }
        }

        // Handle table mode commands
        match cmd {
            Command::FocusToolbar => {
                self.focus_toolbar(cx);
                true
            }
            Command::Execute => {
                if let Some(table_state) = &self.grid_table.table_state {
                    table_state.update(cx, |state, cx| {
                        if state.is_editing() {
                            state.stop_editing(true, cx);
                        } else if let Some(coord) = state.selection().active {
                            state.start_editing(coord, window, cx);
                        }
                    });
                }
                true
            }
            Command::Cancel => {
                if let Some(table_state) = &self.grid_table.table_state {
                    let was_editing = table_state.update(cx, |state, cx| {
                        if state.is_editing() {
                            state.stop_editing(false, cx);
                            true
                        } else {
                            false
                        }
                    });
                    if was_editing {
                        return true;
                    }
                }

                // Esc leaves similar rows for the table page, as the bar's
                // "Back to table" says.
                if self.neighbor_bar_search().is_some() {
                    self.clear_nearest_neighbors(window, cx);
                    return true;
                }

                false
            }
            Command::SelectNext | Command::FocusDown => {
                self.select_next(cx);
                true
            }
            Command::SelectPrev | Command::FocusUp => {
                self.select_prev(cx);
                true
            }
            Command::SelectFirst => {
                self.select_first(cx);
                true
            }
            Command::SelectLast => {
                self.select_last(cx);
                true
            }
            Command::ColumnLeft | Command::FocusLeft => {
                self.column_left(cx);
                true
            }
            Command::ColumnRight => {
                self.column_right(cx);
                true
            }
            Command::FocusRight => self.enter_side_island(window, cx),
            Command::ResultsNextPage | Command::PageDown => {
                self.go_to_next_page(window, cx);
                true
            }
            Command::ResultsPrevPage | Command::PageUp => {
                self.go_to_prev_page(window, cx);
                true
            }
            Command::RefreshSchema => {
                self.request_refresh(window, cx);
                true
            }
            Command::ExportResults => {
                self.export_results(window, cx);
                true
            }
            Command::ClearFilter => self.clear_filter(window, cx),
            Command::OpenContextMenu => {
                use crate::DataViewMode;
                if self.view_config.mode == DataViewMode::Document {
                    self.open_document_context_menu_at_cursor(window, cx);
                } else {
                    self.open_context_menu_at_selection(window, cx);
                }
                true
            }
            Command::ResultsCopyCell => {
                self.handle_copy(window, cx);
                true
            }
            // The document view's menu has no Copy as JSON, and its tree does
            // not drive the grid selection the copy reads.
            Command::CopySelectionAsJson
                if self.view_config.mode != crate::DataViewMode::Document =>
            {
                self.copy_selection_with_format(
                    dbflux_export::ExportFormat::JsonPretty,
                    window,
                    cx,
                );
                true
            }
            Command::CycleDocumentView => {
                self.toggle_view_mode(cx);
                true
            }
            Command::CycleResultView => self.cycle_result_view(window, cx),
            Command::SaveQuery if self.commits_document_patches(cx) => {
                self.commit_document_edits(cx);
                true
            }
            Command::SaveQuery => {
                if let Some(table_state) = &self.grid_table.table_state
                    && table_state.read(cx).has_pending_operations()
                {
                    table_state.update(cx, |state, cx| state.request_save_all(cx));
                    return true;
                }
                false
            }
            Command::RunQuery if self.is_document_collection(cx) => {
                self.find_documents(window, cx);
                true
            }
            Command::ToggleRecordView => {
                if self.record_view_available() {
                    self.set_record_mode(!self.record_mode(), cx);
                }
                true
            }
            Command::ToggleValuePanel => {
                self.toggle_value_panel(cx);
                true
            }
            Command::ToggleRowInspector => {
                self.toggle_row_inspector(cx);
                true
            }
            Command::FindSimilarRows => self.find_similar_rows(cx),
            _ => false,
        }
    }

    // === Referenced-row navigation ===

    /// Whether an applied builder spec owns this panel's next refresh: the
    /// equality a referenced row writes would land in the hidden filter
    /// input while the builder query reruns unchanged. The workspace keeps
    /// such a tab's query and opens the referenced row in a tab of its own.
    pub(crate) fn builder_query_applied(&self) -> bool {
        self.builder.visual_select.is_some()
    }

    /// Maps an outgoing FK reference to the row the workspace should open.
    /// `None` unless the panel browses a table, since the connection and
    /// database the target lives in come from that source — and for an
    /// incoming reference, whose columns run the other way.
    pub(super) fn reference_target(
        &self,
        reference: &row_inspector::FkReference,
    ) -> Option<ReferencedRowTarget> {
        if !matches!(reference.kind, row_inspector::ReferenceKind::Outgoing) {
            return None;
        }

        let DataSource::Table {
            profile_id,
            database,
            ..
        } = &self.source
        else {
            return None;
        };

        let table = match &reference.target_schema {
            Some(schema) => TableRef::with_schema(schema.clone(), reference.target_table.clone()),
            None => TableRef::new(reference.target_table.clone()),
        };

        Some(ReferencedRowTarget {
            profile_id: *profile_id,
            database: database.clone(),
            table,
            key_column: reference.target_pk.clone(),
            key_value: reference.value.clone(),
        })
    }

    /// The referenced-row target for a cell whose column is a single-column
    /// foreign key and whose value is not null; `None` for every other cell.
    /// `row` is a visual row of the table, so pending inserts shift the base
    /// row it resolves to.
    pub(super) fn fk_target_for_cell(
        &self,
        row: usize,
        col: usize,
        cx: &App,
    ) -> Option<ReferencedRowTarget> {
        let column_name = self.result.columns.get(col)?.name.clone();

        let foreign_keys = self
            .table_details_for(cx)?
            .foreign_keys
            .as_deref()
            .unwrap_or(&[]);

        let foreign_key = foreign_keys
            .iter()
            .find(|fk| matches!(fk.columns.as_slice(), [local] if *local == column_name))?;
        let key_column = foreign_key.referenced_columns.first()?.clone();

        let key_value = self
            .resolve_cell_value(row, col, cx)
            .filter(|value| !value.is_null())?;

        self.reference_target(&row_inspector::FkReference {
            column: column_name,
            target_schema: foreign_key.referenced_schema.clone(),
            target_table: foreign_key.referenced_table.clone(),
            target_pk: key_column,
            value: key_value,
            display_label: None,
            kind: row_inspector::ReferenceKind::Outgoing,
        })
    }

    /// Emits the request to open the row `target` points at.
    pub(super) fn navigate_to_reference(
        &mut self,
        target: ReferencedRowTarget,
        cx: &mut Context<Self>,
    ) {
        cx.emit(DataGridEvent::OpenReferencedRow { target });
    }
}
