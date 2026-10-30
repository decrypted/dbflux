use dbflux_components::components::data_table::SelectionTotals;
use gpui::App;

use super::DataGridPanel;

impl DataGridPanel {
    /// The footer's summary of a selection of several cells: how many, and
    /// for the numbers among them their sum, average, minimum and maximum.
    pub(super) fn selection_totals_footer(&self, cx: &App) -> Option<String> {
        let totals = self
            .grid_table
            .table_state
            .as_ref()?
            .read(cx)
            .selection_totals()?;
        Some(selection_totals_label(&totals))
    }
}

pub(super) fn selection_totals_label(totals: &SelectionTotals) -> String {
    let mut parts = vec![dbflux_i18n::t!(
        "document.data.grid.status.selection.cells",
        count = totals.cells
    )];

    let Some(numeric) = &totals.numeric else {
        return parts.join(" · ");
    };

    parts.push(dbflux_i18n::t!(
        "document.data.grid.status.selection.sum",
        value = numeric.sum_text()
    ));
    parts.push(dbflux_i18n::t!(
        "document.data.grid.status.selection.average",
        value = numeric.average_text()
    ));
    parts.push(dbflux_i18n::t!(
        "document.data.grid.status.selection.min",
        value = numeric.min_text()
    ));
    parts.push(dbflux_i18n::t!(
        "document.data.grid.status.selection.max",
        value = numeric.max_text()
    ));

    // The average is over the numbers only, so say what it left out.
    if totals.nulls > 0 {
        parts.push(skipped_label(totals.nulls, SkippedKind::Null));
    }
    let non_numeric = totals.non_numeric();
    if non_numeric > 0 {
        parts.push(skipped_label(non_numeric, SkippedKind::NonNumeric));
    }

    parts.join(" · ")
}

enum SkippedKind {
    Null,
    NonNumeric,
}

/// The singular catalog bucket for exactly one skipped cell, the plural one
/// for every other count, as `pending_change_count_label` does.
fn skipped_label(count: usize, kind: SkippedKind) -> String {
    match (kind, count == 1) {
        (SkippedKind::Null, true) => dbflux_i18n::t!(
            "document.data.grid.status.selection.nulls_skipped.one",
            count = count
        ),
        (SkippedKind::Null, false) => dbflux_i18n::t!(
            "document.data.grid.status.selection.nulls_skipped.many",
            count = count
        ),
        (SkippedKind::NonNumeric, true) => dbflux_i18n::t!(
            "document.data.grid.status.selection.non_numeric_skipped.one",
            count = count
        ),
        (SkippedKind::NonNumeric, false) => dbflux_i18n::t!(
            "document.data.grid.status.selection.non_numeric_skipped.many",
            count = count
        ),
    }
}
