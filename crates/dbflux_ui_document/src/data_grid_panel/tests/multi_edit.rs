use super::selection_totals::{amounts_grid, with_table};
use dbflux_components::components::data_table::selection::CellCoord;
use gpui::{Modifiers, TestAppContext};

#[gpui::test]
fn the_footer_says_how_many_cells_an_edit_writes(cx: &mut TestAppContext) {
    let (panel, window) = amounts_grid(cx);
    with_table(&panel, window, |state, cx| {
        // A result grid is read-only until it knows its key.
        state.set_pk_columns(vec![1]);
        state.click_column(0, Modifiers::default(), cx);
    });

    window.update(|window, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("the result builds a table");
        table.update(cx, |state, cx| {
            assert!(state.start_editing(CellCoord::new(3, 0), window, cx));
        });
    });
    window.run_until_parked();

    let note = window.update(|_, cx| panel.read(cx).multi_edit_footer(cx));
    assert_eq!(note.as_deref(), Some("Editing 4 cells"));
    assert!(
        window.debug_bounds("footer-multi-edit").is_some(),
        "the footer draws the note"
    );

    with_table(&panel, window, |state, cx| state.stop_editing(false, cx));
    let note = window.update(|_, cx| panel.read(cx).multi_edit_footer(cx));
    assert_eq!(note, None, "the note goes with the editor");
}
