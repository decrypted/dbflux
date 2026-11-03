use super::DataGridPanel;
use super::QueryGroupSeparators;
use super::{CONTEXT_MENU_EDGE_GAP, SUBMENU_MAX_WIDTH, SUBMENU_OVERLAP, place_context_menu};
use super::{ExportMenuEntry, step_export_selection};
use dbflux_export::ExportFormat;
use gpui::{Bounds, Pixels, Point, Size, px};

/// The binary copy row cannot run, so the menu keys pass over it in both
/// directions and wrap at the ends.
#[test]
fn export_selection_wraps_and_skips_the_binary_copy_row() {
    let entries = [
        ExportMenuEntry::Save(ExportFormat::Binary),
        ExportMenuEntry::Save(ExportFormat::Hex),
        ExportMenuEntry::Copy(ExportFormat::Binary),
        ExportMenuEntry::Copy(ExportFormat::Hex),
    ];

    assert_eq!(step_export_selection(&entries, 1, true), 3);
    assert_eq!(step_export_selection(&entries, 3, false), 1);
    assert_eq!(step_export_selection(&entries, 3, true), 0);
    assert_eq!(step_export_selection(&entries, 0, false), 3);
    assert_eq!(step_export_selection(&[], 0, true), 0);
}

/// A window whose top-left corner is the panel's: the panel fills it.
fn window_at_panel(width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: Point::default(),
        size: Size {
            width: px(width),
            height: px(height),
        },
    }
}

fn panel() -> Bounds<Pixels> {
    window_at_panel(1000.0, 600.0)
}

#[test]
fn query_group_has_one_separator_before_its_first_member() {
    assert_eq!(
        QueryGroupSeparators::new(true, true, true, true),
        QueryGroupSeparators {
            filter: true,
            order: false,
            generate_sql: false,
            copy_query: false,
        }
    );

    assert_eq!(
        QueryGroupSeparators::new(false, false, true, true),
        QueryGroupSeparators {
            filter: false,
            order: false,
            generate_sql: true,
            copy_query: false,
        }
    );

    assert_eq!(
        QueryGroupSeparators::new(false, false, false, true),
        QueryGroupSeparators {
            filter: false,
            order: false,
            generate_sql: false,
            copy_query: true,
        }
    );
}

#[test]
fn a_menu_that_fits_stays_at_the_click_and_opens_submenus_to_the_right() {
    let click = Point {
        x: px(100.0),
        y: px(100.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), panel());
    assert_eq!(placed.origin, click);
    assert!(!placed.submenus_open_left);
}

#[test]
fn a_menu_near_the_right_edge_is_shifted_in_and_opens_submenus_to_the_left() {
    let click = Point {
        x: px(950.0),
        y: px(100.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), panel());
    assert_eq!(
        placed.origin.x,
        px(1000.0) - px(180.0) - CONTEXT_MENU_EDGE_GAP
    );
    assert_eq!(placed.origin.y, click.y);
    assert!(placed.submenus_open_left);
}

#[test]
fn submenus_open_left_as_soon_as_the_widest_one_would_not_fit() {
    let click = Point {
        x: px(1000.0) - px(180.0) + SUBMENU_OVERLAP - SUBMENU_MAX_WIDTH + px(1.0),
        y: px(100.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), panel());
    assert_eq!(placed.origin.x, click.x, "the menu itself still fits");
    assert!(placed.submenus_open_left);
}

/// A panel too narrow for a submenu on either side keeps them on the
/// right: a submenu pushed off the left edge would be invisible, one
/// hanging past the right edge is at least partly usable.
#[test]
fn a_narrow_panel_keeps_submenus_on_the_right() {
    let narrow = window_at_panel(400.0, 600.0);
    let click = Point {
        x: px(20.0),
        y: px(100.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), narrow);
    assert_eq!(placed.origin, click);
    assert!(!placed.submenus_open_left);
}

#[test]
fn a_menu_near_the_bottom_is_moved_up_to_fit() {
    let click = Point {
        x: px(100.0),
        y: px(500.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), panel());
    assert_eq!(placed.origin.x, click.x);
    assert_eq!(
        placed.origin.y,
        px(600.0) - px(300.0) - CONTEXT_MENU_EDGE_GAP
    );
}

/// A grid below the editor: the panel starts 400px down a 1000px window
/// and is 300px tall, shorter than the menu.
fn short_panel_low_in_window() -> Bounds<Pixels> {
    Bounds {
        origin: Point {
            x: px(-200.0),
            y: px(-400.0),
        },
        size: Size {
            width: px(1400.0),
            height: px(1000.0),
        },
    }
}

/// The menu is drawn at window level, so one taller than the panel is
/// lifted until its bottom clears the window edge instead of hanging off
/// it from the click.
#[test]
fn a_menu_taller_than_the_panel_is_lifted_inside_the_window() {
    let click = Point {
        x: px(100.0),
        y: px(250.0),
    };
    let placed = place_context_menu(click, px(270.0), px(700.0), short_panel_low_in_window());
    assert_eq!(placed.origin.x, click.x);
    assert_eq!(
        placed.origin.y,
        px(600.0) - px(700.0) - CONTEXT_MENU_EDGE_GAP,
        "the menu's bottom sits one gap above the window's bottom"
    );
}

/// Room below the panel inside the window is used before lifting the
/// menu: it hangs past the panel's bottom edge from the click.
#[test]
fn a_menu_that_fits_in_the_window_stays_at_the_click_past_the_panel() {
    let click = Point {
        x: px(100.0),
        y: px(50.0),
    };
    let placed = place_context_menu(click, px(270.0), px(500.0), short_panel_low_in_window());
    assert_eq!(placed.origin, click);
}

/// A menu taller than the window starts at the window's top gap, so its
/// first rows stay reachable.
#[test]
fn a_menu_taller_than_the_window_starts_at_its_top() {
    let click = Point {
        x: px(100.0),
        y: px(250.0),
    };
    let placed = place_context_menu(click, px(270.0), px(1200.0), short_panel_low_in_window());
    assert_eq!(placed.origin.y, px(-400.0) + CONTEXT_MENU_EDGE_GAP);
}

/// The right edge that decides the submenu side is the window's, 1200px
/// right of the panel's origin here: the widest submenu still fits there.
#[test]
fn submenu_side_follows_the_window_edge() {
    let click = Point {
        x: px(600.0),
        y: px(50.0),
    };
    let placed = place_context_menu(click, px(270.0), px(300.0), short_panel_low_in_window());
    assert_eq!(placed.origin.x, click.x);
    assert!(!placed.submenus_open_left);
}

#[test]
fn an_unmeasured_panel_leaves_the_click_alone() {
    let click = Point {
        x: px(950.0),
        y: px(500.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), Bounds::default());
    assert_eq!(placed.origin, click);
    assert!(!placed.submenus_open_left);
}

fn labels(items: &[super::ContextMenuItem]) -> Vec<String> {
    items
        .iter()
        .filter(|item| !item.is_separator)
        .map(|item| item.label.to_string())
        .collect()
}

fn item_label(key: &str) -> String {
    dbflux_i18n::t!(key)
}

#[test]
fn empty_table_menu_keeps_insert_actions_but_hides_row_actions() {
    let items = DataGridPanel::build_context_menu_items(true, false, false, false, true);
    let labels = labels(&items);

    assert!(labels.contains(&item_label("document.data.context_menu.item.add_row")));
    assert!(!labels.contains(&item_label("document.data.context_menu.item.edit")));
    assert!(!labels.contains(&item_label("document.data.context_menu.item.edit_in_modal")));
    assert!(!labels.contains(&item_label("document.data.context_menu.item.duplicate_row")));
    assert!(!labels.contains(&item_label("document.data.context_menu.item.delete_row")));
}

#[test]
fn non_editable_table_menu_stays_unchanged_without_row_target() {
    let items = DataGridPanel::build_context_menu_items(false, false, false, false, true);

    assert_eq!(
        labels(&items),
        vec![item_label("document.data.context_menu.item.copy")]
    );
}

/// The value panel is a reader as much as an editor, so a read-only
/// result still offers it — but only over an actual row.
#[test]
fn non_editable_table_menu_offers_view_value_over_a_row() {
    let with_row = labels(&DataGridPanel::build_context_menu_items(
        false, false, true, false, true,
    ));
    assert!(with_row.contains(&item_label("document.data.context_menu.item.view_value")));

    let without_row = labels(&DataGridPanel::build_context_menu_items(
        false, false, false, false, true,
    ));
    assert!(!without_row.contains(&item_label("document.data.context_menu.item.view_value")));
}

#[test]
fn editable_table_menu_with_row_target_keeps_row_actions() {
    let items = DataGridPanel::build_context_menu_items(true, false, true, false, true);
    let labels = labels(&items);

    assert!(labels.contains(&item_label("document.data.context_menu.item.edit")));
    assert!(labels.contains(&item_label("document.data.context_menu.item.edit_in_modal")));
    assert!(labels.contains(&item_label("document.data.context_menu.item.add_row")));
    assert!(labels.contains(&item_label("document.data.context_menu.item.duplicate_row")));
    assert!(labels.contains(&item_label("document.data.context_menu.item.delete_row")));
}

#[test]
fn chart_this_query_absent_when_can_chart_false() {
    // can_chart = false: item must NOT appear regardless of other flags.
    let table_items = DataGridPanel::build_context_menu_items(false, false, false, false, true);
    let chart_label = item_label("document.data.context_menu.item.chart_this_query");
    assert!(!labels(&table_items).contains(&chart_label));

    let editable_items = DataGridPanel::build_context_menu_items(true, false, true, false, true);
    assert!(!labels(&editable_items).contains(&chart_label));
}

#[test]
fn chart_this_query_present_only_when_can_chart_true() {
    // can_chart = true: item must appear.
    let items = DataGridPanel::build_context_menu_items(false, false, false, true, true);
    assert!(labels(&items).contains(&item_label(
        "document.data.context_menu.item.chart_this_query"
    )));
}

#[test]
fn chart_this_query_absent_in_document_view_regardless_of_can_chart() {
    // Document-view menu never shows Chart this query because the source is never
    // a QueryResult when is_document_view is true.
    let doc_items = DataGridPanel::build_context_menu_items(false, true, false, true, true);
    assert!(!labels(&doc_items).contains(&item_label(
        "document.data.context_menu.item.chart_this_query"
    )));
}

#[test]
fn inspect_row_hidden_when_inspect_row_disabled() {
    let items_with_target =
        DataGridPanel::build_context_menu_items(true, false, true, false, false);
    assert!(
        !labels(&items_with_target)
            .contains(&item_label("document.data.context_menu.item.inspect_row")),
        "Inspect Row must not appear when inspect_row_enabled=false"
    );
}

#[test]
fn inspect_row_present_when_enabled_and_has_target() {
    let items = DataGridPanel::build_context_menu_items(true, false, true, false, true);
    assert!(
        labels(&items).contains(&item_label("document.data.context_menu.item.inspect_row")),
        "Inspect Row must appear when inspect_row_enabled=true and has_row_target=true"
    );
}
