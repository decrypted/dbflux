//! Keeps the workspace palette's database lists current: fetches the
//! databases it can list, a few at a time, saves what it learns off the UI
//! thread, and refills an open palette as results land.

use super::{
    PALETTE_FETCHES_IN_FLIGHT, PaletteFetchQueue, PaletteObjectCache,
    build_resource_items_for_connection, write_palette_objects,
};
use crate::ui::views::workspace::Workspace;
use dbflux_storage::PaletteObjectIndexRepo;
use dbflux_ui_base::object_tree::{
    ObjectTreeEvent, ObjectTreeOutcome, ObjectTreeRequestKey, ObjectTreeRequestStatus,
};
use gpui::{Context, Task};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Fetches landing within this window refill an open palette once.
const REFRESH_COALESCE: Duration = Duration::from_millis(150);

/// What the workspace palette keeps between opens.
pub(in crate::ui::views::workspace) struct PaletteSync {
    pub(in crate::ui::views::workspace) cache: PaletteObjectCache,
    fetches: PaletteFetchQueue,
    /// Whether the open palette lists the workspace's items, and not a
    /// narrower list such as the saved charts.
    lists_workspace: bool,
    refresh: Option<Task<()>>,
    /// Dropped with the workspace on quit; the index then trails the last
    /// lists by one write, which the next session's fetches make up.
    last_write: Option<Task<()>>,
}

impl PaletteSync {
    pub(in crate::ui::views::workspace) fn new(repo: Arc<PaletteObjectIndexRepo>) -> Self {
        Self {
            cache: PaletteObjectCache::new(repo),
            fetches: PaletteFetchQueue::default(),
            lists_workspace: false,
            refresh: None,
            last_write: None,
        }
    }

    /// Records whether the palette about to open lists the workspace's items.
    pub(in crate::ui::views::workspace) fn set_lists_workspace(&mut self, lists_workspace: bool) {
        self.lists_workspace = lists_workspace;
    }
}

impl Workspace {
    /// Queues the databases the workspace palette could list, saves the
    /// lists it built, and starts fetching.
    pub(in crate::ui::views::workspace) fn sync_palette_after_build(
        &mut self,
        to_fetch: Vec<(Uuid, String)>,
        cx: &mut Context<Self>,
    ) {
        self.palette_sync
            .fetches
            .enqueue(to_fetch.into_iter().map(|(profile_id, database)| {
                ObjectTreeRequestKey::DatabaseSchema {
                    profile_id,
                    database,
                }
            }));
        self.flush_palette_writes(cx);
        self.pump_palette_fetches(cx);
    }

    pub(in crate::ui::views::workspace) fn on_palette_object_tree_event(
        &mut self,
        event: &ObjectTreeEvent,
        cx: &mut Context<Self>,
    ) {
        let profile_id = match &event.key {
            ObjectTreeRequestKey::DatabaseSchema { profile_id, .. }
            | ObjectTreeRequestKey::DatabaseList { profile_id } => profile_id,
            ObjectTreeRequestKey::TableDetails { .. } => return,
        };

        if event.outcome == ObjectTreeOutcome::Applied {
            self.remember_palette_profile(*profile_id, cx);
            self.schedule_palette_refresh(cx);
        }

        if self.palette_sync.fetches.settled(&event.key) {
            self.pump_palette_fetches(cx);
        }
    }

    fn pump_palette_fetches(&mut self, cx: &mut Context<Self>) {
        while let Some(key) = self.palette_sync.fetches.next(PALETTE_FETCHES_IN_FLIGHT) {
            let status = self.app_state.update(cx, |state, cx| {
                state.object_tree_request_in_background(key.clone(), cx)
            });

            if matches!(
                status,
                ObjectTreeRequestStatus::Cached | ObjectTreeRequestStatus::Failed(_)
            ) {
                self.palette_sync.fetches.finished(&key);
            }
        }
    }

    /// Records the lists of one profile after a fetch, so they are kept
    /// even when no palette is open to rebuild them.
    fn remember_palette_profile(&mut self, profile_id: Uuid, cx: &mut Context<Self>) {
        let state = self.app_state.read(cx);
        if let Some(connected) = state.connections().get(&profile_id).filter(|connected| {
            connected.connection.schema_loading_strategy()
                == dbflux_core::SchemaLoadingStrategy::LazyPerDatabase
        }) {
            let mut scratch = Vec::new();
            build_resource_items_for_connection(
                state,
                profile_id,
                connected,
                &mut self.palette_sync.cache,
                &mut scratch,
            );
        }

        self.flush_palette_writes(cx);
    }

    /// Writes the queued index changes off the UI thread, each batch after
    /// the previous one, so a newer list never lands before an older one.
    fn flush_palette_writes(&mut self, cx: &mut Context<Self>) {
        let writes = self.palette_sync.cache.take_writes();
        if writes.is_empty() {
            return;
        }

        let repo = self.palette_sync.cache.repo();
        let previous = self.palette_sync.last_write.take();
        self.palette_sync.last_write = Some(cx.background_executor().spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            write_palette_objects(&repo, writes);
        }));
    }

    fn schedule_palette_refresh(&mut self, cx: &mut Context<Self>) {
        if self.palette_sync.refresh.is_some() {
            return;
        }

        self.palette_sync.refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_COALESCE).await;

            if let Err(error) = this.update(cx, |this, cx| {
                this.palette_sync.refresh = None;
                this.refresh_open_palette(cx);
            }) {
                log::debug!("The palette refresh outlived its workspace: {error}");
            }
        }));
    }

    /// Lists the databases fetched since the palette opened, keeping what
    /// the user typed, and fetches any database listed since.
    fn refresh_open_palette(&mut self, cx: &mut Context<Self>) {
        if !self.palette_sync.lists_workspace || !self.command_palette.read(cx).is_visible() {
            return;
        }

        let (items, to_fetch) = self.build_palette_items(cx);
        self.command_palette.update(cx, |palette, cx| {
            palette.replace_items(items, cx);
        });
        self.sync_palette_after_build(to_fetch, cx);
    }
}
