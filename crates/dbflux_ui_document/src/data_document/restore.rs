//! Session restore for table and collection tabs.
//!
//! A restored tab usually comes back before its connection does: startup
//! restores tabs first and reconnects afterwards, if at all. Such a tab holds a
//! grid that only names its source and builds the real one, through the same
//! constructor a sidebar click uses, once the connection can serve it.

use super::DataDocument;
use crate::data_grid_panel::{DataGridPanel, DataSource, TableSessionView};
use crate::handle::DocumentEvent;
use crate::pane::CodeSessionTabSnapshot;
use crate::types::DataSourceKind;
use dbflux_components::composites::EmptyState;
use dbflux_components::icons::AppIcon;
use dbflux_components::tokens::DocumentMetrics;
use dbflux_core::{CollectionRef, ExecutionContext, QueryLanguage, TableRef};
use dbflux_ui_base::{AppStateChanged, AppStateEntity};
use gpui::*;
use gpui_component::ActiveTheme;
use uuid::Uuid;

/// What a restored tab still needs once its connection is up.
pub(super) struct PendingRestore {
    app_state: Entity<AppStateEntity>,
    view: TableSessionView,
    _app_state_subscription: Subscription,
}

impl DataDocument {
    /// Session tab kind of a table tab.
    pub const TABLE_SESSION_TAB_KIND: &'static str = "Table";
    /// Session tab kind of a collection tab.
    pub const COLLECTION_SESSION_TAB_KIND: &'static str = "Collection";

    /// Reopens a table tab from the session. With its connection up it opens
    /// as a sidebar click would, reading its first page under `view`;
    /// otherwise it waits for the connection without reading anything.
    pub fn restore_table(
        profile_id: Uuid,
        table: TableRef,
        database: Option<String>,
        view: TableSessionView,
        app_state: Entity<AppStateEntity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = table.qualified_name();

        if table_source_ready(&app_state, profile_id, database.as_deref(), cx) {
            return Self::open_restored_table(
                title, profile_id, table, database, &view, app_state, window, cx,
            );
        }

        let data_grid = cx.new(|cx| {
            DataGridPanel::new_for_pending_table(
                profile_id,
                table,
                database,
                app_state.clone(),
                window,
                cx,
            )
        });

        let mut document = Self::new_with_grid(title, DataSourceKind::Table, data_grid, window, cx);
        document.await_connection(app_state, view, window, cx);
        document
    }

    /// Reopens a collection tab from the session, waiting for its connection
    /// the way [`Self::restore_table`] does.
    pub fn restore_collection(
        profile_id: Uuid,
        collection: CollectionRef,
        app_state: Entity<AppStateEntity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if app_state.read(cx).connections().contains_key(&profile_id) {
            return Self::new_for_collection(profile_id, collection, app_state, window, cx);
        }

        let title = collection.qualified_name();
        let data_grid = cx.new(|cx| {
            DataGridPanel::new_for_pending_collection(
                profile_id,
                collection,
                app_state.clone(),
                window,
                cx,
            )
        });

        let mut document =
            Self::new_with_grid(title, DataSourceKind::Collection, data_grid, window, cx);
        document.await_connection(app_state, TableSessionView::default(), window, cx);
        document
    }

    /// Whether this tab is still waiting for its connection to load.
    pub fn is_awaiting_connection(&self) -> bool {
        self.pending_restore.is_some()
    }

    #[allow(clippy::too_many_arguments)]
    fn open_restored_table(
        title: String,
        profile_id: Uuid,
        table: TableRef,
        database: Option<String>,
        view: &TableSessionView,
        app_state: Entity<AppStateEntity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let schema = table.schema.clone();
        let console_database = database.clone();
        let data_grid = cx.new(|cx| {
            DataGridPanel::new_for_table_with_view(
                profile_id,
                table,
                database,
                Some(view),
                app_state.clone(),
                window,
                cx,
            )
        });

        let mut document = Self::new_with_grid(title, DataSourceKind::Table, data_grid, window, cx);
        document.attach_table_console(profile_id, console_database, schema, app_state, window, cx);
        document
    }

    fn await_connection(
        &mut self,
        app_state: Entity<AppStateEntity>,
        view: TableSessionView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subscription = cx.subscribe_in(
            &app_state,
            window,
            |this, _, _: &AppStateChanged, window, cx| this.finish_pending_restore(window, cx),
        );

        self.pending_restore = Some(PendingRestore {
            app_state,
            view,
            _app_state_subscription: subscription,
        });
    }

    /// Builds the real grid once the connection can serve the source. Until
    /// then it only repaints, since the waiting message names what is missing.
    fn finish_pending_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_restore.as_ref() else {
            return;
        };
        let app_state = pending.app_state.clone();
        let source = self.data_grid.read(cx).source().clone();

        let data_grid = match source {
            DataSource::Table {
                profile_id,
                database,
                table,
                ..
            } => {
                if !table_source_ready(&app_state, profile_id, database.as_deref(), cx) {
                    cx.notify();
                    return;
                }

                let view = pending.view.clone();
                let schema = table.schema.clone();
                let console_database = database.clone();
                let data_grid = cx.new(|cx| {
                    DataGridPanel::new_for_table_with_view(
                        profile_id,
                        table,
                        database,
                        Some(&view),
                        app_state.clone(),
                        window,
                        cx,
                    )
                });
                self.install_grid(data_grid.clone(), cx);
                self.attach_table_console(
                    profile_id,
                    console_database,
                    schema,
                    app_state,
                    window,
                    cx,
                );
                data_grid
            }
            DataSource::Collection {
                profile_id,
                collection,
                ..
            } => {
                if !app_state.read(cx).connections().contains_key(&profile_id) {
                    cx.notify();
                    return;
                }

                let database = collection.database.clone();
                let data_grid = cx.new(|cx| {
                    DataGridPanel::new_for_collection(
                        profile_id,
                        collection,
                        app_state.clone(),
                        window,
                        cx,
                    )
                });
                self.install_grid(data_grid.clone(), cx);
                self.attach_console(
                    profile_id,
                    Some(database.clone()),
                    database,
                    app_state,
                    window,
                    cx,
                );
                data_grid
            }
            DataSource::QueryResult { .. } => {
                self.pending_restore = None;
                return;
            }
        };

        if self.is_active_tab {
            data_grid.update(cx, |grid, cx| grid.set_active_tab(true, cx));
        }

        cx.emit(DocumentEvent::MetaChanged);
        cx.notify();
    }

    /// Replaces the placeholder grid, and everything wired to it, with
    /// `data_grid`. Dropping the pending state ends the wait.
    fn install_grid(&mut self, data_grid: Entity<DataGridPanel>, cx: &mut Context<Self>) {
        let (result_panel, subscriptions) = Self::wire_grid(&data_grid, cx);
        self.data_grid = data_grid;
        self.result_panel = result_panel;
        self._subscriptions = subscriptions;
        self.pending_restore = None;
    }

    /// The tab as the session stores it: the source in the execution
    /// context, and for a table the read it last applied. Query results are
    /// not restored, since their rows are all they have.
    pub(super) fn session_tab_snapshot(&self, cx: &App) -> Option<CodeSessionTabSnapshot> {
        let grid = self.data_grid.read(cx);

        let (kind, exec_ctx, view) = match grid.source() {
            DataSource::Table {
                profile_id,
                database,
                table,
                ..
            } => {
                // A tab still waiting has read nothing, so what it would
                // replay is what it was restored with.
                let view = match &self.pending_restore {
                    Some(pending) => Some(pending.view.clone()),
                    None => grid.session_view(),
                };

                let exec_ctx = ExecutionContext {
                    connection_id: Some(*profile_id),
                    database: database.clone(),
                    schema: table.schema.clone(),
                    container: Some(table.name.clone()),
                    source: None,
                };

                (Self::TABLE_SESSION_TAB_KIND, exec_ctx, view)
            }
            DataSource::Collection {
                profile_id,
                collection,
                ..
            } => {
                let exec_ctx = ExecutionContext {
                    connection_id: Some(*profile_id),
                    database: Some(collection.database.clone()),
                    schema: None,
                    container: Some(collection.name.clone()),
                    source: None,
                };

                (Self::COLLECTION_SESSION_TAB_KIND, exec_ctx, None)
            }
            DataSource::QueryResult { .. } => return None,
        };

        Some(CodeSessionTabSnapshot {
            kind,
            id: self.id,
            title: self.title.clone(),
            language: QueryLanguage::Sql,
            exec_ctx,
            file_path: None,
            scratch_path: None,
            shadow_path: None,
            view_state_json: view.and_then(|view| view.to_json()),
        })
    }

    /// The notice a waiting tab shows instead of an empty grid: which
    /// connection, or which database of it, has to be opened.
    pub(super) fn render_pending_restore(&self, cx: &Context<Self>) -> AnyElement {
        let message = match (&self.pending_restore, self.data_grid.read(cx).source()) {
            (
                Some(pending),
                DataSource::Table {
                    profile_id,
                    database: Some(database),
                    ..
                },
            ) if pending
                .app_state
                .read(cx)
                .connections()
                .contains_key(profile_id) =>
            {
                crate::labels::restored_tab_waiting_database_message(database, &self.title)
            }
            (Some(pending), source) => {
                let profile_name = source_profile_id(source)
                    .and_then(|profile_id| {
                        pending
                            .app_state
                            .read(cx)
                            .profiles()
                            .iter()
                            .find(|profile| profile.id == profile_id)
                            .map(|profile| profile.name.clone())
                    })
                    .unwrap_or_default();

                crate::labels::restored_tab_waiting_connection_message(&profile_name, &self.title)
            }
            (None, _) => String::new(),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(DocumentMetrics::PADDING_X)
            .bg(cx.theme().background)
            .child(
                EmptyState::new(AppIcon::Unplug, message)
                    .title(crate::labels::restored_tab_waiting_title()),
            )
            .into_any_element()
    }
}

fn source_profile_id(source: &DataSource) -> Option<Uuid> {
    match source {
        DataSource::Table { profile_id, .. } | DataSource::Collection { profile_id, .. } => {
            Some(*profile_id)
        }
        DataSource::QueryResult { profile_id, .. } => *profile_id,
    }
}

/// Whether a table can be read now: its profile is connected and, on drivers
/// with one connection per database, that database is open as well.
fn table_source_ready(
    app_state: &Entity<AppStateEntity>,
    profile_id: Uuid,
    database: Option<&str>,
    cx: &App,
) -> bool {
    app_state
        .read(cx)
        .connections()
        .get(&profile_id)
        .is_some_and(|connected| connected.resolve_connection_for_execution(database).is_ok())
}

#[cfg(test)]
mod tests;
