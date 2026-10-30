//! Identifiers generated on the client (UUID v4 and v7, KSUID, ULID) and
//! written into cells as ordinary values, so every driver stores them.

use std::collections::HashMap;

use dbflux_core::GeneratedIdKind;
use gpui::Context;

use super::DataTableState;
use crate::components::data_table::model::{CellValue, VisualRowSource};
use crate::components::data_table::selection::CellCoord;

/// The identifiers a column can take, and whether its editor lists them
/// without being asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeneratedIdOffer {
    pub kinds: Vec<GeneratedIdKind>,
    /// The column looks like an identifier (a uuid type, a key, a name such
    /// as `user_id`), so the inline editor lists the kinds first.
    pub in_editor: bool,
}

impl DataTableState {
    /// Sets the identifier kinds each column can take. A column the map
    /// lacks takes none; a new model clears the map.
    pub fn set_generated_id_offers(&mut self, offers: HashMap<usize, GeneratedIdOffer>) {
        self.generated_id_offers = offers;
    }

    /// The identifier kinds a cell of `col` can take.
    pub fn generated_id_kinds(&self, col: usize) -> &[GeneratedIdKind] {
        if self.readonly_columns.contains(&col) {
            return &[];
        }
        self.generated_id_offers
            .get(&col)
            .map(|offer| offer.kinds.as_slice())
            .unwrap_or_default()
    }

    /// The identifier kinds the inline editor lists for `col`.
    pub(crate) fn editor_id_kinds(&self, col: usize) -> &[GeneratedIdKind] {
        match self.generated_id_offers.get(&col) {
            Some(offer) if offer.in_editor => self.generated_id_kinds(col),
            _ => &[],
        }
    }

    /// Writes a new identifier of `kind` into the cell at `coord`, or, when
    /// `coord` is part of a larger selection, into every selected cell that
    /// can take one. Each cell gets its own value, and one undo takes them
    /// all back. Returns how many cells were written.
    pub fn generate_ids(
        &mut self,
        kind: GeneratedIdKind,
        coord: CellCoord,
        cx: &mut Context<Self>,
    ) -> usize {
        let targets = self.generated_id_targets(kind, coord);
        let count = self.stage_generated_ids(kind, &targets, None);
        cx.notify();
        count
    }

    /// The cells a `kind` generated at `coord` goes to: the selection's
    /// editable cells whose column takes `kind`, or `coord` alone.
    fn generated_id_targets(&self, kind: GeneratedIdKind, coord: CellCoord) -> Vec<CellCoord> {
        let selected = self.multi_edit_targets(coord);
        let candidates = if selected.is_empty() {
            vec![coord]
        } else {
            selected
        };

        candidates
            .into_iter()
            .filter(|target| {
                self.generated_id_kinds(target.col).contains(&kind) && self.can_edit_row(target.row)
            })
            .collect()
    }

    /// Stages a fresh `kind` value in each of `targets` as one undo step;
    /// `first` is the value the first target gets, when the editor already
    /// shows one.
    pub(super) fn stage_generated_ids(
        &mut self,
        kind: GeneratedIdKind,
        targets: &[CellCoord],
        first: Option<String>,
    ) -> usize {
        // Resolved once: `stage_cell_value` computes the visual order per
        // call, which over a whole selected column would be quadratic.
        let visual_order = self.edit_buffer.compute_visual_order();
        let mut first = first;

        self.edit_buffer.begin_undo_group();
        for target in targets {
            let value = CellValue::text(&first.take().unwrap_or_else(|| kind.generate()));
            match visual_order.get(target.row).copied() {
                Some(VisualRowSource::Base(base_idx)) => {
                    self.stage_base_cell_value(base_idx, target.col, value);
                }
                Some(VisualRowSource::Insert(insert_idx)) => {
                    self.edit_buffer
                        .set_insert_cell(insert_idx, target.col, value);
                }
                None => {}
            }
        }
        self.edit_buffer.end_undo_group();

        targets.len()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use dbflux_core::GeneratedIdKind;
    use gpui::{
        AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, TextAlign,
        VisualTestContext, Window, div,
    };

    use super::{DataTableState, GeneratedIdOffer};
    use crate::components::data_table::model::{
        CellKind, CellValue, ColumnKind, ColumnSpec, RowData, TableModel,
    };
    use crate::components::data_table::selection::CellCoord;
    use crate::components::data_table::state::EditorSuggestion;

    struct Harness {
        _state: Entity<DataTableState>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn column(title: &str) -> ColumnSpec {
        ColumnSpec {
            id: title.into(),
            title: title.into(),
            kind: ColumnKind::Text,
            align: TextAlign::Left,
            type_name: "".into(),
        }
    }

    /// Three rows of `id` (key, int), `ref` (takes UUIDs, listed in the
    /// editor) and `note` (takes nothing).
    fn id_state(cx: &mut TestAppContext) -> (Entity<DataTableState>, &mut VisualTestContext) {
        let holder = std::rc::Rc::new(std::cell::RefCell::new(None));
        let holder_for_view = holder.clone();
        let (_, window) = cx.add_window_view(move |_window, cx| {
            let rows = (0..3)
                .map(|row| RowData {
                    cells: vec![
                        CellValue::int(row),
                        CellValue::text("old"),
                        CellValue::text("note"),
                    ],
                })
                .collect();
            let model = Arc::new(TableModel::new(
                vec![column("id"), column("ref"), column("note")],
                rows,
            ));
            let state = cx.new(|cx| {
                let mut state = DataTableState::new(model, cx);
                state.set_pk_columns(vec![0]);
                state.set_insertable(true);
                state.set_generated_id_offers(HashMap::from([(
                    1,
                    GeneratedIdOffer {
                        kinds: vec![GeneratedIdKind::UuidV4, GeneratedIdKind::UuidV7],
                        in_editor: true,
                    },
                )]));
                state
            });
            holder_for_view.replace(Some(state.clone()));
            Harness { _state: state }
        });
        let state = holder.borrow().clone().expect("state entity");
        (state, window)
    }

    fn staged_text(
        state: &Entity<DataTableState>,
        window: &mut VisualTestContext,
        row: usize,
        col: usize,
    ) -> String {
        window.update(|_, cx| {
            let state = state.read(cx);
            let base = state.model().cell(row, col).cloned().expect("a base cell");
            match &state.edit_buffer().get_cell(row, col, &base).kind {
                CellKind::Text(text) => text.to_string(),
                other => panic!("not text: {other:?}"),
            }
        })
    }

    fn is_uuid(text: &str, version: usize) -> bool {
        uuid::Uuid::parse_str(text).is_ok_and(|uuid| uuid.get_version_num() == version)
    }

    #[gpui::test]
    fn every_selected_cell_gets_its_own_id_in_one_undo_step(cx: &mut TestAppContext) {
        let (state, window) = id_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.click_column(1, gpui::Modifiers::default(), cx)
            })
        });

        let written = window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.generate_ids(GeneratedIdKind::UuidV7, CellCoord::new(0, 1), cx)
            })
        });
        assert_eq!(written, 3);

        let values: Vec<String> = (0..3)
            .map(|row| staged_text(&state, window, row, 1))
            .collect();
        assert!(values.iter().all(|value| is_uuid(value, 7)), "{values:?}");
        assert_eq!(
            values.iter().collect::<HashSet<_>>().len(),
            3,
            "no value is shared: {values:?}"
        );

        window.update(|_, cx| state.update(cx, |state, _| assert!(state.edit_buffer_mut().undo())));
        for row in 0..3 {
            assert_eq!(staged_text(&state, window, row, 1), "old");
        }
    }

    #[gpui::test]
    fn a_column_that_takes_no_id_is_left_alone(cx: &mut TestAppContext) {
        let (state, window) = id_state(cx);

        let written = window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.generate_ids(GeneratedIdKind::UuidV4, CellCoord::new(0, 2), cx)
            })
        });

        assert_eq!(written, 0);
        assert_eq!(staged_text(&state, window, 0, 2), "note");
    }

    #[gpui::test]
    fn a_new_row_takes_a_generated_id(cx: &mut TestAppContext) {
        let (state, window) = id_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.edit_buffer_mut().add_pending_insert(vec![
                    CellValue::null(),
                    CellValue::null(),
                    CellValue::null(),
                ]);
                state.generate_ids(GeneratedIdKind::UuidV4, CellCoord::new(3, 1), cx);
            })
        });

        let cell = window.update(|_, cx| {
            state
                .read(cx)
                .edit_buffer()
                .get_pending_insert_by_idx(0)
                .unwrap()[1]
                .clone()
        });
        assert!(is_uuid(&cell.edit_text(), 4), "{cell:?}");
    }

    #[gpui::test]
    fn the_editor_lists_ids_and_a_pick_over_a_selection_gives_each_cell_its_own(
        cx: &mut TestAppContext,
    ) {
        let (state, window) = id_state(cx);
        window.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.click_column(1, gpui::Modifiers::default(), cx)
            })
        });

        let input = window.update(|window, cx| {
            state.update(cx, |state, cx| {
                assert!(state.start_editing(CellCoord::new(0, 1), window, cx));
                assert_eq!(
                    state.editor_suggestions(1),
                    [
                        EditorSuggestion::GeneratedId(GeneratedIdKind::UuidV4),
                        EditorSuggestion::GeneratedId(GeneratedIdKind::UuidV7),
                    ]
                );
                state.step_expression_suggestion(true, window, cx);
                state.cell_input().cloned().unwrap()
            })
        });
        let shown = window.update(|_, cx| input.read(cx).value().to_string());
        assert!(is_uuid(&shown, 4), "{shown}");

        window.update(|_, cx| state.update(cx, |state, cx| state.stop_editing(true, cx)));

        let values: Vec<String> = (0..3)
            .map(|row| staged_text(&state, window, row, 1))
            .collect();
        assert_eq!(values[0], shown, "the edited cell keeps the value shown");
        assert!(values.iter().all(|value| is_uuid(value, 4)), "{values:?}");
        assert_eq!(values.iter().collect::<HashSet<_>>().len(), 3, "{values:?}");

        window.update(|_, cx| state.update(cx, |state, _| assert!(state.edit_buffer_mut().undo())));
        assert_eq!(staged_text(&state, window, 2, 1), "old", "one undo step");
    }
}
