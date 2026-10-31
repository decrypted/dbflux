//! Database work the grid sends through the per-connection query queue.

use super::{DataGridPanel, GridState};
use dbflux_core::{DbError, TaskId};
use dbflux_ui_base::work_queue::{QueryWork, spawn_query_db};
use gpui::{App, Context, Task};
use uuid::Uuid;

impl DataGridPanel {
    /// Background work of this grid. It is scoped to the tab, so closing the
    /// tab drops what still waits for a slot.
    pub(super) fn grid_work(&self, profile_id: Uuid) -> QueryWork {
        QueryWork::background(profile_id).scoped(&self.work_scope)
    }

    /// Work the user just asked for (a save, an explicit search), served
    /// before the connection's queued background reads. Not tied to the tab:
    /// a save that waits when its tab closes (an apply-then-close) still runs.
    pub(super) fn interactive_work(&self, profile_id: Uuid) -> QueryWork {
        QueryWork::interactive(profile_id)
    }

    /// Runs one driver call once the connection has a free query slot; a
    /// refusal (cancelled while queued, or a full queue) reads as
    /// [`DbError::Cancelled`].
    pub(super) fn spawn_grid_call<T: Send + 'static>(
        &self,
        work: QueryWork,
        job: impl std::future::Future<Output = Result<T, DbError>> + Send + 'static,
        cx: &mut App,
    ) -> Task<Result<T, DbError>> {
        spawn_query_db(&self.app_state, work, job, cx)
    }

    /// A table view's call (profile, partitions, read estimate) on the grid's
    /// own connection, queued as background work of the tab.
    pub(super) fn spawn_table_view_call<T: Send + 'static>(
        &self,
        description: String,
        job: impl std::future::Future<Output = Result<T, DbError>> + Send + 'static,
        cx: &mut App,
    ) -> Task<Result<T, DbError>> {
        match self.source_profile_id() {
            Some(profile_id) => {
                self.spawn_grid_call(self.grid_work(profile_id).described(description), job, cx)
            }
            None => cx.background_executor().spawn(job),
        }
    }

    /// The connection the grid's rows come from, if any.
    pub(super) fn source_profile_id(&self) -> Option<Uuid> {
        match &self.source {
            super::DataSource::Table { profile_id, .. }
            | super::DataSource::Collection { profile_id, .. } => Some(*profile_id),
            super::DataSource::QueryResult { profile_id, .. } => *profile_id,
        }
    }

    /// Drops the grid's queued work, as closing its tab does.
    pub(crate) fn drop_queued_work(&mut self) {
        self.work_scope.cancel();
        self.work_scope = dbflux_core::CancelToken::new();
    }

    /// A read that ended cancelled (by the user, by the queue, or because a
    /// newer read replaced it) leaves the grid ready for a reload, unless the
    /// newer read is still loading.
    pub(super) fn settle_cancelled_read(&mut self, task_id: TaskId, cx: &mut Context<Self>) {
        self.runner.clear_primary(task_id);

        if !self.runner.is_primary_active() && self.refresh.state == GridState::Loading {
            self.refresh.state = GridState::Ready;
            cx.notify();
        }
    }
}
