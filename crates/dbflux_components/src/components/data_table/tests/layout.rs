use gpui::{Modifiers, MouseButton, TestAppContext};

use super::super::model::CellValue;
use super::super::selection::CellCoord;
use super::super::{DataTableState, ModelSwap};
use super::{model, render_table};

fn names_model(rows: usize) -> std::sync::Arc<super::super::model::TableModel> {
    model(&["id", "name", "email", "created_at"], rows, |row, col| {
        CellValue::text(&format!("r{row}c{col}"))
    })
}

/// Left edges of the drawn headers, by model column; `None` for a header
/// that is not drawn.
fn header_lefts(
    visual: &mut gpui::VisualTestContext,
    col_count: usize,
) -> Vec<Option<gpui::Pixels>> {
    (0..col_count)
        .map(|col| {
            visual
                .debug_bounds(Box::leak(format!("header-col-{col}").into_boxed_str()))
                .map(|bounds| bounds.origin.x)
        })
        .collect()
}

/// A hidden column draws no header, a moved one draws where it was moved,
/// and the arrow keys walk the columns in that screen order.
#[gpui::test]
fn hidden_and_moved_columns_follow_the_screen_order(cx: &mut TestAppContext) {
    let (state, visual) = render_table(cx, names_model(3));

    visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            assert!(state.hide_column(1, cx));
            assert!(state.move_column(3, 0, cx));
        });
    });
    visual.run_until_parked();

    let lefts = header_lefts(visual, 4);
    assert_eq!(lefts[1], None, "the hidden `name` draws no header");
    let (id, email, created) = (
        lefts[0].expect("id"),
        lefts[2].expect("email"),
        lefts[3].expect("created_at"),
    );
    assert!(
        created < id && id < email,
        "screen order created_at, id, email"
    );

    let walk = visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.select_cell(CellCoord::new(0, 3), cx);
            let mut cols = vec![3];
            for _ in 0..3 {
                state.move_active(super::super::Direction::Right, false, cx);
                cols.push(state.selection().active.map_or(usize::MAX, |cell| cell.col));
            }
            cols
        })
    });
    assert_eq!(
        walk,
        [3, 0, 2, 2],
        "Right walks created_at, id, email and stops"
    );
}

/// Dragging a header onto another moves its column into that place.
#[gpui::test]
fn dragging_a_header_onto_another_moves_the_column(cx: &mut TestAppContext) {
    let (state, visual) = render_table(cx, names_model(3));

    let source = visual
        .debug_bounds("header-col-2")
        .expect("the `email` header is drawn");
    let target = visual
        .debug_bounds("header-col-0")
        .expect("the `id` header is drawn");
    let start = gpui::point(source.origin.x + gpui::px(10.0), source.center().y);
    let end = gpui::point(target.origin.x + gpui::px(10.0), target.center().y);

    visual.simulate_event(gpui::MouseDownEvent {
        position: start,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    for step in 1..=4 {
        let x = start.x + (end.x - start.x) * (step as f32 / 4.0);
        visual.simulate_event(gpui::MouseMoveEvent {
            position: gpui::point(x, start.y),
            modifiers: Modifiers::default(),
            pressed_button: Some(MouseButton::Left),
        });
    }
    visual.simulate_event(gpui::MouseUpEvent {
        position: end,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 1,
    });
    visual.run_until_parked();

    let order = visual.update(|_, cx| state.read(cx).visible_columns().to_vec());
    assert_eq!(order, [2, 0, 1, 3], "`email` takes the place of `id`");
}

/// Copy reads the selection in screen order without the hidden column,
/// and a reload of the same columns keeps the layout.
#[gpui::test]
fn copy_follows_the_screen_and_a_reload_keeps_the_layout(cx: &mut TestAppContext) {
    let (state, visual) = render_table(cx, names_model(2));

    let copied = visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.hide_column(1, cx);
            state.move_column(3, 0, cx);
            state.select_all(cx);
            state.copy_selection()
        })
    });
    assert_eq!(
        copied.as_deref(),
        Some("r0c3\tr0c0\tr0c2\nr1c3\tr1c0\tr1c2"),
        "columns as shown: created_at, id, email"
    );

    // The same columns, reordered in the model, come back with the layout.
    let reloaded = model(&["created_at", "email", "name", "id"], 1, |_, col| {
        CellValue::int(col as i64)
    });
    let shown = visual.update(|_, cx| {
        state.update(cx, |state: &mut DataTableState, cx| {
            state.set_model(reloaded, ModelSwap::KeepCursor, cx);
            state.visible_columns().to_vec()
        })
    });
    assert_eq!(
        shown,
        [0, 3, 1],
        "created_at, id, email; `name` stays hidden"
    );
}
