//! The cell menu as drawn: it stays inside the window, its submenus open on
//! the side with room, a menu taller than the window scrolls the keyboard's
//! row into view, and the menu keys walk into and out of nested submenus.

use super::filter_history::{hosted_table, register_filter_stub};
use super::*;
use crate::data_grid_panel::TableContextMenu;
use crate::data_grid_panel::context_menu::tree::{self, MenuNode, MenuTarget};
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_components::components::data_table::ContextMenuAction;
use gpui::{Bounds, Entity, Pixels, Point, point, px, size};

fn table(cx: &mut TestAppContext) -> (Entity<DataGridPanel>, &mut VisualTestContext) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let (profile_id, _log) = cx.update(|cx| {
        register_filter_stub(&app_state, super::filter_history::method_rows(), None, cx)
    });
    let (_host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");
    (panel, window)
}

/// Opens the cell menu of the first row's `method` cell at `position`, in
/// window coordinates, with the keyboard in it.
fn open_menu_at(
    panel: &Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    position: Point<Pixels>,
) {
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.context_menu = Some(TableContextMenu::new(0, 1, position));
            panel.focus.context_menu_focus.focus(window, cx);
            cx.notify();
        });
    });
    window.run_until_parked();
}

fn keys(window: &mut VisualTestContext, keystrokes: &str) {
    for keystroke in keystrokes.split(' ') {
        window.simulate_keystrokes(keystroke);
        window.run_until_parked();
    }
}

fn viewport(window: &mut VisualTestContext) -> Bounds<Pixels> {
    let size = window.update(|window, _| window.viewport_size());
    Bounds::new(Point::default(), size)
}

fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    // Layout rounds to whole pixels.
    let slack = px(1.0);
    inner.left() >= outer.left() - slack
        && inner.top() >= outer.top() - slack
        && inner.right() <= outer.right() + slack
        && inner.bottom() <= outer.bottom() + slack
}

/// A menu opened at the window's bottom-right corner is moved up and left
/// until all of it is inside the window.
#[gpui::test]
fn a_menu_opened_in_the_bottom_right_corner_stays_inside_the_window(cx: &mut TestAppContext) {
    let (panel, window) = table(cx);
    window.simulate_resize(size(px(1000.0), px(800.0)));
    window.run_until_parked();
    let screen = viewport(window);

    open_menu_at(
        &panel,
        window,
        point(screen.right() - px(10.0), screen.bottom() - px(10.0)),
    );

    let menu = window
        .debug_bounds("context-menu")
        .expect("the menu is drawn");
    assert!(inside(menu, screen), "{menu:?} inside {screen:?}");
    assert!(
        menu.bottom() <= screen.bottom() - px(10.0) + px(2.0),
        "it opens upwards from the click: {menu:?}"
    );
}

/// Near the right edge a submenu opens to the left of its menu, where the
/// room is.
#[gpui::test]
fn a_submenu_near_the_right_edge_opens_to_the_left(cx: &mut TestAppContext) {
    let (panel, window) = table(cx);
    window.simulate_resize(size(px(1000.0), px(800.0)));
    window.run_until_parked();
    let screen = viewport(window);

    open_menu_at(&panel, window, point(screen.right() - px(300.0), px(40.0)));
    // Copy, then Copy as, opened.
    keys(window, "j l");

    let menu = window.debug_bounds("context-menu").expect("the menu");
    let flyout = window
        .debug_bounds("context-menu-flyout-copy-as")
        .expect("the Copy as submenu is open");
    assert!(
        flyout.right() <= menu.left() + tree::SUBMENU_OVERLAP + px(1.0),
        "{flyout:?} opens left of {menu:?}"
    );
    assert!(inside(flyout, screen), "{flyout:?} inside {screen:?}");
}

/// A menu taller than the window is capped at its height and scrolls: the
/// last row, reached with Up from the first, is scrolled into view.
#[gpui::test]
fn a_menu_taller_than_the_window_scrolls_the_selected_row_into_view(cx: &mut TestAppContext) {
    let (panel, window) = table(cx);
    window.simulate_resize(size(px(600.0), px(260.0)));
    window.run_until_parked();
    let screen = viewport(window);

    open_menu_at(&panel, window, point(px(40.0), px(40.0)));
    let menu = window.debug_bounds("context-menu").expect("the menu");
    assert!(inside(menu, screen), "capped at the window: {menu:?}");

    let last = window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            let menu = panel.context_menu.as_ref().expect("open");
            let nodes = panel.context_menu_tree(menu, cx);
            match nodes.iter().rev().find(|node| node.is_selectable()) {
                Some(MenuNode::Submenu(submenu)) => submenu.id,
                other => panic!("the menu ends with a submenu: {other:?}"),
            }
        })
    });
    assert_eq!(last, "toolbar");
    assert!(
        window
            .debug_bounds("context-menu-row-toolbar")
            .is_none_or(|row| !inside(row, menu)),
        "the last row starts out of view"
    );

    // Up from the first row wraps to the last one.
    keys(window, "k");

    let row = window
        .debug_bounds("context-menu-row-toolbar")
        .expect("the last row is drawn");
    assert!(inside(row, menu), "{row:?} scrolled into {menu:?}");
}

/// Enter (or Right) opens a submenu and moves the keys into it, a nested
/// submenu (More, Generate SQL) the same way, and Left steps back out.
#[gpui::test]
fn the_menu_keys_walk_into_and_out_of_nested_submenus(cx: &mut TestAppContext) {
    let (panel, window) = table(cx);
    open_menu_at(&panel, window, point(px(40.0), px(40.0)));

    let selected = |window: &mut VisualTestContext| {
        window.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                let menu = panel.context_menu.as_ref().expect("open");
                let nodes = panel.context_menu_tree(menu, cx);
                let path = tree::normalize(&nodes, &menu.path);
                let target = match tree::node_at(&nodes, &path) {
                    Some(MenuNode::Entry(entry)) => Some(entry.target.clone()),
                    _ => None,
                };
                (path.len(), target)
            })
        })
    };

    // Up twice from the first row: Toolbar, then More; into More and into
    // its first row, Generate SQL.
    keys(window, "k k l l");
    assert_eq!(
        selected(window),
        (
            3,
            Some(MenuTarget::Action(ContextMenuAction::GenerateSelectWhere))
        )
    );
    assert!(
        window
            .debug_bounds("context-menu-flyout-generate-sql")
            .is_some()
    );

    keys(window, "h");
    assert_eq!(selected(window).0, 2, "Left steps back into More");
    keys(window, "h h");
    assert!(
        window.update(|_, cx| panel.read(cx).context_menu.is_none()),
        "Left on the menu's own level closes it"
    );
}
