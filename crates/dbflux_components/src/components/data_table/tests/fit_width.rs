use gpui::{Modifiers, MouseButton, TestAppContext};

use super::super::model::CellValue;
use super::super::theme::{AUTO_WIDTH_SAMPLE_ROWS, MAX_AUTO_COLUMN_WIDTH, MAX_FIT_COLUMN_WIDTH};
use super::{model, render_table};

/// A double-click on a header's resize edge fits the column to its longest
/// loaded value, also one past the rows the opening width samples, and
/// leaves the other columns alone.
#[gpui::test]
fn double_click_on_the_resize_edge_fits_the_column(cx: &mut TestAppContext) {
    let late_row = AUTO_WIDTH_SAMPLE_ROWS + 10;
    let long_value = "x".repeat(40);
    let model = model(&["id", "note"], late_row + 1, move |row, col| match col {
        0 => CellValue::int(row as i64),
        _ if row == late_row => CellValue::text(&long_value),
        _ => CellValue::text("a"),
    });
    let (state, visual) = render_table(cx, model);

    let before = visual.update(|_, cx| state.read(cx).column_widths().to_vec());
    let handle = visual
        .debug_bounds("resize-handle-1")
        .expect("the resize edge of `note` is drawn");

    for click_count in 1..=2 {
        visual.simulate_event(gpui::MouseDownEvent {
            position: handle.center(),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count,
            first_mouse: false,
        });
        visual.simulate_event(gpui::MouseUpEvent {
            position: handle.center(),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count,
        });
    }
    visual.run_until_parked();

    let after = visual.update(|_, cx| state.read(cx).column_widths().to_vec());
    assert_eq!(after[0], before[0], "the other column keeps its width");
    assert!(
        after[1] > before[1],
        "the fitted column grows to its value past the sampled rows: {} -> {}",
        before[1],
        after[1]
    );
    let char_advance = visual.update(|_, cx| crate::fonts::grid_char_advance(cx));
    assert!(
        after[1] >= 40.0 * char_advance,
        "the whole 40-character value fits: {} for {char_advance} per character",
        after[1]
    );
}

/// Fitting reads every loaded row but stops at its own cap, which is wider
/// than the width a column opens with.
#[gpui::test]
fn fit_width_is_capped_for_a_huge_value(cx: &mut TestAppContext) {
    let huge = "{\"k\": 1}, ".repeat(400);
    let model = model(&["payload"], 3, move |row, _| {
        if row == 2 {
            CellValue::text(&huge)
        } else {
            CellValue::text("{}")
        }
    });
    let (state, visual) = render_table(cx, model);

    let opening = visual.update(|_, cx| state.read(cx).column_widths()[0]);
    assert!(
        opening <= MAX_AUTO_COLUMN_WIDTH.ceil(),
        "opens at {opening}"
    );

    visual.update(|_, cx| state.update(cx, |state, cx| state.fit_column_width(0, cx)));
    let fitted = visual.update(|_, cx| state.read(cx).column_widths()[0]);
    assert!(
        fitted > opening && fitted <= MAX_FIT_COLUMN_WIDTH.ceil(),
        "fits to the cap: opened at {opening}, fitted to {fitted}"
    );
}
