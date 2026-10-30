use gpui::{Modifiers, TestAppContext};

use super::super::model::CellValue;
use super::{model, render_table};

/// Selection totals add up the shown columns only: a hidden numeric column
/// is outside every selected range.
#[gpui::test]
fn totals_leave_out_a_hidden_column(cx: &mut TestAppContext) {
    let (state, visual) = render_table(
        cx,
        model(&["a", "b"], 3, |row, col| {
            CellValue::int((row * 10 + col) as i64)
        }),
    );

    let counted = visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.hide_column(1, cx);
            state.select_all(cx);
            state
                .selection_totals()
                .and_then(|totals| totals.numeric.map(|numeric| (totals.cells, numeric.count)))
        })
    });

    assert_eq!(counted, Some((3, 3)), "only column `a` is counted");
}

/// Copy with column names heads the text with the copied columns in the
/// order the grid shows them.
#[gpui::test]
fn copy_with_column_names_follows_the_screen_order(cx: &mut TestAppContext) {
    let (state, visual) = render_table(
        cx,
        model(&["id", "name", "city"], 1, |_, col| {
            CellValue::text(["1", "Ann", "Oslo"][col])
        }),
    );

    let copied = visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.set_copy_column_names(true);
            state.move_column(2, 0, cx);
            state.hide_column(0, cx);
            state.select_all(cx);
            state.copy_selection()
        })
    });

    assert_eq!(copied.as_deref(), Some("city\tname\nOslo\tAnn"));
}

/// The sort arrow of a pinned column, drawn in the strip at the left edge,
/// still sorts and adds a sort key with Alt+click, and its rank shows there.
#[gpui::test]
fn alt_click_on_a_pinned_sort_arrow_adds_a_sort_key(cx: &mut TestAppContext) {
    let (state, visual) = render_table(
        cx,
        // Short values, so every sort arrow fits in the 600 px window.
        model(&["id", "a", "b", "created_at"], 2, |row, col| {
            CellValue::text(&format!("{row}{col}"))
        }),
    );
    visual.update(|_, cx| {
        state.update(cx, |state, cx| {
            state.set_multi_sort_enabled(true);
            state.pin_column(3, cx);
        });
    });
    visual.run_until_parked();

    let click = |visual: &mut gpui::VisualTestContext, arrow: &'static str, modifiers| {
        let bounds = visual.debug_bounds(arrow).expect("the sort arrow is drawn");
        visual.simulate_click(bounds.center(), modifiers);
        visual.run_until_parked();
    };
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };

    click(visual, "header-sort-1", Modifiers::default());
    click(visual, "header-sort-3", alt);

    let keys = visual.update(|_, cx| {
        state
            .read(cx)
            .sort_keys()
            .iter()
            .map(|key| key.column_ix)
            .collect::<Vec<_>>()
    });
    assert_eq!(keys, [1, 3], "the pinned column became the second key");
    assert!(
        visual.debug_bounds("header-sort-rank-3").is_some(),
        "the pinned header shows its rank"
    );
}
