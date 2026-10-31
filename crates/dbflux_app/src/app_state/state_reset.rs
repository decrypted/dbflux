//! Settings → Reset on the running app: the storage reset, plus the
//! in-memory copies and pending writes that would otherwise bring the
//! cleared state straight back.

use dbflux_core::GeneralSettings;
use dbflux_storage::error::StorageError;
use dbflux_storage::{ResetCategory, ResetStats};

use super::AppState;

/// Writes the app holds back for the rest of the run, because the state they
/// would write was just reset and is meant to stay gone until the restart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PersistenceHold {
    /// The open tabs (session manifest) and the connected profiles at quit.
    session: bool,
    /// The main window's geometry at quit.
    window_geometry: bool,
}

impl AppState {
    /// What `category` holds in storage right now.
    pub fn stored_state_stats(&self, category: ResetCategory) -> Result<ResetStats, StorageError> {
        self.storage_runtime.state_reset().stats(category)
    }

    /// Clears `categories` in one transaction and brings the running app in
    /// line: the query history and recent files it holds in memory are
    /// reloaded, the general settings fall back to their defaults, and the
    /// writes at quit that would restore the session or the main window size
    /// are held back. Returns how many keys or rows were removed.
    ///
    /// Window sizes the UI keeps in its own globals (the cell editor) are
    /// the caller's to forget, since this crate has no window.
    pub fn reset_stored_state(
        &mut self,
        categories: &[ResetCategory],
    ) -> Result<u64, StorageError> {
        let removed = self.storage_runtime.state_reset().reset_all(categories)?;

        if categories.iter().any(|category| {
            matches!(
                category,
                ResetCategory::QueryHistory | ResetCategory::RecentFiles
            )
        }) {
            self.history_manager =
                crate::history_manager_sqlite::HistoryManager::new(&self.storage_runtime);
            self.history_manager
                .set_max_entries(self.general_settings.max_history_entries);
        }

        if categories.contains(&ResetCategory::GeneralSettings) {
            self.update_general_settings(GeneralSettings::default());
        }

        if categories.contains(&ResetCategory::Session) {
            self.persistence_hold.session = true;
        }

        if categories.contains(&ResetCategory::WindowLayout) {
            self.persistence_hold.window_geometry = true;
        }

        Ok(removed)
    }

    /// Whether the open tabs must not be written as the session to restore,
    /// because the session was reset during this run.
    pub fn session_persistence_held(&self) -> bool {
        self.persistence_hold.session
    }

    /// Whether the main window geometry must not be stored at quit, because
    /// the window sizes were reset during this run.
    pub fn window_geometry_persistence_held(&self) -> bool {
        self.persistence_hold.window_geometry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbflux_core::{HistoryEntry, ThemeSetting};
    use dbflux_storage::bootstrap::StorageRuntime;

    fn state() -> AppState {
        AppState::new_with_storage_runtime(StorageRuntime::in_memory().expect("storage runtime"))
            .expect("app state")
    }

    fn history_entry(sql: &str) -> HistoryEntry {
        HistoryEntry::new(
            sql.to_string(),
            None,
            None,
            std::time::Duration::from_millis(3),
            Some(1),
        )
    }

    #[test]
    fn resetting_the_query_history_empties_the_in_memory_list_too() {
        let mut state = state();
        state.add_history_entry(history_entry("SELECT 1"));
        state.add_history_entry(history_entry("SELECT 2"));
        assert_eq!(state.history_entries().len(), 2);

        let removed = state
            .reset_stored_state(&[ResetCategory::QueryHistory])
            .expect("reset");

        assert_eq!(removed, 2);
        assert!(state.history_entries().is_empty());
        assert_eq!(
            state
                .stored_state_stats(ResetCategory::QueryHistory)
                .expect("stats")
                .count,
            0
        );
    }

    #[test]
    fn resetting_the_general_settings_restores_the_defaults_in_memory() {
        let mut state = state();
        let mut settings = state.general_settings().clone();
        settings.theme = ThemeSetting::Light;
        crate::config_loader::save_general_settings(state.storage_runtime(), &settings)
            .expect("save settings");
        state.update_general_settings(settings);

        state
            .reset_stored_state(&[ResetCategory::GeneralSettings])
            .expect("reset");

        assert_eq!(
            state.general_settings().theme,
            GeneralSettings::default().theme
        );
        assert_eq!(
            state
                .stored_state_stats(ResetCategory::GeneralSettings)
                .expect("stats")
                .count,
            0
        );
    }

    #[test]
    fn session_and_window_resets_hold_back_the_writes_at_quit() {
        let mut state = state();
        assert!(!state.session_persistence_held());
        assert!(!state.window_geometry_persistence_held());

        state
            .reset_stored_state(&[ResetCategory::ColumnLayouts])
            .expect("reset");
        assert!(!state.session_persistence_held());
        assert!(!state.window_geometry_persistence_held());

        state
            .reset_stored_state(&ResetCategory::BULK)
            .expect("reset");
        assert!(state.session_persistence_held());
        assert!(state.window_geometry_persistence_held());
    }

    #[test]
    fn a_held_session_does_not_record_the_connected_profiles() {
        let mut state = state();
        state
            .storage_runtime()
            .ui_state()
            .set_last_connected_profiles(&[uuid::Uuid::new_v4()])
            .expect("seed");

        state
            .reset_stored_state(&[ResetCategory::Session])
            .expect("reset");
        state.remember_connected_profiles();

        assert!(state.last_connected_profiles().is_empty());
    }
}
