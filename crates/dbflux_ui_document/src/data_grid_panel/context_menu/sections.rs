//! Draws the grid's menus from their `tree` and runs the menu keys on the
//! same tree.

use super::tree::{
    self, HeaderTone, MenuEntry, MenuHeader, MenuNode, MenuPlacement, MenuTarget, RowHeights,
    Submenu,
};
use super::{
    COLUMN_HEADER_MENU_WIDTH, CONTEXT_MENU_WIDTH, DataGridEvent, DataGridPanel, TableContextMenu,
};
use dbflux_app::keymap::{Command, ContextId};
use dbflux_components::components::data_table::{ContextMenuAction, context_menu_keystroke};
use dbflux_components::composites::{
    MenuItem, menu_frame, menu_row, render_menu_header, render_separator,
};
use dbflux_components::tokens::MenuMetrics;
use dbflux_ui_base::keymap::{chord_display_parts, effective_keymap, key_chord_from_gpui};
use gpui::prelude::FluentBuilder;
use gpui::{deferred, *};

/// The frame a submenu hangs in.
///
/// It sits beside its row, on the right unless `open_left`, and `anchored`
/// slides it back into the window when it would run past the bottom or a
/// side: a long filter list opened from a row near the bottom of the window
/// is shifted up rather than cut off. The flyout is deferred, so the
/// scrolling level it hangs off does not clip it.
pub(super) fn submenu_frame(open_left: bool, flyout: impl IntoElement) -> Div {
    let (frame, corner) = if open_left {
        (div().absolute().right_full(), Anchor::TopRight)
    } else {
        (div().absolute().left_full(), Anchor::TopLeft)
    };
    // Deferred above the parent menu (priority 2): in a shared layer GPUI
    // paints icon sprites after quads, so the parent rows' chevrons would
    // otherwise show through the flyout's background.
    frame.top(-MenuMetrics::PADDING_Y).child(
        deferred(
            anchored()
                .anchor(corner)
                .snap_to_window_with_margin(tree::MENU_EDGE_GAP)
                .child(flyout),
        )
        .with_priority(2),
    )
}

/// The rows of one menu level in a column at most `max_height` tall (the
/// frame's padding included); a taller level scrolls, and `scroll` lets the
/// menu keys bring the highlighted row into view.
fn level_rows(
    depth: usize,
    max_height: Pixels,
    scroll: Option<&ScrollHandle>,
    rows: Vec<AnyElement>,
) -> Stateful<Div> {
    div()
        .id(("context-menu-level", depth))
        .flex()
        .flex_col()
        .max_h((max_height - MenuMetrics::PADDING_Y * 2.0).max(px(0.0)))
        .overflow_y_scroll()
        .when_some(scroll, |level, handle| level.track_scroll(handle))
        .children(rows)
}

/// The shortcut shown on a menu row, formatted like the other keycaps in the
/// app (`Ctrl C`, `Delete`).
///
/// Row actions the data table binds come from its GPUI bindings; the ones
/// dispatched through the app keymap (inspect row, view value, copy as JSON,
/// add to filter) come from the Results layer.
pub(in crate::data_grid_panel) fn action_shortcut(
    action: ContextMenuAction,
    cx: &App,
) -> Option<SharedString> {
    if let Some(keystroke) = context_menu_keystroke(action, cx) {
        let label = chord_display_parts(&key_chord_from_gpui(&keystroke)).join(" ");

        return Some(label.into());
    }

    let command = match action {
        ContextMenuAction::InspectRow => Command::ToggleRowInspector,
        ContextMenuAction::ViewValue => Command::ToggleValuePanel,
        ContextMenuAction::FindSimilarRows => Command::FindSimilarRows,
        ContextMenuAction::CopyAsJson => Command::CopySelectionAsJson,
        ContextMenuAction::AddToFilter => Command::AddToFilter,
        ContextMenuAction::AddRow => Command::ResultsAddRow,
        _ => return None,
    };

    effective_keymap()
        .chord_for_command(ContextId::Results, command)
        .map(|chord| chord_display_parts(chord).join(" ").into())
}

fn target_shortcut(target: &MenuTarget, cx: &App) -> Option<SharedString> {
    match target {
        MenuTarget::Action(action) => action_shortcut(*action, cx),
        MenuTarget::Toolbar(action) => action.shortcut(),
        MenuTarget::RowAction(_) => None,
    }
}

fn header_row(header: &MenuHeader, cx: &App) -> AnyElement {
    let theme = gpui_component::ActiveTheme::theme(cx);
    let mut item = MenuItem::header(header.label.clone());
    if let Some(icon) = header.icon {
        item = item.icon(icon);
    }
    item = match header.tone {
        HeaderTone::Muted => item,
        HeaderTone::Warning => item.header_icon_color(theme.warning),
        HeaderTone::Info => item.header_icon_color(theme.info),
    };

    render_menu_header(&item, cx).into_any_element()
}

/// `open` with `index` appended: the path of a row of the level `open`
/// leads to.
fn row_path(open: &[usize], index: usize) -> Vec<usize> {
    let mut path = open.to_vec();
    path.push(index);
    path
}

/// Where a level of the menu sits, for the side its submenus open on.
#[derive(Clone, Copy)]
struct LevelGeometry {
    left: Pixels,
    width: Pixels,
    viewport: Size<Pixels>,
}

impl DataGridPanel {
    /// The open context menu (cell, column header or document view): its
    /// rows from `context_menu_tree`, placed inside the window.
    pub(in crate::data_grid_panel) fn render_context_menu(
        &self,
        menu: &TableContextMenu,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let nodes = self.context_menu_tree(menu, cx);
        let path = tree::normalize(&nodes, &menu.path);
        let width = if menu.is_column_header {
            COLUMN_HEADER_MENU_WIDTH
        } else {
            CONTEXT_MENU_WIDTH
        };

        let height = tree::menu_height(&nodes, RowHeights::from_app(cx));
        let placement = tree::place_menu(menu.position, size(width, height), viewport);
        let geometry = LevelGeometry {
            left: placement.origin.x,
            width,
            viewport,
        };
        let rows = self.render_menu_level(&nodes, &[], &path, geometry, cx);

        self.render_context_menu_overlay(placement, width, rows, cx)
    }

    /// The rows of the level `open` leads to (the menu itself when empty),
    /// with the submenu `path` opens drawn beside its row.
    fn render_menu_level(
        &self,
        nodes: &[MenuNode],
        open: &[usize],
        path: &[usize],
        geometry: LevelGeometry,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let depth = open.len();
        let on_this_level = path.get(..depth) == Some(open);
        let highlighted = path.get(depth).copied().filter(|_| on_this_level);
        let open_child = highlighted.filter(|_| path.len() > depth + 1);

        nodes
            .iter()
            .enumerate()
            .map(|(index, node)| match node {
                MenuNode::Separator => render_separator(cx).into_any_element(),
                MenuNode::Header(header) => header_row(header, cx),
                MenuNode::Entry(entry) => self.render_menu_entry(
                    entry,
                    row_path(open, index),
                    highlighted == Some(index),
                    cx,
                ),
                MenuNode::Submenu(submenu) => {
                    let flyout = (open_child == Some(index)).then(|| {
                        self.render_submenu_flyout(
                            submenu,
                            &row_path(open, index),
                            path,
                            geometry,
                            cx,
                        )
                    });
                    Self::render_submenu_trigger(
                        submenu,
                        row_path(open, index),
                        highlighted == Some(index),
                        flyout,
                        cx,
                    )
                }
            })
            .collect()
    }

    /// A row that runs its target. Hovering it highlights it and closes any
    /// submenu open beside its level, as native menus do.
    fn render_menu_entry(
        &self,
        entry: &MenuEntry,
        path: Vec<usize>,
        highlighted: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut item = MenuItem::new(entry.label.clone());
        if let Some(icon) = entry.icon {
            item = item.icon(icon);
        }
        if entry.danger {
            item = item.danger();
        }
        if let Some(shortcut) = target_shortcut(&entry.target, cx) {
            item = item.shortcut(shortcut);
        }
        if let Some(detail) = &entry.detail {
            item = item.detail(detail.clone());
        }
        if entry.disabled {
            item = item.disabled();
        }

        let selector = format!("context-menu-row-{}", entry.id);
        let row = menu_row(entry.id.clone(), &item, highlighted, cx)
            .debug_selector(move || selector.clone())
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                this.set_context_menu_path(path.clone(), false, cx);
            }));

        if entry.disabled {
            return row.into_any_element();
        }

        let target = entry.target.clone();
        row.on_click(cx.listener(move |this, _, window, cx| {
            this.run_menu_target(target.clone(), window, cx);
        }))
        .into_any_element()
    }

    /// A row that opens `submenu`. Hovering opens it, as native menus do;
    /// a click opens or closes it.
    fn render_submenu_trigger(
        submenu: &Submenu,
        path: Vec<usize>,
        highlighted: bool,
        flyout: Option<(bool, Div)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = flyout.is_some();
        let item = MenuItem::new(submenu.label.clone())
            .icon(submenu.icon)
            .submenu();
        let first = tree::first_selectable(&submenu.children).unwrap_or(0);
        let opened = row_path(&path, first);
        let selector = format!("context-menu-row-{}", submenu.id);

        let hover_path = path.clone();
        let hover_opened = opened.clone();
        menu_row(submenu.id, &item, highlighted || is_open, cx)
            .debug_selector(move || selector.clone())
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                let already_open = this.context_menu.as_ref().is_some_and(|menu| {
                    menu.path.len() > hover_path.len() && menu.path.starts_with(&hover_path)
                });
                if !already_open {
                    this.open_context_submenu(hover_opened.clone(), false, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if is_open {
                    this.set_context_menu_path(path.clone(), false, cx);
                } else {
                    this.open_context_submenu(opened.clone(), false, cx);
                }
            }))
            .when_some(flyout, |row, (open_left, flyout)| {
                row.child(submenu_frame(open_left, flyout))
            })
            .into_any_element()
    }

    /// The flyout of `submenu`, the row at `open` (the path of its trigger),
    /// and whether it opens to the left of that row.
    fn render_submenu_flyout(
        &self,
        submenu: &Submenu,
        open: &[usize],
        path: &[usize],
        parent: LevelGeometry,
        cx: &mut Context<Self>,
    ) -> (bool, Div) {
        let (open_left, left) = tree::submenu_side(
            parent.left,
            parent.width,
            submenu.width,
            parent.viewport.width,
        );
        let geometry = LevelGeometry {
            left,
            width: submenu.width,
            viewport: parent.viewport,
        };
        let rows = self.render_menu_level(&submenu.children, open, path, geometry, cx);
        let depth = open.len();
        let max_height = (parent.viewport.height - tree::MENU_EDGE_GAP * 2.0).max(px(0.0));

        let flyout = menu_frame(cx)
            .w(submenu.width)
            .occlude()
            .debug_selector({
                let id = submenu.id;
                move || format!("context-menu-flyout-{id}")
            })
            .child(level_rows(
                depth,
                max_height,
                self.focus.context_menu_scroll.get(depth),
                rows,
            ));

        (open_left, flyout)
    }

    /// Moves the highlight to `path`, scrolling its level to it when the
    /// keys moved it (a hovered row is in view already).
    fn set_context_menu_path(
        &mut self,
        path: Vec<usize>,
        scroll_into_view: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.context_menu.as_mut() else {
            return;
        };
        if scroll_into_view
            && let Some(index) = path.last()
            && let Some(handle) = self.focus.context_menu_scroll.get(path.len() - 1)
        {
            handle.scroll_to_item(*index);
        }
        if menu.path != path {
            menu.path = path;
            cx.notify();
        }
    }

    /// Opens the submenu whose first row `path` points at, from its top.
    fn open_context_submenu(
        &mut self,
        path: Vec<usize>,
        scroll_into_view: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = self
            .focus
            .context_menu_scroll
            .get(path.len().saturating_sub(1))
        {
            handle.set_offset(Point::default());
        }
        self.set_context_menu_path(path, scroll_into_view, cx);
    }

    /// Runs a row of the menu, closing it.
    pub(in crate::data_grid_panel) fn run_menu_target(
        &mut self,
        target: MenuTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            MenuTarget::Action(action) => self.handle_context_menu_action(action, window, cx),
            MenuTarget::Toolbar(action) => self.run_toolbar_action_from_menu(action, window, cx),
            MenuTarget::RowAction(action) => {
                let Some(menu) = self.context_menu.take() else {
                    return;
                };
                let row_values = self.collect_row_values(menu.row, cx);
                self.restore_focus_after_context_menu(false, window, cx);
                cx.emit(DataGridEvent::RowActionRequested {
                    row: menu.row,
                    action_id: action.id,
                    action_label: action.label,
                    is_destructive: action.is_destructive,
                    row_values,
                    position: menu.position,
                });
                cx.notify();
            }
        }
    }

    /// The menu keys while a context menu is open: Up and Down walk the
    /// open level, Enter or Right runs the row or opens its submenu, Left
    /// or Escape steps back out and closes the menu from its first level.
    pub(in crate::data_grid_panel) fn dispatch_menu_command(
        &mut self,
        cmd: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.context_menu.as_ref() else {
            return false;
        };
        let nodes = self.context_menu_tree(menu, cx);
        let mut path = tree::normalize(&nodes, &menu.path);

        match cmd {
            Command::MenuDown | Command::MenuUp => {
                if let Some((last, open)) = path.split_last()
                    && let Some(level) = tree::level(&nodes, open)
                {
                    let next = tree::step(level, *last, cmd == Command::MenuDown);
                    path.pop();
                    path.push(next);
                }
                self.set_context_menu_path(path, true, cx);
                true
            }
            Command::MenuSelect => {
                match tree::node_at(&nodes, &path) {
                    Some(MenuNode::Submenu(submenu)) => {
                        if let Some(first) = tree::first_selectable(&submenu.children) {
                            path.push(first);
                            self.open_context_submenu(path, true, cx);
                        }
                    }
                    Some(MenuNode::Entry(entry)) if !entry.disabled => {
                        let target = entry.target.clone();
                        self.run_menu_target(target, window, cx);
                    }
                    _ => self.set_context_menu_path(path, false, cx),
                }
                true
            }
            Command::MenuBack | Command::Cancel => {
                if path.len() > 1 {
                    path.pop();
                    self.set_context_menu_path(path, true, cx);
                } else {
                    self.dismiss_context_menu(window, cx);
                }
                true
            }
            _ => false,
        }
    }

    /// Close the menu without running anything and hand focus back.
    fn dismiss_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_document_view = self
            .context_menu
            .as_ref()
            .map(|menu| menu.is_document_view)
            .unwrap_or(false);

        self.context_menu = None;
        self.restore_focus_after_context_menu(is_document_view, window, cx);
        cx.notify();
    }

    /// Wraps the menu's rows in the deferred overlay: a full-size
    /// click-catcher (closes the menu) plus the menu, at `placement` in
    /// window coordinates.
    fn render_context_menu_overlay(
        &self,
        placement: MenuPlacement,
        width: Pixels,
        rows: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        deferred(
            div()
                .id("context-menu-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .track_focus(&self.focus.context_menu_focus)
                // The grid reports the ContextMenu context while the menu is
                // open, so the keymap's menu keys arrive here first.
                .on_action(cx.listener(
                    |this, action: &dbflux_ui_base::keymap::RunCommand, window, cx| {
                        let handled = dbflux_ui_base::keymap::run_command(action)
                            .is_some_and(|command| this.dispatch_menu_command(command, window, cx));

                        if !handled {
                            cx.propagate();
                        }
                    },
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.dismiss_context_menu(window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, window, cx| this.dismiss_context_menu(window, cx)),
                )
                // The overlay covers the grid, so a wheel here would scroll
                // nothing; closing instead keeps the menu from hanging over a
                // grid that has scrolled away from under it.
                .on_scroll_wheel(
                    cx.listener(|this, _, window, cx| this.dismiss_context_menu(window, cx)),
                )
                .child(
                    anchored()
                        .position(placement.origin)
                        .snap_to_window_with_margin(tree::MENU_EDGE_GAP)
                        .child(
                            menu_frame(cx)
                                .id("context-menu")
                                .debug_selector(|| "context-menu".to_string())
                                .w(width)
                                .occlude()
                                // A menu taller than the window scrolls; the
                                // wheel over it must not reach the overlay,
                                // which closes the menu.
                                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                                .child(level_rows(
                                    0,
                                    placement.max_height,
                                    self.focus.context_menu_scroll.first(),
                                    rows,
                                )),
                        ),
                ),
        )
        .with_priority(1)
    }
}
