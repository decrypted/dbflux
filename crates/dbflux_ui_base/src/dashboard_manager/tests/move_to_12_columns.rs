//! Moving a dashboard stored for a narrower grid onto the 12-column grid. Rows are
//! read back through a fresh manager, as the next start reads them.

use super::*;

/// `(grid_row, grid_column, grid_width, grid_height)`
type Rect = (u32, u32, u32, u32);

fn manager_over(rt: &StorageRuntime) -> DashboardManager {
    let conn = rt.viz_connection().expect("viz connection");
    DashboardManager::new(
        Arc::new(DashboardsRepository::new(Arc::clone(&conn))),
        Arc::new(DashboardPanelsRepository::new(conn)),
    )
}

/// A dashboard stored with these rows and `grid_columns`, as if from an older version.
fn stored(grid_columns: u32, drafts: Vec<DashboardPanelDraft>) -> (StorageRuntime, Uuid) {
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
    mgr.append_panels(dash_id, drafts).unwrap();

    set_grid_columns(&rt, dash_id, grid_columns);
    (rt, dash_id)
}

fn set_grid_columns(rt: &StorageRuntime, dash_id: Uuid, grid_columns: u32) {
    let conn = rt.viz_connection().expect("viz connection");
    let conn = conn.lock().unwrap();
    conn.execute(
        "UPDATE viz_dashboards SET grid_columns = ?1 WHERE id = ?2",
        rusqlite::params![grid_columns, dash_id.to_string()],
    )
    .unwrap();
}

fn layout(rect: Rect) -> Option<DraftGridLayout> {
    Some(DraftGridLayout {
        grid_row: rect.0,
        grid_column: rect.1,
        grid_width: rect.2,
        grid_height: rect.3,
    })
}

fn chart(rect: Rect) -> DashboardPanelDraft {
    DashboardPanelDraft::Chart {
        saved_chart_id: Uuid::new_v4(),
        layout: layout(rect),
    }
}

fn rects(mgr: &DashboardManager, dash_id: Uuid) -> Vec<Rect> {
    mgr.panels_for_dashboard(dash_id)
        .iter()
        .map(|p| (p.grid_row, p.grid_column, p.grid_width, p.grid_height))
        .collect()
}

fn stored_grid_columns(rt: &StorageRuntime, dash_id: Uuid) -> u32 {
    manager_over(rt)
        .dashboard_by_id(dash_id)
        .expect("the dashboard")
        .grid_columns
}

/// `K` columns of one panel each, and one panel across all of them, at every `K` the
/// data can hold under 12.
#[test]
fn every_narrower_grid_moves_once() {
    for (grid_columns, factor) in [
        (1, 12),
        (2, 6),
        (3, 4),
        (4, 3),
        (6, 2),
        (5, 2),
        (7, 1),
        (11, 1),
    ] {
        let mut drafts: Vec<DashboardPanelDraft> = (0..grid_columns)
            .map(|column| chart((0, column, 1, 2)))
            .collect();
        drafts.push(chart((2, 0, grid_columns, 2)));
        let (rt, dash_id) = stored(grid_columns, drafts);

        let mut expected: Vec<Rect> = (0..grid_columns)
            .map(|column| (0, column * factor, factor, 2))
            .collect();
        expected.push((2, 0, grid_columns * factor, 2));

        for open in 0..3 {
            let mut mgr = manager_over(&rt);
            mgr.move_to_12_columns(dash_id).unwrap();

            assert_eq!(
                rects(&mgr, dash_id),
                expected,
                "{grid_columns}, open {open}"
            );
            assert_eq!(stored_grid_columns(&rt, dash_id), 12, "{grid_columns}");
            assert_eq!(rects(&manager_over(&rt), dash_id), expected, "stored");
        }
    }
}

/// A dashboard on the 12-column grid is left alone.
#[test]
fn a_12_column_dashboard_is_left_alone() {
    let rows = vec![chart((0, 0, 5, 2)), chart((0, 5, 7, 2))];
    let (rt, dash_id) = stored(12, rows);

    let mut mgr = manager_over(&rt);
    let before = rects(&mgr, dash_id);
    mgr.move_to_12_columns(dash_id).unwrap();

    assert_eq!(rects(&mgr, dash_id), before);
}

/// Dividers, inspectors and hidden duplicate rows are on the same grid as the rest.
#[test]
fn every_kind_of_row_moves_and_positions_still_mean_what_is_shown() {
    let twice = Uuid::new_v4();
    let repeated = |rect| DashboardPanelDraft::Chart {
        saved_chart_id: twice,
        layout: layout(rect),
    };
    let (rt, dash_id) = stored(
        2,
        vec![
            repeated((0, 0, 1, 2)),
            repeated((4, 1, 1, 2)),
            DashboardPanelDraft::Inspector {
                metric_id: "sessions".to_string(),
                layout: layout((2, 1, 1, 2)),
            },
        ],
    );
    {
        let conn = rt.viz_connection().expect("viz connection");
        conn.lock()
            .unwrap()
            .execute(
                "INSERT INTO viz_dashboard_panels
                     (dashboard_id, panel_index, panel_kind, saved_chart_id, divider_markdown,
                      grid_row, grid_column, grid_width, grid_height)
                 VALUES (?1, 3, 'divider', '', '# section', 8, 0, 2, 1)",
                [dash_id.to_string()],
            )
            .unwrap();
    }

    let mut mgr = manager_over(&rt);
    mgr.move_to_12_columns(dash_id).unwrap();

    assert_eq!(
        rects(&mgr, dash_id),
        [(0, 0, 6, 2), (4, 6, 6, 2), (2, 6, 6, 2), (8, 0, 12, 1)]
    );
    let shown: Vec<u32> = mgr
        .shown_panels(dash_id)
        .iter()
        .map(|p| p.grid_column)
        .collect();
    assert_eq!(shown, [0, 6, 0], "the hidden row isn't shown");

    mgr.update_panel_position(dash_id, 1, 0, 9, 12, 2).unwrap();
    assert_eq!(
        rects(&manager_over(&rt), dash_id)[2],
        (9, 0, 12, 2),
        "position 1 is the inspector"
    );
}

/// Earlier versions rescaled on every open, so a `grid_columns = 2` dashboard can hold
/// rows already on 12 columns. They aren't scaled again, and they aren't repaired.
#[test]
fn rows_rescaled_by_an_earlier_version_stay_as_they_are() {
    for rows in [
        vec![(0, 0, 6, 4), (0, 6, 6, 4)],
        vec![(0, 0, 12, 4), (0, 11, 12, 4)],
    ] {
        let (rt, dash_id) = stored(2, rows.iter().map(|rect| chart(*rect)).collect());

        let mut mgr = manager_over(&rt);
        mgr.move_to_12_columns(dash_id).unwrap();

        assert_eq!(rects(&mgr, dash_id), rows);
        assert_eq!(rects(&manager_over(&rt), dash_id), rows);
        assert_eq!(stored_grid_columns(&rt, dash_id), 12);
    }
}

/// The rows and `grid_columns` go together. If the second write fails the first is
/// undone, later edits are refused, and the dashboard still draws on 12 columns from
/// what is stored, however often it opens.
#[test]
fn a_failed_write_changes_nothing_and_refuses_later_edits() {
    let rows = vec![(0, 0, 1, 2), (0, 1, 1, 2)];
    let (rt, dash_id) = stored(2, rows.iter().map(|rect| chart(*rect)).collect());
    {
        let conn = rt.viz_connection().expect("viz connection");
        let conn = conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_grid BEFORE UPDATE OF grid_columns ON viz_dashboards
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
    }

    let mut mgr = manager_over(&rt);
    assert!(mgr.move_to_12_columns(dash_id).is_err());

    assert_eq!(rects(&mgr, dash_id), rows, "the cache is as it was");
    assert_eq!(rects(&manager_over(&rt), dash_id), rows, "and storage");
    assert_eq!(stored_grid_columns(&rt, dash_id), 2);

    let drawn = |mgr: &DashboardManager| -> Vec<u32> {
        mgr.shown_panels_on_12_columns(dash_id)
            .iter()
            .map(|p| p.grid_column)
            .collect()
    };
    assert_eq!(drawn(&mgr), [0, 6]);
    assert!(mgr.move_to_12_columns(dash_id).is_err());
    assert_eq!(drawn(&mgr), [0, 6], "opening again doesn't scale again");

    assert!(mgr.update_panel_position(dash_id, 1, 0, 9, 1, 2).is_err());
    assert_eq!(rects(&manager_over(&rt), dash_id), rows);
}

/// A row wider than its grid stays on the 12-column one.
#[test]
fn a_rescaled_panel_stays_on_the_grid() {
    assert_eq!(rescale_panel_to_12_cols(1, 2, 2), (6, 6));
    assert_eq!(rescale_panel_to_12_cols(5, 1, 2), (11, 1));
}

/// The schema's CHECK is why only values up to 12 need handling.
#[test]
fn a_grid_above_12_cannot_be_stored() {
    let (rt, dash_id) = stored(12, vec![chart((0, 0, 12, 2))]);
    let conn = rt.viz_connection().expect("viz connection");
    let conn = conn.lock().unwrap();

    let refused = conn.execute(
        "UPDATE viz_dashboards SET grid_columns = 24 WHERE id = ?1",
        [dash_id.to_string()],
    );
    assert!(refused.is_err(), "the schema's CHECK refuses it");
}
