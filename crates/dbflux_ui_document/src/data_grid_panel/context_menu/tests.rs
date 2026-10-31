use super::columns::ColumnMenuItem;
use super::items::{
    CellMenuParts, DocumentMenuParts, build_cell_menu, build_document_menu, filter_nodes,
};
use super::tree::{
    self, MENU_EDGE_GAP, MenuEntry, MenuNode, MenuTarget, SUBMENU_OVERLAP, Submenu, place_menu,
    submenu_side,
};
use super::{ExportMenuEntry, like_contains_filter, step_export_selection};
use dbflux_components::components::data_table::{ContextMenuAction, FilterOperator};
use dbflux_components::icons::AppIcon;
use dbflux_core::{GeneratedIdKind, SortDirection};
use dbflux_export::ExportFormat;
use gpui::{Point, Size, px, size};

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

/// The export menu draws a caption before each section and a separator
/// between sections, so the row the keys scroll to sits past them.
#[test]
fn export_rows_are_found_past_the_section_captions() {
    let entries = [
        ExportMenuEntry::Save(ExportFormat::Csv),
        ExportMenuEntry::Save(ExportFormat::Hex),
        ExportMenuEntry::Copy(ExportFormat::Csv),
    ];

    let index = crate::data_grid_panel::render::export_menu_child_index;
    // caption, Save CSV, Save Hex, separator, caption, Copy CSV
    assert_eq!(index(&entries, 0), 1);
    assert_eq!(index(&entries, 1), 2);
    assert_eq!(index(&entries, 2), 5);
}

fn window() -> Size<gpui::Pixels> {
    size(px(1000.0), px(600.0))
}

#[test]
fn a_menu_that_fits_opens_at_the_click() {
    let click = Point::new(px(100.0), px(100.0));
    let placed = place_menu(click, size(px(270.0), px(300.0)), window());

    assert_eq!(placed.origin, click);
    assert_eq!(placed.max_height, px(600.0) - MENU_EDGE_GAP * 2.0);
}

/// Without the room below the click the menu opens upwards from it, so its
/// bottom edge is at the click.
#[test]
fn a_menu_near_the_bottom_opens_upwards() {
    let click = Point::new(px(100.0), px(500.0));
    let placed = place_menu(click, size(px(270.0), px(300.0)), window());

    assert_eq!(placed.origin.x, click.x);
    assert_eq!(placed.origin.y, px(200.0), "it ends at the click");
}

/// With room on neither side of the click the menu is shifted up just
/// enough to end at the bottom edge.
#[test]
fn a_menu_without_room_above_or_below_ends_at_the_bottom_edge() {
    let click = Point::new(px(100.0), px(300.0));
    let placed = place_menu(click, size(px(270.0), px(400.0)), window());

    assert_eq!(placed.origin.y, px(600.0) - px(400.0) - MENU_EDGE_GAP);
}

#[test]
fn a_menu_near_the_right_edge_is_shifted_left() {
    let click = Point::new(px(950.0), px(100.0));
    let placed = place_menu(click, size(px(270.0), px(300.0)), window());

    assert_eq!(placed.origin.x, px(1000.0) - px(270.0) - MENU_EDGE_GAP);
    assert_eq!(placed.origin.y, click.y);
}

/// A menu taller than the window is capped at the window's height, which
/// makes it scroll, and starts at the top.
#[test]
fn a_menu_taller_than_the_window_is_capped_and_pinned_to_the_top() {
    let click = Point::new(px(100.0), px(300.0));
    let placed = place_menu(click, size(px(270.0), px(900.0)), window());

    assert_eq!(placed.origin.y, MENU_EDGE_GAP);
    assert_eq!(placed.max_height, px(600.0) - MENU_EDGE_GAP * 2.0);
}

#[test]
fn an_unmeasured_window_leaves_the_click_alone() {
    let click = Point::new(px(950.0), px(500.0));
    let placed = place_menu(click, size(px(270.0), px(300.0)), Size::default());

    assert_eq!(placed.origin, click);
}

#[test]
fn a_submenu_opens_right_where_it_fits_and_left_near_the_right_edge() {
    let (left, at) = submenu_side(px(100.0), px(270.0), px(240.0), px(1000.0));
    assert!(!left);
    assert_eq!(at, px(370.0) - SUBMENU_OVERLAP);

    let (left, at) = submenu_side(px(700.0), px(270.0), px(240.0), px(1000.0));
    assert!(left, "700 + 270 + 240 runs past 1000");
    assert_eq!(at, px(700.0) + SUBMENU_OVERLAP - px(240.0));
}

/// A window too narrow for a submenu on either side keeps it on the right:
/// pushed off the left edge it would be invisible, hanging past the right
/// edge at least its start shows.
#[test]
fn a_narrow_window_keeps_submenus_on_the_right() {
    let (left, _) = submenu_side(px(20.0), px(270.0), px(240.0), px(400.0));
    assert!(!left);
}

fn action_entry(id: &'static str, action: ContextMenuAction) -> MenuEntry {
    MenuEntry::action(id, id, AppIcon::Copy, action)
}

#[test]
fn the_keys_skip_headers_and_separators_and_wrap() {
    let nodes = vec![
        MenuNode::Header(tree::MenuHeader {
            label: "h".into(),
            icon: None,
            tone: tree::HeaderTone::Muted,
        }),
        MenuNode::Entry(action_entry("a", ContextMenuAction::Copy)),
        MenuNode::Separator,
        MenuNode::Entry(action_entry("b", ContextMenuAction::Paste)),
    ];

    assert_eq!(tree::first_selectable(&nodes), Some(1));
    assert_eq!(tree::step(&nodes, 1, true), 3);
    assert_eq!(tree::step(&nodes, 3, true), 1, "wraps past the header");
    assert_eq!(tree::step(&nodes, 1, false), 3);
    assert_eq!(
        tree::normalize(&nodes, &[0]),
        vec![1],
        "a header is not a row"
    );
    assert_eq!(tree::normalize(&nodes, &[9]), vec![1], "a stale index");
}

/// What the grid offers on a cell of an editable table with every feature
/// on, as the panel resolves it.
fn full_parts() -> CellMenuParts {
    let operators = [
        FilterOperator::Eq,
        FilterOperator::NotEq,
        FilterOperator::Gt,
        FilterOperator::Gte,
        FilterOperator::Lt,
        FilterOperator::Lte,
        FilterOperator::Like,
    ];
    let mut filter_items: Vec<(String, ContextMenuAction)> = operators
        .iter()
        .map(|op| (format!("{op:?}"), ContextMenuAction::FilterByValue(*op)))
        .collect();
    filter_items.push(("null".into(), ContextMenuAction::FilterIsNull));
    filter_items.push(("not null".into(), ContextMenuAction::FilterIsNotNull));
    filter_items.push(("clear".into(), ContextMenuAction::RemoveFilter));

    let column = |action| ColumnMenuItem {
        label: format!("{action:?}"),
        action,
        icon: AppIcon::Columns,
    };

    CellMenuParts {
        is_editable: true,
        has_row_target: true,
        header: None,
        add_to_filter: true,
        copy_as: vec![
            action_entry("copy-as-csv", ContextMenuAction::CopyAsCsv),
            action_entry("copy-as-json", ContextMenuAction::CopyAsJson),
            action_entry("copy-as-in-list", ContextMenuAction::CopyAsSqlInList),
            action_entry("copy-as-inserts", ContextMenuAction::CopyAsInsertStatements),
            action_entry("copy-as-markdown", ContextMenuAction::CopyAsMarkdown),
        ],
        copy_query: Some("Copy as SQL".into()),
        accepts_expression: true,
        document_patches: false,
        generated_ids: GeneratedIdKind::ALL
            .iter()
            .map(|kind| action_entry("id", ContextMenuAction::GenerateId(*kind)))
            .collect(),
        filter: filter_nodes(filter_items, operators.len(), Some("Cell value".into())),
        sort_column: Some("id".into()),
        columns: vec![
            column(ContextMenuAction::FitColumnWidth),
            column(ContextMenuAction::PinColumn),
            column(ContextMenuAction::UnpinColumn),
            column(ContextMenuAction::HideColumn),
            column(ContextMenuAction::ChooseColumns),
            column(ContextMenuAction::MoveColumnLeft),
            column(ContextMenuAction::MoveColumnRight),
        ],
        inspect_row: true,
        row_actions: vec![dbflux_core::InspectorRowAction {
            id: "kill".into(),
            label: "Kill".into(),
            description: None,
            is_destructive: true,
        }],
        generate_sql: true,
        can_chart: true,
        find_similar: Some(action_entry(
            "find-similar-rows",
            ContextMenuAction::FindSimilarRows,
        )),
        toolbar: vec![MenuEntry::new(
            "toolbar-action-export",
            "Export",
            AppIcon::Download,
            MenuTarget::Toolbar(super::toolbar::ToolbarAction::Export),
        )],
    }
}

/// Every variant of the menu's actions. The match makes this fail to build
/// when a variant is added, so a new action is listed here and checked for a
/// place in the menus.
fn every_action() -> Vec<ContextMenuAction> {
    use ContextMenuAction as A;

    let mut actions = vec![
        A::Copy,
        A::CopyAsCsv,
        A::CopyAsJson,
        A::CopyAsSqlInList,
        A::CopyAsInsertStatements,
        A::CopyAsMarkdown,
        A::Paste,
        A::Edit,
        A::EditAsExpression,
        A::EditInModal,
        A::ViewValue,
        A::AddToFilter,
        A::SetDefault,
        A::SetNull,
        A::AddRow,
        A::DuplicateRow,
        A::DeleteRow,
        A::GenerateSelectWhere,
        A::GenerateInsert,
        A::GenerateUpdate,
        A::GenerateDelete,
        A::CopyAsInsert,
        A::CopyAsUpdate,
        A::CopyAsDelete,
        A::FilterIsNull,
        A::FilterIsNotNull,
        A::RemoveFilter,
        A::Order(SortDirection::Ascending),
        A::Order(SortDirection::Descending),
        A::AddToSort(SortDirection::Ascending),
        A::AddToSort(SortDirection::Descending),
        A::RemoveOrdering,
        A::InspectRow,
        A::ChartThisQuery,
        A::UnsetField,
        A::RevertCell,
        A::FindSimilarRows,
        A::FitColumnWidth,
        A::HideColumn,
        A::ChooseColumns,
        A::MoveColumnLeft,
        A::MoveColumnRight,
        A::PinColumn,
        A::UnpinColumn,
    ];
    actions.extend(GeneratedIdKind::ALL.iter().map(|kind| A::GenerateId(*kind)));
    actions.extend(
        [
            FilterOperator::Eq,
            FilterOperator::NotEq,
            FilterOperator::Gt,
            FilterOperator::Gte,
            FilterOperator::Lt,
            FilterOperator::Lte,
            FilterOperator::Like,
        ]
        .map(A::FilterByValue),
    );

    for action in &actions {
        match action {
            A::Copy
            | A::CopyAsCsv
            | A::CopyAsJson
            | A::CopyAsSqlInList
            | A::CopyAsInsertStatements
            | A::CopyAsMarkdown
            | A::Paste
            | A::Edit
            | A::EditAsExpression
            | A::GenerateId(_)
            | A::EditInModal
            | A::ViewValue
            | A::AddToFilter
            | A::SetDefault
            | A::SetNull
            | A::AddRow
            | A::DuplicateRow
            | A::DeleteRow
            | A::GenerateSelectWhere
            | A::GenerateInsert
            | A::GenerateUpdate
            | A::GenerateDelete
            | A::CopyAsInsert
            | A::CopyAsUpdate
            | A::CopyAsDelete
            | A::FilterByValue(_)
            | A::FilterIsNull
            | A::FilterIsNotNull
            | A::RemoveFilter
            | A::Order(_)
            | A::AddToSort(_)
            | A::RemoveOrdering
            | A::InspectRow
            | A::ChartThisQuery
            | A::UnsetField
            | A::RevertCell
            | A::FindSimilarRows
            | A::FitColumnWidth
            | A::HideColumn
            | A::ChooseColumns
            | A::MoveColumnLeft
            | A::MoveColumnRight
            | A::PinColumn
            | A::UnpinColumn => {}
        }
    }
    actions
}

fn actions_of(nodes: &[MenuNode]) -> Vec<ContextMenuAction> {
    tree::targets(nodes)
        .into_iter()
        .filter_map(|target| match target {
            MenuTarget::Action(action) => Some(*action),
            _ => None,
        })
        .collect()
}

/// Every action the flat menu offered is still reachable from the grouped
/// cell menu (with the document-grid variants where a document's field
/// takes Unset field and Revert change instead of the column default).
#[test]
fn every_menu_action_is_reachable_in_the_grouped_menu() {
    let mut reachable = actions_of(&build_cell_menu(full_parts()));
    reachable.extend(actions_of(&build_cell_menu(CellMenuParts {
        document_patches: true,
        ..full_parts()
    })));

    let missing: Vec<_> = every_action()
        .into_iter()
        .filter(|action| !reachable.contains(action))
        .collect();
    assert!(missing.is_empty(), "not in the menu: {missing:?}");

    let menu = build_cell_menu(full_parts());
    let all = tree::targets(&menu);
    assert!(
        all.iter()
            .any(|target| matches!(target, MenuTarget::RowAction(action) if action.id == "kill")),
        "driver row actions are in Row"
    );
    assert!(
        all.iter()
            .any(|target| matches!(target, MenuTarget::Toolbar(_))),
        "the toolbar buttons are in Toolbar"
    );
}

/// The top level of the cell menu: the cell's own actions, editing, the
/// query and layout submenus, then More and Toolbar, each group apart.
#[test]
fn the_cell_menu_groups_its_rows() {
    let menu = build_cell_menu(full_parts());
    let rows: Vec<&str> = menu
        .iter()
        .map(|node| match node {
            MenuNode::Entry(entry) => entry.id.as_ref(),
            MenuNode::Submenu(submenu) => submenu.id,
            MenuNode::Separator => "--",
            MenuNode::Header(_) => "header",
        })
        .collect();

    assert_eq!(
        rows,
        vec![
            "copy",
            "copy-as",
            "view-value",
            "add-to-filter",
            "--",
            "edit",
            "edit-in-modal",
            "edit-as-expression",
            "paste",
            "set-value",
            "--",
            "filter",
            "sort",
            "column",
            "row",
            "--",
            "more",
            "toolbar",
        ]
    );
}

fn submenu<'a>(nodes: &'a [MenuNode], id: &str) -> &'a Submenu {
    nodes
        .iter()
        .find_map(|node| match node {
            MenuNode::Submenu(submenu) if submenu.id == id => Some(submenu),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {id} submenu"))
}

fn ids(nodes: &[MenuNode]) -> Vec<String> {
    nodes
        .iter()
        .map(|node| match node {
            MenuNode::Entry(entry) => entry.id.to_string(),
            MenuNode::Submenu(submenu) => submenu.id.to_string(),
            MenuNode::Separator => "--".to_string(),
            MenuNode::Header(_) => "header".to_string(),
        })
        .collect()
}

#[test]
fn the_submenus_hold_their_groups() {
    let menu = build_cell_menu(full_parts());

    let copy_as = &submenu(&menu, "copy-as").children;
    assert_eq!(
        ids(copy_as),
        vec![
            "copy-as-csv",
            "copy-as-json",
            "copy-as-in-list",
            "copy-as-inserts",
            "copy-as-markdown",
            "--",
            "header",
            "copy-query-insert",
            "copy-query-update",
            "copy-query-delete",
        ]
    );

    let set_value = ids(&submenu(&menu, "set-value").children);
    assert_eq!(
        set_value.get(..3).map(|ids| ids.to_vec()),
        Some(vec![
            "set-null".to_string(),
            "set-default".to_string(),
            "--".to_string()
        ])
    );

    assert_eq!(
        ids(&submenu(&menu, "column").children),
        vec![
            "column-hide",
            "column-move-left",
            "column-move-right",
            "column-pin",
            "column-unpin",
            "column-fit-width",
            "column-choose",
        ]
    );

    assert_eq!(
        ids(&submenu(&menu, "row").children),
        vec![
            "inspect-row",
            "add-row",
            "duplicate-row",
            "--",
            "row-action-0",
            "--",
            "delete-row",
        ]
    );

    let more = &submenu(&menu, "more").children;
    assert_eq!(
        ids(more),
        vec!["generate-sql", "chart-this-query", "find-similar-rows"]
    );
    assert_eq!(
        ids(&submenu(more, "generate-sql").children),
        vec![
            "generate-select-where",
            "generate-insert",
            "generate-update",
            "generate-delete",
        ],
        "More nests Generate SQL a level down"
    );
}

#[test]
fn an_empty_table_menu_keeps_add_row_but_no_row_actions() {
    let menu = build_cell_menu(CellMenuParts {
        has_row_target: false,
        ..full_parts()
    });
    let actions = actions_of(&menu);

    assert!(actions.contains(&ContextMenuAction::AddRow));
    for absent in [
        ContextMenuAction::Edit,
        ContextMenuAction::EditInModal,
        ContextMenuAction::DuplicateRow,
        ContextMenuAction::DeleteRow,
        ContextMenuAction::ViewValue,
        ContextMenuAction::AddToFilter,
    ] {
        assert!(!actions.contains(&absent), "{absent:?} needs a row");
    }
}

/// A read-only result still reads: Copy, Copy as, View value; nothing that
/// writes.
#[test]
fn a_read_only_result_offers_reading_but_no_editing() {
    let menu = build_cell_menu(CellMenuParts {
        is_editable: false,
        ..full_parts()
    });
    let actions = actions_of(&menu);

    assert!(actions.contains(&ContextMenuAction::Copy));
    assert!(actions.contains(&ContextMenuAction::CopyAsCsv));
    assert!(actions.contains(&ContextMenuAction::ViewValue));
    for absent in [
        ContextMenuAction::Edit,
        ContextMenuAction::Paste,
        ContextMenuAction::SetNull,
        ContextMenuAction::AddRow,
        ContextMenuAction::DeleteRow,
        ContextMenuAction::InspectRow,
    ] {
        assert!(!actions.contains(&absent), "{absent:?} writes");
    }
}

#[test]
fn chart_and_inspect_row_follow_their_flags() {
    let without = actions_of(&build_cell_menu(CellMenuParts {
        can_chart: false,
        inspect_row: false,
        ..full_parts()
    }));
    assert!(!without.contains(&ContextMenuAction::ChartThisQuery));
    assert!(!without.contains(&ContextMenuAction::InspectRow));

    let with = actions_of(&build_cell_menu(full_parts()));
    assert!(with.contains(&ContextMenuAction::ChartThisQuery));
    assert!(with.contains(&ContextMenuAction::InspectRow));
}

/// A submenu with nothing to offer is left out rather than drawn empty.
#[test]
fn an_empty_submenu_is_left_out() {
    let menu = build_cell_menu(CellMenuParts {
        is_editable: false,
        filter: Vec::new(),
        sort_column: None,
        columns: Vec::new(),
        row_actions: Vec::new(),
        generate_sql: false,
        can_chart: false,
        find_similar: None,
        toolbar: Vec::new(),
        ..full_parts()
    });

    assert_eq!(
        ids(&menu),
        vec!["copy", "copy-as", "view-value", "add-to-filter"]
    );
}

#[test]
fn the_document_menu_keeps_its_own_rows() {
    let menu = build_document_menu(DocumentMenuParts {
        is_editable: true,
        has_row_target: true,
        filter: filter_nodes(
            vec![("clear".into(), ContextMenuAction::RemoveFilter)],
            0,
            None,
        ),
        copy_query: Some("Copy as query".into()),
        toolbar: Vec::new(),
    });

    assert_eq!(
        ids(&menu),
        vec![
            "copy",
            "view-document",
            "--",
            "add-document",
            "duplicate-document",
            "delete-document",
            "--",
            "filter",
            "copy-query",
        ]
    );
}

/// Opening a submenu from the keys walks into its rows, and the path
/// through a nested one resolves to the row it names.
#[test]
fn a_path_reaches_a_nested_submenu_row() {
    let menu = build_cell_menu(full_parts());
    let more = menu
        .iter()
        .position(|node| matches!(node, MenuNode::Submenu(submenu) if submenu.id == "more"))
        .expect("More is in the menu");

    let path = tree::normalize(&menu, &[more, 0, 1]);
    assert_eq!(path, vec![more, 0, 1]);
    match tree::node_at(&menu, &path) {
        Some(MenuNode::Entry(entry)) => {
            assert_eq!(
                entry.target,
                MenuTarget::Action(ContextMenuAction::GenerateInsert)
            );
        }
        other => panic!("not a row: {other:?}"),
    }
}

/// Copy as JSON and Add to filter show their Results-layer shortcuts on the
/// menu row, so a label cannot drop off silently if the binding or the
/// lookup changes.
#[gpui::test]
fn the_menu_rows_show_their_results_shortcuts(cx: &mut gpui::TestAppContext) {
    let shortcut = |action| cx.update(|cx| super::sections::action_shortcut(action, cx));

    let json = shortcut(ContextMenuAction::CopyAsJson);
    assert!(
        json.as_ref()
            .is_some_and(|label| label.to_lowercase().contains('c')),
        "{json:?}"
    );
    assert_eq!(
        shortcut(ContextMenuAction::AddToFilter).as_deref(),
        Some("="),
        "Add to filter is `=`"
    );
}

/// The renderer only draws what the builders labelled; translating per row
/// on every frame is the builders' job, done once per menu.
#[test]
fn the_renderer_translates_nothing() {
    let source = include_str!("sections.rs");
    assert!(
        !source.contains("dbflux_i18n::t!("),
        "translate labels when the tree is built, not while drawing it"
    );
}

#[test]
fn the_menu_labels_resolve_in_every_locale() {
    let keys = [
        "document.data.context_menu.item.add_to_filter",
        "document.data.context_menu.item.edit_in_modal",
        "document.data.context_menu.copy_as.title",
        "document.data.context_menu.copy_as.csv",
        "document.data.context_menu.copy_as.json",
        "document.data.context_menu.copy_as.sql_in_list",
        "document.data.context_menu.copy_as.insert_statements",
        "document.data.context_menu.copy_as.markdown",
        "document.data.context_menu.set_value.title",
        "document.data.context_menu.set_value.to_null",
        "document.data.context_menu.set_value.to_default",
        "document.data.context_menu.sort.title",
        "document.data.context_menu.sort.ascending",
        "document.data.context_menu.sort.descending",
        "document.data.context_menu.row.title",
        "document.data.context_menu.more.title",
        "document.data.context_menu.filter.title",
        "document.data.context_menu.generate_sql.title",
        "document.data.grid.edit_bar.add_row",
        "document.data.grid.edit_bar.delete_rows",
        "document.data.grid.edit_bar.with_shortcut",
        "document.data.grid.edit_bar.add_row_disabled",
        "document.data.grid.edit_bar.delete_rows_no_selection",
    ];

    for key in keys {
        for locale in ["en", "es", "pt_BR", "ko", "zh_Hans"] {
            let value = dbflux_i18n::t!(key, locale = locale);

            assert!(!value.is_empty(), "{key} resolved empty in {locale}");
            assert_ne!(value, key, "{key} resolved to its own key in {locale}");
            assert_ne!(
                value,
                format!("{locale}.{key}"),
                "{key} missing from {locale} catalog"
            );
        }
    }
}

#[test]
fn like_filter_escapes_wildcards_without_a_backslash_escape_literal() {
    let dialect = dbflux_core::DefaultSqlDialect;
    let filter = like_contains_filter(&dialect, "\"path\"", "50%_a!\\b");

    assert_eq!(filter, "\"path\" LIKE '%50!%!_a!!\\b%' ESCAPE '!'");
    assert!(!filter.contains("'\\'"), "{filter}");

    // A lone backslash is LIKE's default escape on MySQL and PostgreSQL,
    // so it also needs the explicit clause to match literally.
    assert_eq!(
        like_contains_filter(&dialect, "\"path\"", "a\\b"),
        "\"path\" LIKE '%a\\b%' ESCAPE '!'"
    );
    assert_eq!(
        like_contains_filter(&dialect, "\"name\"", "bob"),
        "\"name\" LIKE '%bob%'"
    );
}
