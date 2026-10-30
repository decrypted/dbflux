//! Editing a cell as a SQL expression (`NOW()`, `"qty" + 1`) instead of a
//! value, with expressions suggested for the column's type.

use std::collections::HashMap;

use gpui::{Context, Window};

use super::DataTableState;
use crate::components::data_table::model::VisualRowSource;
use crate::components::data_table::selection::CellCoord;

/// An entry of the list under the inline editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorSuggestion {
    /// A new identifier of this kind, written as a value.
    GeneratedId(dbflux_core::GeneratedIdKind),
    /// A SQL expression, written while `fx` is on.
    Expression(String),
}

impl DataTableState {
    /// Sets the expressions offered per column for a cell edited as a SQL
    /// expression, and with it which columns can hold one. An empty map
    /// turns expression editing off; a new model clears it.
    pub fn set_cell_expression_suggestions(&mut self, suggestions: HashMap<usize, Vec<String>>) {
        self.expression_suggestions = suggestions;
    }

    /// Whether a cell of `col` can be set to a SQL expression.
    pub fn accepts_cell_expressions(&self, col: usize) -> bool {
        self.expression_suggestions.contains_key(&col) && !self.readonly_columns.contains(&col)
    }

    /// The expressions offered for a cell of `col`.
    pub fn cell_expression_suggestions(&self, col: usize) -> &[String] {
        self.expression_suggestions
            .get(&col)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Whether the open inline editor writes a SQL expression.
    pub fn is_editing_expression(&self) -> bool {
        self.editing_expression && self.cell_input.is_some()
    }

    /// The suggestion the arrow keys last stepped to in the open expression
    /// editor.
    pub fn expression_highlight(&self) -> Option<usize> {
        self.expression_highlight
    }

    /// The expression staged for the cell the table shows at `coord`.
    pub(super) fn staged_expression(&self, coord: CellCoord) -> Option<String> {
        let cell = match self.edit_buffer.visual_row_source(coord.row)? {
            VisualRowSource::Base(base_idx) => {
                if !self.edit_buffer.is_cell_dirty(base_idx, coord.col) {
                    return None;
                }
                let base = self.model.cell(base_idx, coord.col)?;
                self.edit_buffer.get_cell(base_idx, coord.col, base)
            }
            VisualRowSource::Insert(insert_idx) => self
                .edit_buffer
                .get_pending_insert_by_idx(insert_idx)?
                .get(coord.col)?,
        };

        cell.sql_expression().map(str::to_string)
    }

    /// Opens the inline editor on `coord` to type a SQL expression, starting
    /// from the expression staged there, if any. Like any inline edit it
    /// covers the whole selection when `coord` is part of a larger one.
    ///
    /// Returns false when the cell cannot be edited or its column cannot
    /// hold an expression.
    pub fn start_editing_expression(
        &mut self,
        coord: CellCoord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts_cell_expressions(coord.col) || !self.can_edit_row(coord.row) {
            return false;
        }

        let initial = self.staged_expression(coord).unwrap_or_default();

        self.edit_targets.clear();
        self.open_inline_input(coord, &initial, window, cx);
        self.editing_expression = true;
        self.expression_highlight = None;
        cx.notify();
        true
    }

    /// The `fx` switch: flips the open text editor between a value and an
    /// expression, keeping what was typed. With no editor open, or with the
    /// dropdown of an enum column open, it opens the expression editor on
    /// the active cell.
    pub fn toggle_cell_expression(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(coord) = self.editing_cell.or(self.selection.active) else {
            return;
        };

        if self.cell_input.is_some() {
            if self.accepts_cell_expressions(coord.col) {
                self.editing_expression = !self.editing_expression;
                self.expression_highlight = None;
                cx.notify();
            }
            return;
        }

        if self.enum_dropdown.is_some() {
            if !self.accepts_cell_expressions(coord.col) {
                return;
            }
            // Opening the text editor drops the dropdown's subscriptions, so
            // its dismissal cannot close the editor that replaces it.
            self.enum_dropdown = None;
            self.editing_cell = None;
        }

        self.start_editing_expression(coord, window, cx);
    }

    /// What the list under the open inline editor offers for `col`: the
    /// identifiers it can generate, then, while `fx` is on, the expressions
    /// suggested for it.
    pub fn editor_suggestions(&self, col: usize) -> Vec<EditorSuggestion> {
        let ids = self
            .editor_id_kinds(col)
            .iter()
            .copied()
            .map(EditorSuggestion::GeneratedId);
        let expressions: &[String] = if self.is_editing_expression() {
            self.cell_expression_suggestions(col)
        } else {
            &[]
        };

        ids.chain(
            expressions
                .iter()
                .cloned()
                .map(EditorSuggestion::Expression),
        )
        .collect()
    }

    /// Steps to the next (or previous) entry of the list under the open
    /// inline editor and writes it into the editor.
    pub fn step_expression_suggestion(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(coord) = self.editing_cell.filter(|_| self.cell_input.is_some()) else {
            return;
        };

        let count = self.editor_suggestions(coord.col).len();
        if count == 0 {
            return;
        }

        let next = match (self.expression_highlight, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(index), true) => (index + 1) % count,
            (Some(index), false) => (index + count - 1) % count,
        };

        self.apply_expression_suggestion(next, window, cx);
    }

    /// Writes entry `index` of the list under the open inline editor into
    /// it. A generated identifier is a value, so it switches `fx` off.
    pub fn apply_expression_suggestion(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(coord) = self.editing_cell.filter(|_| self.cell_input.is_some()) else {
            return;
        };
        let Some(suggestion) = self.editor_suggestions(coord.col).into_iter().nth(index) else {
            return;
        };

        let text = match suggestion {
            EditorSuggestion::GeneratedId(kind) => {
                let value = kind.generate();
                self.editing_expression = false;
                self.generated_in_editor = Some((kind, value.clone()));
                value
            }
            EditorSuggestion::Expression(expression) => {
                self.generated_in_editor = None;
                expression
            }
        };

        if let Some(input) = &self.cell_input {
            input.update(cx, |input, cx| {
                input.set_value(&text, window, cx);
                input.focus(window, cx);
            });
        }

        self.expression_highlight = Some(index);
        cx.notify();
    }

    /// Whether the row the table shows at `visual_row` can be edited: a base
    /// row of an editable table, or a pending insert.
    pub(super) fn can_edit_row(&self, visual_row: usize) -> bool {
        match self.edit_buffer.visual_row_source(visual_row) {
            Some(VisualRowSource::Base(base_idx)) => {
                self.is_editable && !self.edit_buffer.is_pending_delete(base_idx)
            }
            Some(VisualRowSource::Insert(_)) => self.is_insertable || self.is_editable,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use gpui::{
        AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, TextAlign,
        VisualTestContext, Window, div,
    };

    use super::DataTableState;
    use crate::components::data_table::events::DataTableEvent;
    use crate::components::data_table::model::{
        CellKind, CellValue, ColumnKind, ColumnSpec, RowData, TableModel,
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

    fn column(title: &str, kind: ColumnKind, type_name: &str) -> ColumnSpec {
        ColumnSpec {
            id: title.into(),
            title: title.into(),
            kind,
            align: TextAlign::Left,
            type_name: type_name.into(),
        }
    }

    /// Three rows of `id` (key), `seen_at` and `note`, where `seen_at`
    /// suggests `NOW()` and `CURRENT_TIMESTAMP` and `note` takes no
    /// expression.
    fn expression_state(
        cx: &mut TestAppContext,
    ) -> (Entity<DataTableState>, &mut VisualTestContext) {
        let holder = std::rc::Rc::new(std::cell::RefCell::new(None));
        let holder_for_view = holder.clone();
        let (_, window) = cx.add_window_view(move |_window, cx| {
            let rows = (0..3)
                .map(|row| RowData {
                    cells: vec![
                        CellValue::int(row),
                        CellValue::text("2026-01-01 00:00:00"),
                        CellValue::text("note"),
                    ],
                })
                .collect();
            let model = Arc::new(TableModel::new(
                vec![
                    column("id", ColumnKind::Integer, "int"),
                    column("seen_at", ColumnKind::Text, "datetime"),
                    column("note", ColumnKind::Text, "text"),
                ],
                rows,
            ));
            let state = cx.new(|cx| {
                let mut state = DataTableState::new(model, cx);
                state.set_pk_columns(vec![0]);
                state.set_insertable(true);
                state.set_cell_expression_suggestions(HashMap::from([
                    (0, vec!["`id` + 1".to_string()]),
                    (
                        1,
                        vec!["NOW()".to_string(), "CURRENT_TIMESTAMP".to_string()],
                    ),
                ]));
                state
            });
            holder_for_view.replace(Some(state.clone()));
            Harness { _state: state }
        });
        let state = holder.borrow().clone().expect("state entity");
        (state, window)
    }

    fn open_expression_editor(
        state: &Entity<DataTableState>,
        window: &mut VisualTestContext,
        coord: CellCoord,
    ) -> Entity<crate::controls::InputState> {
        window.update(|window, cx| {
            state.update(cx, |state, cx| {
                assert!(state.start_editing_expression(coord, window, cx));
                assert!(state.is_editing_expression());
                state.cell_input().cloned().expect("an inline editor")
            })
        })
    }

    fn type_and_commit(
        state: &Entity<DataTableState>,
        window: &mut VisualTestContext,
        input: &Entity<crate::controls::InputState>,
        text: &str,
    ) {
        window.update(|window, cx| input.update(cx, |input, cx| input.set_value(text, window, cx)));
        window.update(|_, cx| state.update(cx, |state, cx| state.stop_editing(true, cx)));
    }

    fn staged(
        state: &Entity<DataTableState>,
        window: &mut VisualTestContext,
        row: usize,
        col: usize,
    ) -> CellKind {
        window.update(|_, cx| {
            let state = state.read(cx);
            let base = state.model().cell(row, col).cloned().expect("a base cell");
            state.edit_buffer().get_cell(row, col, &base).kind.clone()
        })
    }

    #[gpui::test]
    fn an_expression_edit_stages_an_expression_and_undoes(cx: &mut TestAppContext) {
        let (state, window) = expression_state(cx);

        let input = open_expression_editor(&state, window, CellCoord::new(1, 1));
        type_and_commit(&state, window, &input, " NOW() ");

        assert!(
            matches!(staged(&state, window, 1, 1), CellKind::Expression(sql) if &*sql == "NOW()"),
            "the trimmed text is staged as an expression"
        );
        window.update(|_, cx| {
            let state = state.read(cx);
            let base = state.model().cell(1, 1).cloned().unwrap();
            assert_eq!(
                &*state.edit_buffer().get_cell(1, 1, &base).display_text(),
                "fx NOW()"
            );
        });

        window.update(|_, cx| state.update(cx, |state, _| assert!(state.edit_buffer_mut().undo())));
        assert!(
            matches!(staged(&state, window, 1, 1), CellKind::Text(_)),
            "undo restores the loaded value"
        );
        window.update(|_, cx| state.update(cx, |state, _| assert!(state.edit_buffer_mut().redo())));
        assert!(matches!(
            staged(&state, window, 1, 1),
            CellKind::Expression(_)
        ));
    }

    #[gpui::test]
    fn the_same_text_typed_as_a_value_stays_a_value(cx: &mut TestAppContext) {
        let (state, window) = expression_state(cx);

        let input = open_expression_editor(&state, window, CellCoord::new(0, 1));
        window.update(|window, cx| {
            state.update(cx, |state, cx| state.toggle_cell_expression(window, cx));
        });
        assert!(
            window.update(|_, cx| !state.read(cx).is_editing_expression()),
            "fx switches the open editor back to a value"
        );
        type_and_commit(&state, window, &input, "NOW()");

        assert!(matches!(staged(&state, window, 0, 1), CellKind::Text(text) if &*text == "NOW()"));
    }

    #[gpui::test]
    fn an_expression_goes_to_every_selected_cell_as_one_undo(cx: &mut TestAppContext) {
        let (state, window) = expression_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.click_column(1, gpui::Modifiers::default(), cx)
            })
        });

        let input = open_expression_editor(&state, window, CellCoord::new(0, 1));
        assert_eq!(window.update(|_, cx| state.read(cx).multi_edit_count()), 3);
        type_and_commit(&state, window, &input, "CURRENT_TIMESTAMP");

        for row in 0..3 {
            assert!(matches!(
                staged(&state, window, row, 1),
                CellKind::Expression(_)
            ));
        }

        window.update(|_, cx| state.update(cx, |state, _| assert!(state.edit_buffer_mut().undo())));
        for row in 0..3 {
            assert!(matches!(staged(&state, window, row, 1), CellKind::Text(_)));
        }
    }

    #[gpui::test]
    fn a_new_row_takes_an_expression(cx: &mut TestAppContext) {
        let (state, window) = expression_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, _| {
                state.edit_buffer_mut().add_pending_insert(vec![
                    CellValue::null(),
                    CellValue::null(),
                    CellValue::null(),
                ]);
            })
        });

        // The insert follows the three loaded rows.
        let input = open_expression_editor(&state, window, CellCoord::new(3, 1));
        type_and_commit(&state, window, &input, "NOW()");

        let cell = window.update(|_, cx| {
            state
                .read(cx)
                .edit_buffer()
                .get_pending_insert_by_idx(0)
                .unwrap()[1]
                .clone()
        });
        assert_eq!(cell.sql_expression(), Some("NOW()"));

        // Editing it again starts from the expression, in expression mode.
        let input = window.update(|window, cx| {
            state.update(cx, |state, cx| {
                assert!(state.start_editing(CellCoord::new(3, 1), window, cx));
                assert!(state.is_editing_expression());
                state.cell_input().cloned().unwrap()
            })
        });
        assert_eq!(
            window.update(|_, cx| input.read(cx).value().to_string()),
            "NOW()"
        );
    }

    #[gpui::test]
    fn an_expression_that_would_escape_its_slot_keeps_the_editor_open(cx: &mut TestAppContext) {
        let (state, window) = expression_state(cx);
        let rejected = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = rejected.clone();
        window.update(|_, cx| {
            cx.subscribe(&state, move |_, event: &DataTableEvent, _| {
                if let DataTableEvent::CellExpressionRejected { message } = event {
                    sink.borrow_mut().push(message.clone());
                }
            })
            .detach();
        });

        let input = open_expression_editor(&state, window, CellCoord::new(0, 1));
        type_and_commit(&state, window, &input, "NOW() -- WHERE");
        window.run_until_parked();

        assert_eq!(rejected.borrow().len(), 1);
        assert!(window.update(|_, cx| state.read(cx).is_editing_expression()));
        assert!(matches!(staged(&state, window, 0, 1), CellKind::Text(_)));
    }

    #[gpui::test]
    fn arrows_write_the_suggestions_into_the_editor(cx: &mut TestAppContext) {
        let (state, window) = expression_state(cx);
        let input = open_expression_editor(&state, window, CellCoord::new(0, 1));

        let step = |forward: bool, window: &mut VisualTestContext| {
            window.update(|window, cx| {
                state.update(cx, |state, cx| {
                    state.step_expression_suggestion(forward, window, cx)
                })
            });
            window.update(|_, cx| input.read(cx).value().to_string())
        };

        assert_eq!(step(true, window), "NOW()");
        assert_eq!(step(true, window), "CURRENT_TIMESTAMP");
        assert_eq!(step(true, window), "NOW()", "the list wraps");
        assert_eq!(step(false, window), "CURRENT_TIMESTAMP");
    }

    #[gpui::test]
    fn a_column_without_suggestions_takes_no_expression(cx: &mut TestAppContext) {
        let (state, window) = expression_state(cx);

        let opened = window.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.start_editing_expression(CellCoord::new(0, 2), window, cx)
            })
        });

        assert!(!opened);
        assert!(window.update(|_, cx| !state.read(cx).is_editing()));
    }
}
