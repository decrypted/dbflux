use super::cell_expressions::{EXPRESSION_DIALECT, events_grid, stage};
use super::*;
use crate::data_grid_panel::review_changes::ReviewedStatement;
use dbflux_components::components::data_table::model::CellValue;
use dbflux_core::SqlQueryBuilder;
use dbflux_test_support::fake_driver::{CrudOp, FakeDriver};

fn driver() -> FakeDriver {
    FakeDriver::new(dbflux_core::DbKind::SQLite).with_dialect(&EXPRESSION_DIALECT)
}

/// An expression edit on the first row, a delete of the second and a new
/// row: one statement of each kind.
fn stage_one_of_each(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) {
    stage(panel, window, 0, 1, CellValue::expression("NOW()"));
    stage(panel, window, 0, 2, CellValue::text("it's"));
    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        table.update(cx, |state, cx| {
            state.edit_buffer_mut().mark_for_delete(1);
            state.edit_buffer_mut().add_pending_insert(vec![
                CellValue::int(3),
                CellValue::expression("NOW()"),
                CellValue::text("new"),
            ]);
            cx.notify();
        });
    });
}

fn reviewed_sql(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> Vec<String> {
    window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .build_change_review(cx)
                .expect("staged changes have a review")
                .statements
                .into_iter()
                .map(|statement| match statement {
                    ReviewedStatement::Sql(sql) => sql,
                    ReviewedStatement::Skipped(reason) => panic!("skipped: {reason}"),
                })
                .collect()
        })
    })
}

/// The statements a recorded operation renders to, built the way the fake
/// connection renders its previews.
fn rendered(op: &CrudOp) -> String {
    let builder = SqlQueryBuilder::new(&EXPRESSION_DIALECT);
    match op {
        CrudOp::Update(patch) => builder.build_update(patch, false),
        CrudOp::Insert(insert) => builder.build_insert(insert, false),
        CrudOp::Delete(delete) => builder.build_delete(delete, false),
    }
    .expect("a recorded operation renders")
}

#[gpui::test]
fn the_review_lists_the_statements_save_runs_in_order(cx: &mut TestAppContext) {
    let driver = driver();
    let (panel, window) = events_grid(cx, &driver);
    stage_one_of_each(&panel, window);

    let reviewed = reviewed_sql(&panel, window);
    assert_eq!(
        reviewed,
        [
            "DELETE FROM \"public\".\"events\" WHERE \"id\" = 2",
            "INSERT INTO \"public\".\"events\" (\"id\", \"seen_at\", \"note\") VALUES (3, NOW(), 'new')",
            "UPDATE \"public\".\"events\" SET \"seen_at\" = NOW(), \"note\" = 'it''s' WHERE \"id\" = 1",
        ]
    );

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            assert!(panel.open_change_review(window, cx));
            panel.confirm_change_review(window, cx);
        })
    });
    window.run_until_parked();

    let executed: Vec<String> = driver.stats().crud_ops.iter().map(rendered).collect();
    assert_eq!(
        executed, reviewed,
        "Save runs exactly what the review showed"
    );
}

#[gpui::test]
fn cancelling_the_review_runs_nothing_and_keeps_the_edits(cx: &mut TestAppContext) {
    let driver = driver();
    let (panel, window) = events_grid(cx, &driver);
    stage_one_of_each(&panel, window);

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            assert!(panel.open_change_review(window, cx));
            panel.cancel_change_review(window, cx);
        })
    });
    window.run_until_parked();

    assert!(driver.stats().crud_ops.is_empty());
    let counts = window.update(|_, cx| panel.read(cx).pending_edit_counts(cx));
    assert_eq!(counts, (1, 1, 1), "insert, update and delete stay staged");
    assert!(window.update(|_, cx| panel.read(cx).change_review.is_none()));
}

#[gpui::test]
fn copy_sql_copies_the_reviewed_script(cx: &mut TestAppContext) {
    let driver = driver();
    let (panel, window) = events_grid(cx, &driver);
    stage(&panel, window, 0, 1, CellValue::expression("NOW()"));

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            assert!(panel.open_change_review(window, cx));
            panel.copy_change_review(cx);
        })
    });

    let copied = window.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        copied.as_deref(),
        Some("UPDATE \"public\".\"events\" SET \"seen_at\" = NOW() WHERE \"id\" = 1;")
    );
    assert!(driver.stats().crud_ops.is_empty(), "copying runs nothing");
}

#[gpui::test]
fn with_the_setting_on_save_opens_the_review_instead(cx: &mut TestAppContext) {
    let driver = driver();
    let (panel, window) = events_grid(cx, &driver);
    window.update(|_, cx| {
        let app_state = panel.read(cx).app_state.clone();
        app_state.update(cx, |app, _| {
            let mut settings = app.general_settings().clone();
            settings.grid_review_before_save = true;
            app.update_general_settings(settings);
        });
    });
    stage(&panel, window, 0, 1, CellValue::expression("NOW()"));

    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        table.update(cx, |state, cx| state.request_save_all(cx));
    });
    window.run_until_parked();

    assert!(
        driver.stats().crud_ops.is_empty(),
        "nothing runs before the review"
    );
    assert!(window.update(|_, cx| panel.read(cx).change_review.is_some()));
}

#[gpui::test]
fn without_the_setting_save_runs_directly(cx: &mut TestAppContext) {
    let driver = driver();
    let (panel, window) = events_grid(cx, &driver);
    stage(&panel, window, 0, 1, CellValue::expression("NOW()"));

    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        table.update(cx, |state, cx| state.request_save_all(cx));
    });
    window.run_until_parked();

    assert_eq!(driver.stats().crud_ops.len(), 1);
    assert!(window.update(|_, cx| panel.read(cx).change_review.is_none()));
}
