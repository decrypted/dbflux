//! A dashboard with a row a past bug stored twice: what it shows, and what an edit
//! by position reaches.

use super::*;

/// A manager holding rows for `[A, A, B]`, as a past bug could leave them: the
/// dashboard shows `[A, B]`, and the second A's row is hidden.
fn dashboard_with_a_hidden_row() -> (DashboardManager, StorageRuntime, Uuid, Uuid, Uuid) {
    let (mut mgr, rt) = make_manager_with_rt();
    let profile_id = insert_profile(&rt);
    let dash_id = mgr
        .create_dashboard(
            "D".to_string(),
            None,
            profile_id,
            None,
            SavedChartRefreshPolicy::Off,
        )
        .unwrap();
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let chart = |saved_chart_id| DashboardPanelDraft::Chart {
        saved_chart_id,
        layout: None,
    };
    mgr.append_panels(dash_id, vec![chart(a), chart(a), chart(b)])
        .unwrap();

    (mgr, rt, dash_id, a, b)
}

fn stored_charts(mgr: &DashboardManager, dash_id: Uuid) -> Vec<Option<Uuid>> {
    mgr.panels_for_dashboard(dash_id)
        .iter()
        .map(|p| p.saved_chart_id())
        .collect()
}

/// The panel the user removed is the one that goes: the hidden row before it
/// doesn't shift the index onto another chart.
#[test]
fn removing_a_panel_past_a_hidden_row_removes_that_panel() {
    let (mut mgr, _rt, dash_id, a, _b) = dashboard_with_a_hidden_row();
    mgr.update_panel_title_override(dash_id, 0, Some("first".to_string()))
        .unwrap();

    mgr.remove_panel(dash_id, 1).unwrap();

    assert_eq!(stored_charts(&mgr, dash_id), [Some(a), Some(a)]);
    assert_eq!(
        mgr.panels_for_dashboard(dash_id)[0]
            .title_override
            .as_deref(),
        Some("first"),
        "the panel left keeps its title"
    );
}

/// Removing the panel with the hidden row takes both rows.
#[test]
fn removing_a_panel_removes_its_hidden_row() {
    let (mut mgr, _rt, dash_id, _a, b) = dashboard_with_a_hidden_row();

    mgr.remove_panel(dash_id, 0).unwrap();

    assert_eq!(stored_charts(&mgr, dash_id), [Some(b)]);
}

/// Every edit by position reaches the panel at that position on screen.
#[test]
fn edits_past_a_hidden_row_reach_the_panel_shown_there() {
    let (mut mgr, _rt, dash_id, _a, b) = dashboard_with_a_hidden_row();
    let row_of_b = |mgr: &DashboardManager| {
        mgr.panels_for_dashboard(dash_id)
            .iter()
            .find(|p| p.saved_chart_id() == Some(b))
            .cloned()
            .unwrap()
    };

    mgr.update_panel_title_override(dash_id, 1, Some("b".to_string()))
        .unwrap();
    assert_eq!(row_of_b(&mgr).title_override.as_deref(), Some("b"));

    mgr.update_panel_position(dash_id, 1, 3, 9, 4, 5).unwrap();
    let row = row_of_b(&mgr);
    assert_eq!(
        (
            row.grid_column,
            row.grid_row,
            row.grid_width,
            row.grid_height
        ),
        (3, 9, 4, 5)
    );

    mgr.resize_panel(dash_id, 1, 2, 3).unwrap();
    let row = row_of_b(&mgr);
    assert_eq!((row.grid_width, row.grid_height), (2, 3));

    assert_eq!(mgr.shown_panels(dash_id).len(), 2);
    assert_eq!(mgr.shown_panels(dash_id)[1].saved_chart_id(), Some(b));
}
