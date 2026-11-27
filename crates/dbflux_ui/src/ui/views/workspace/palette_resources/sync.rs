//! Keeps the workspace palette's database lists current: fetches the
//! databases it can list, a few at a time and only while it is open, reads
//! and saves the stored lists off the UI thread, and refills an open palette
//! as results land.

use super::{
    PALETTE_FETCHES_IN_FLIGHT, PALETTE_FETCHES_PER_OPEN, PaletteFetchQueue, PaletteObjectCache,
    build_resource_items_for_connection, database_items, item_owner, load_palette_objects,
    remember_database, write_palette_objects,
};
use crate::ui::views::workspace::Workspace;
use dbflux_core::SchemaLoadingStrategy;
use dbflux_storage::{IndexedDatabase, PaletteObjectIndexRepo};
use dbflux_ui_base::object_tree::{
    ObjectTreeEvent, ObjectTreeOutcome, ObjectTreeRequestKey, ObjectTreeRequestStatus,
};
use gpui::{Context, Task};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Changes landing within this window refill an open palette once.
const REFRESH_COALESCE: Duration = Duration::from_millis(150);

/// What changed since the open palette was last filled.
#[derive(Default)]
struct StaleItems {
    /// Profiles whose items are listed again from scratch.
    profiles: HashSet<Uuid>,
    /// Single databases of lazily loaded profiles whose items are listed
    /// again, leaving the rest of the profile as it is.
    databases: HashSet<(Uuid, String)>,
}

impl StaleItems {
    fn is_empty(&self) -> bool {
        self.profiles.is_empty() && self.databases.is_empty()
    }
}

/// What the workspace palette keeps between opens.
pub(in crate::ui::views::workspace) struct PaletteSync {
    pub(in crate::ui::views::workspace) cache: PaletteObjectCache,
    fetches: PaletteFetchQueue,
    /// Fetches started since the palette opened, against
    /// [`PALETTE_FETCHES_PER_OPEN`].
    fetches_this_open: usize,
    /// Whether the open palette lists the workspace's items, and not a
    /// narrower list such as the saved charts.
    lists_workspace: bool,
    stale: StaleItems,
    refresh: Option<Task<()>>,
    /// The last of the index reads and writes, which run one after another:
    /// a list is never written before an older one, and a profile's rows
    /// are read only after the writes and deletes queued before. Dropped
    /// with the workspace on quit; the index then trails the last lists by
    /// one write, which the next session's fetches make up.
    last_write: Option<Task<()>>,
    /// Full listings of every profile, counted for tests that check a
    /// fetched database does not rebuild the rest.
    #[cfg(test)]
    pub(in crate::ui::views::workspace) full_listings: usize,
}

impl PaletteSync {
    pub(in crate::ui::views::workspace) fn new(repo: Arc<PaletteObjectIndexRepo>) -> Self {
        Self {
            cache: PaletteObjectCache::new(repo),
            fetches: PaletteFetchQueue::default(),
            fetches_this_open: 0,
            lists_workspace: false,
            stale: StaleItems::default(),
            refresh: None,
            last_write: None,
            #[cfg(test)]
            full_listings: 0,
        }
    }

    /// Records whether the palette about to open lists the workspace's
    /// items, and starts its count of fetches.
    pub(in crate::ui::views::workspace) fn set_lists_workspace(&mut self, lists_workspace: bool) {
        self.lists_workspace = lists_workspace;
        self.fetches_this_open = 0;
        self.stale = StaleItems::default();
    }

    /// Drops the databases not fetched yet once the palette closes, however
    /// it closes; the fetches already running still settle and are kept.
    pub(in crate::ui::views::workspace) fn stop_fetching(&mut self) {
        self.fetches.clear_queued();
        self.stale = StaleItems::default();
    }

    #[cfg(test)]
    pub(in crate::ui::views::workspace) fn fetches_in_flight(&self) -> usize {
        self.fetches.in_flight()
    }

    #[cfg(test)]
    pub(in crate::ui::views::workspace) fn fetches_queued(&self) -> usize {
        self.fetches.queued()
    }
}

impl Workspace {
    /// Whether the palette is open on the workspace's items.
    fn palette_lists_workspace(&self, cx: &Context<Self>) -> bool {
        self.palette_sync.lists_workspace && self.command_palette.read(cx).is_visible()
    }

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

    /// Runs `work` off the UI thread after the index reads and writes
    /// queued before it.
    fn after_palette_writes(
        &mut self,
        work: impl AsyncFnOnce(gpui::WeakEntity<Self>, &mut gpui::AsyncApp) + 'static,
        cx: &mut Context<Self>,
    ) {
        let previous = self.palette_sync.last_write.take();
        self.palette_sync.last_write = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            work(this, cx).await;
        }));
    }

    /// Reads the stored lists of each profile the palette listed for the
    /// first time, off the UI thread: a background index write holds the
    /// repository's connection while it runs. The palette shows what it has
    /// meanwhile and refills once the lists arrive.
    fn load_stored_palette_lists(&mut self, cx: &mut Context<Self>) {
        for profile_id in self.palette_sync.cache.take_loads() {
            let repo = self.palette_sync.cache.repo();
            self.after_palette_writes(
                async move |this, cx| {
                    let stored = cx
                        .background_executor()
                        .spawn(async move { load_palette_objects(&repo, profile_id) })
                        .await;

                    if let Err(error) = this.update(cx, |this, cx| {
                        this.on_stored_palette_lists(profile_id, stored, cx);
                    }) {
                        log::debug!("The palette's index read outlived its workspace: {error}");
                    }
                },
                cx,
            );
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
        self.palette_sync.stale.profiles.insert(profile_id);
        self.schedule_palette_refresh(cx);
    }

    /// Drops what the palette knows of profiles that were deleted or now
    /// point at another server, and deletes their stored rows after the
    /// index writes already queued, so none of those writes brings them
    /// back.
    pub(in crate::ui::views::workspace) fn forget_stale_palette_profiles(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let profiles = self
            .app_state
            .update(cx, |state, _| state.take_palette_profiles_to_forget());
        if profiles.is_empty() {
            return;
        }

        for profile_id in &profiles {
            self.palette_sync.cache.forget_profile(*profile_id);
        }

        let repo = self.palette_sync.cache.repo();
        self.after_palette_writes(
            async move |_, cx| {
                cx.background_executor()
                    .spawn(async move {
                        for profile_id in profiles {
                            if let Err(error) = repo.delete_profile(&profile_id.to_string()) {
                                log::warn!(
                                    "Could not delete the palette's objects of a profile: {error}"
                                );
                            }
                        }
                    })
                    .await;
            },
            cx,
        );
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
                    self.mark_palette_database_stale(*profile_id, database, cx);
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
                self.palette_sync.stale.profiles.insert(*profile_id);
                self.schedule_palette_refresh(cx);
            }
            _ => {}
        }

        if ours {
            self.pump_palette_fetches(cx);
        }
    }

    /// Starts queued fetches while the palette is open on the workspace's
    /// items, a few at a time and at most [`PALETTE_FETCHES_PER_OPEN`] per
    /// open. Once it is closed, by any path, the queue is dropped instead.
    fn pump_palette_fetches(&mut self, cx: &mut Context<Self>) {
        if !self.palette_lists_workspace(cx) {
            self.palette_sync.stop_fetching();
            return;
        }

        while self.palette_sync.fetches_this_open < PALETTE_FETCHES_PER_OPEN
            && let Some(key) = self.palette_sync.fetches.next(PALETTE_FETCHES_IN_FLIGHT)
        {
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
                | ObjectTreeRequestStatus::WaitingForSlotInstall => {
                    self.palette_sync.fetches_this_open += 1;
                }
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

    /// Marks the items of a database that loaded for listing again. On a
    /// lazily loaded profile only that database is listed again; elsewhere
    /// a database can change how the whole profile is listed.
    fn mark_palette_database_stale(
        &mut self,
        profile_id: Uuid,
        database: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.palette_lists_workspace(cx) {
            return;
        }

        let lazy = self
            .app_state
            .read(cx)
            .connections()
            .get(&profile_id)
            .is_some_and(|connected| {
                connected.connection.schema_loading_strategy()
                    == SchemaLoadingStrategy::LazyPerDatabase
            });
        if lazy {
            self.palette_sync
                .stale
                .databases
                .insert((profile_id, database.to_string()));
        } else {
            self.palette_sync.stale.profiles.insert(profile_id);
        }

        self.schedule_palette_refresh(cx);
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

    /// Writes the queued index changes off the UI thread, after the reads
    /// and writes queued before. A list that failed to save is dropped from
    /// memory so it is written again next time.
    fn flush_palette_writes(&mut self, cx: &mut Context<Self>) {
        let writes = self.palette_sync.cache.take_writes();
        if writes.is_empty() {
            return;
        }

        let repo = self.palette_sync.cache.repo();
        self.after_palette_writes(
            async move |this, cx| {
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
            },
            cx,
        );
    }

    /// Refills the open palette once the changes of the next few moments
    /// have landed, so a burst of fetches costs one refill.
    fn schedule_palette_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.palette_lists_workspace(cx) {
            self.palette_sync.stale = StaleItems::default();
            return;
        }
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

    /// Lists again only what changed since the palette was filled: the
    /// profiles and databases marked stale. The other items, the typed
    /// query and the selected item stay. Databases listed since are
    /// fetched too.
    fn refresh_open_palette(&mut self, cx: &mut Context<Self>) {
        let stale = std::mem::take(&mut self.palette_sync.stale);
        if stale.is_empty() || !self.palette_lists_workspace(cx) {
            return;
        }

        let state = self.app_state.read(cx);
        let mut fresh = Vec::new();
        let mut to_fetch = Vec::new();
        for &profile_id in &stale.profiles {
            if let Some(connected) = state.connections().get(&profile_id) {
                to_fetch.extend(
                    build_resource_items_for_connection(
                        state,
                        profile_id,
                        connected,
                        &mut self.palette_sync.cache,
                        &mut fresh,
                    )
                    .into_iter()
                    .map(|database| (profile_id, database)),
                );
            }
        }

        // A database keeps its items until it has new ones to show.
        let mut databases: HashMap<Uuid, HashSet<String>> = HashMap::new();
        for (profile_id, database) in stale.databases {
            if stale.profiles.contains(&profile_id) {
                continue;
            }
            if let Some(items) = state
                .connections()
                .get(&profile_id)
                .and_then(|connected| database_items(state, profile_id, connected, &database))
            {
                fresh.extend(items);
                databases.entry(profile_id).or_default().insert(database);
            }
        }

        let profiles = stale.profiles;
        self.command_palette.update(cx, |palette, cx| {
            palette.splice_items(
                |item| {
                    item_owner(item).is_some_and(|(profile_id, database)| {
                        profiles.contains(&profile_id)
                            || database.is_some_and(|database| {
                                databases
                                    .get(&profile_id)
                                    .is_some_and(|stale| stale.contains(database))
                            })
                    })
                },
                fresh,
                cx,
            );
        });
        self.sync_palette_after_build(to_fetch, cx);
    }
}
