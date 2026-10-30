use gpui::{Modifiers, MouseButton, TestAppContext, point, px};

use super::super::ROW_NUMBER_WIDTH;
use super::super::model::CellValue;
use super::super::selection::CellCoord;
use super::{model, render_table};

/// A pinned column stays at the left edge, right after the row numbers,
/// while the grid scrolls sideways; its cells, its header, its resize edge
/// and the row-number gutter keep working there.
#[gpui::test]
fn a_pinned_column_stays_at_the_left_edge_and_keeps_working(cx: &mut TestAppContext) {
    let names = ["id", "a", "b", "created_at", "c", "d"];
    let (state, visual) = render_table(
        cx,
        model(&names, 5, |row, col| {
            CellValue::text(&format!("{}-{row}-{col}", "v".repeat(30)))
        }),
    );

    visual.update(|_, cx| {
        state.update(cx, |state, cx| assert!(state.pin_column(3, cx)));
    });
    visual.run_until_parked();
    let pinned_before = visual
        .debug_bounds("header-col-3")
        .expect("the pinned header is drawn");
    let scrolling_before = visual
        .debug_bounds("header-col-4")
        .expect("a scrolling header is drawn");

    // Scroll sideways by 120 px.
    visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            state
                .horizontal_scroll_handle()
                .set_offset(point(px(-120.0), px(0.0)));
            state.sync_horizontal_offset(cx);
        });
    });
    visual.run_until_parked();

    let pinned_after = visual
        .debug_bounds("header-col-3")
        .expect("the pinned header is still drawn");
    let scrolling_after = visual
        .debug_bounds("header-col-4")
        .expect("the scrolling header is still drawn");
    let origin = visual
        .debug_bounds("table-header")
        .expect("the header is drawn")
        .origin;
    assert_eq!(
        pinned_after.origin.x, pinned_before.origin.x,
        "the pinned column does not scroll"
    );
    assert_eq!(pinned_after.origin.x, origin.x + ROW_NUMBER_WIDTH);
    assert_eq!(
        scrolling_after.origin.x,
        scrolling_before.origin.x - px(120.0),
        "the others scroll"
    );

    // A cell of the pinned column selects its model coordinate.
    let row_height = visual.update(|_, cx| crate::fonts::grid_row_height(cx));
    let header_height = visual.update(|_, cx| state.read(cx).header_height(cx));
    let row_y = |row: usize| origin.y + header_height + row_height * (row as f32 + 0.5);
    visual.simulate_click(
        point(pinned_after.origin.x + px(10.0), row_y(1)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let active = visual.update(|_, cx| state.read(cx).selection().active);
    assert_eq!(active, Some(CellCoord::new(1, 3)));

    // The row-number gutter, held in place with the pinned column, still
    // selects the whole row.
    visual.simulate_click(
        point(origin.x + ROW_NUMBER_WIDTH / 2.0, row_y(2)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let full_row = visual.update(|_, cx| state.read(cx).selection().is_full_row_selected(2, 6));
    assert!(full_row, "a click on the gutter selects the row");

    // The pinned header's resize edge still fits the column.
    let before = visual.update(|_, cx| state.read(cx).column_widths()[3]);
    let handle = visual
        .debug_bounds("resize-handle-3")
        .expect("the pinned header's resize edge is drawn");
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
    let after = visual.update(|_, cx| state.read(cx).column_widths()[3]);
    assert!(after > before, "fits: {before} -> {after}");
}

/// Moving the cursor onto a column that hides under the pinned strip
/// scrolls it out from under the strip.
#[gpui::test]
fn scrolling_to_a_column_clears_the_pinned_strip(cx: &mut TestAppContext) {
    let names = ["id", "a", "b", "c", "d", "e"];
    let (state, visual) = render_table(
        cx,
        model(&names, 3, |row, col| {
            CellValue::text(&format!("{}-{row}-{col}", "w".repeat(30)))
        }),
    );

    let offset = visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.pin_column(0, cx);
            state
                .horizontal_scroll_handle()
                .set_offset(point(px(-400.0), px(0.0)));
            state.sync_horizontal_offset(cx);
            // `a` is the first scrolling column; at 400 px it sits under the
            // strip.
            state.select_cell(CellCoord::new(0, 1), cx);
            state.scroll_to_column(1);
            state.sync_horizontal_offset(cx);
            state.horizontal_offset()
        })
    });

    assert_eq!(offset, px(0.0), "the first scrolling column shows in full");
}
