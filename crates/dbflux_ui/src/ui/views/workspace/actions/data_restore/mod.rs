use super::*;
use crate::ui::document::{DataDocument, TableSessionView};
use dbflux_storage::repositories::state::sessions::RestoredTab;

impl Workspace {
    /// Reopens a table or collection tab from the session.
    ///
    /// The tab comes back whether or not its connection is up; it loads once
    /// the connection is. A tab whose profile was deleted, or whose stored
    /// source is incomplete, is dropped with a log line, as other kinds are.
    pub(in crate::ui::views::workspace) fn restore_data_tab(
        &mut self,
        tab: &RestoredTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let exec_ctx: dbflux_core::ExecutionContext =
            serde_json::from_str(&tab.exec_ctx_json).unwrap_or_default();

        let Some(profile_id) = exec_ctx.connection_id else {
            log::warn!(
                "{} tab '{}' has no profile in its session entry; skipping",
                tab.tab_kind,
                tab.title
            );
            return;
        };

        let profile_exists = self
            .app_state
            .read(cx)
            .profiles()
            .iter()
            .any(|profile| profile.id == profile_id);
        if !profile_exists {
            log::info!(
                "{} tab '{}' belongs to a deleted connection; not restoring it",
                tab.tab_kind,
                tab.title
            );
            return;
        }

        let Some(name) = exec_ctx.container.clone() else {
            log::warn!(
                "{} tab '{}' has no object name in its session entry; skipping",
                tab.tab_kind,
                tab.title
            );
            return;
        };

        let app_state = self.app_state.clone();

        let doc = if tab.tab_kind == DataDocument::TABLE_SESSION_TAB_KIND {
            let table = dbflux_core::TableRef {
                schema: exec_ctx.schema.clone(),
                name,
            };
            let view = TableSessionView::from_json(tab.view_state_json.as_deref());

            cx.new(|cx| {
                DataDocument::restore_table(
                    profile_id,
                    table,
                    exec_ctx.database.clone(),
                    view,
                    app_state,
                    window,
                    cx,
                )
            })
        } else {
            let Some(database) = exec_ctx.database.clone() else {
                log::warn!(
                    "Collection tab '{}' has no database in its session entry; skipping",
                    tab.title
                );
                return;
            };

            cx.new(|cx| {
                DataDocument::restore_collection(
                    profile_id,
                    dbflux_core::CollectionRef::new(database, name),
                    app_state,
                    window,
                    cx,
                )
            })
        };

        let pane = DataDocument::into_pane(doc, cx);
        self.tab_manager.update(cx, |mgr, cx| {
            mgr.open(Tab::Pane(Box::new(pane)), cx);
        });
    }
}

#[cfg(test)]
mod tests;
