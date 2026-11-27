//! One value typed into the editor written to every editable cell of a
//! selection, as a database IDE's grid does.

use gpui::{Context, Modifiers};

use super::DataTableState;
use crate::components::data_table::events::DataTableEvent;
use crate::components::data_table::model::{CellValue, ColumnKind, VisualRowSource};
use crate::components::data_table::selection::{CellCoord, CellRange, SelectionState};

impl DataTableState {
    /// How many cells the open editor writes to when committed: two or more
    /// while it edits a selection of several cells, zero otherwise.
    pub fn multi_edit_count(&self) -> usize {
        if self.editing_cell.is_some() && self.edit_targets.len() > 1 {
            self.edit_targets.len()
        } else {
            0
        }
    }

    /// The cells an edit started at `coord` writes to: `coord` first, then
    /// every other selected cell that takes the same kind of value. Empty
    /// when `coord` is outside the selection or the only cell in it.
    ///
    /// Skipped are columns of another type than `coord`'s (text typed into
    /// a name must not land in an integer or date column), key columns when
    /// the selection spans several rows (one value in every row's key could
    /// never be saved), rows marked for deletion, read-only columns, rows the
    /// table cannot edit, values the inline editor does not edit (JSON,
    /// long or multi-line text, unsupported types, nested documents), and
    /// columns whose list of allowed values differs from `coord`'s, since a
    /// dropdown choice is only meaningful among the same options.
    pub(super) fn multi_edit_targets(&self, coord: CellCoord) -> Vec<CellCoord> {
        if !self.selection.is_selected(coord) || selected_cell_count(&self.selection) < 2 {
            return Vec::new();
        }

        let visual_order = self.edit_buffer.compute_visual_order();
        let row_count = visual_order.len();
        let col_count = self.col_count();
        let ranges: Vec<CellRange> = self.selection.ranges();
        let options = self.enum_options.get(&coord.col);
        let kind_of = |col: usize| self.model.columns.get(col).map(|column| column.kind);
        let anchor_kind = kind_of(coord.col);

        let mut others = Vec::new();
        for (index, range) in ranges.iter().enumerate() {
            for target in range.iter() {
                if target == coord
                    || target.row >= row_count
                    || target.col >= col_count
                    || ranges
                        .iter()
                        .take(index)
                        .any(|other| other.contains(target))
                {
                    continue;
                }
                if kind_of(target.col) == anchor_kind
                    && self.enum_options.get(&target.col) == options
                    && self.takes_multi_edit(target, &visual_order)
                {
                    others.push(target);
                }
            }
        }

        if others.iter().any(|target| target.row != coord.row) {
            others.retain(|target| !self.pk_columns.contains(&target.col));
        }

        if others.is_empty() {
            return Vec::new();
        }
        let mut targets = Vec::with_capacity(others.len() + 1);
        targets.push(coord);
        targets.extend(others);
        targets
    }

    fn takes_multi_edit(&self, target: CellCoord, visual_order: &[VisualRowSource]) -> bool {
        if self.readonly_columns.contains(&target.col) {
            return false;
        }
        if self
            .model
            .columns
            .get(target.col)
            .is_some_and(|column| column.kind == ColumnKind::Json)
        {
            return false;
        }

        let cell = match visual_order.get(target.row).copied() {
            Some(VisualRowSource::Base(base_idx)) => {
                if !self.is_editable || self.edit_buffer.is_pending_delete(base_idx) {
                    return false;
                }
                self.model
                    .cell(base_idx, target.col)
                    .map(|base| self.edit_buffer.get_cell(base_idx, target.col, base))
            }
            Some(VisualRowSource::Insert(insert_idx)) => {
                if !(self.is_insertable || self.is_editable) {
                    return false;
                }
                self.edit_buffer
                    .get_pending_insert_by_idx(insert_idx)
                    .and_then(|data| data.get(target.col))
            }
            None => return false,
        };

        cell.is_some_and(|cell| {
            !cell.is_unsupported()
                && !cell.is_json()
                && !cell.needs_modal_editor()
                && !cell.is_nested()
        })
    }

    /// Stage `value` for the cell the editor was open on, or, when the edit
    /// covered several cells, for each of them as one undo step.
    pub(super) fn stage_edit_value(&mut self, coord: CellCoord, value: CellValue) {
        let targets = std::mem::take(&mut self.edit_targets);
        if targets.len() < 2 {
            self.stage_cell_value(coord.row, coord.col, value);
            return;
        }

        // Resolved once: `stage_cell_value` computes the visual order per
        // call, which over a whole selected column would be quadratic.
        let visual_order = self.edit_buffer.compute_visual_order();
        self.edit_buffer.begin_undo_group();
        for target in targets {
            match visual_order.get(target.row).copied() {
                Some(VisualRowSource::Base(base_idx)) => {
                    self.stage_base_cell_value(base_idx, target.col, value.clone());
                }
                Some(VisualRowSource::Insert(insert_idx)) => {
                    self.edit_buffer
                        .set_insert_cell(insert_idx, target.col, value.clone());
                }
                None => {}
            }
        }
        self.edit_buffer.end_undo_group();
    }

    /// Left button pressed on a cell, with the press's click count.
    ///
    /// The first press of a double-click on a cell of a larger selection
    /// collapses it to that cell, as any plain press does; the second press
    /// brings the selection back so the edit the double-click opens covers
    /// all of it. The second press of a Ctrl+double-click changes nothing:
    /// the release opens the editor, and toggling again would take the cell
    /// back out first.
    pub fn press_cell(
        &mut self,
        coord: CellCoord,
        modifiers: Modifiers,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        let before = self.selection_before_press.take();
        if click_count >= 2
            && let Some((pressed, selection)) = before
            && pressed == coord
        {
            self.selection = selection;
            cx.emit(DataTableEvent::SelectionChanged(self.selection.clone()));
            cx.notify();
            return;
        }
        if click_count >= 2 && modifiers.secondary() {
            return;
        }

        let plain = modifiers == Modifiers::default();
        if plain && self.selection.is_selected(coord) && selected_cell_count(&self.selection) > 1 {
            self.selection_before_press = Some((coord, self.selection.clone()));
        }
        self.begin_drag_select(coord, modifiers, cx);
    }
}

/// Selected cells counted range by range, stopping once there are two:
/// enough to tell one cell from several.
fn selected_cell_count(selection: &SelectionState) -> usize {
    let mut count = 0;
    for range in selection.ranges() {
        count += range.row_count() * range.col_count();
        if count >= 2 {
            return 2;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use gpui::{
        AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, TextAlign,
        VisualTestContext, Window, div,
    };

    use super::DataTableState;
    use crate::components::data_table::model::{
        CellValue, ColumnKind, ColumnSpec, RowData, TableModel,
    };
    use crate::components::data_table::selection::CellCoord;

    struct Harness {
        _state: Entity<DataTableState>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn column(title: &str, kind: ColumnKind) -> ColumnSpec {
        ColumnSpec {
            id: title.into(),
            title: title.into(),
            kind,
            align: TextAlign::Left,
            type_name: "".into(),
        }
    }

    /// Four rows of `id` (key), `name` and `note`; `note` is read-only.
    fn editable_state(cx: &mut TestAppContext) -> (Entity<DataTableState>, &mut VisualTestContext) {
        let holder = std::rc::Rc::new(std::cell::RefCell::new(None));
        let holder_for_view = holder.clone();
        let (_, window) = cx.add_window_view(move |_window, cx| {
            let rows = (0..4)
                .map(|row| RowData {
                    cells: vec![
                        CellValue::int(row),
                        CellValue::text(&format!("name {row}")),
                        CellValue::text("note"),
                    ],
                })
                .collect();
            let model = Arc::new(TableModel::new(
                vec![
                    column("id", ColumnKind::Integer),
                    column("name", ColumnKind::Text),
                    column("note", ColumnKind::Text),
                ],
                rows,
            ));
            let state = cx.new(|cx| {
                let mut state = DataTableState::new(model, cx);
                state.set_pk_columns(vec![0]);
                state.set_readonly_columns(HashSet::from([2]));
                state
            });
            holder_for_view.replace(Some(state.clone()));
            Harness { _state: state }
        });
        let state = holder.borrow().clone().expect("state entity");
        (state, window)
    }

    /// Opens the editor on `coord`, types `text` and commits it with Enter's path.
    fn edit(
        state: &Entity<DataTableState>,
        window: &mut VisualTestContext,
        coord: CellCoord,
        text: &str,
    ) -> usize {
        let (input, count) = window.update(|window, cx| {
            state.update(cx, |state, cx| {
                assert!(state.start_editing(coord, window, cx));
                (
                    state.cell_input().cloned().expect("an inline editor"),
                    state.multi_edit_count(),
                )
            })
        });
        window.update(|window, cx| input.update(cx, |input, cx| input.set_value(text, window, cx)));
        window.update(|_, cx| state.update(cx, |state, cx| state.stop_editing(true, cx)));
        count
    }

    fn dirty(
        state: &Entity<DataTableState>,
        window: &mut VisualTestContext,
    ) -> Vec<(usize, usize)> {
        window.update(|_, cx| {
            let buffer = state.read(cx).edit_buffer();
            let mut cells: Vec<(usize, usize)> = (0..4)
                .flat_map(|row| (0..3).map(move |col| (row, col)))
                .filter(|&(row, col)| buffer.is_cell_dirty(row, col))
                .collect();
            cells.sort_unstable();
            cells
        })
    }

    #[gpui::test]
    fn one_edit_writes_every_selected_cell_and_undoes_as_one(cx: &mut TestAppContext) {
        let (state, window) = editable_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.edit_buffer_mut().mark_for_delete(2);
                state.click_column(1, gpui::Modifiers::default(), cx);
            })
        });

        let count = edit(&state, window, CellCoord::new(3, 1), "same");
        assert_eq!(count, 3, "the row marked for deletion is left out");
        assert_eq!(dirty(&state, window), vec![(0, 1), (1, 1), (3, 1)]);

        window.update(|_, cx| {
            state.update(cx, |state, _| assert!(state.edit_buffer_mut().undo()));
        });
        assert_eq!(
            dirty(&state, window),
            vec![],
            "one undo takes back the whole edit"
        );

        window.update(|_, cx| {
            state.update(cx, |state, _| assert!(state.edit_buffer_mut().redo()));
        });
        assert_eq!(dirty(&state, window), vec![(0, 1), (1, 1), (3, 1)]);

        // The delete marked before the edit is still the next step back.
        window.update(|_, cx| {
            state.update(cx, |state, _| {
                assert!(state.edit_buffer_mut().undo());
                assert!(state.edit_buffer_mut().undo());
                assert!(!state.edit_buffer().is_pending_delete(2));
            });
        });
    }

    #[gpui::test]
    fn read_only_columns_are_skipped(cx: &mut TestAppContext) {
        let (state, window) = editable_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.click_cell(CellCoord::new(0, 1), gpui::Modifiers::default(), cx);
                state.click_cell(
                    CellCoord::new(1, 2),
                    gpui::Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                    cx,
                );
            })
        });

        let count = edit(&state, window, CellCoord::new(1, 1), "x");
        assert_eq!(count, 2, "two editable cells in the range");
        assert_eq!(dirty(&state, window), vec![(0, 1), (1, 1)]);
    }

    #[gpui::test]
    fn a_single_selected_cell_edits_alone(cx: &mut TestAppContext) {
        let (state, window) = editable_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, cx| state.select_cell(CellCoord::new(1, 1), cx))
        });

        let count = edit(&state, window, CellCoord::new(1, 1), "x");
        assert_eq!(count, 0);
        assert_eq!(dirty(&state, window), vec![(1, 1)]);
    }

    /// A table of `columns` holding `rows`, set up by `setup`.
    fn state_with(
        cx: &mut TestAppContext,
        columns: Vec<ColumnSpec>,
        rows: Vec<Vec<CellValue>>,
        setup: impl FnOnce(&mut DataTableState) + 'static,
    ) -> (Entity<DataTableState>, &mut VisualTestContext) {
        let holder = std::rc::Rc::new(std::cell::RefCell::new(None));
        let holder_for_view = holder.clone();
        let (_, window) = cx.add_window_view(move |_window, cx| {
            let rows = rows.into_iter().map(|cells| RowData { cells }).collect();
            let model = Arc::new(TableModel::new(columns, rows));
            let state = cx.new(|cx| {
                let mut state = DataTableState::new(model, cx);
                setup(&mut state);
                state
            });
            holder_for_view.replace(Some(state.clone()));
            Harness { _state: state }
        });
        let state = holder.borrow().clone().expect("state entity");
        (state, window)
    }

    /// Selects the cells from `from` to `to`, as a click and a Shift+click.
    fn select(
        state: &Entity<DataTableState>,
        window: &mut VisualTestContext,
        from: CellCoord,
        to: CellCoord,
    ) {
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.click_cell(from, gpui::Modifiers::default(), cx);
                state.click_cell(
                    to,
                    gpui::Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                    cx,
                );
            })
        });
    }

    fn staged_text(
        state: &Entity<DataTableState>,
        window: &mut VisualTestContext,
        row: usize,
        col: usize,
    ) -> Option<String> {
        window.update(|_, cx| {
            let state = state.read(cx);
            let base = state.model.cell(row, col)?;
            state.edit_buffer().is_cell_dirty(row, col).then(|| {
                state
                    .edit_buffer()
                    .get_cell(row, col, base)
                    .display_text()
                    .to_string()
            })
        })
    }

    #[gpui::test]
    fn columns_of_another_type_are_skipped(cx: &mut TestAppContext) {
        let (state, window) = editable_state(cx);
        select(&state, window, CellCoord::new(0, 0), CellCoord::new(3, 1));

        let count = edit(&state, window, CellCoord::new(1, 1), "same");
        assert_eq!(count, 4, "the integer id column takes no text");
        assert_eq!(dirty(&state, window), vec![(0, 1), (1, 1), (2, 1), (3, 1)]);
    }

    fn text_key_columns() -> Vec<ColumnSpec> {
        vec![
            column("code", ColumnKind::Text),
            column("name", ColumnKind::Text),
        ]
    }

    fn text_key_rows() -> Vec<Vec<CellValue>> {
        (0..2)
            .map(|row| {
                vec![
                    CellValue::text(&format!("k{row}")),
                    CellValue::text(&format!("name {row}")),
                ]
            })
            .collect()
    }

    #[gpui::test]
    fn key_columns_are_skipped_when_the_edit_spans_rows(cx: &mut TestAppContext) {
        let (state, window) = state_with(cx, text_key_columns(), text_key_rows(), |state| {
            state.set_pk_columns(vec![0])
        });
        select(&state, window, CellCoord::new(0, 0), CellCoord::new(1, 1));

        let count = edit(&state, window, CellCoord::new(0, 1), "same");
        assert_eq!(
            count, 2,
            "one value in every row's key could never be saved"
        );
        assert_eq!(staged_text(&state, window, 0, 0), None);
        assert_eq!(staged_text(&state, window, 1, 0), None);
        assert_eq!(staged_text(&state, window, 1, 1).as_deref(), Some("same"));
    }

    #[gpui::test]
    fn key_columns_of_the_edited_row_alone_are_written(cx: &mut TestAppContext) {
        let (state, window) = state_with(cx, text_key_columns(), text_key_rows(), |state| {
            state.set_pk_columns(vec![0])
        });
        select(&state, window, CellCoord::new(0, 0), CellCoord::new(0, 1));

        let count = edit(&state, window, CellCoord::new(0, 1), "same");
        assert_eq!(count, 2, "one row cannot collide with another row's key");
        assert_eq!(staged_text(&state, window, 0, 0).as_deref(), Some("same"));
    }

    #[gpui::test]
    fn pending_inserts_in_the_selection_are_written(cx: &mut TestAppContext) {
        let (state, window) = editable_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.edit_buffer_mut().add_pending_insert(vec![
                    CellValue::null(),
                    CellValue::text(""),
                    CellValue::text("note"),
                ]);
                state.click_column(1, gpui::Modifiers::default(), cx);
            })
        });

        let count = edit(&state, window, CellCoord::new(0, 1), "same");
        assert_eq!(count, 5, "four rows and the pending insert after them");
        let inserted = window.update(|_, cx| {
            state
                .read(cx)
                .edit_buffer()
                .get_pending_insert_by_idx(0)
                .and_then(|cells| cells.get(1))
                .map(|cell| cell.display_text().to_string())
        });
        assert_eq!(inserted.as_deref(), Some("same"));
    }

    #[gpui::test]
    fn values_the_inline_editor_does_not_edit_are_skipped(cx: &mut TestAppContext) {
        let rows = vec![
            vec![CellValue::text("x"), CellValue::text("line 1\nline 2")],
            vec![CellValue::text("y"), CellValue::json("{\"a\": 1}")],
            vec![CellValue::text("z"), CellValue::nested(false, 2)],
        ];
        let (state, window) = state_with(
            cx,
            vec![column("a", ColumnKind::Text), column("b", ColumnKind::Text)],
            rows,
            |state| state.set_positional_editing(true),
        );
        select(&state, window, CellCoord::new(0, 0), CellCoord::new(2, 1));

        let count = edit(&state, window, CellCoord::new(0, 0), "v");
        assert_eq!(
            count, 3,
            "multi-line text, JSON and a nested document are left out"
        );
        for row in 0..3 {
            assert_eq!(staged_text(&state, window, row, 0).as_deref(), Some("v"));
            assert_eq!(staged_text(&state, window, row, 1), None);
        }
    }

    /// A dropdown choice goes to every selected cell of a column with the
    /// same allowed values, also when the dropdown reports its dismissal
    /// before the selection, as its auto-close can.
    #[gpui::test]
    fn an_enum_choice_is_written_to_columns_with_the_same_values(cx: &mut TestAppContext) {
        use crate::controls::{DropdownDismissed, DropdownItem, DropdownSelectionChanged};

        let rows = (0..2)
            .map(|_| {
                vec![
                    CellValue::text("a"),
                    CellValue::text("a"),
                    CellValue::text("a"),
                ]
            })
            .collect();
        let (state, window) = state_with(
            cx,
            vec![
                column("status", ColumnKind::Text),
                column("backup", ColumnKind::Text),
                column("other", ColumnKind::Text),
            ],
            rows,
            |state| {
                state.set_positional_editing(true);
                let options = vec!["a".to_string(), "b".to_string()];
                state.set_enum_options(0, options.clone());
                state.set_enum_options(1, options);
                state.set_enum_options(2, vec!["a".to_string(), "c".to_string()]);
            },
        );
        select(&state, window, CellCoord::new(0, 0), CellCoord::new(1, 2));

        let (dropdown, count) = window.update(|window, cx| {
            state.update(cx, |state, cx| {
                assert!(state.start_editing(CellCoord::new(0, 0), window, cx));
                (
                    state.enum_dropdown().cloned().expect("an enum dropdown"),
                    state.multi_edit_count(),
                )
            })
        });
        assert_eq!(count, 4, "the column with other allowed values is left out");

        window.update(|_, cx| {
            dropdown.update(cx, |_, cx| {
                cx.emit(DropdownDismissed);
                cx.emit(DropdownSelectionChanged {
                    index: 1,
                    item: DropdownItem::new("b"),
                });
            })
        });
        window.run_until_parked();

        for row in 0..2 {
            assert_eq!(staged_text(&state, window, row, 0).as_deref(), Some("b"));
            assert_eq!(staged_text(&state, window, row, 1).as_deref(), Some("b"));
            assert_eq!(staged_text(&state, window, row, 2), None);
        }
        window.update(|_, cx| {
            state.update(cx, |state, _| {
                assert_eq!(state.multi_edit_count(), 0, "the edit is closed");
                assert!(state.edit_buffer_mut().undo());
                assert!(
                    !state.edit_buffer().has_changes(),
                    "one undo takes back the whole choice"
                );
            })
        });
    }
}
