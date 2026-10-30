//! The cell menu and the document view's menu, built from what the grid
//! offers for the cell under it.
//!
//! The builders take everything the panel resolved (which rows apply,
//! their labels and details) and only arrange it, so the layout can be
//! checked without a grid.

use super::columns::ColumnMenuItem;
use super::tree::{
    MenuEntry, MenuHeader, MenuNode, MenuTarget, first_selectable, push_group, submenu,
};
use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_components::icons::AppIcon;
use dbflux_core::SortDirection;
use gpui::{Pixels, SharedString, px};

/// Widths of the cell menu's flyouts, sized to their longest labels.
const COPY_AS_WIDTH: Pixels = px(230.0);
const SET_VALUE_WIDTH: Pixels = px(190.0);
const FILTER_WIDTH: Pixels = px(280.0);
const SORT_WIDTH: Pixels = px(240.0);
const COLUMN_WIDTH: Pixels = px(200.0);
const ROW_WIDTH: Pixels = px(220.0);
const MORE_WIDTH: Pixels = px(230.0);
const GENERATE_SQL_WIDTH: Pixels = px(180.0);
const TOOLBAR_WIDTH: Pixels = px(240.0);
const COPY_QUERY_WIDTH: Pixels = px(160.0);

/// What the cell menu offers for the cell it opened on. Each optional part
/// is left out of the menu when it is empty or `false`.
#[derive(Default)]
pub(in crate::data_grid_panel) struct CellMenuParts {
    pub(in crate::data_grid_panel) is_editable: bool,
    /// The menu is over a row of the result, not an empty grid.
    pub(in crate::data_grid_panel) has_row_target: bool,
    /// The `column · row N` caption.
    pub(in crate::data_grid_panel) header: Option<MenuHeader>,
    /// The table's WHERE filter takes `column = value` from the cell.
    pub(in crate::data_grid_panel) add_to_filter: bool,
    /// Copy as ▸, its text formats in order (CSV, JSON, the extractors).
    pub(in crate::data_grid_panel) copy_as: Vec<MenuEntry>,
    /// Copy as ▸ also copies the row as a statement; the caption names the
    /// connection's language ("Copy as SQL").
    pub(in crate::data_grid_panel) copy_query: Option<SharedString>,
    /// The cell can hold a SQL expression.
    pub(in crate::data_grid_panel) accepts_expression: bool,
    /// The grid stages document patches: the column default becomes Unset
    /// field, and a staged edit can be reverted.
    pub(in crate::data_grid_panel) document_patches: bool,
    /// The identifiers the cell can be given, for Set value ▸.
    pub(in crate::data_grid_panel) generated_ids: Vec<MenuEntry>,
    /// Filter ▸: the operators for the cell's value and Clear filter.
    pub(in crate::data_grid_panel) filter: Vec<MenuNode>,
    /// Sort ▸ for the named column.
    pub(in crate::data_grid_panel) sort_column: Option<String>,
    /// Column ▸, in the header menu's order.
    pub(in crate::data_grid_panel) columns: Vec<ColumnMenuItem>,
    pub(in crate::data_grid_panel) inspect_row: bool,
    /// Driver-supplied row actions (Kill, Cancel).
    pub(in crate::data_grid_panel) row_actions: Vec<dbflux_core::InspectorRowAction>,
    pub(in crate::data_grid_panel) generate_sql: bool,
    pub(in crate::data_grid_panel) can_chart: bool,
    pub(in crate::data_grid_panel) find_similar: Option<MenuEntry>,
    /// Toolbar ▸: the buttons the grid shows right now.
    pub(in crate::data_grid_panel) toolbar: Vec<MenuEntry>,
}

/// What the document view's menu offers for the document it opened on.
#[derive(Default)]
pub(in crate::data_grid_panel) struct DocumentMenuParts {
    pub(in crate::data_grid_panel) is_editable: bool,
    pub(in crate::data_grid_panel) has_row_target: bool,
    pub(in crate::data_grid_panel) filter: Vec<MenuNode>,
    pub(in crate::data_grid_panel) copy_query: Option<SharedString>,
    pub(in crate::data_grid_panel) toolbar: Vec<MenuEntry>,
}

fn t(key: &str) -> SharedString {
    dbflux_i18n::t!(key).into()
}

fn entry(id: &'static str, key: &str, icon: AppIcon, action: ContextMenuAction) -> MenuNode {
    MenuNode::Entry(MenuEntry::action(id, t(key), icon, action))
}

/// The cell menu: the cell's own actions on top, then editing, then the
/// query, column and row submenus, then the rest.
pub(in crate::data_grid_panel) fn build_cell_menu(parts: CellMenuParts) -> Vec<MenuNode> {
    let mut nodes = Vec::new();
    if let Some(header) = parts.header.clone() {
        nodes.push(MenuNode::Header(header));
    }

    push_group(&mut nodes, read_group(&parts));
    push_group(&mut nodes, edit_group(&parts));

    let mut query = Vec::new();
    query.extend(submenu(
        "filter",
        t("document.data.context_menu.filter.title"),
        AppIcon::ListFilter,
        FILTER_WIDTH,
        parts.filter.clone(),
    ));
    if let Some(column) = &parts.sort_column {
        query.extend(submenu(
            "sort",
            t("document.data.context_menu.sort.title"),
            AppIcon::ArrowUpDown,
            SORT_WIDTH,
            sort_nodes(column),
        ));
    }
    query.extend(submenu(
        "column",
        t("document.data.context_menu.column.title"),
        AppIcon::Columns,
        COLUMN_WIDTH,
        cell_column_nodes(&parts.columns),
    ));
    query.extend(submenu(
        "row",
        t("document.data.context_menu.row.title"),
        AppIcon::Rows3,
        ROW_WIDTH,
        row_nodes(&parts),
    ));
    push_group(&mut nodes, query);

    let mut rest = Vec::new();
    rest.extend(submenu(
        "more",
        t("document.data.context_menu.more.title"),
        AppIcon::Layers,
        MORE_WIDTH,
        more_nodes(&parts),
    ));
    rest.extend(toolbar_submenu(parts.toolbar));
    push_group(&mut nodes, rest);

    nodes
}

/// Copy, Copy as ▸, View value and Add to filter: what reads the cell.
fn read_group(parts: &CellMenuParts) -> Vec<MenuNode> {
    let mut group = vec![entry(
        "copy",
        "document.data.context_menu.item.copy",
        AppIcon::Copy,
        ContextMenuAction::Copy,
    )];

    let mut copy_as: Vec<MenuNode> = parts.copy_as.iter().cloned().map(MenuNode::Entry).collect();
    if let Some(caption) = &parts.copy_query {
        push_group(
            &mut copy_as,
            std::iter::once(MenuNode::Header(MenuHeader {
                label: caption.clone(),
                icon: None,
                tone: super::tree::HeaderTone::Muted,
            }))
            .chain(copy_query_nodes())
            .collect(),
        );
    }
    group.extend(submenu(
        "copy-as",
        t("document.data.context_menu.copy_as.title"),
        AppIcon::Clipboard,
        COPY_AS_WIDTH,
        copy_as,
    ));

    // Offered for read-only results too: the panel is how a long JSON or
    // XML value gets read at all. It still needs a row under the cursor.
    if parts.has_row_target {
        group.push(entry(
            "view-value",
            "document.data.context_menu.item.view_value",
            AppIcon::Eye,
            ContextMenuAction::ViewValue,
        ));
    }
    if parts.add_to_filter && parts.has_row_target {
        group.push(entry(
            "add-to-filter",
            "document.data.context_menu.item.add_to_filter",
            AppIcon::Crosshair,
            ContextMenuAction::AddToFilter,
        ));
    }

    group
}

/// Edit, Edit in editor, Edit as SQL expression, Paste and Set value ▸.
fn edit_group(parts: &CellMenuParts) -> Vec<MenuNode> {
    if !parts.is_editable || !parts.has_row_target {
        return Vec::new();
    }

    let mut group = vec![
        entry(
            "edit",
            "document.data.context_menu.item.edit",
            AppIcon::Pencil,
            ContextMenuAction::Edit,
        ),
        entry(
            "edit-in-modal",
            "document.data.context_menu.item.edit_in_modal",
            AppIcon::Maximize2,
            ContextMenuAction::EditInModal,
        ),
    ];
    if parts.accepts_expression {
        group.push(entry(
            "edit-as-expression",
            "document.data.context_menu.item.edit_as_expression",
            AppIcon::Code,
            ContextMenuAction::EditAsExpression,
        ));
    }
    group.push(entry(
        "paste",
        "document.data.context_menu.item.paste",
        AppIcon::Clipboard,
        ContextMenuAction::Paste,
    ));

    let mut set_value = vec![entry(
        "set-null",
        "document.data.context_menu.set_value.to_null",
        AppIcon::CircleX,
        ContextMenuAction::SetNull,
    )];
    if parts.document_patches {
        set_value.push(entry(
            "unset-field",
            "document.collection.menu.unset_field",
            AppIcon::CircleX,
            ContextMenuAction::UnsetField,
        ));
        set_value.push(entry(
            "revert-cell",
            "document.collection.menu.revert_change",
            AppIcon::RotateCcw,
            ContextMenuAction::RevertCell,
        ));
    } else {
        set_value.push(entry(
            "set-default",
            "document.data.context_menu.set_value.to_default",
            AppIcon::RotateCcw,
            ContextMenuAction::SetDefault,
        ));
    }
    push_group(
        &mut set_value,
        parts
            .generated_ids
            .iter()
            .cloned()
            .map(MenuNode::Entry)
            .collect(),
    );
    group.extend(submenu(
        "set-value",
        t("document.data.context_menu.set_value.title"),
        AppIcon::Hash,
        SET_VALUE_WIDTH,
        set_value,
    ));

    group
}

/// Sort ▸ of the cell menu: the column alone in either direction, as an
/// added key in either direction, and Remove ordering.
fn sort_nodes(column: &str) -> Vec<MenuNode> {
    let order = |id: &'static str, label: String, icon, action| {
        MenuNode::Entry(MenuEntry::action(id, label, icon, action))
    };
    let add_to_sort = |direction: &str| {
        dbflux_i18n::t!(
            "document.data.context_menu.order.add_to_sort",
            column = format!("{column} {direction}")
        )
    };

    vec![
        order(
            "sort-asc",
            dbflux_i18n::t!("document.data.context_menu.sort.ascending"),
            AppIcon::ArrowUp,
            ContextMenuAction::Order(SortDirection::Ascending),
        ),
        order(
            "sort-desc",
            dbflux_i18n::t!("document.data.context_menu.sort.descending"),
            AppIcon::ArrowDown,
            ContextMenuAction::Order(SortDirection::Descending),
        ),
        MenuNode::Separator,
        order(
            "sort-add-asc",
            add_to_sort("ASC"),
            AppIcon::ArrowUp,
            ContextMenuAction::AddToSort(SortDirection::Ascending),
        ),
        order(
            "sort-add-desc",
            add_to_sort("DESC"),
            AppIcon::ArrowDown,
            ContextMenuAction::AddToSort(SortDirection::Descending),
        ),
        MenuNode::Separator,
        MenuNode::Entry(
            MenuEntry::action(
                "sort-remove",
                t("document.data.context_menu.order.remove"),
                AppIcon::X,
                ContextMenuAction::RemoveOrdering,
            )
            .danger(),
        ),
    ]
}

/// Column ▸ of the cell menu: the entries that move or hide the column
/// first, then its width and the chooser.
fn cell_column_nodes(columns: &[ColumnMenuItem]) -> Vec<MenuNode> {
    let rank = |action: ContextMenuAction| match action {
        ContextMenuAction::HideColumn => 0,
        ContextMenuAction::MoveColumnLeft => 1,
        ContextMenuAction::MoveColumnRight => 2,
        ContextMenuAction::PinColumn | ContextMenuAction::UnpinColumn => 3,
        ContextMenuAction::FitColumnWidth => 4,
        _ => 5,
    };
    let mut ordered: Vec<&ColumnMenuItem> = columns.iter().collect();
    ordered.sort_by_key(|item| rank(item.action));

    ordered
        .into_iter()
        .map(|item| {
            MenuNode::Entry(MenuEntry::action(
                column_entry_id(item.action),
                item.label.clone(),
                item.icon,
                item.action,
            ))
        })
        .collect()
}

pub(in crate::data_grid_panel) fn column_entry_id(action: ContextMenuAction) -> &'static str {
    match action {
        ContextMenuAction::HideColumn => "column-hide",
        ContextMenuAction::MoveColumnLeft => "column-move-left",
        ContextMenuAction::MoveColumnRight => "column-move-right",
        ContextMenuAction::PinColumn => "column-pin",
        ContextMenuAction::UnpinColumn => "column-unpin",
        ContextMenuAction::FitColumnWidth => "column-fit-width",
        _ => "column-choose",
    }
}

/// Row ▸: inspecting and adding rows, the driver's row actions, and
/// Delete row on its own at the end.
fn row_nodes(parts: &CellMenuParts) -> Vec<MenuNode> {
    let mut rows = Vec::new();
    if parts.is_editable && parts.has_row_target && parts.inspect_row {
        rows.push(entry(
            "inspect-row",
            "document.data.context_menu.item.inspect_row",
            AppIcon::Info,
            ContextMenuAction::InspectRow,
        ));
    }
    if parts.is_editable {
        rows.push(entry(
            "add-row",
            "document.data.context_menu.item.add_row",
            AppIcon::Plus,
            ContextMenuAction::AddRow,
        ));
    }
    if parts.is_editable && parts.has_row_target {
        rows.push(entry(
            "duplicate-row",
            "document.data.context_menu.item.duplicate_row",
            AppIcon::Copy,
            ContextMenuAction::DuplicateRow,
        ));
    }

    push_group(&mut rows, row_action_nodes(&parts.row_actions));

    if parts.is_editable && parts.has_row_target {
        push_group(
            &mut rows,
            vec![MenuNode::Entry(
                MenuEntry::action(
                    "delete-row",
                    t("document.data.context_menu.item.delete_row"),
                    AppIcon::Delete,
                    ContextMenuAction::DeleteRow,
                )
                .danger(),
            )],
        );
    }

    rows
}

fn row_action_nodes(actions: &[dbflux_core::InspectorRowAction]) -> Vec<MenuNode> {
    actions
        .iter()
        .enumerate()
        .map(|(slot, action)| {
            let icon = if action.is_destructive {
                AppIcon::Power
            } else {
                AppIcon::Zap
            };
            let mut row = MenuEntry::new(
                format!("row-action-{slot}"),
                action.label.clone(),
                icon,
                MenuTarget::RowAction(action.clone()),
            );
            row.danger = action.is_destructive;
            MenuNode::Entry(row)
        })
        .collect()
}

/// More ▸: Generate SQL ▸, Chart this query and Find similar rows.
fn more_nodes(parts: &CellMenuParts) -> Vec<MenuNode> {
    let mut more = Vec::new();
    if parts.generate_sql {
        more.extend(submenu(
            "generate-sql",
            t("document.data.context_menu.generate_sql.title"),
            AppIcon::Code,
            GENERATE_SQL_WIDTH,
            generate_sql_nodes(),
        ));
    }
    if parts.can_chart {
        more.push(entry(
            "chart-this-query",
            "document.data.context_menu.item.chart_this_query",
            AppIcon::ChartSpline,
            ContextMenuAction::ChartThisQuery,
        ));
    }
    if let Some(find_similar) = &parts.find_similar {
        more.push(MenuNode::Entry(find_similar.clone()));
    }
    more
}

fn generate_sql_nodes() -> Vec<MenuNode> {
    [
        (
            "generate-select-where",
            "SELECT WHERE",
            ContextMenuAction::GenerateSelectWhere,
        ),
        (
            "generate-insert",
            "INSERT",
            ContextMenuAction::GenerateInsert,
        ),
        (
            "generate-update",
            "UPDATE",
            ContextMenuAction::GenerateUpdate,
        ),
        (
            "generate-delete",
            "DELETE",
            ContextMenuAction::GenerateDelete,
        ),
    ]
    .into_iter()
    .map(|(id, label, action)| MenuNode::Entry(MenuEntry::action(id, label, AppIcon::Code, action)))
    .collect()
}

fn copy_query_nodes() -> Vec<MenuNode> {
    [
        (
            "copy-query-insert",
            "INSERT",
            ContextMenuAction::CopyAsInsert,
        ),
        (
            "copy-query-update",
            "UPDATE",
            ContextMenuAction::CopyAsUpdate,
        ),
        (
            "copy-query-delete",
            "DELETE",
            ContextMenuAction::CopyAsDelete,
        ),
    ]
    .into_iter()
    .map(|(id, label, action)| {
        MenuNode::Entry(MenuEntry::action(id, label, AppIcon::Columns, action))
    })
    .collect()
}

fn toolbar_submenu(toolbar: Vec<MenuEntry>) -> Option<MenuNode> {
    submenu(
        "toolbar",
        t("document.data.context_menu.toolbar.title"),
        AppIcon::Settings,
        TOOLBAR_WIDTH,
        toolbar.into_iter().map(MenuNode::Entry).collect(),
    )
}

/// The document view's menu: the document's own actions, then adding and
/// removing documents, then Filter ▸ and Copy as query ▸, then Toolbar ▸.
pub(in crate::data_grid_panel) fn build_document_menu(parts: DocumentMenuParts) -> Vec<MenuNode> {
    let mut nodes = Vec::new();

    if parts.has_row_target {
        push_group(
            &mut nodes,
            vec![
                entry(
                    "copy",
                    "document.data.context_menu.item.copy",
                    AppIcon::Copy,
                    ContextMenuAction::Copy,
                ),
                entry(
                    "view-document",
                    "document.data.context_menu.item.view_document",
                    AppIcon::Maximize2,
                    ContextMenuAction::EditInModal,
                ),
            ],
        );
    }

    if parts.is_editable {
        let mut documents = vec![entry(
            "add-document",
            "document.data.context_menu.item.add_document",
            AppIcon::Plus,
            ContextMenuAction::AddRow,
        )];
        if parts.has_row_target {
            documents.push(entry(
                "duplicate-document",
                "document.data.context_menu.item.duplicate_document",
                AppIcon::Copy,
                ContextMenuAction::DuplicateRow,
            ));
            documents.push(MenuNode::Entry(
                MenuEntry::action(
                    "delete-document",
                    t("document.data.context_menu.item.delete_document"),
                    AppIcon::Delete,
                    ContextMenuAction::DeleteRow,
                )
                .danger(),
            ));
        }
        push_group(&mut nodes, documents);
    }

    let mut query = Vec::new();
    query.extend(submenu(
        "filter",
        t("document.data.context_menu.filter.title"),
        AppIcon::ListFilter,
        FILTER_WIDTH,
        parts.filter,
    ));
    if let Some(label) = parts.copy_query {
        query.extend(submenu(
            "copy-query",
            label,
            AppIcon::Table,
            COPY_QUERY_WIDTH,
            copy_query_nodes(),
        ));
    }
    push_group(&mut nodes, query);
    push_group(
        &mut nodes,
        toolbar_submenu(parts.toolbar).into_iter().collect(),
    );

    nodes
}

/// The filter operators of a cell as rows: the value operators (under
/// `caption` when given), then IS NULL / IS NOT NULL, then Clear filter.
pub(in crate::data_grid_panel) fn filter_nodes(
    items: Vec<(String, ContextMenuAction)>,
    value_ops_count: usize,
    caption: Option<SharedString>,
) -> Vec<MenuNode> {
    let mut nodes = Vec::new();
    if value_ops_count > 0
        && let Some(caption) = caption
    {
        nodes.push(MenuNode::Header(MenuHeader {
            label: caption,
            icon: None,
            tone: super::tree::HeaderTone::Muted,
        }));
    }

    for (index, (label, action)) in items.into_iter().enumerate() {
        let is_remove = matches!(action, ContextMenuAction::RemoveFilter);
        if (value_ops_count > 0 && index == value_ops_count) || is_remove {
            nodes.push(MenuNode::Separator);
        }
        let row = if is_remove {
            MenuEntry::action(format!("filter-{index}"), label, AppIcon::X, action).danger()
        } else {
            MenuEntry::action(
                format!("filter-{index}"),
                label,
                AppIcon::ListFilter,
                action,
            )
        };
        nodes.push(MenuNode::Entry(row));
    }

    if first_selectable(&nodes).is_none() {
        nodes.clear();
    }
    nodes
}
