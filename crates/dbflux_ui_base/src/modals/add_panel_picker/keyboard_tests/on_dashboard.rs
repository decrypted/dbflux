//! The Add Panel picker on a dashboard that already shows some of the charts.

use super::*;

/// A chart the dashboard already shows can't be added by click or by key:
/// a second panel for it would never appear.
#[gpui::test]
fn a_chart_on_the_dashboard_cannot_be_picked(cx: &mut TestAppContext) {
    let shown = named_chart("Latency");
    let other = named_chart("Throughput");
    let (shown_id, other_id) = (shown.id, other.id);
    let (modal, window, outcomes) =
        open_modal_on_dashboard(cx, vec![shown, other], vec![shown_id], false);

    // Starts on the first chart that can be added.
    assert_eq!(window.update(|_, cx| modal.read(cx).saved_highlight), 1);

    window.update(|_, cx| modal.update(cx, |modal, cx| modal.toggle_chart(shown_id, cx)));
    assert!(window.update(|_, cx| modal.read(cx).selected_ids.is_empty()));

    window.update(|window, cx| {
        let list = modal.read(cx).saved_list_focus.clone();
        list.focus(window, cx);
    });
    window.run_until_parked();

    window.simulate_keystrokes("k space");
    assert!(
        window.update(|_, cx| modal.read(cx).selected_ids.is_empty()),
        "Space does not check it"
    );
    window.simulate_keystrokes("enter");
    assert!(
        outcomes.borrow().is_empty(),
        "Enter does not add it, and the dialog stays open"
    );
    assert!(window.update(|_, cx| modal.read(cx).is_visible()));

    window.simulate_keystrokes("j enter");
    assert!(matches!(
        outcomes.borrow().as_slice(),
        [AddPanelOutcome::Confirmed { chart_ids, .. }] if chart_ids == &[other_id]
    ));
}

/// The row says why it can't be picked, and only that row does.
#[gpui::test]
fn a_chart_on_the_dashboard_says_so(cx: &mut TestAppContext) {
    let shown = named_chart("Latency");
    let shown_id = shown.id;
    let (modal, window, _outcomes) =
        open_modal_on_dashboard(cx, vec![shown], vec![shown_id], false);

    assert!(window.debug_bounds("add-panel-on-dashboard").is_some());
    assert_eq!(
        window.update(|_, cx| modal.read(cx).submit_label()),
        "Add panels"
    );
    assert!(!window.update(|_, cx| modal.read(cx).can_confirm(cx)));
    assert!(!window.update(|_, cx| modal.read(cx).can_confirm_from_keys(cx)));

    let (_modal, window, _outcomes) = open_modal_with(cx, vec![named_chart("Latency")], false);
    assert!(window.debug_bounds("add-panel-on-dashboard").is_none());
}

/// The scenario of a search right after opening: the highlight starts on the
/// second chart, and the search leaves one row.
#[gpui::test]
fn a_search_after_opening_on_a_later_chart_still_adds_it(cx: &mut TestAppContext) {
    let shown = named_chart("Latency");
    let other = named_chart("Throughput");
    let (shown_id, other_id) = (shown.id, other.id);
    let (_modal, window, outcomes) =
        open_modal_on_dashboard(cx, vec![shown, other], vec![shown_id], false);

    window.simulate_keystrokes("t h r enter");

    assert!(matches!(
        outcomes.borrow().as_slice(),
        [AddPanelOutcome::Confirmed { chart_ids, .. }] if chart_ids == &[other_id]
    ));
}

/// A search moves the highlight to the first chart it leaves that can be added,
/// past one the dashboard already shows.
#[gpui::test]
fn a_search_skips_a_chart_on_the_dashboard(cx: &mut TestAppContext) {
    let shown = named_chart("Latency one");
    let other = named_chart("Latency two");
    let third = named_chart("Throughput");
    let (shown_id, other_id) = (shown.id, other.id);
    let (modal, window, outcomes) =
        open_modal_on_dashboard(cx, vec![third, shown, other], vec![shown_id], false);
    assert_eq!(window.update(|_, cx| modal.read(cx).saved_highlight), 0);

    window.simulate_keystrokes("l a t");
    assert_eq!(window.update(|_, cx| modal.read(cx).saved_highlight), 1);

    window.simulate_keystrokes("enter");
    assert!(matches!(
        outcomes.borrow().as_slice(),
        [AddPanelOutcome::Confirmed { chart_ids, .. }] if chart_ids == &[other_id]
    ));
}

/// A search that leaves only the chart on the dashboard: Enter adds nothing.
#[gpui::test]
fn a_search_leaving_only_a_chart_on_the_dashboard_adds_nothing(cx: &mut TestAppContext) {
    let shown = named_chart("Latency");
    let shown_id = shown.id;
    let (modal, window, outcomes) = open_modal_on_dashboard(
        cx,
        vec![shown, named_chart("Throughput")],
        vec![shown_id],
        false,
    );

    window.simulate_keystrokes("l a t enter");

    assert!(outcomes.borrow().is_empty());
    assert!(window.update(|_, cx| modal.read(cx).is_visible()));
}

/// However the highlight got past the list, it reads as the row the list draws.
#[gpui::test]
fn a_highlight_past_the_list_reads_as_its_last_row(cx: &mut TestAppContext) {
    let only = named_chart("Latency");
    let only_id = only.id;
    let (modal, window, outcomes) = open_modal_with(cx, vec![only], false);

    window.update(|_, cx| modal.update(cx, |modal, _| modal.saved_highlight = 3));
    window.simulate_keystrokes("enter");

    assert!(matches!(
        outcomes.borrow().as_slice(),
        [AddPanelOutcome::Confirmed { chart_ids, .. }] if chart_ids == &[only_id]
    ));
}

/// Enter refuses the chart itself: what decides whether Enter runs is a flag of
/// the last frame, which can be older than the list.
#[gpui::test]
fn the_enter_action_refuses_a_chart_on_the_dashboard(cx: &mut TestAppContext) {
    let shown = named_chart("Latency");
    let shown_id = shown.id;
    let (modal, window, outcomes) = open_modal_on_dashboard(cx, vec![shown], vec![shown_id], false);

    window.update(|window, cx| {
        modal.update(cx, |modal, cx| modal.confirm_from_keys(window, cx));
    });

    assert!(outcomes.borrow().is_empty());
    assert!(window.update(|_, cx| modal.read(cx).selected_ids.is_empty()));
    assert!(window.update(|_, cx| modal.read(cx).is_visible()));
}
