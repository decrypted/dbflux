//! Keeps the workspace palette's database lists current: fetches the
//! databases it can list, a few at a time, reads and saves the stored lists
//! off the UI thread, and refills an open palette as results land.

use super::{
    PALETTE_FETCHES_IN_FLIGHT, PaletteFetchQueue, PaletteObjectCache, load_palette_objects,
    remember_database, write_palette_objects,
};
use crate::ui::views::workspace::Workspace;
use dbflux_storage::{IndexedDatabase, PaletteObjectIndexRepo};
use dbflux_ui_base::object_tree::{
    ObjectTreeEvent, ObjectTreeOutcome, ObjectTreeRequestKey, ObjectTreeRequestStatus,
};
use gpui::{Context, Task};
use std::collections::HashSet;
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

    /// Drops the databases not fetched yet once the palette closes; the
    /// fetches already running still settle and are kept.
    pub(in crate::ui::views::workspace) fn stop_fetching(&mut self) {
        self.fetches.clear_queued();
    }
}

impl Workspace {
    /// Queues the databases the workspace palette could list, reads the
    /// stored lists it asked for, saves the lists it built, and starts
    /// fetching.
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
        self.load_stored_palette_lists(cx);
        self.flush_palette_writes(cx);
        self.pump_palette_fetches(cx);
    }

    /// Reads the stored lists of each profile the palette listed for the
    /// first time, off the UI thread: a background index write holds the
    /// repository's connection while it runs. The palette shows what it has
    /// meanwhile and refills once the lists arrive.
    fn load_stored_palette_lists(&mut self, cx: &mut Context<Self>) {
        for profile_id in self.palette_sync.cache.take_loads() {
            let repo = self.palette_sync.cache.repo();
            cx.spawn(async move |this, cx| {
                let stored = cx
                    .background_executor()
                    .spawn(async move { load_palette_objects(&repo, profile_id) })
                    .await;

                if let Err(error) = this.update(cx, |this, cx| {
                    this.on_stored_palette_lists(profile_id, stored, cx);
                }) {
                    log::debug!("The palette's index read outlived its workspace: {error}");
                }
            })
            .detach();
        }
    }

    fn on_stored_palette_lists(
        &mut self,
        profile_id: Uuid,
        stored: Option<Vec<IndexedDatabase>>,
        cx: &mut Context<Self>,
    ) {
        if !self.palette_sync.cache.finish_load(profile_id, stored) {
            return;
        }

        // The stored lists can name databases the server dropped since.
        self.forget_unlisted_palette_databases(profile_id, cx);
        self.flush_palette_writes(cx);
        self.refresh_open_palette(cx);
    }

    pub(in crate::ui::views::workspace) fn on_palette_object_tree_event(
        &mut self,
        event: &ObjectTreeEvent,
        cx: &mut Context<Self>,
    ) {
        let ours = self.palette_sync.fetches.settled(&event.key);

        match (&event.key, &event.outcome) {
            (
                ObjectTreeRequestKey::DatabaseSchema {
                    profile_id,
                    database,
                },
                outcome,
            ) => match outcome {
                ObjectTreeOutcome::Applied => {
                    self.remember_palette_database(*profile_id, database, cx);
                    self.schedule_palette_refresh(cx);
                }
                ObjectTreeOutcome::Failed(_) if ours => {
                    self.palette_sync.cache.mark_fetch_failed(
                        *profile_id,
                        database,
                        self.app_state
                            .read(cx)
                            .profile_session_generation(*profile_id),
                    );
                }
                _ => {}
            },
            (ObjectTreeRequestKey::DatabaseList { profile_id }, ObjectTreeOutcome::Applied) => {
                self.forget_unlisted_palette_databases(*profile_id, cx);
                self.schedule_palette_refresh(cx);
            }
            _ => {}
        }

        if ours {
            self.pump_palette_fetches(cx);
        }
    }

    fn pump_palette_fetches(&mut self, cx: &mut Context<Self>) {
        while let Some(key) = self.palette_sync.fetches.next(PALETTE_FETCHES_IN_FLIGHT) {
            let status = self.app_state.update(cx, |state, cx| {
                state.object_tree_request_in_background(key.clone(), cx)
            });

            match status {
                ObjectTreeRequestStatus::Cached => self.palette_sync.fetches.finished(&key),
                ObjectTreeRequestStatus::Failed(_) => {
                    self.palette_sync.fetches.finished(&key);
                    if let ObjectTreeRequestKey::DatabaseSchema {
                        profile_id,
                        database,
                    } = &key
                    {
                        self.palette_sync.cache.mark_fetch_failed(
                            *profile_id,
                            database,
                            self.app_state
                                .read(cx)
                                .profile_session_generation(*profile_id),
                        );
                    }
                }
                ObjectTreeRequestStatus::Dispatched
                | ObjectTreeRequestStatus::Pending
                | ObjectTreeRequestStatus::WaitingForSlotInstall => {}
            }
        }
    }

    /// Records the list of one fetched database, even with no palette open.
    fn remember_palette_database(
        &mut self,
        profile_id: Uuid,
        database: &str,
        cx: &mut Context<Self>,
    ) {
        let state = self.app_state.read(cx);
        if let Some(connected) = state.connections().get(&profile_id) {
            remember_database(
                state,
                profile_id,
                connected,
                database,
                &mut self.palette_sync.cache,
            );
        }

        self.flush_palette_writes(cx);
    }

    fn forget_unlisted_palette_databases(&mut self, profile_id: Uuid, cx: &mut Context<Self>) {
        if !self.palette_sync.cache.has_profile(profile_id) {
            return;
        }

        let state = self.app_state.read(cx);
        if let Some(list) = state.get_database_list(profile_id) {
            let listed: HashSet<&str> = list.iter().map(|entry| entry.name.as_str()).collect();
            self.palette_sync.cache.forget_unlisted(profile_id, &listed);
        }

        self.flush_palette_writes(cx);
    }

    /// Writes the queued index changes off the UI thread, each batch after
    /// the previous one, so a newer list never lands before an older one. A
    /// list that failed to save is dropped from memory so it is written
    /// again next time.
    fn flush_palette_writes(&mut self, cx: &mut Context<Self>) {
        let writes = self.palette_sync.cache.take_writes();
        if writes.is_empty() {
            return;
        }

        let repo = self.palette_sync.cache.repo();
        let previous = self.palette_sync.last_write.take();
        self.palette_sync.last_write = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }

            let failed = cx
                .background_executor()
                .spawn(async move { write_palette_objects(&repo, writes) })
                .await;
            if failed.is_empty() {
                return;
            }

            if let Err(error) = this.update(cx, |this, _| {
                for (profile_id, database) in failed {
                    this.palette_sync
                        .cache
                        .forget_unsaved(profile_id, &database);
                }
            }) {
                log::debug!("The palette's index writes outlived their workspace: {error}");
            }
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
