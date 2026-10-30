//! The WHERE filters a table was read with, kept per table across restarts.
//!
//! Only filters whose read succeeded are kept: a typo that the database
//! rejected is not worth offering again. The list holds filter text only,
//! never row data, and lives in the UI state store next to the other
//! per-table view settings.

mod render;
#[cfg(test)]
mod tests;

use super::{DataGridPanel, DataSource, TableReload};
use dbflux_components::controls::{InputEvent, InputState};
use dbflux_ui_base::AppStateEntity;
use gpui::{App, AppContext as _, Context, Entity, Focusable as _, ScrollHandle, Window};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::ops::Range;

/// Prefix of the `st_ui_state` keys of table filters. The version lets a
/// later format use new keys instead of misreading old rows.
const TABLE_KEY_PREFIX: &str = dbflux_storage::ui_state_keys::GRID_FILTER_HISTORY_PREFIX;

/// Prefix of the `st_ui_state` keys of document collection queries.
const COLLECTION_KEY_PREFIX: &str = dbflux_storage::ui_state_keys::COLLECTION_QUERY_HISTORY_PREFIX;

/// Filters kept per table.
pub(super) const TABLE_HISTORY_LIMIT: usize = 50;

/// The history popup of the WHERE field.
#[derive(Debug, Default)]
pub(super) struct FilterHistoryPopup {
    pub(super) open: bool,
    /// Entries of the table, newest first, as last read from the store.
    pub(super) entries: Vec<String>,
    /// Highlighted row of the shown list. `shown().len()` is the "Clear
    /// history" row.
    pub(super) selected: Option<usize>,
    /// The text of the popup's search field. Every word of it must occur in
    /// an entry for the entry to be shown.
    pub(super) query: String,
    /// Scrolls the highlighted row into view while the keys walk a list
    /// longer than the popup.
    pub(super) scroll: ScrollHandle,
    /// Where the press that last closed the popup from outside landed, so
    /// the toggle's click from that same press does not reopen it.
    pub(super) dismiss_press: Option<gpui::Point<gpui::Pixels>>,
}

impl FilterHistoryPopup {
    /// Indexes into `entries` of the rows shown, in order.
    pub(super) fn shown(&self) -> Vec<usize> {
        matching_entries(&self.entries, &self.query)
    }

    /// The first shown row, highlighted after the list changes so that Enter
    /// picks the best match, as a shell's reverse search does.
    fn first_shown(&self) -> Option<usize> {
        (!self.shown().is_empty()).then_some(0)
    }
}

/// Puts `entry` first, dropping an older copy and the entries past `limit`.
pub(super) fn record_entry<T: PartialEq>(entries: &mut Vec<T>, entry: T, limit: usize) {
    entries.retain(|existing| *existing != entry);
    entries.insert(0, entry);
    entries.truncate(limit);
}

/// The lowercased words of a search, in the order typed.
pub(super) fn search_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Indexes of the entries that contain every word of `query`, in any order
/// and ignoring case. An empty query matches every entry.
pub(super) fn matching_entries(entries: &[String], query: &str) -> Vec<usize> {
    let words = search_words(query);

    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            let entry = entry.to_lowercase();
            words.iter().all(|word| entry.contains(word.as_str()))
        })
        .map(|(index, _)| index)
        .collect()
}

/// Byte ranges of `text` where one of `words` (lowercased) occurs, sorted
/// and merged, for highlighting a row.
///
/// Lowercasing can change a character's byte length, so the search runs on
/// a lowercased copy and maps each hit back to the characters of `text`
/// that produced it.
pub(super) fn word_ranges(text: &str, words: &[String]) -> Vec<Range<usize>> {
    let mut lowered = String::with_capacity(text.len());
    // For each byte of `lowered`, the start of the character of `text` it
    // came from.
    let mut origin = Vec::with_capacity(text.len());
    for (start, character) in text.char_indices() {
        for lower in character.to_lowercase() {
            let before = lowered.len();
            lowered.push(lower);
            origin.extend(std::iter::repeat_n(start, lowered.len() - before));
        }
    }
    let original_at =
        |lowered_index: usize| origin.get(lowered_index).copied().unwrap_or(text.len());

    let mut ranges: Vec<Range<usize>> = words
        .iter()
        .filter(|word| !word.is_empty())
        .flat_map(|word| {
            lowered
                .match_indices(word.as_str())
                .map(|(start, hit)| original_at(start)..original_at(start + hit.len()))
                .collect::<Vec<_>>()
        })
        .filter(|range| !range.is_empty())
        .collect();
    ranges.sort_by_key(|range| range.start);

    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// The popup's search field. Its keys reach the popup through
/// [`DataGridPanel::render_filter_history_search`]; this wires its events.
pub(super) fn new_history_search(
    window: &mut Window,
    cx: &mut Context<DataGridPanel>,
) -> Entity<InputState> {
    let search = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(dbflux_i18n::t!("document.data.grid.filter.history.search"))
    });

    cx.subscribe_in(
        &search,
        window,
        |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                let text = input.read(cx).value().to_string();
                this.filter_history_search_changed(&text, cx);
            }
            InputEvent::PressEnter {
                secondary: false, ..
            } => {
                this.activate_filter_history(window, cx);
            }
            InputEvent::Blur => this.filter_history_search_blurred(window, cx),
            _ => {}
        },
    )
    .detach();

    search
}

/// A store key: the prefix and the parts as a JSON array, which keeps names
/// holding a separator apart.
fn store_key(prefix: &str, parts: &[String]) -> Option<String> {
    let parts = serde_json::to_string(parts).ok()?;
    Some(format!("{prefix}:{parts}"))
}

/// Reads the list kept under `key`. A missing or unreadable row is an empty
/// history: losing old entries is better than failing to show the grid.
pub(super) fn load_history<T: DeserializeOwned>(
    app_state: &Entity<AppStateEntity>,
    key: &str,
    cx: &App,
) -> Vec<T> {
    let stored = match app_state.read(cx).storage_runtime().ui_state().get(key) {
        Ok(stored) => stored,
        Err(error) => {
            log::warn!("Could not read the filter history of {key}: {error}");
            return Vec::new();
        }
    };

    let Some(stored) = stored else {
        return Vec::new();
    };

    serde_json::from_str(&stored).unwrap_or_else(|error| {
        log::warn!("Ignoring the unreadable filter history of {key}: {error}");
        Vec::new()
    })
}

/// Writes `entries` under `key`; an empty list removes the row.
pub(super) fn save_history<T: Serialize>(
    app_state: &Entity<AppStateEntity>,
    key: &str,
    entries: &[T],
    cx: &App,
) {
    let store = app_state.read(cx).storage_runtime().ui_state();

    let result = if entries.is_empty() {
        store.delete(key)
    } else {
        match serde_json::to_string(entries) {
            Ok(json) => store.set(key, &json),
            Err(error) => {
                log::error!("Could not encode the filter history of {key}: {error}");
                return;
            }
        }
    };

    if let Err(error) = result {
        log::error!("Could not save the filter history of {key}: {error}");
    }
}

impl DataGridPanel {
    /// The store key of this grid's filters: profile, database, schema and
    /// table. `None` for sources that are not a table.
    pub(super) fn filter_history_key(&self) -> Option<String> {
        let DataSource::Table {
            profile_id,
            database,
            table,
            ..
        } = &self.source
        else {
            return None;
        };

        store_key(
            TABLE_KEY_PREFIX,
            &[
                profile_id.to_string(),
                database.clone().unwrap_or_default(),
                table.schema.clone().unwrap_or_default(),
                table.name.clone(),
            ],
        )
    }

    /// The store key of a document collection's query history.
    pub(super) fn collection_history_key(&self) -> Option<String> {
        let DataSource::Collection {
            profile_id,
            collection,
            ..
        } = &self.source
        else {
            return None;
        };

        store_key(
            COLLECTION_KEY_PREFIX,
            &[
                profile_id.to_string(),
                collection.database.clone(),
                collection.name.clone(),
            ],
        )
    }

    /// Keeps `filter` after the read it ran with succeeded. Read back from
    /// the store first: another tab on the same table may have added
    /// entries since this one last looked.
    pub(super) fn record_filter_history(&mut self, filter: Option<String>, cx: &mut Context<Self>) {
        let Some(filter) = filter.map(|text| text.trim().to_string()) else {
            return;
        };
        if filter.is_empty() {
            return;
        }
        let Some(key) = self.filter_history_key() else {
            return;
        };

        let mut entries: Vec<String> = load_history(&self.app_state, &key, cx);
        record_entry(&mut entries, filter, TABLE_HISTORY_LIMIT);
        save_history(&self.app_state, &key, &entries, cx);
        self.filter_bar.history.entries = entries;
    }

    /// Whether this grid keeps a filter history: a table whose WHERE field
    /// is shown.
    pub(super) fn offers_filter_history(&self) -> bool {
        self.filter_history_key().is_some() && self.filter_input_visible()
    }

    /// Opens the history popup under the WHERE field with an empty search,
    /// the keyboard in the search field and the newest entry highlighted.
    pub(super) fn open_filter_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.filter_history_key() else {
            return;
        };

        let popup = &mut self.filter_bar.history;
        popup.entries = load_history(&self.app_state, &key, cx);
        popup.open = true;
        popup.query.clear();
        popup.selected = popup.first_shown();
        popup.scroll.scroll_to_item(0);

        self.filter_bar
            .history_search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.focus_filter_history_search(window, cx);
        cx.notify();
    }

    pub(super) fn close_filter_history(&mut self, cx: &mut Context<Self>) {
        if self.filter_bar.history.open {
            self.filter_bar.history.open = false;
            cx.notify();
        }
    }

    /// Closes the popup from its search field and hands the keyboard back to
    /// the WHERE field, which the popup belongs to.
    pub(super) fn dismiss_filter_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_filter_history(cx);
        self.focus_filter_input(window, cx);
    }

    /// Ctrl+R (Cmd+R on macOS) in the WHERE field or the popup's search: opens
    /// the popup, or, while it is open, moves to the next older match, like a
    /// shell's reverse search. Elsewhere the key is not the popup's.
    pub(super) fn search_filter_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.offers_filter_history() {
            return false;
        }

        let in_search = self.filter_history_search_focused(window, cx);
        let in_filter = self
            .filter_bar
            .filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        if !in_search && !in_filter {
            return false;
        }

        if self.filter_bar.history.open {
            self.step_filter_history(true, cx);
            if !in_search {
                self.focus_filter_history_search(window, cx);
            }
        } else {
            self.open_filter_history(window, cx);
        }
        true
    }

    /// Escape in the search field: clears a search first, so a second
    /// Escape is needed to close the popup.
    pub(super) fn filter_history_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_bar.history.query.is_empty() {
            self.dismiss_filter_history(window, cx);
            return;
        }

        self.filter_bar
            .history_search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.filter_history_search_changed("", cx);
    }

    /// The list follows the search field: it narrows to the entries holding
    /// every word, and the best (newest) match is highlighted.
    pub(super) fn filter_history_search_changed(&mut self, text: &str, cx: &mut Context<Self>) {
        let popup = &mut self.filter_bar.history;
        if popup.query == text {
            return;
        }

        popup.query = text.to_string();
        popup.selected = popup.first_shown();
        popup.scroll.scroll_to_item(0);
        cx.notify();
    }

    /// The search field lost the keyboard. Going back to the WHERE field
    /// keeps the grid in edit mode (the popup closes on that click anyway);
    /// anywhere else the popup closes and the grid leaves edit mode, as when
    /// the WHERE field loses it.
    fn filter_history_search_blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to_filter = self
            .filter_bar
            .filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        if to_filter {
            return;
        }

        self.close_filter_history(cx);
        self.exit_edit_mode(window, cx);
    }

    pub(super) fn filter_history_search_focused(&self, window: &Window, cx: &App) -> bool {
        self.filter_bar
            .history_search
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }

    /// Gives the popup's search field the keyboard. The grid stays in the
    /// toolbar's edit mode, so typed letters are text and not grid keys.
    fn focus_filter_history_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use super::{EditState, GridFocusMode, ToolbarFocus};

        self.focus.focus_mode = GridFocusMode::Toolbar;
        self.focus.toolbar_focus = ToolbarFocus::Filter;
        self.focus.edit_state = EditState::Editing;
        self.filter_bar
            .history_search
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Moves the highlight one row down or up, wrapping, over the shown
    /// entries and the "Clear history" row after them.
    pub(super) fn step_filter_history(&mut self, forward: bool, cx: &mut Context<Self>) {
        let popup = &mut self.filter_bar.history;
        let rows = popup.shown().len() + usize::from(!popup.entries.is_empty());
        if rows == 0 {
            return;
        }

        let row = match (popup.selected, forward) {
            (None, true) => 0,
            (None, false) => rows - 1,
            (Some(row), true) => (row + 1) % rows,
            (Some(row), false) => (row + rows - 1) % rows,
        };
        popup.selected = Some(row);
        // The list holds the shown entries, then a separator, then "Clear
        // history".
        let child = if row < popup.shown().len() {
            row
        } else {
            row + 1
        };
        popup.scroll.scroll_to_item(child);
        cx.notify();
    }

    /// Enter while the popup is open: runs the highlighted row, or the only
    /// match when nothing is highlighted. Returns false when there is
    /// neither.
    pub(super) fn activate_filter_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let popup = &self.filter_bar.history;
        if !popup.open {
            return false;
        }
        let shown = popup.shown();
        let Some(row) = popup.selected.or_else(|| (shown.len() == 1).then_some(0)) else {
            return false;
        };

        match shown.get(row) {
            Some(&index) => self.apply_filter_history_entry(index, window, cx),
            None => {
                self.clear_filter_history(cx);
                // The search field goes with the popup.
                self.focus_filter_input(window, cx);
            }
        }
        true
    }

    /// Puts entry `index` into the WHERE field and reads the table with it.
    pub(super) fn apply_filter_history_entry(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.filter_bar.history.entries.get(index).cloned() else {
            return;
        };

        self.close_filter_history(cx);
        self.filter_bar
            .filter_input
            .update(cx, |input, cx| input.set_value(&entry, window, cx));
        self.apply_filter_input(window, cx);
    }

    /// Removes entry `index` from the table's history (Shift+Delete, or the
    /// × of its row).
    pub(super) fn delete_filter_history_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(key) = self.filter_history_key() else {
            return;
        };

        let popup = &mut self.filter_bar.history;
        if index >= popup.entries.len() {
            return;
        }
        popup.entries.remove(index);
        save_history(&self.app_state, &key, &self.filter_bar.history.entries, cx);

        let popup = &mut self.filter_bar.history;
        let rows = popup.shown().len();
        popup.selected = match popup.selected {
            _ if rows == 0 => None,
            Some(row) => Some(row.min(rows - 1)),
            None => None,
        };
        if popup.entries.is_empty() {
            popup.open = false;
        }
        cx.notify();
    }

    /// Shift+Delete in the open popup: removes the highlighted entry.
    pub(super) fn delete_selected_filter_history(&mut self, cx: &mut Context<Self>) -> bool {
        let popup = &self.filter_bar.history;
        let Some(index) = popup
            .selected
            .filter(|_| popup.open)
            .and_then(|row| popup.shown().get(row).copied())
        else {
            return false;
        };

        self.delete_filter_history_entry(index, cx);
        true
    }

    /// Forgets every filter of the table.
    pub(super) fn clear_filter_history(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.filter_history_key() else {
            return;
        };

        self.filter_bar.history = FilterHistoryPopup::default();
        save_history::<String>(&self.app_state, &key, &[], cx);
        cx.notify();
    }

    /// Up / Down in the WHERE field. They open the popup when the field is
    /// empty or the caret is at its start, where they would otherwise do
    /// nothing, and walk it if it is still open. An open completion menu
    /// keeps them.
    pub(super) fn filter_history_vertical_key(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.offers_filter_history() {
            return false;
        }

        let input = self.filter_bar.filter_input.read(cx);
        if input.completion_menu_state().open {
            return false;
        }
        let at_start = input.value().is_empty() || input.cursor() == 0;

        if self.filter_bar.history.open {
            self.step_filter_history(forward, cx);
            return true;
        }

        if !at_start {
            return false;
        }

        self.open_filter_history(window, cx);
        true
    }

    /// Enter in the WHERE field: the highlighted history entry when the
    /// popup is open, otherwise the typed filter.
    pub(super) fn submit_filter_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.activate_filter_history(window, cx) {
            self.apply_filter_input(window, cx);
        }
    }

    /// Reads the table with the filter in the WHERE field: Enter, or a
    /// history entry picked.
    pub(super) fn apply_filter_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reload_blocked_by_pending_edits(cx) {
            return;
        }

        self.close_filter_history(cx);
        // A new filter selects a different row set.
        self.grid_table.reload = TableReload::ResetRows;
        self.refresh(window, cx);
        self.focus_table(window, cx);
    }

    /// Gives the WHERE field the keyboard, the way clicking it does.
    pub(super) fn focus_filter_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use super::{EditState, GridFocusMode, ToolbarFocus};

        self.focus.focus_mode = GridFocusMode::Toolbar;
        self.focus.toolbar_focus = ToolbarFocus::Filter;
        self.focus.edit_state = EditState::Editing;
        self.filter_bar
            .filter_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Loads a document collection's query history for its history menu.
    pub(super) fn load_collection_query_history(&mut self, cx: &App) {
        if let Some(key) = self.collection_history_key() {
            self.collection.history = load_history(&self.app_state, &key, cx);
        }
    }

    /// Writes a document collection's query history after a run added to it.
    pub(super) fn save_collection_query_history(&self, cx: &App) {
        if let Some(key) = self.collection_history_key() {
            save_history(&self.app_state, &key, &self.collection.history, cx);
        }
    }
}
