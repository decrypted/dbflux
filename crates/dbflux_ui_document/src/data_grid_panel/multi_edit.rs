use gpui::App;

use super::DataGridPanel;

impl DataGridPanel {
    /// The footer's note while the open editor writes to several cells, so
    /// the user sees how many a commit changes before making it.
    pub(super) fn multi_edit_footer(&self, cx: &App) -> Option<String> {
        let count = self
            .grid_table
            .table_state
            .as_ref()?
            .read(cx)
            .multi_edit_count();
        (count > 1).then(|| dbflux_i18n::t!("document.data.grid.status.multi_edit", count = count))
    }
}
