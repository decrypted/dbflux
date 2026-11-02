//! Escape in the Chart view of a result.

use super::super::{DataGridPanel, ResultViewMode};
use super::table_has_focus;
use dbflux_app::keymap::Command;
use dbflux_components::theme;
use dbflux_core::{ColumnKind, ColumnMeta, QueryResult, Value};
use dbflux_storage::bootstrap::StorageRuntime;
use dbflux_ui_base::AppStateEntity;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::sync::Arc;
use std::time::Duration;

fn column(name: &str, kind: ColumnKind) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        type_name: String::new(),
        kind,
        nullable: true,
        is_primary_key: false,
    }
}

fn chartable_result() -> QueryResult {
    QueryResult::table(
        vec![
            column("ts", ColumnKind::Timestamp),
            column("value", ColumnKind::Float),
        ],
        vec![
            vec![Value::Int(0), Value::Float(1.0)],
            vec![Value::Int(1_000), Value::Float(2.0)],
        ],
        None,
        Duration::ZERO,
    )
}

fn result_grid(cx: &mut TestAppContext) -> (Entity<DataGridPanel>, &mut VisualTestContext) {
    cx.update(theme::init);

    let app_state = cx.update(|cx| {
        cx.new(|_| {
            let runtime = StorageRuntime::in_memory().expect("isolated storage runtime");
            AppStateEntity::new_with_storage_runtime(runtime).expect("test storage setup")
        })
    });

    let (panel, window) = cx.add_window_view(|window, cx| {
        DataGridPanel::new_for_result(
            Arc::new(chartable_result()),
            "SELECT 1".to_string(),
            None,
            app_state,
            window,
            cx,
        )
    });
    window.run_until_parked();

    (panel, window)
}

fn press_escape(
    panel: &Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> (bool, ResultViewMode) {
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            let handled = panel.dispatch_command(Command::Cancel, window, cx);
            (handled, panel.result_view_mode())
        })
    })
}

fn show(panel: &Entity<DataGridPanel>, window: &mut VisualTestContext, mode: ResultViewMode) {
    window.update(|_, cx| {
        panel.update(cx, |panel, cx| panel.set_result_view_mode(mode, cx));
    });
}

/// Back to the view the user came from, with the keyboard in the results.
#[gpui::test]
fn escape_returns_to_the_view_the_chart_was_entered_from(cx: &mut TestAppContext) {
    let (panel, window) = result_grid(cx);

    show(&panel, window, ResultViewMode::Json);
    show(&panel, window, ResultViewMode::Chart);

    // The keyboard is elsewhere when the press arrives.
    window.update(|window, cx| cx.focus_handle().focus(window, cx));
    assert!(!table_has_focus(&panel, window));

    assert_eq!(
        press_escape(&panel, window),
        (true, ResultViewMode::Json),
        "the previous view, not the first one"
    );
    window.run_until_parked();
    assert!(table_has_focus(&panel, window), "focus is in the results");
}

/// A chart the result opened in has no previous view to go back to.
#[gpui::test]
fn escape_without_a_previous_view_shows_the_first_view(cx: &mut TestAppContext) {
    let (panel, window) = result_grid(cx);

    window.update(|_, cx| {
        panel.update(cx, |panel, _| {
            panel.chrome.result_view_mode = ResultViewMode::Chart;
            panel.chrome.previous_result_view = None;
        });
    });

    assert_eq!(press_escape(&panel, window), (true, ResultViewMode::Table));
}

/// The highlighted point goes first, then the rail, then the view.
#[gpui::test]
fn escape_clears_the_point_and_the_rail_before_leaving_the_chart(cx: &mut TestAppContext) {
    let (panel, window) = result_grid(cx);
    show(&panel, window, ResultViewMode::Chart);

    window.update(|_, cx| panel.update(cx, |panel, cx| panel.ensure_chart_view(cx)));
    let shell = window
        .update(|_, cx| panel.read(cx).chart.chart_shell.clone())
        .expect("a chartable result has a chart");

    let step = window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.dispatch_command(Command::ColumnRight, window, cx)
        })
    });
    assert!(step, "the chart view takes the point keys");
    window.update(|_, cx| {
        shell.update(cx, |shell, _| shell.chart_rail_open = true);
    });

    assert_eq!(press_escape(&panel, window), (true, ResultViewMode::Chart));
    assert!(
        window.update(|_, cx| {
            shell
                .read(cx)
                .chart_view()
                .is_some_and(|view| view.read(cx).keyboard_point().is_none())
        }),
        "the first press clears the point"
    );
    assert!(window.update(|_, cx| shell.read(cx).chart_rail_open));

    assert_eq!(press_escape(&panel, window), (true, ResultViewMode::Chart));
    assert!(!window.update(|_, cx| shell.read(cx).chart_rail_open));

    assert_eq!(press_escape(&panel, window), (true, ResultViewMode::Table));
}

/// Table + Chart keeps its keys with the table, Escape included.
#[gpui::test]
fn escape_leaves_table_and_chart_alone(cx: &mut TestAppContext) {
    let (panel, window) = result_grid(cx);
    show(&panel, window, ResultViewMode::Both);

    assert_eq!(press_escape(&panel, window), (false, ResultViewMode::Both));
}
