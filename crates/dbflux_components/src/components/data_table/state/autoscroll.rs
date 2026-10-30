//! Scrolling the grid while a drag selection is held past the edge of its
//! body, so a range can grow beyond the rows and columns on screen.

use std::time::Duration;

use gpui::{Bounds, Context, Pixels, Point, Task, px};

use super::DataTableState;
use crate::components::data_table::selection::CellCoord;
use crate::components::data_table::theme::ROW_NUMBER_WIDTH;

/// Time between two scroll steps while the pointer stays past an edge.
const TICK: Duration = Duration::from_millis(16);

/// Scroll step per tick right past the edge. Never zero, so the grid moves
/// as soon as the pointer leaves the body.
const MIN_STEP: f32 = 4.0;

/// Extra step per pixel the pointer is past the edge: the further out, the
/// faster, as in a database IDE's grid.
const STEP_PER_PIXEL: f32 = 0.5;

/// Fastest step, reached about a hundred pixels out.
const MAX_STEP: f32 = 60.0;

/// Where the pointer of a running drag selection is, relative to the grid
/// body, and the task scrolling the grid while it is past an edge.
pub(super) struct DragAutoscroll {
    pointer: Point<Pixels>,
    body: Bounds<Pixels>,
    row_height: Pixels,
    task: Option<Task<()>>,
}

/// Pixels to scroll per tick along each axis for a pointer at `pointer`:
/// negative towards the start (up, left), positive towards the end, zero
/// while it is inside `body` on that axis.
fn edge_step(pointer: Point<Pixels>, body: Bounds<Pixels>) -> Point<Pixels> {
    let axis = |position: Pixels, start: Pixels, end: Pixels| {
        if position < start {
            -step_for(start - position)
        } else if position > end {
            step_for(position - end)
        } else {
            px(0.0)
        }
    };
    Point::new(
        axis(pointer.x, body.left(), body.right()),
        axis(pointer.y, body.top(), body.bottom()),
    )
}

fn step_for(distance: Pixels) -> Pixels {
    px((MIN_STEP + f32::from(distance) * STEP_PER_PIXEL).min(MAX_STEP))
}

impl DataTableState {
    /// Whether the left button, pressed on a cell, is still held.
    pub fn is_drag_selecting(&self) -> bool {
        self.drag_selecting
    }

    /// The pointer of a running drag selection moved to `pointer`, anywhere
    /// in the window. Past an edge of `body` (the visible cell area) the grid
    /// keeps scrolling that way until the pointer comes back or the button is
    /// released; inside it the cells' own move handlers extend the selection.
    pub fn drag_pointer_moved(
        &mut self,
        pointer: Point<Pixels>,
        body: Bounds<Pixels>,
        row_height: Pixels,
        cx: &mut Context<Self>,
    ) {
        if !self.drag_selecting {
            return;
        }

        let past_edge = edge_step(pointer, body) != Point::default();
        let task = self
            .drag_autoscroll
            .take()
            .and_then(|autoscroll| autoscroll.task)
            .filter(|_| past_edge);
        let task = match task {
            Some(task) => Some(task),
            None if past_edge => Some(Self::spawn_autoscroll(cx)),
            None => None,
        };

        self.drag_autoscroll = Some(DragAutoscroll {
            pointer,
            body,
            row_height,
            task,
        });
    }

    fn spawn_autoscroll(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let keep_going = this
                    .update(cx, |state, cx| state.autoscroll_tick(cx))
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
    }

    /// One scroll step towards the edge the pointer is past, then the
    /// selection extended to the cell now under the pointer, clamped into the
    /// body. Returns whether the pointer is still past an edge.
    fn autoscroll_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(autoscroll) = self.drag_autoscroll.as_mut() else {
            return false;
        };
        let step = edge_step(autoscroll.pointer, autoscroll.body);
        if !self.drag_selecting || step == Point::default() {
            // This runs inside the task itself: detached, it ends on its own
            // when this returns, and the next move past an edge starts anew.
            if let Some(task) = autoscroll.task.take() {
                task.detach();
            }
            return false;
        }
        let (pointer, body, row_height) =
            (autoscroll.pointer, autoscroll.body, autoscroll.row_height);

        if step.y != px(0.0) {
            let base = self.vertical_scroll_handle.0.borrow().base_handle.clone();
            let offset = base.offset();
            let max_y = base.max_offset().y.max(px(0.0));
            let y = (offset.y - step.y).clamp(-max_y, px(0.0));
            base.set_offset(Point::new(offset.x, y));
        }
        if step.x != px(0.0) && self.apply_horizontal_wheel_delta(-step.x) {
            self.sync_horizontal_offset(cx);
        }

        if let Some(coord) = self.cell_at(pointer, body, row_height)
            && self.selection.active != Some(coord)
        {
            self.extend_selection(coord, cx);
        }
        cx.notify();
        true
    }

    /// The cell drawn under `pointer`, or the nearest one inside `body`
    /// when the pointer is outside it.
    fn cell_at(
        &self,
        pointer: Point<Pixels>,
        body: Bounds<Pixels>,
        row_height: Pixels,
    ) -> Option<CellCoord> {
        let row_count = self.row_count();
        let col_count = self.col_count();
        if row_count == 0
            || col_count == 0
            || row_height <= px(0.0)
            || body.size.width < px(1.0)
            || body.size.height < px(1.0)
        {
            return None;
        }

        let inside_x = pointer.x.clamp(body.left(), body.right() - px(1.0));
        let inside_y = pointer.y.clamp(body.top(), body.bottom() - px(1.0));

        let scroll_y = self
            .vertical_scroll_handle
            .0
            .borrow()
            .base_handle
            .offset()
            .y;
        let content_y = f32::from(inside_y - body.top() - scroll_y);
        let row = ((content_y / f32::from(row_height)).max(0.0) as usize).min(row_count - 1);

        // Rows are drawn shifted left by the horizontal offset, row number
        // first, except for the pinned strip at the left edge, which does not
        // scroll. Offsets run in screen order, so the hit is a screen
        // position, mapped back to the model column the selection holds.
        let screen_x = f32::from(inside_x - body.left() - ROW_NUMBER_WIDTH);
        let in_pinned_strip =
            self.column_layout.pinned_count() > 0 && screen_x < self.pinned_width();
        let content_x = if in_pinned_strip {
            screen_x
        } else {
            screen_x + f32::from(self.horizontal_offset)
        };
        let last_position = self.column_layout.visible_count().checked_sub(1)?;
        let position = self
            .column_offsets
            .partition_point(|offset| *offset <= content_x)
            .saturating_sub(1)
            .min(last_position);
        let col = self.column_layout.model_column(position)?;

        Some(CellCoord::new(row, col))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    fn body() -> Bounds<Pixels> {
        Bounds::new(point(px(100.0), px(100.0)), size(px(400.0), px(300.0)))
    }

    #[test]
    fn inside_the_body_nothing_scrolls() {
        assert_eq!(
            edge_step(point(px(300.0), px(200.0)), body()),
            Point::default()
        );
    }

    #[test]
    fn past_an_edge_the_grid_scrolls_that_way() {
        let below = edge_step(point(px(300.0), px(410.0)), body());
        assert_eq!(below.x, px(0.0));
        assert!(below.y > px(0.0), "below the bottom scrolls down");

        let above_left = edge_step(point(px(90.0), px(90.0)), body());
        assert!(above_left.x < px(0.0), "left of the body scrolls left");
        assert!(above_left.y < px(0.0), "above the body scrolls up");
    }

    #[test]
    fn the_further_out_the_faster_up_to_a_limit() {
        let near = edge_step(point(px(300.0), px(402.0)), body()).y;
        let far = edge_step(point(px(300.0), px(460.0)), body()).y;
        let very_far = edge_step(point(px(300.0), px(2000.0)), body()).y;
        assert!(far > near);
        assert_eq!(very_far, px(MAX_STEP));
    }
}

#[cfg(test)]
mod layout_tests {
    use std::sync::Arc;

    use gpui::{AppContext as _, TestAppContext, point, px, size};

    use super::super::DataTableState;
    use crate::components::data_table::model::{
        CellValue, ColumnKind, ColumnSpec, RowData, TableModel,
    };
    use crate::components::data_table::selection::CellCoord;
    use crate::components::data_table::theme::ROW_NUMBER_WIDTH;

    /// The cell under a dragged pointer is found by screen position: a
    /// column pinned to the left edge is hit where the strip draws it, and
    /// a scrolling column through the scroll, as the model column it is.
    #[gpui::test]
    fn the_drag_hit_test_follows_moved_and_pinned_columns(cx: &mut TestAppContext) {
        let columns = ["a", "b", "c", "d"]
            .iter()
            .map(|name| ColumnSpec {
                id: (*name).into(),
                title: (*name).into(),
                kind: ColumnKind::Text,
                align: gpui::TextAlign::Left,
                type_name: "text".into(),
            })
            .collect();
        let rows = (0..3)
            .map(|_| RowData {
                cells: (0..4).map(|_| CellValue::text("x")).collect(),
            })
            .collect();
        let model = Arc::new(TableModel::new(columns, rows));

        let state = cx.new(|cx| DataTableState::new(model, cx));
        let hit = cx.update(|cx| {
            state.update(cx, |state, cx| {
                for col in 0..4 {
                    state.set_column_width(col, 100.0, cx);
                }
                // Screen order c (pinned), a, b, d, scrolled 150 px.
                state.pin_column(2, cx);
                state.horizontal_offset = px(150.0);

                let body = gpui::Bounds::new(point(px(0.0), px(0.0)), size(px(600.0), px(300.0)));
                let at = |x: f32| point(ROW_NUMBER_WIDTH + px(x), px(5.0));
                (
                    state.cell_at(at(50.0), body, px(20.0)),
                    state.cell_at(at(130.0), body, px(20.0)),
                )
            })
        });

        assert_eq!(hit.0, Some(CellCoord::new(0, 2)), "inside the pinned strip");
        assert_eq!(
            hit.1,
            Some(CellCoord::new(0, 1)),
            "130 px plus the 150 px scroll lands on `b`, the third on screen"
        );
    }
}
