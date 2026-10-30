use super::isolated_test_app_state;
use crate::data_grid_panel::DataGridPanel;
use dbflux_components::components::data_table::model::CellValue;
use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_components::theme;
use dbflux_core::{ColumnKind, ColumnMeta, QueryResult, Value};
use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};
use std::sync::Arc;
use std::time::Duration;

fn column(name: &str, type_name: &str, kind: ColumnKind) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        type_name: type_name.to_string(),
        kind,
        nullable: true,
        is_primary_key: false,
    }
}

/// `amount` holds 10, 20, NULL and 5; `name` holds text.
pub(super) fn amounts_grid(
    cx: &mut TestAppContext,
) -> (gpui::Entity<DataGridPanel>, &mut VisualTestContext) {
    cx.update(theme::init);
    let app_state = isolated_test_app_state(cx);
    let rows = [Some(10), Some(20), None, Some(5)]
        .into_iter()
        .enumerate()
        .map(|(index, amount)| {
            vec![
                amount.map_or(Value::Null, Value::Int),
                Value::Text(format!("row {index}")),
            ]
        })
        .collect();
    let result = QueryResult::table(
        vec![
            column("amount", "int8", ColumnKind::Integer),
            column("name", "text", ColumnKind::Text),
        ],
        rows,
        None,
        Duration::ZERO,
    );

    let (panel, window) = cx.add_window_view(move |window, cx| {
        DataGridPanel::new_for_result(
            Arc::new(result),
            "SELECT amount, name FROM orders".to_string(),
            None,
            app_state,
            window,
            cx,
        )
    });
    window.run_until_parked();

    (panel, window)
}

pub(super) fn with_table(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    change: impl FnOnce(
        &mut dbflux_components::components::data_table::DataTableState,
        &mut gpui::Context<dbflux_components::components::data_table::DataTableState>,
    ),
) {
    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("the result builds a table");
        table.update(cx, change);
    });
    window.run_until_parked();
}

#[gpui::test]
fn a_selected_column_shows_its_totals_in_the_footer(cx: &mut TestAppContext) {
    let (panel, window) = amounts_grid(cx);

    with_table(&panel, window, |state, cx| {
        state.click_column(0, Modifiers::default(), cx)
    });

    let footer = window.update(|_, cx| panel.read(cx).selection_totals_footer(cx));
    assert_eq!(
        footer.as_deref(),
        Some("4 cells · Sum 35 · Avg 11.6667 · Min 5 · Max 20 · 1 NULL skipped")
    );
    assert!(
        window.debug_bounds("footer-selection-totals").is_some(),
        "the footer draws the totals"
    );
}

#[gpui::test]
fn ctrl_click_ranges_add_up_and_text_only_counts(cx: &mut TestAppContext) {
    let (panel, window) = amounts_grid(cx);

    with_table(&panel, window, |state, cx| {
        state.click_cell(CellCoord::new(0, 0), Modifiers::default(), cx);
        state.click_cell(CellCoord::new(1, 0), Modifiers::secondary_key(), cx);
        state.click_cell(CellCoord::new(1, 1), Modifiers::secondary_key(), cx);
    });

    let footer = window.update(|_, cx| panel.read(cx).selection_totals_footer(cx));
    assert_eq!(
        footer.as_deref(),
        Some("3 cells · Sum 30 · Avg 15 · Min 10 · Max 20 · 1 non-numeric skipped")
    );
}

#[gpui::test]
fn one_selected_cell_shows_no_totals(cx: &mut TestAppContext) {
    let (panel, window) = amounts_grid(cx);

    with_table(&panel, window, |state, cx| {
        state.click_column(0, Modifiers::default(), cx)
    });
    let footer = window.update(|_, cx| panel.read(cx).selection_totals_footer(cx));
    assert!(footer.is_some(), "the column fills the totals first");

    with_table(&panel, window, |state, cx| {
        state.select_cell(CellCoord::new(1, 0), cx);
    });

    let footer = window.update(|_, cx| panel.read(cx).selection_totals_footer(cx));
    assert_eq!(footer, None, "a single cell is its own total");
    assert!(window.debug_bounds("footer-selection-totals").is_none());
}

#[gpui::test]
fn a_staged_edit_updates_the_shown_totals(cx: &mut TestAppContext) {
    let (panel, window) = amounts_grid(cx);

    with_table(&panel, window, |state, cx| {
        state.click_column(0, Modifiers::default(), cx)
    });
    let footer = window.update(|_, cx| panel.read(cx).selection_totals_footer(cx));
    assert_eq!(
        footer.as_deref(),
        Some("4 cells · Sum 35 · Avg 11.6667 · Min 5 · Max 20 · 1 NULL skipped")
    );

    with_table(&panel, window, |state, cx| {
        state
            .edit_buffer_mut()
            .set_cell(2, 0, CellValue::text("65"));
        cx.notify();
    });

    let footer = window.update(|_, cx| panel.read(cx).selection_totals_footer(cx));
    assert_eq!(
        footer.as_deref(),
        Some("4 cells · Sum 100 · Avg 25 · Min 5 · Max 65"),
        "the edited NULL is now a number"
    );
}

#[gpui::test]
fn two_skipped_nulls_use_the_plural_label(cx: &mut TestAppContext) {
    let (panel, window) = amounts_grid(cx);

    with_table(&panel, window, |state, cx| {
        state.click_column(0, Modifiers::default(), cx);
        state.edit_buffer_mut().set_cell(0, 0, CellValue::null());
    });

    let footer = window.update(|_, cx| panel.read(cx).selection_totals_footer(cx));
    assert_eq!(
        footer.as_deref(),
        Some("4 cells · Sum 25 · Avg 12.5 · Min 5 · Max 20 · 2 NULLs skipped")
    );
}
