//! Resolves what the grid offers for the cell, column or document a menu
//! opened on, and hands it to the layouts in `items`.

use super::items::{CellMenuParts, DocumentMenuParts, build_cell_menu, build_document_menu};
use super::tree::{HeaderTone, MenuEntry, MenuHeader, MenuNode};
use super::{DataGridPanel, FilterBackend, TableContextMenu};
use dbflux_components::components::data_table::ContextMenuAction;
use dbflux_components::icons::AppIcon;
use dbflux_core::SortDirection;
use gpui::{App, Context, SharedString};

impl DataGridPanel {
    /// The rows of the open menu, as the renderer draws them and the menu
    /// keys walk them.
    pub(in crate::data_grid_panel) fn context_menu_tree(
        &self,
        menu: &TableContextMenu,
        cx: &Context<Self>,
    ) -> Vec<MenuNode> {
        let backend = self.filter_backend(cx);
        if menu.is_column_header {
            self.column_header_menu_tree(menu, backend, cx)
        } else if menu.is_document_view {
            self.document_menu_tree(menu, backend, cx)
        } else {
            build_cell_menu(self.cell_menu_parts(menu, backend, cx))
        }
    }

    fn cell_menu_parts(
        &self,
        menu: &TableContextMenu,
        backend: Option<FilterBackend>,
        cx: &Context<Self>,
    ) -> CellMenuParts {
        let is_editable = self.check_is_editable(cx);
        let has_row_target = self.has_context_menu_row_target(menu.row, false, cx);
        let table_queries = self.offers_table_query_actions();

        let filter = if table_queries && self.has_filter_submenu(backend, false, cx) {
            let (_, _, items, value_ops_count) = self.build_filter_items(menu, backend, cx);
            super::items::filter_nodes(
                items,
                value_ops_count,
                Some(dbflux_i18n::t!("document.data.context_menu.filter.cell_value").into()),
            )
        } else {
            Vec::new()
        };

        let sort_column = (table_queries && matches!(backend, Some(FilterBackend::Sql)))
            .then(|| self.column_name(menu.col))
            .flatten();

        let accepts_expression = self
            .grid_table
            .table_state
            .as_ref()
            .is_some_and(|state| state.read(cx).accepts_cell_expressions(menu.col));

        CellMenuParts {
            is_editable,
            has_row_target,
            header: has_row_target.then(|| self.cell_menu_header(menu, cx)),
            add_to_filter: self.can_add_to_filter(),
            copy_as: self.copy_as_entries(cx),
            copy_query: (table_queries && self.has_copy_query_support())
                .then(|| self.copy_query_submenu_label(cx).into()),
            accepts_expression,
            document_patches: self.commits_document_patches(cx),
            generated_ids: self.generate_id_entries(menu, cx),
            filter,
            sort_column,
            columns: self.column_menu_items(menu.col, cx),
            inspect_row: !self.is_grouped_result(),
            row_actions: menu.row_actions.clone(),
            generate_sql: table_queries,
            can_chart: self.can_chart_from_context_menu(cx),
            find_similar: self.find_similar_entry(menu, has_row_target, cx),
            toolbar: self.toolbar_menu_entries(cx),
        }
    }

    fn document_menu_tree(
        &self,
        menu: &TableContextMenu,
        backend: Option<FilterBackend>,
        cx: &Context<Self>,
    ) -> Vec<MenuNode> {
        let table_queries = self.offers_table_query_actions();
        let filter = if table_queries && self.has_filter_submenu(backend, true, cx) {
            let (_, _, items, value_ops_count) = self.build_filter_items(menu, backend, cx);
            super::items::filter_nodes(
                items,
                value_ops_count,
                Some(dbflux_i18n::t!("document.data.context_menu.filter.cell_value").into()),
            )
        } else {
            Vec::new()
        };

        build_document_menu(DocumentMenuParts {
            is_editable: self.check_is_editable(cx),
            has_row_target: self.has_context_menu_row_target(menu.row, true, cx),
            filter,
            copy_query: (table_queries && self.has_copy_query_support())
                .then(|| self.copy_query_submenu_label(cx).into()),
            toolbar: self.toolbar_menu_entries(cx),
        })
    }

    /// The DBeaver-style flat menu of a column header: ordering first, then
    /// every filter operator in the same panel, then the Column section.
    fn column_header_menu_tree(
        &self,
        menu: &TableContextMenu,
        backend: Option<FilterBackend>,
        cx: &App,
    ) -> Vec<MenuNode> {
        let column = self.column_name(menu.col).unwrap_or_default();
        let header = |key: &str, icon| {
            MenuNode::Header(MenuHeader {
                label: dbflux_i18n::t!(key).into(),
                icon: Some(icon),
                tone: HeaderTone::Muted,
            })
        };
        let order = |id: &'static str, label: String, icon, action| {
            MenuNode::Entry(MenuEntry::action(id, label, icon, action))
        };
        let add_to_sort = |direction: &str| {
            dbflux_i18n::t!(
                "document.data.context_menu.order.add_to_sort",
                column = format!("{column} {direction}")
            )
        };

        let mut nodes = vec![
            header(
                "document.data.context_menu.order.title",
                AppIcon::ArrowUpDown,
            ),
            order(
                "order-asc",
                format!("{column} ASC"),
                AppIcon::ArrowUp,
                ContextMenuAction::Order(SortDirection::Ascending),
            ),
            order(
                "order-desc",
                format!("{column} DESC"),
                AppIcon::ArrowDown,
                ContextMenuAction::Order(SortDirection::Descending),
            ),
            MenuNode::Separator,
            order(
                "order-add-asc",
                add_to_sort("ASC"),
                AppIcon::ArrowUp,
                ContextMenuAction::AddToSort(SortDirection::Ascending),
            ),
            order(
                "order-add-desc",
                add_to_sort("DESC"),
                AppIcon::ArrowDown,
                ContextMenuAction::AddToSort(SortDirection::Descending),
            ),
            MenuNode::Separator,
            MenuNode::Entry(
                MenuEntry::action(
                    "order-remove",
                    dbflux_i18n::t!("document.data.context_menu.order.remove"),
                    AppIcon::X,
                    ContextMenuAction::RemoveOrdering,
                )
                .danger(),
            ),
        ];

        let (_, _, filter_items, value_ops_count) = self.build_filter_items(menu, backend, cx);
        let filter = super::items::filter_nodes(filter_items, value_ops_count, None);
        if !filter.is_empty() {
            nodes.push(MenuNode::Separator);
            nodes.push(header(
                "document.data.context_menu.filter.title",
                AppIcon::ListFilter,
            ));
            nodes.extend(filter);
        }

        let columns = self.column_menu_items(menu.col, cx);
        if !columns.is_empty() {
            nodes.push(MenuNode::Separator);
            nodes.push(header(
                "document.data.context_menu.column.title",
                AppIcon::Columns,
            ));
            nodes.extend(columns.into_iter().map(|item| {
                MenuNode::Entry(MenuEntry::action(
                    super::items::column_entry_id(item.action),
                    item.label,
                    item.icon,
                    item.action,
                ))
            }));
        }

        nodes
    }

    fn column_name(&self, col: usize) -> Option<String> {
        self.result
            .columns
            .get(col)
            .map(|column| column.name.clone())
    }

    /// The caption of the cell menu: the column and row it acts on, with the
    /// column's key icon (PK in the warning color, FK in the info color)
    /// when the table metadata marks it as a key.
    fn cell_menu_header(&self, menu: &TableContextMenu, cx: &App) -> MenuHeader {
        let label = dbflux_i18n::t!(
            "document.data.context_menu.header",
            column = self.column_name(menu.col).unwrap_or_default(),
            row = menu.row + 1
        );

        let table_state = self
            .grid_table
            .table_state
            .as_ref()
            .map(|table| table.read(cx));
        let (icon, tone) =
            if table_state.is_some_and(|state| state.pk_columns().contains(&menu.col)) {
                (AppIcon::KeyRound, HeaderTone::Warning)
            } else if table_state.is_some_and(|state| state.fk_columns().contains(&menu.col)) {
                (AppIcon::Cable, HeaderTone::Info)
            } else {
                (AppIcon::Columns, HeaderTone::Muted)
            };

        MenuHeader {
            label: label.into(),
            icon: Some(icon),
            tone,
        }
    }

    /// Copy as ▸: CSV and JSON, then the extractors the result supports.
    pub(in crate::data_grid_panel) fn copy_as_entries(&self, cx: &App) -> Vec<MenuEntry> {
        let entry = |id: &'static str, key: &str, icon, action| {
            MenuEntry::action(id, SharedString::from(dbflux_i18n::t!(key)), icon, action)
        };

        let mut entries = vec![
            entry(
                "copy-as-csv",
                "document.data.context_menu.copy_as.csv",
                AppIcon::FileSpreadsheet,
                ContextMenuAction::CopyAsCsv,
            ),
            entry(
                "copy-as-json",
                "document.data.context_menu.copy_as.json",
                AppIcon::Braces,
                ContextMenuAction::CopyAsJson,
            ),
        ];
        if self.offers_copy_as_in_list(cx) {
            entries.push(entry(
                "copy-as-in-list",
                "document.data.context_menu.copy_as.sql_in_list",
                AppIcon::Parentheses,
                ContextMenuAction::CopyAsSqlInList,
            ));
        }
        if self.offers_copy_as_inserts(cx) {
            entries.push(entry(
                "copy-as-inserts",
                "document.data.context_menu.copy_as.insert_statements",
                AppIcon::Database,
                ContextMenuAction::CopyAsInsertStatements,
            ));
        }
        entries.push(entry(
            "copy-as-markdown",
            "document.data.context_menu.copy_as.markdown",
            AppIcon::Table,
            ContextMenuAction::CopyAsMarkdown,
        ));

        entries
    }

    /// "Find similar rows" when the table has a vector column and the menu
    /// is on a row of the grid. The entry names the column and K it reads,
    /// or, disabled, why it cannot run from this row.
    pub(in crate::data_grid_panel) fn find_similar_entry(
        &self,
        menu: &TableContextMenu,
        has_row_target: bool,
        cx: &Context<Self>,
    ) -> Option<MenuEntry> {
        if menu.is_document_view || !has_row_target || !self.can_find_similar_rows(menu.col, cx) {
            return None;
        }

        let block = self.similar_rows_block(menu.row, menu.col, cx);
        let detail = match &block {
            Some(block) => Some(block.menu_reason()),
            None => self.similar_rows_menu_detail(menu.col, cx),
        };

        let mut entry = MenuEntry::action(
            "find-similar-rows",
            dbflux_i18n::t!("document.data.context_menu.item.find_similar_rows"),
            AppIcon::ChartScatter,
            ContextMenuAction::FindSimilarRows,
        );
        entry.detail = detail.map(Into::into);
        entry.disabled = block.is_some();
        Some(entry)
    }

    /// Whether the cell under the menu can be added to the WHERE filter, as
    /// a middle-click or `=` adds it: a table whose filter row is shown.
    pub(in crate::data_grid_panel) fn can_add_to_filter(&self) -> bool {
        self.source.is_table()
            && self.filter_input_visible()
            && self.neighbor_bar_search().is_none()
    }
}
