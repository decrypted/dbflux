//! The grid's menus as data.
//!
//! The cell menu, the column header menu and the document view's menu are
//! each built once per frame as a tree of [`MenuNode`]s, and both the
//! renderer and the menu keys walk that same tree, so the row drawn
//! highlighted and the row Enter runs can never come from two diverging
//! counts.

use super::toolbar::ToolbarAction;
use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_components::icons::AppIcon;
use dbflux_components::tokens::{Borders, MenuMetrics, Spacing};
use gpui::{Pixels, Point, SharedString, Size, px};

/// Gap kept between a menu and the window edge.
pub(in crate::data_grid_panel) const MENU_EDGE_GAP: Pixels = Spacing::XS;

/// How far a submenu overlaps the row it hangs off: the flyout starts at
/// the row's edge, which sits this far inside the menu.
pub(in crate::data_grid_panel) const SUBMENU_OVERLAP: Pixels = MenuMetrics::ROW_INSET;

/// What a menu row runs.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::data_grid_panel) enum MenuTarget {
    Action(ContextMenuAction),
    /// A driver-supplied row action (Kill, Cancel), emitted to the host.
    RowAction(dbflux_core::InspectorRowAction),
    /// A button of the results toolbar or header.
    Toolbar(ToolbarAction),
}

/// Color of a header row's icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::data_grid_panel) enum HeaderTone {
    Muted,
    /// A primary key column.
    Warning,
    /// A foreign key column.
    Info,
}

/// A caption row: never highlighted, never run.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::data_grid_panel) struct MenuHeader {
    pub(in crate::data_grid_panel) label: SharedString,
    pub(in crate::data_grid_panel) icon: Option<AppIcon>,
    pub(in crate::data_grid_panel) tone: HeaderTone,
}

/// A row that runs something.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::data_grid_panel) struct MenuEntry {
    /// Element id of the row, stable across locales.
    pub(in crate::data_grid_panel) id: SharedString,
    pub(in crate::data_grid_panel) label: SharedString,
    pub(in crate::data_grid_panel) icon: Option<AppIcon>,
    /// A muted second line under the label.
    pub(in crate::data_grid_panel) detail: Option<SharedString>,
    pub(in crate::data_grid_panel) danger: bool,
    /// Drawn and reachable by the keys, but not run; `detail` says why.
    pub(in crate::data_grid_panel) disabled: bool,
    pub(in crate::data_grid_panel) target: MenuTarget,
}

impl MenuEntry {
    pub(in crate::data_grid_panel) fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: AppIcon,
        target: MenuTarget,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: Some(icon),
            detail: None,
            danger: false,
            disabled: false,
            target,
        }
    }

    pub(in crate::data_grid_panel) fn action(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: AppIcon,
        action: ContextMenuAction,
    ) -> Self {
        Self::new(id, label, icon, MenuTarget::Action(action))
    }

    pub(in crate::data_grid_panel) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// A row that opens a flyout of further rows.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::data_grid_panel) struct Submenu {
    /// Element id of the trigger row, stable across locales.
    pub(in crate::data_grid_panel) id: &'static str,
    pub(in crate::data_grid_panel) label: SharedString,
    pub(in crate::data_grid_panel) icon: AppIcon,
    pub(in crate::data_grid_panel) width: Pixels,
    pub(in crate::data_grid_panel) children: Vec<MenuNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::data_grid_panel) enum MenuNode {
    Header(MenuHeader),
    Separator,
    Entry(MenuEntry),
    Submenu(Submenu),
}

impl MenuNode {
    pub(in crate::data_grid_panel) fn is_selectable(&self) -> bool {
        matches!(self, MenuNode::Entry(_) | MenuNode::Submenu(_))
    }
}

/// Appends `group` to `nodes`, with a separator before it when both hold
/// rows, so an empty group leaves no doubled or trailing line.
pub(in crate::data_grid_panel) fn push_group(nodes: &mut Vec<MenuNode>, group: Vec<MenuNode>) {
    if group.is_empty() {
        return;
    }
    if nodes.iter().any(MenuNode::is_selectable) {
        nodes.push(MenuNode::Separator);
    }
    nodes.extend(group);
}

/// A submenu node, or nothing when it would open on no rows.
pub(in crate::data_grid_panel) fn submenu(
    id: &'static str,
    label: impl Into<SharedString>,
    icon: AppIcon,
    width: Pixels,
    children: Vec<MenuNode>,
) -> Option<MenuNode> {
    children.iter().any(MenuNode::is_selectable).then(|| {
        MenuNode::Submenu(Submenu {
            id,
            label: label.into(),
            icon,
            width,
            children,
        })
    })
}

pub(in crate::data_grid_panel) fn first_selectable(nodes: &[MenuNode]) -> Option<usize> {
    nodes.iter().position(MenuNode::is_selectable)
}

/// The selectable row one step from `from`, wrapping at both ends and
/// passing over headers and separators. Returns `from` when there is none
/// other.
pub(in crate::data_grid_panel) fn step(nodes: &[MenuNode], from: usize, forward: bool) -> usize {
    let count = nodes.len();
    if count == 0 {
        return 0;
    }

    let mut index = from.min(count - 1);
    for _ in 0..count {
        index = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        if nodes.get(index).is_some_and(MenuNode::is_selectable) {
            return index;
        }
    }

    from
}

/// The rows of the flyout `open` leads to: `open` holds the index of each
/// open submenu, from the menu inwards. `None` when an index does not name
/// a submenu.
pub(in crate::data_grid_panel) fn level<'a>(
    root: &'a [MenuNode],
    open: &[usize],
) -> Option<&'a [MenuNode]> {
    let mut nodes = root;
    for &index in open {
        match nodes.get(index)? {
            MenuNode::Submenu(submenu) => nodes = &submenu.children,
            _ => return None,
        }
    }
    Some(nodes)
}

/// `path` with every index on a selectable row: a stale index (the menu
/// changed under it) falls back to the level's first row, and a level
/// past a row that is no longer a submenu is dropped. Never empty while
/// the menu has a row.
pub(in crate::data_grid_panel) fn normalize(root: &[MenuNode], path: &[usize]) -> Vec<usize> {
    let mut normalized = Vec::with_capacity(path.len().max(1));
    let mut nodes = root;

    for (depth, &index) in path.iter().enumerate() {
        let index = if nodes.get(index).is_some_and(MenuNode::is_selectable) {
            index
        } else {
            match first_selectable(nodes) {
                Some(first) => first,
                None => break,
            }
        };
        normalized.push(index);

        if depth + 1 < path.len() {
            match nodes.get(index) {
                Some(MenuNode::Submenu(submenu))
                    if first_selectable(&submenu.children).is_some() =>
                {
                    nodes = &submenu.children;
                }
                _ => break,
            }
        }
    }

    if normalized.is_empty()
        && let Some(first) = first_selectable(root)
    {
        normalized.push(first);
    }

    normalized
}

/// The node `path` points at.
pub(in crate::data_grid_panel) fn node_at<'a>(
    root: &'a [MenuNode],
    path: &[usize],
) -> Option<&'a MenuNode> {
    let (last, open) = path.split_last()?;
    level(root, open)?.get(*last)
}

/// Every row target in `nodes` and their submenus, depth first.
#[cfg(test)]
pub(in crate::data_grid_panel) fn targets(nodes: &[MenuNode]) -> Vec<&MenuTarget> {
    let mut found = Vec::new();
    for node in nodes {
        match node {
            MenuNode::Entry(entry) => found.push(&entry.target),
            MenuNode::Submenu(submenu) => found.extend(targets(&submenu.children)),
            MenuNode::Header(_) | MenuNode::Separator => {}
        }
    }
    found
}

/// Heights of the row kinds, in pixels at the current UI font size.
#[derive(Debug, Clone, Copy)]
pub(in crate::data_grid_panel) struct RowHeights {
    pub(in crate::data_grid_panel) row: Pixels,
    pub(in crate::data_grid_panel) detail_row: Pixels,
    pub(in crate::data_grid_panel) header: Pixels,
}

impl RowHeights {
    pub(in crate::data_grid_panel) fn from_app(cx: &gpui::App) -> Self {
        use dbflux_components::fonts::ui_px;

        Self {
            row: ui_px(cx, MenuMetrics::ROW_HEIGHT),
            detail_row: ui_px(cx, MenuMetrics::DETAIL_ROW_HEIGHT),
            // A caption sets no line height of its own, so it takes GPUI's
            // default of the golden ratio times its font size.
            header: ui_px(cx, MenuMetrics::HEADER_FONT) * 1.618
                + MenuMetrics::HEADER_PADDING_TOP
                + MenuMetrics::HEADER_PADDING_BOTTOM,
        }
    }
}

/// Height of a menu frame holding `nodes`, padding included.
pub(in crate::data_grid_panel) fn menu_height(nodes: &[MenuNode], heights: RowHeights) -> Pixels {
    let separator = Borders::THIN + MenuMetrics::SEPARATOR_MARGIN_Y * 2.0;
    let rows: Pixels = nodes
        .iter()
        .map(|node| match node {
            MenuNode::Separator => separator,
            MenuNode::Header(_) => heights.header,
            MenuNode::Entry(entry) if entry.detail.is_some() => heights.detail_row,
            MenuNode::Entry(_) | MenuNode::Submenu(_) => heights.row,
        })
        .fold(px(0.0), |total, height| total + height);

    rows + MenuMetrics::PADDING_Y * 2.0
}

/// Where a menu goes, in window coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::data_grid_panel) struct MenuPlacement {
    pub(in crate::data_grid_panel) origin: Point<Pixels>,
    /// The tallest the menu may be; a taller one scrolls.
    pub(in crate::data_grid_panel) max_height: Pixels,
}

/// Keep a menu `size` big, anchored at `anchor`, inside the window.
///
/// The menu opens down and to the right of the anchor. Without the room
/// below it opens upwards from the anchor, as native menus do, and when
/// neither side has the room it is shifted up just enough to end at the
/// bottom edge. Past the right edge it is shifted left. A menu taller than
/// the window is capped at the window height (it scrolls) and pinned to the
/// top. A window not measured yet (zero size) leaves the anchor alone.
pub(in crate::data_grid_panel) fn place_menu(
    anchor: Point<Pixels>,
    size: Size<Pixels>,
    viewport: Size<Pixels>,
) -> MenuPlacement {
    if viewport.width <= px(0.0) || viewport.height <= px(0.0) {
        return MenuPlacement {
            origin: anchor,
            max_height: size.height,
        };
    }

    let max_height = (viewport.height - MENU_EDGE_GAP * 2.0).max(px(0.0));
    let height = size.height.min(max_height);

    let x = anchor
        .x
        .min(viewport.width - size.width - MENU_EDGE_GAP)
        .max(MENU_EDGE_GAP);
    let y = if anchor.y + height + MENU_EDGE_GAP <= viewport.height {
        anchor.y.max(MENU_EDGE_GAP)
    } else if anchor.y - height >= MENU_EDGE_GAP {
        anchor.y - height
    } else {
        (viewport.height - height - MENU_EDGE_GAP).max(MENU_EDGE_GAP)
    };

    MenuPlacement {
        origin: Point { x, y },
        max_height,
    }
}

/// Which side a flyout `child_width` wide opens on, hanging off a menu
/// whose left edge is at `left` and which is `width` wide, and the flyout's
/// left edge there.
///
/// It opens to the right unless it would run past the window's right edge
/// and fits on the left. A window too narrow for either keeps it on the
/// right, where at least its start is visible.
pub(in crate::data_grid_panel) fn submenu_side(
    left: Pixels,
    width: Pixels,
    child_width: Pixels,
    viewport_width: Pixels,
) -> (bool, Pixels) {
    let right_left = left + width - SUBMENU_OVERLAP;
    let left_left = left + SUBMENU_OVERLAP - child_width;

    let fits_right = right_left + child_width + MENU_EDGE_GAP <= viewport_width;
    let fits_left = left_left >= MENU_EDGE_GAP;

    if viewport_width > px(0.0) && !fits_right && fits_left {
        (true, left_left)
    } else {
        (false, right_left)
    }
}
