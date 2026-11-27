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

/// Scrolled further than the pinned strip is wide, the strip covers
/// scrolling cells. A click on the right edge of a pinned cell, or on the
/// gutter, reaches only the pinned cell or row, never the hidden cell
/// under it.
#[gpui::test]
fn the_pinned_strip_keeps_clicks_from_the_cells_under_it(cx: &mut TestAppContext) {
    let names = ["id", "a", "b", "c", "d", "e", "f"];
    let (state, visual) = render_table(
        cx,
        model(&names, 4, |row, col| {
            CellValue::text(&format!("{}-{row}-{col}", "x".repeat(20)))
        }),
    );

    let pinned_width = visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            assert!(state.pin_column(0, cx));
            let scroll = px(state.pinned_width()) + ROW_NUMBER_WIDTH + px(150.0);
            state
                .horizontal_scroll_handle()
                .set_offset(point(-scroll, px(0.0)));
            state.sync_horizontal_offset(cx);
            state.pinned_width()
        })
    });
    visual.run_until_parked();
    assert!(
        visual.update(|_, cx| state.read(cx).horizontal_offset()) > px(pinned_width),
        "scrolled past the strip"
    );

    let origin = visual
        .debug_bounds("table-header")
        .expect("the header is drawn")
        .origin;
    let row_height = visual.update(|_, cx| crate::fonts::grid_row_height(cx));
    let header_height = visual.update(|_, cx| state.read(cx).header_height(cx));
    let row_y = |row: usize| origin.y + header_height + row_height * (row as f32 + 0.5);

    // The pinned cell's right edge, where a scrolling cell lies underneath.
    let edge = origin.x + ROW_NUMBER_WIDTH + px(pinned_width) - px(3.0);
    visual.simulate_click(point(edge, row_y(1)), Modifiers::default());
    visual.run_until_parked();
    let selection = visual.update(|_, cx| {
        let selection = state.read(cx).selection().clone();
        (selection.active, selection.anchor)
    });
    assert_eq!(
        selection,
        (Some(CellCoord::new(1, 0)), Some(CellCoord::new(1, 0))),
        "only the pinned cell is selected"
    );

    visual.simulate_click(
        point(origin.x + ROW_NUMBER_WIDTH / 2.0, row_y(2)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let (full_row, active) = visual.update(|_, cx| {
        let state = state.read(cx);
        (
            state.selection().is_full_row_selected(2, names.len()),
            state.selection().active,
        )
    });
    assert!(full_row, "the gutter selects the row");
    assert_eq!(active.map(|cell| cell.row), Some(2));
}

/// Dragging a pinned column's resize edge to the left, over the strip,
/// resizes the column: the strip repeats the drag tracking the table root
/// does, since it blocks the mouse from the root.
#[gpui::test]
fn a_pinned_column_resizes_by_dragging_over_the_strip(cx: &mut TestAppContext) {
    let names = ["id", "a", "b"];
    let (state, visual) = render_table(
        cx,
        model(&names, 2, |row, col| {
            CellValue::text(&format!("{}-{row}-{col}", "y".repeat(20)))
        }),
    );
    visual.update(|_, cx| state.update(cx, |state, cx| assert!(state.pin_column(1, cx))));
    visual.run_until_parked();

    let before = visual.update(|_, cx| state.read(cx).column_widths()[1]);
    let handle = visual
        .debug_bounds("resize-handle-1")
        .expect("the pinned header's resize edge is drawn");
    let start = handle.center();
    visual.simulate_event(gpui::MouseDownEvent {
        position: start,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    for step in 1..=4 {
        visual.simulate_event(gpui::MouseMoveEvent {
            position: point(start.x - px(10.0 * step as f32), start.y),
            modifiers: Modifiers::default(),
            pressed_button: Some(MouseButton::Left),
        });
    }
    visual.simulate_event(gpui::MouseUpEvent {
        position: point(start.x - px(40.0), start.y),
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 1,
    });
    visual.run_until_parked();

    let after = visual.update(|_, cx| state.read(cx).column_widths()[1]);
    assert!(
        (after - (before - 40.0)).abs() < 0.5,
        "the drag over the strip resizes: {before} -> {after}"
    );
}

/// Pinned columns wider than two thirds of the grid do not all stay at the
/// left edge: the ones past that scroll with the others, so the scrolling
/// columns keep room and scrolling to one still brings it into view.
#[gpui::test]
fn pinned_columns_never_cover_the_whole_grid(cx: &mut TestAppContext) {
    let names = ["id", "a", "b", "c", "d"];
    let (state, visual) = render_table(
        cx,
        model(&names, 2, |row, col| {
            CellValue::text(&format!("{row}-{col}"))
        }),
    );

    let (pinned, shown, width) = visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            // The harness grid is 600 px wide.
            state.set_column_width(0, 250.0, cx);
            state.set_column_width(1, 250.0, cx);
            assert!(state.pin_column(0, cx));
            assert!(state.pin_column(1, cx));
            (
                state.column_layout().pinned_count(),
                state.pinned_shown_count(),
                state.pinned_width(),
            )
        })
    });
    visual.run_until_parked();
    assert_eq!(pinned, 2, "both columns stay pinned in the layout");
    assert_eq!(shown, 1, "only the first fits in two thirds of 600 px");
    assert_eq!(width, 250.0);

    let header_lefts = |visual: &mut gpui::VisualTestContext| {
        (
            visual.debug_bounds("header-col-0").map(|b| b.origin.x),
            visual.debug_bounds("header-col-1").map(|b| b.origin.x),
        )
    };
    let (pinned_before, second_before) = header_lefts(visual);

    visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            state
                .horizontal_scroll_handle()
                .set_offset(point(px(-100.0), px(0.0)));
            state.sync_horizontal_offset(cx);
        });
    });
    visual.run_until_parked();
    let (pinned_after, second_after) = header_lefts(visual);
    assert_eq!(pinned_after, pinned_before, "`id` stays at the left edge");
    assert_eq!(
        second_after,
        second_before.map(|left| left - px(100.0)),
        "`a`, past the cap, scrolls"
    );
}
