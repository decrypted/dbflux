//! Menus are drawn at window level, so the window bounds them and not the
//! panel that opened them. These tests host the grid in part of a larger
//! window, as a result pane under an editor sits, and read where the menu is
//! drawn. A menu opened from the keyboard has to hang off the selected cell
//! wherever the grid is scrolled to, which a click on its anchor proves.

use super::filter_history::{method_rows, register_filter_stub};
use super::*;
use crate::data_grid_panel::TableContextMenu;
use crate::data_grid_panel::context_menu::tree::{MENU_EDGE_GAP, SUBMENU_OVERLAP};
use crate::keyboard_test_support::{KeymapHost, host_document, init_keyboard_runtime};
use dbflux_app::keymap::ContextId;
use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_components::components::data_table::{
    ContextMenuAction, DataTableState, ROW_NUMBER_WIDTH,
};
use dbflux_components::fonts;
use gpui::{
    App, Bounds, Context, Entity, InteractiveElement as _, IntoElement, Modifiers, MouseButton,
    ParentElement as _, Pixels, Point, Render, ScrollStrategy, Size, Styled as _, Window, div,
    point, px, size,
};

/// A window with the grid in `slot` and nothing elsewhere. The slot clips, as
/// the workspace's panes do, so what is drawn past it had to escape it.
struct GridSlot {
    panel: Entity<DataGridPanel>,
    slot: Bounds<Pixels>,
}

impl Render for GridSlot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(
            div()
                .absolute()
                .left(self.slot.left())
                .top(self.slot.top())
                .w(self.slot.size.width)
                .h(self.slot.size.height)
                .overflow_hidden()
                .child(self.panel.clone()),
        )
    }
}

fn slot_context(slot: &GridSlot, cx: &App) -> ContextId {
    slot.panel.read(cx).active_context(cx)
}

fn slot_dispatch(
    slot: &mut GridSlot,
    command: Command,
    window: &mut Window,
    cx: &mut Context<GridSlot>,
) -> bool {
    slot.panel
        .update(cx, |panel, cx| panel.dispatch_command(command, window, cx))
}

fn slot_at(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

fn window_of(width: f32, height: f32) -> Size<Pixels> {
    size(px(width), px(height))
}

type Hosted<'a> = (
    Entity<KeymapHost<GridSlot>>,
    Entity<DataGridPanel>,
    &'a mut VisualTestContext,
);

fn host_in_slot<'a>(
    cx: &'a mut TestAppContext,
    slot: Bounds<Pixels>,
    window_size: Size<Pixels>,
    build: impl FnOnce(&mut Window, &mut Context<DataGridPanel>) -> DataGridPanel + 'static,
) -> Hosted<'a> {
    let (host, window) = host_document(
        cx,
        move |window, cx| {
            let panel = cx.new(|cx| build(window, cx));
            cx.new(|_| GridSlot { panel, slot })
        },
        slot_context,
        slot_dispatch,
    );
    let panel = window.update(|_, cx| host.read(cx).document.read(cx).panel.clone());
    window.simulate_resize(window_size);
    window.run_until_parked();

    let table = table_state(&panel, window);
    window.update(|window, cx| {
        let focus_handle = table.read(cx).focus_handle().clone();
        focus_handle.focus(window, cx);
    });
    window.run_until_parked();

    (host, panel, window)
}

fn numbers(rows: i64, columns: usize) -> QueryResult {
    QueryResult::table(
        (0..columns)
            .map(|col| key_column(&format!("col_{col}"), false))
            .collect(),
        (0..rows)
            .map(|row| (0..columns).map(|_| dbflux_core::Value::Int(row)).collect())
            .collect(),
        None,
        Duration::ZERO,
    )
}

/// A `rows` by `columns` query result, which draws no toolbar over its grid.
fn result_in_slot(
    cx: &mut TestAppContext,
    rows: i64,
    columns: usize,
    slot: Bounds<Pixels>,
    window_size: Size<Pixels>,
) -> Hosted<'_> {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);

    host_in_slot(cx, slot, window_size, move |window, cx| {
        DataGridPanel::new_for_result(
            Arc::new(numbers(rows, columns)),
            "SELECT * FROM numbers".to_string(),
            None,
            app_state,
            window,
            cx,
        )
    })
}

/// A table of `rows` rows, which draws its filter toolbar over its grid.
fn table_in_slot(
    cx: &mut TestAppContext,
    rows: i64,
    slot: Bounds<Pixels>,
    window_size: Size<Pixels>,
) -> Hosted<'_> {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let (profile_id, _log) =
        cx.update(|cx| register_filter_stub(&app_state, method_rows(), None, cx));

    host_in_slot(cx, slot, window_size, move |window, cx| {
        let source = DataSource::Table {
            profile_id,
            database: Some("app".to_string()),
            table: TableRef::with_schema("public", "numbers"),
            pagination: Pagination::default(),
            order_by: Vec::new(),
            total_rows: Some(rows as u64),
        };

        let mut panel =
            DataGridPanel::new_internal(source, app_state, vec!["col_0".to_string()], window, cx);
        panel.set_result(numbers(rows, 3), cx);
        panel
    })
}

fn table_state(
    panel: &Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> Entity<DataTableState> {
    window.update(|_, cx| {
        panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("the result builds a table")
    })
}

fn screen(window: &mut VisualTestContext) -> Bounds<Pixels> {
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

/// `point` is in `bounds`, edges included: an anchor kept in the grid can sit
/// on its edge.
fn within(point: Point<Pixels>, bounds: Bounds<Pixels>) -> bool {
    point.x >= bounds.left()
        && point.x <= bounds.right()
        && point.y >= bounds.top()
        && point.y <= bounds.bottom()
}

fn close_to(a: Pixels, b: Pixels) -> bool {
    (a - b).abs() <= px(1.0)
}

/// The grid as drawn: its header and rows, from the header's own bounds.
fn grid_bounds(panel: &Entity<DataGridPanel>, window: &mut VisualTestContext) -> Bounds<Pixels> {
    let table = table_state(panel, window);
    let header = window
        .debug_bounds("table-header")
        .expect("the grid is drawn");
    let size = window.update(|_, cx| table.read(cx).viewport_size());
    Bounds::new(header.origin, size)
}

/// The middle of row `row`'s first data column, in a grid not scrolled.
fn cell_point(window: &mut VisualTestContext, row: usize) -> Point<Pixels> {
    let header = window
        .debug_bounds("table-header")
        .expect("the grid is drawn");
    let row_height = window.update(|_, cx| fonts::grid_row_height(cx));
    point(
        header.left() + ROW_NUMBER_WIDTH + px(30.0),
        header.bottom() + row_height * (row as f32 + 0.5),
    )
}

fn right_click(window: &mut VisualTestContext, at: Point<Pixels>) {
    window.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    window.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    window.run_until_parked();
}

fn menu_bounds(window: &mut VisualTestContext) -> Bounds<Pixels> {
    window
        .debug_bounds("context-menu")
        .expect("the menu is drawn")
}

fn keys(window: &mut VisualTestContext, keystrokes: &str) {
    for keystroke in keystrokes.split(' ') {
        window.simulate_keystrokes(keystroke);
        window.run_until_parked();
    }
}

fn resize(window: &mut VisualTestContext, to: Size<Pixels>) {
    window.simulate_resize(to);
    window.run_until_parked();
}

/// Selects `coord` and scrolls the grid to put row `top` first, without the
/// selection following.
fn select_and_scroll(
    panel: &Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    coord: CellCoord,
    top: usize,
) {
    let table = table_state(panel, window);
    window.update(|_, cx| {
        table.update(cx, |state, cx| {
            state.select_cell(coord, cx);
            state
                .vertical_scroll_handle()
                .scroll_to_item(top, ScrollStrategy::Top);
            cx.notify();
        })
    });
    window.run_until_parked();
}

fn open_from_keyboard(panel: &Entity<DataGridPanel>, window: &mut VisualTestContext) {
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open_context_menu_at_selection(window, cx)
        })
    });
    window.run_until_parked();
}

fn close_menu(panel: &Entity<DataGridPanel>, window: &mut VisualTestContext) {
    keys(window, "escape");
    assert!(window.update(|_, cx| panel.read(cx).context_menu.is_none()));
}

fn anchor(panel: &Entity<DataGridPanel>, window: &mut VisualTestContext) -> Point<Pixels> {
    window.update(|_, cx| {
        panel
            .read(cx)
            .context_menu
            .as_ref()
            .map(|menu| menu.position)
            .expect("the menu opens")
    })
}

/// The anchor of the open menu is on the cell `expected`: closing the menu
/// and clicking just below the anchor lands on that cell. This reads the row
/// off what the grid draws, so an anchor that forgets the scroll, or that
/// guesses where the grid starts, ends up on another row.
fn assert_anchor_is_on(
    panel: &Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    expected: CellCoord,
) {
    let at = anchor(panel, window);
    let row_height = window.update(|_, cx| fonts::grid_row_height(cx));
    close_menu(panel, window);
    let table = table_state(panel, window);
    let elsewhere = CellCoord::new(expected.row.saturating_sub(1), expected.col + 1);
    window.update(|_, cx| table.update(cx, |state, cx| state.select_cell(elsewhere, cx)));
    window.run_until_parked();

    window.simulate_click(at + point(px(0.0), row_height / 2.0), Modifiers::default());
    window.run_until_parked();

    let active = window.update(|_, cx| table.read(cx).selection().active);
    assert_eq!(
        active,
        Some(expected),
        "a click at the anchor {at:?} lands on {active:?}"
    );
}

/// The case the window bound exists for: the grid is shorter than the menu
/// and low in the window, so the menu is lifted over what is above the grid.
#[gpui::test]
fn a_right_click_in_a_grid_shorter_than_the_menu_stays_inside_the_window(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 640.0, 700.0, 150.0);
    let (_host, _panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1000.0, 800.0));

    let at = cell_point(window, 0);
    right_click(window, at);

    let menu = menu_bounds(window);
    let screen = screen(window);
    assert!(
        menu.size.height > slot.size.height,
        "the menu {menu:?} must be taller than the grid {slot:?}"
    );
    assert!(inside(menu, screen), "{menu:?} inside {screen:?}");
    assert!(menu.top() < slot.top(), "lifted over the grid: {menu:?}");
}

/// With room in the window the menu stays at the click and hangs past the
/// bottom of the panel.
#[gpui::test]
fn a_menu_that_fits_in_the_window_stays_at_the_click_past_the_panels_edge(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 100.0, 700.0, 150.0);
    let (_host, _panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1000.0, 800.0));

    let at = cell_point(window, 1);
    right_click(window, at);

    let menu = menu_bounds(window);
    assert!(close_to(menu.left(), at.x) && close_to(menu.top(), at.y));
    assert!(menu.bottom() > slot.bottom(), "past the panel: {menu:?}");
}

/// Without the room below the click, a menu that fits above it opens upwards.
#[gpui::test]
fn a_menu_without_room_below_the_click_opens_upwards_from_it(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 400.0, 700.0, 390.0);
    let (_host, _panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1000.0, 800.0));

    let at = cell_point(window, 5);
    right_click(window, at);

    let menu = menu_bounds(window);
    assert!(
        at.y + menu.size.height > px(800.0),
        "the test needs a click without room below it: {at:?}, {menu:?}"
    );
    assert!(close_to(menu.bottom(), at.y), "{menu:?} ends at {at:?}");
}

/// A menu taller than the window starts at the window's top gap and is
/// capped at the window's height, wherever the panel that opened it sits.
#[gpui::test]
fn a_menu_taller_than_the_window_starts_at_its_top_gap(cx: &mut TestAppContext) {
    let slot = slot_at(20.0, 100.0, 500.0, 100.0);
    let (_host, panel, window) = result_in_slot(cx, 20, 3, slot, window_of(600.0, 200.0));

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.context_menu = Some(TableContextMenu::new(0, 0, point(px(40.0), px(150.0))));
            panel.focus.context_menu_focus.focus(window, cx);
            cx.notify();
        })
    });
    window.run_until_parked();

    let menu = menu_bounds(window);
    assert!(close_to(menu.top(), MENU_EDGE_GAP), "{menu:?}");
    assert!(
        close_to(menu.size.height, px(200.0) - MENU_EDGE_GAP * 2.0),
        "capped at the window: {menu:?}"
    );
}

/// The window's right edge decides the side, so a menu in a panel with no
/// room to its right still opens its submenu there when the window has it.
#[gpui::test]
fn a_submenu_hangs_past_the_panel_when_the_window_has_the_room(cx: &mut TestAppContext) {
    let slot = slot_at(400.0, 100.0, 300.0, 500.0);
    let (_host, _panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1200.0, 800.0));

    let at = cell_point(window, 1);
    right_click(window, at);
    keys(window, "j l");

    let menu = menu_bounds(window);
    let flyout = window
        .debug_bounds("context-menu-flyout-copy-as")
        .expect("the Copy as submenu is open");
    assert!(
        flyout.left() >= menu.right() - SUBMENU_OVERLAP - px(1.0),
        "{flyout:?} opens right of {menu:?}"
    );
    assert!(
        flyout.right() > slot.right(),
        "{flyout:?} past the panel {slot:?}"
    );
}

/// At the window's right edge the submenu goes left, out of the panel.
#[gpui::test]
fn a_submenu_at_the_windows_right_edge_opens_left_past_the_panel(cx: &mut TestAppContext) {
    let slot = slot_at(800.0, 100.0, 400.0, 500.0);
    let (_host, _panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1200.0, 800.0));

    let at = cell_point(window, 1);
    right_click(window, at);
    keys(window, "j l");

    let menu = menu_bounds(window);
    let flyout = window
        .debug_bounds("context-menu-flyout-copy-as")
        .expect("the Copy as submenu is open");
    let screen = screen(window);
    assert!(
        flyout.right() <= menu.left() + SUBMENU_OVERLAP + px(1.0),
        "{flyout:?} opens left of {menu:?}"
    );
    assert!(flyout.left() < slot.left(), "{flyout:?} left of {slot:?}");
    assert!(inside(flyout, screen), "{flyout:?} inside {screen:?}");
}

/// A row of a menu hanging past the panel takes the mouse like any other,
/// since the deferred menu escapes what clips the panel.
#[gpui::test]
fn a_menu_row_past_the_panels_edge_takes_the_click(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 100.0, 700.0, 150.0);
    let (_host, panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1000.0, 800.0));

    let at = cell_point(window, 1);
    right_click(window, at);

    let menu = menu_bounds(window);
    let row = point(menu.center().x, menu.bottom() - px(14.0));
    assert!(row.y > slot.bottom(), "the row {row:?} is past {slot:?}");

    let path = |window: &mut VisualTestContext| {
        window.update(|_, cx| {
            panel
                .read(cx)
                .context_menu
                .as_ref()
                .map(|menu| menu.path.clone())
        })
    };
    let before = path(window);
    assert!(before.is_some());

    window.simulate_click(row, Modifiers::default());
    window.run_until_parked();

    assert_ne!(path(window), before, "the click reached the row");
}

/// The bug: `row * row_height` with no scroll in it lies a table's height
/// below the grid. Row 95 of a grid scrolled to row 90 is drawn sixth.
#[gpui::test]
fn keyboard_menu_on_a_scrolled_grid_hangs_off_the_selected_row(cx: &mut TestAppContext) {
    let slot = slot_at(0.0, 0.0, 1000.0, 400.0);
    let (_host, panel, window) = result_in_slot(cx, 200, 3, slot, window_of(1000.0, 800.0));
    let cell = CellCoord::new(95, 1);

    select_and_scroll(&panel, window, cell, 90);
    open_from_keyboard(&panel, window);

    let grid = grid_bounds(&panel, window);
    let at = anchor(&panel, window);
    assert!(within(at, grid), "{at:?} inside the grid {grid:?}");
    assert_anchor_is_on(&panel, window, cell);
}

/// A table draws its toolbar over the grid, a result does not: the anchor
/// follows the grid wherever it is in the panel, and the panel in the window.
#[gpui::test]
fn keyboard_menu_on_a_table_low_in_the_window_hangs_off_the_selected_row(cx: &mut TestAppContext) {
    let slot = slot_at(120.0, 250.0, 800.0, 500.0);
    let (_host, panel, window) = table_in_slot(cx, 200, slot, window_of(1000.0, 800.0));
    let cell = CellCoord::new(95, 1);

    select_and_scroll(&panel, window, cell, 92);
    open_from_keyboard(&panel, window);

    assert_anchor_is_on(&panel, window, cell);
}

/// A grid not scrolled at all, for the rows the old formula got close to.
#[gpui::test]
fn keyboard_menu_on_an_unscrolled_grid_hangs_off_the_selected_row(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 200.0, 800.0, 500.0);
    let (_host, panel, window) = result_in_slot(cx, 200, 3, slot, window_of(1000.0, 800.0));
    let cell = CellCoord::new(2, 1);

    select_and_scroll(&panel, window, cell, 0);
    open_from_keyboard(&panel, window);

    assert_anchor_is_on(&panel, window, cell);
}

/// The real keys on the case that showed the bug: down to the last row with
/// `Ctrl+End`, then `m` and `Shift+F10`.
#[gpui::test]
fn the_menu_keys_on_the_last_row_of_a_long_grid_open_on_that_row(cx: &mut TestAppContext) {
    let slot = slot_at(0.0, 0.0, 1000.0, 400.0);
    let (_host, panel, window) = result_in_slot(cx, 200, 3, slot, window_of(1000.0, 800.0));
    let table = table_state(&panel, window);

    keys(window, "j ctrl-end");
    let active = window
        .update(|_, cx| table.read(cx).selection().active)
        .expect("a cell is selected");
    assert_eq!(active.row, 199);

    for key in ["m", "shift-f10"] {
        keys(window, key);
        let grid = grid_bounds(&panel, window);
        let at = anchor(&panel, window);
        assert!(within(at, grid), "{key}: {at:?} in {grid:?}");
        assert_anchor_is_on(&panel, window, active);
    }
}

/// A selected row scrolled out of view has no place on screen, so the anchor
/// stays in the grid, on the side the row went.
#[gpui::test]
fn keyboard_menu_for_a_row_scrolled_out_of_view_stays_inside_the_grid(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 100.0, 800.0, 400.0);
    let (_host, panel, window) = result_in_slot(cx, 200, 3, slot, window_of(1000.0, 800.0));

    select_and_scroll(&panel, window, CellCoord::new(199, 0), 0);
    open_from_keyboard(&panel, window);
    let grid = grid_bounds(&panel, window);
    let below = anchor(&panel, window);
    assert!(close_to(below.y, grid.bottom()), "{below:?} in {grid:?}");

    close_menu(&panel, window);
    select_and_scroll(&panel, window, CellCoord::new(0, 0), 150);
    open_from_keyboard(&panel, window);
    let above = anchor(&panel, window);
    assert!(close_to(above.y, grid.top()), "{above:?} in {grid:?}");
}

/// The same across: the active column scrolled out to either side of a grid
/// narrower than its columns.
#[gpui::test]
fn keyboard_menu_for_a_column_scrolled_out_of_view_stays_inside_the_grid(cx: &mut TestAppContext) {
    let slot = slot_at(100.0, 100.0, 400.0, 500.0);
    let (_host, panel, window) = result_in_slot(cx, 20, 30, slot, window_of(1000.0, 800.0));
    let table = table_state(&panel, window);

    window
        .update(|_, cx| table.update(cx, |state, cx| state.select_cell(CellCoord::new(1, 29), cx)));
    window.run_until_parked();
    open_from_keyboard(&panel, window);
    let grid = grid_bounds(&panel, window);
    let at = anchor(&panel, window);
    assert!(within(at, grid), "column to the right: {at:?} in {grid:?}");

    close_menu(&panel, window);
    window.update(|_, cx| {
        table.update(cx, |state, cx| {
            state.scroll_to_column(29);
            state.select_cell(CellCoord::new(1, 0), cx);
            cx.notify();
        })
    });
    window.run_until_parked();
    // The horizontal offset syncs from the handle when the grid is drawn.
    window.update(|_, cx| table.update(cx, |_, cx| cx.notify()));
    window.run_until_parked();
    assert!(
        window.update(|_, cx| table.read(cx).horizontal_offset()) > px(0.0),
        "the grid scrolled"
    );

    open_from_keyboard(&panel, window);
    let at = anchor(&panel, window);
    assert!(within(at, grid), "column to the left: {at:?} in {grid:?}");
}

/// The record view has no rows to scroll to, and the anchor stays in it.
#[gpui::test]
fn keyboard_menu_in_the_record_view_stays_inside_the_grid(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 100.0, 800.0, 400.0);
    let (_host, panel, window) = result_in_slot(cx, 200, 3, slot, window_of(1000.0, 800.0));
    let table = table_state(&panel, window);

    window.update(|_, cx| {
        table.update(cx, |state, cx| {
            state.select_cell(CellCoord::new(150, 1), cx)
        });
        panel.update(cx, |panel, cx| panel.set_record_mode(true, cx));
    });
    window.run_until_parked();
    assert!(window.update(|_, cx| panel.read(cx).record_mode()));

    open_from_keyboard(&panel, window);
    let at = anchor(&panel, window);
    assert!(within(at, slot), "{at:?} in {slot:?}");
}

/// `Shift+m` opens the column menu right under its header, whether or not a
/// toolbar sits over the grid.
#[gpui::test]
fn the_column_menu_from_the_keyboard_hangs_under_its_header(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 200.0, 800.0, 500.0);
    let (_host, panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1000.0, 800.0));

    keys(window, "j shift-m");

    let header = window.debug_bounds("table-header").expect("the header");
    let column = window.debug_bounds("header-col-0").expect("the column");
    let at = anchor(&panel, window);
    assert!(close_to(at.y, header.bottom()), "{at:?} under {header:?}");
    assert!(at.x >= column.left() && at.x <= column.right());
}

/// The same with a table's toolbar over the grid.
#[gpui::test]
fn the_column_menu_on_a_table_hangs_under_its_header(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 200.0, 800.0, 500.0);
    let (_host, panel, window) = table_in_slot(cx, 20, slot, window_of(1000.0, 800.0));

    keys(window, "j shift-m");

    let header = window.debug_bounds("table-header").expect("the header");
    let at = anchor(&panel, window);
    assert!(close_to(at.y, header.bottom()), "{at:?} under {header:?}");
}

/// The column menu's column scrolled out of view: the anchor stays in the grid.
#[gpui::test]
fn the_column_menu_for_a_column_out_of_view_stays_inside_the_grid(cx: &mut TestAppContext) {
    let slot = slot_at(100.0, 100.0, 400.0, 500.0);
    let (_host, panel, window) = result_in_slot(cx, 20, 30, slot, window_of(1000.0, 800.0));
    let table = table_state(&panel, window);

    window
        .update(|_, cx| table.update(cx, |state, cx| state.select_cell(CellCoord::new(1, 29), cx)));
    window.run_until_parked();
    keys(window, "shift-m");

    let grid = grid_bounds(&panel, window);
    let at = anchor(&panel, window);
    assert!(within(at, grid), "{at:?} in {grid:?}");
}

/// `place_menu` gets the size of the frame being drawn: a window shrunk under
/// an open menu caps it at the new height at once. `anchored` could slide a
/// menu placed against an older size back in, but not make it shorter.
#[gpui::test]
fn a_menu_follows_the_window_in_the_frame_it_shrinks(cx: &mut TestAppContext) {
    let slot = slot_at(0.0, 0.0, 600.0, 160.0);
    let (_host, _panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1000.0, 800.0));

    let at = cell_point(window, 0);
    right_click(window, at);
    let before = menu_bounds(window);
    assert!(before.size.height > px(160.0), "taller than {before:?}");

    resize(window, window_of(600.0, 160.0));

    let menu = menu_bounds(window);
    let screen = screen(window);
    assert!(inside(menu, screen), "{menu:?} inside {screen:?}");
}

fn open_chooser(panel: &Entity<DataGridPanel>, window: &mut VisualTestContext) {
    keys(window, "j shift-m");
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.handle_context_menu_action(ContextMenuAction::ChooseColumns, window, cx);
        })
    });
    window.run_until_parked();
}

fn chooser_bounds(window: &mut VisualTestContext) -> Bounds<Pixels> {
    window
        .debug_bounds("column-chooser")
        .expect("the chooser is drawn")
}

/// The Columns chooser is placed like the menu that opened it: lifted inside
/// the window from a grid shorter than it.
#[gpui::test]
fn the_columns_chooser_from_a_short_grid_stays_inside_the_window(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 640.0, 700.0, 150.0);
    let (_host, panel, window) = result_in_slot(cx, 20, 16, slot, window_of(1000.0, 800.0));

    open_chooser(&panel, window);

    let chooser = chooser_bounds(window);
    let screen = screen(window);
    assert!(
        chooser.size.height > slot.size.height,
        "{chooser:?} taller than {slot:?}"
    );
    assert!(inside(chooser, screen), "{chooser:?} inside {screen:?}");
    assert!(chooser.top() < slot.top(), "lifted over the grid");
}

/// With the room in the window it stays where the menu was, past the panel.
#[gpui::test]
fn the_columns_chooser_stays_at_its_menu_past_the_panels_edge(cx: &mut TestAppContext) {
    let slot = slot_at(50.0, 100.0, 700.0, 150.0);
    let (_host, panel, window) = result_in_slot(cx, 20, 16, slot, window_of(1000.0, 800.0));

    open_chooser(&panel, window);

    let chooser = chooser_bounds(window);
    let position = window.update(|_, cx| {
        panel
            .read(cx)
            .column_chooser
            .as_ref()
            .map(|chooser| chooser.position)
            .expect("the chooser is open")
    });
    assert!(close_to(chooser.left(), position.x));
    assert!(close_to(chooser.top(), position.y));
    assert!(chooser.bottom() > slot.bottom(), "past the panel");
}

/// The chooser is capped by the frame being drawn too.
#[gpui::test]
fn the_columns_chooser_follows_the_window_in_the_frame_it_shrinks(cx: &mut TestAppContext) {
    let slot = slot_at(0.0, 0.0, 600.0, 160.0);
    let (_host, panel, window) = result_in_slot(cx, 20, 16, slot, window_of(1000.0, 800.0));

    open_chooser(&panel, window);
    assert!(chooser_bounds(window).size.height > px(160.0));

    resize(window, window_of(600.0, 160.0));

    let chooser = chooser_bounds(window);
    let screen = screen(window);
    assert!(inside(chooser, screen), "{chooser:?} inside {screen:?}");
}

/// The export menu takes its cap from the size this frame stored.
#[gpui::test]
fn the_export_menu_follows_the_window_in_the_frame_it_shrinks(cx: &mut TestAppContext) {
    let slot = slot_at(0.0, 0.0, 600.0, 160.0);
    let (_host, _panel, window) = result_in_slot(cx, 20, 3, slot, window_of(1000.0, 800.0));

    keys(window, "ctrl-e");
    let before = window.debug_bounds("export-menu").expect("the menu");
    assert!(before.size.height > px(160.0), "taller than {before:?}");

    resize(window, window_of(600.0, 160.0));

    let menu = window.debug_bounds("export-menu").expect("the menu");
    let screen = screen(window);
    assert!(inside(menu, screen), "{menu:?} inside {screen:?}");
}
