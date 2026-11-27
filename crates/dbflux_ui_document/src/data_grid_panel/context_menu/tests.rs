use super::DataGridPanel;
use super::{CONTEXT_MENU_EDGE_GAP, place_context_menu};
use super::{ExportMenuEntry, step_export_selection};
use dbflux_components::composites::{FlyoutSide, FlyoutSpec, place_flyout};
use dbflux_components::tokens::MenuMetrics;
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

/// The window the menu is placed in, in window coordinates.
fn window() -> Size<Pixels> {
    Size {
        width: px(1000.0),
        height: px(600.0),
    }
}

/// Where the flyout of a `menu_width` panel at `origin` goes: Bolt's
/// placement owns the side the flyout opens on, so the flip the old
/// `submenus_open_left` arithmetic decided is asserted through it.
fn flyout_side(origin: Point<Pixels>, menu_width: Pixels, viewport: Size<Pixels>) -> FlyoutSide {
    place_flyout(
        Bounds {
            origin,
            size: Size {
                width: menu_width,
                height: px(300.0),
            },
        },
        origin.y,
        MenuMetrics::PADDING_Y,
        FlyoutSpec {
            size: Size {
                width: px(280.0),
                height: px(200.0),
            },
            viewport,
            margin: MenuMetrics::FLYOUT_MARGIN,
            overlap: MenuMetrics::SUBMENU_OVERLAP,
        },
    )
    .side
}

#[test]
fn a_menu_that_fits_stays_at_the_click_and_opens_submenus_to_the_right() {
    let click = Point {
        x: px(100.0),
        y: px(100.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), window());
    assert_eq!(placed, click);
    assert_eq!(flyout_side(placed, px(180.0), window()), FlyoutSide::Right);
}

#[test]
fn a_menu_near_the_right_edge_is_shifted_in_and_opens_submenus_to_the_left() {
    let click = Point {
        x: px(950.0),
        y: px(100.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), window());
    assert_eq!(placed.x, px(1000.0) - px(180.0) - CONTEXT_MENU_EDGE_GAP);
    assert_eq!(placed.y, click.y);
    assert_eq!(flyout_side(placed, px(180.0), window()), FlyoutSide::Left);
}

#[test]
fn submenus_open_left_as_soon_as_the_widest_one_would_not_fit() {
    let click = Point {
        x: px(1000.0) - px(180.0) + MenuMetrics::SUBMENU_OVERLAP - px(280.0) + px(1.0),
        y: px(100.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), window());
    assert_eq!(placed.x, click.x, "the menu itself still fits");
    assert_eq!(flyout_side(placed, px(180.0), window()), FlyoutSide::Left);
}

/// A window too narrow for a submenu on either side keeps them on the
/// right: a submenu pushed off the left edge would be invisible, one
/// hanging past the right edge is at least partly usable.
#[test]
fn a_narrow_window_keeps_submenus_on_the_right() {
    let narrow = Size {
        width: px(400.0),
        height: px(600.0),
    };
    let click = Point {
        x: px(20.0),
        y: px(100.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), narrow);
    assert_eq!(placed, click);
    assert_eq!(flyout_side(placed, px(180.0), narrow), FlyoutSide::Right);
}

#[test]
fn a_menu_near_the_bottom_is_moved_up_to_fit() {
    let click = Point {
        x: px(100.0),
        y: px(500.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), window());
    assert_eq!(placed.x, click.x);
    assert_eq!(placed.y, px(600.0) - px(300.0) - CONTEXT_MENU_EDGE_GAP);
}

/// The root panel of a menu in an unmeasured window stays at the click;
/// where its flyout goes is clamped by Bolt's placement into the
/// viewport's edge, rather than hanging off it.
#[test]
fn an_unmeasured_window_leaves_the_click_alone() {
    let click = Point {
        x: px(950.0),
        y: px(500.0),
    };
    let placed = place_context_menu(click, px(180.0), px(300.0), Size::default());
    assert_eq!(placed, click);

    let flyout = place_flyout(
        Bounds {
            origin: placed,
            size: Size {
                width: px(180.0),
                height: px(300.0),
            },
        },
        placed.y,
        MenuMetrics::PADDING_Y,
        FlyoutSpec {
            size: Size {
                width: px(280.0),
                height: px(200.0),
            },
            viewport: Size::default(),
            margin: MenuMetrics::FLYOUT_MARGIN,
            overlap: MenuMetrics::SUBMENU_OVERLAP,
        },
    );
    assert_eq!(flyout.origin.x, MenuMetrics::FLYOUT_MARGIN);
}

/// A grid below the editor: the panel starts 400px down a 1400x1000 window
/// and is 300px tall, shorter than the menu.
fn window_with_a_short_panel_low_in_it() -> Size<Pixels> {
    Size {
        width: px(1400.0),
        height: px(1000.0),
    }
}

/// The menu is drawn at window level, so one taller than the panel is
/// lifted until its bottom clears the window edge instead of hanging off
/// it from the click, or being given up on and left at the click.
#[test]
fn a_menu_taller_than_the_panel_is_lifted_inside_the_window() {
    let click = Point {
        x: px(300.0),
        y: px(650.0),
    };
    let placed = place_context_menu(
        click,
        px(270.0),
        px(700.0),
        window_with_a_short_panel_low_in_it(),
    );
    assert_eq!(placed.x, click.x);
    assert_eq!(
        placed.y,
        px(1000.0) - px(700.0) - CONTEXT_MENU_EDGE_GAP,
        "the menu's bottom sits one gap above the window's bottom"
    );
}

/// Room below the panel inside the window is used before lifting the
/// menu: it hangs past the panel's bottom edge from the click.
#[test]
fn a_menu_that_fits_in_the_window_stays_at_the_click_past_the_panel() {
    let click = Point {
        x: px(300.0),
        y: px(450.0),
    };
    let placed = place_context_menu(
        click,
        px(270.0),
        px(500.0),
        window_with_a_short_panel_low_in_it(),
    );
    assert_eq!(placed, click);
}

/// A menu taller than the window starts at the window's top gap, so its
/// first rows stay reachable; Bolt caps its height and scrolls it.
#[test]
fn a_menu_taller_than_the_window_starts_at_its_top() {
    let click = Point {
        x: px(300.0),
        y: px(650.0),
    };
    let placed = place_context_menu(
        click,
        px(270.0),
        px(1200.0),
        window_with_a_short_panel_low_in_it(),
    );
    assert_eq!(placed.y, CONTEXT_MENU_EDGE_GAP);
}

/// The right edge that decides the submenu side is the window's, not the
/// panel's: a menu near a narrow panel's right edge still opens its
/// submenus to the right while the window has the room.
#[test]
fn submenu_side_follows_the_window_edge() {
    let click = Point {
        x: px(800.0),
        y: px(450.0),
    };
    let window = window_with_a_short_panel_low_in_it();
    let placed = place_context_menu(click, px(270.0), px(300.0), window);
    assert_eq!(placed.x, click.x);
    assert_eq!(flyout_side(placed, px(270.0), window), FlyoutSide::Right);
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
    assert!(labels.contains(&item_label(
        "document.data.context_menu.item.copy_column_name"
    )));
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
        vec![
            item_label("document.data.context_menu.item.copy"),
            item_label("document.data.context_menu.item.copy_as_markdown"),
            item_label("document.data.context_menu.item.copy_column_name"),
        ]
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

/// Copy as Markdown and Copy column name sit right after Copy, and the
/// document menu, which has no columns or table, leaves both out.
#[test]
fn copy_column_name_follows_copy_in_the_table_menu_only() {
    let table = DataGridPanel::build_context_menu_items(false, false, true, false, true);
    assert_eq!(
        table[1].action,
        Some(dbflux_components::components::data_table::ContextMenuAction::CopyAsMarkdown)
    );
    assert_eq!(
        table[2].action,
        Some(dbflux_components::components::data_table::ContextMenuAction::CopyColumnName)
    );

    let document = DataGridPanel::build_context_menu_items(true, true, true, false, true);
    assert!(!document.iter().any(|item| matches!(
        item.action,
        Some(
            dbflux_components::components::data_table::ContextMenuAction::CopyColumnName
                | dbflux_components::components::data_table::ContextMenuAction::CopyAsMarkdown
        )
    )));
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
