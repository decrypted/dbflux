//! Quotes inserted after a comparison typed in the WHERE field.

use super::filter_history::{filter_text, hosted_table, register_filter_stub};
use super::*;
use crate::keyboard_test_support::init_keyboard_runtime;

/// `id` is an integer, `method` text, `created_at` a timestamp.
fn typed_rows() -> QueryResult {
    let column = |name: &str, type_name: &str, kind: ColumnKind| ColumnMeta {
        name: name.to_string(),
        type_name: type_name.to_string(),
        kind,
        nullable: true,
        is_primary_key: name == "id",
    };

    QueryResult::table(
        vec![
            column("id", "int4", ColumnKind::Integer),
            column("method", "varchar", ColumnKind::Text),
            column("created_at", "timestamptz", ColumnKind::Timestamp),
        ],
        Vec::new(),
        None,
        Duration::ZERO,
    )
}

/// A table grid whose WHERE field has the keyboard, with the setting set
/// to `enabled`.
fn focused_filter(
    cx: &mut TestAppContext,
    enabled: bool,
) -> (gpui::Entity<DataGridPanel>, &mut VisualTestContext) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let (profile_id, _log) =
        cx.update(|cx| register_filter_stub(&app_state, typed_rows(), None, cx));
    cx.update(|cx| {
        app_state.update(cx, |app, _| {
            let mut settings = app.general_settings().clone();
            settings.grid_filter_autoquote = enabled;
            app.update_general_settings(settings);
        });
    });

    let (_host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.set_result(typed_rows(), cx);
            panel.focus_filter_input(window, cx);
        });
    });
    window.run_until_parked();

    (panel, window)
}

fn type_keys(window: &mut VisualTestContext, keys: &str) {
    for key in keys.split(' ') {
        window.simulate_keystrokes(key);
        window.run_until_parked();
    }
}

fn caret(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> usize {
    window.update(|_, cx| panel.read(cx).filter_bar.filter_input.read(cx).cursor())
}

#[gpui::test]
fn equals_after_a_text_column_leaves_the_caret_between_quotes(cx: &mut TestAppContext) {
    let (panel, window) = focused_filter(cx, true);

    type_keys(window, "m e t h o d space =");

    assert_eq!(filter_text(&panel, window), "method = ''");
    assert_eq!(caret(&panel, window), "method = '".len());

    type_keys(window, "g e t");
    assert_eq!(filter_text(&panel, window), "method = 'get'");
}

#[gpui::test]
fn a_quote_typed_before_the_inserted_one_steps_over_it(cx: &mut TestAppContext) {
    let (panel, window) = focused_filter(cx, true);

    type_keys(window, "m e t h o d space = g e t ' space");

    assert_eq!(filter_text(&panel, window), "method = 'get' ");
    assert_eq!(caret(&panel, window), "method = 'get' ".len());
}

#[gpui::test]
fn the_space_and_quote_typed_out_of_habit_keep_the_value_inside(cx: &mut TestAppContext) {
    let (panel, window) = focused_filter(cx, true);

    type_keys(window, "m e t h o d space = space ' g e t ' space");

    assert_eq!(filter_text(&panel, window), "method = 'get' ");
    assert_eq!(caret(&panel, window), "method = 'get' ".len());
}

#[gpui::test]
fn backspace_right_after_the_quotes_removes_them(cx: &mut TestAppContext) {
    let (panel, window) = focused_filter(cx, true);

    type_keys(window, "c r e a t e d _ a t space > space backspace");

    assert_eq!(filter_text(&panel, window), "created_at > ");
}

#[gpui::test]
fn numbers_and_string_literals_get_no_quotes(cx: &mut TestAppContext) {
    let (panel, window) = focused_filter(cx, true);
    type_keys(window, "i d space =");
    assert_eq!(filter_text(&panel, window), "id =", "an integer column");

    type_keys(
        window,
        "space 1 space a n d space m e t h o d space = a space =",
    );
    assert_eq!(
        filter_text(&panel, window),
        "id = 1 and method = 'a ='",
        "an `=` inside the string literal"
    );
}

#[gpui::test]
fn with_the_setting_off_nothing_is_inserted(cx: &mut TestAppContext) {
    let (panel, window) = focused_filter(cx, false);
    type_keys(window, "m e t h o d space =");
    assert_eq!(
        filter_text(&panel, window),
        "method =",
        "the setting is off"
    );
}
