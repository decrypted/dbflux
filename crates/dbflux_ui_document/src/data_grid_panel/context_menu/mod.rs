use super::utils::{extract_pk_columns, value_to_json};
use super::{
    DataGridEvent, DataGridPanel, DataSource, EditState, PendingDeleteConfirm,
    PendingDocumentPreview, PendingModalOpen, PendingToast, SqlGenerateKind, TableContextMenu,
};
use dbflux_app::keymap::{Command, ContextId};
use dbflux_components::chart::detect_chart_columns;
use dbflux_components::components::data_table::ROW_NUMBER_WIDTH;
use dbflux_components::components::data_table::{ContextMenuAction, FilterOperator};
use dbflux_components::composites::{MenuItem, render_menu_header};
use dbflux_components::controls::Button;
use dbflux_components::fonts;
use dbflux_components::icons::AppIcon;
use dbflux_components::modals::Modal;
use dbflux_components::primitives::{Icon, SurfaceRole, Text, overlay_bg, surface};
use dbflux_components::tokens::{FontSizes, Heights, MenuMetrics, Radii, Spacing};
use dbflux_core::{
    DocumentDelete, DocumentFilter, DocumentInsert, DocumentUpdate, MutationRequest, RowDelete,
    RowIdentity, RowInsert, RowPatch, Value,
};
use dbflux_export::ExportFormat;
use dbflux_ui_base::toast::{Toast, copy_action, now_hms};
use dbflux_ui_base::work_queue::QueryWork;
use dbflux_ui_base::{AsyncUpdateResultExt, SaveTargetOutcome};
use gpui::prelude::FluentBuilder;
use gpui::{deferred, *};
use gpui_component::ActiveTheme;
use std::fs::File;
use std::io::BufWriter;

mod add_to_filter;
mod build;
mod columns;
mod copy_extractors;
mod items;
mod sections;
pub(super) mod toolbar;
pub(super) mod tree;
pub(in crate::data_grid_panel) use sections::action_shortcut;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterBackend {
    Sql,
    Mongo,
}

/// Width of the cell context menu (AppByzMenu).
const CONTEXT_MENU_WIDTH: Pixels = px(270.0);

/// Width of the menu opened from a column header, which lists every filter
/// operator inline.
const COLUMN_HEADER_MENU_WIDTH: Pixels = px(300.0);

/// One row of the data grid's export menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportMenuEntry {
    /// A Rows, Columns or CSV choice; the format rows below follow it.
    Option(super::export_options::ExportOptionRow),
    /// Write the chosen rows and columns to a file in this format.
    Save(ExportFormat),
    /// Put the chosen rows and columns on the clipboard in this format.
    Copy(ExportFormat),
    /// Page through the whole table and stream it to a file in this format.
    SaveAll(dbflux_transfer::FileFormat),
}

impl ExportMenuEntry {
    /// Raw binary has no text form, so it cannot go to the clipboard.
    pub(super) fn is_enabled(self) -> bool {
        !matches!(self, ExportMenuEntry::Copy(ExportFormat::Binary))
    }
}

/// The export menu row one step from `from`, wrapping at both ends and
/// passing over disabled rows. Returns `from` when no other row is enabled.
// index starts at from.min(count - 1) and only moves by modular steps over
// count, so entries[index] stays below entries.len(); count > 0 after the
// early return.
#[expect(
    clippy::indexing_slicing,
    reason = "index starts at from.min(count - 1) and moves only by modular steps over count, with count > 0 after the early return"
)]
pub(super) fn step_export_selection(
    entries: &[ExportMenuEntry],
    from: usize,
    forward: bool,
) -> usize {
    let count = entries.len();
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

        if entries[index].is_enabled() {
            return index;
        }
    }

    from
}

impl DataGridPanel {
    pub(in crate::data_grid_panel) fn restore_focus_after_context_menu(
        &mut self,
        is_document_view: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus_mode = super::GridFocusMode::Table;
        self.focus.edit_state = EditState::Navigating;

        if is_document_view {
            if let Some(tree_state) = &self.document_view.document_tree_state {
                tree_state.update(cx, |state, cx| state.focus(window, cx));
            } else {
                self.focus_handle.focus(window, cx);
            }
        } else {
            self.focus_handle.focus(window, cx);
        }

        cx.emit(DataGridEvent::Focused);
    }

    /// Opens context menu at the current selection.
    pub(super) fn open_context_menu_at_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        let (row, col, cell_x) = {
            let ts = table_state.read(cx);

            let (row, col) = ts
                .selection()
                .active
                .map(|c| (c.row, c.col))
                .unwrap_or((0, 0));

            // Left edge of the cell's column as the screen draws it, scroll
            // and pinned columns included.
            let cell_x = ts.column_screen_left(col).unwrap_or(ROW_NUMBER_WIDTH);

            (row, col, cell_x)
        };

        // Calculate position in window coordinates:
        // x: panel_origin.x + cell_x - horizontal_scroll + some padding
        // y: panel_origin.y + HEADER_HEIGHT + (row * ROW_HEIGHT) + some padding for toolbar
        let toolbar_height = px(36.0); // Approximate toolbar height
        let position = Point {
            x: self.panel_origin.x + cell_x + px(20.0),
            y: self.panel_origin.y
                + toolbar_height
                + fonts::grid_header_height(cx)
                + fonts::grid_row_height(cx) * row,
        };

        self.context_menu = Some(TableContextMenu {
            row_actions: self.menu_row_actions(),
            ..TableContextMenu::new(row, col, position)
        });

        // Focus the context menu to receive keyboard events
        self.focus.context_menu_focus.focus(window, cx);
        cx.emit(DataGridEvent::Focused);
        cx.notify();
    }

    /// Opens context menu for document view at the specified position.
    #[allow(dead_code)]
    pub(super) fn open_document_context_menu(
        &mut self,
        doc_index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(TableContextMenu {
            is_document_view: true,
            ..TableContextMenu::new(doc_index, 0, position)
        });

        self.focus.context_menu_focus.focus(window, cx);
        cx.emit(DataGridEvent::Focused);
        cx.notify();
    }

    /// Opens context menu for document view at the current cursor position (keyboard triggered).
    pub(super) fn open_document_context_menu_at_cursor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tree_state) = &self.document_view.document_tree_state else {
            return;
        };

        let (doc_index, field_path, field_value) = tree_state.update(cx, |ts, _cx| {
            let cursor_id = ts.cursor().cloned();
            let idx = cursor_id
                .as_ref()
                .and_then(|id| id.doc_index())
                .unwrap_or(0);

            let (fp, fv) = cursor_id
                .as_ref()
                .and_then(|cid| {
                    let node = ts.visible_nodes().iter().find(|n| &n.id == cid)?;
                    // NodeId::root(), child() and parent() all keep the
                    // leading document index segment, and no crate builds a
                    // NodeId from its public path field, so the
                    // slice after it is in range.
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "cursor ids come from NodeId::root()/child()/parent(), which all keep the leading document index segment, so path has at least one element and [1..] is in range"
                    )]
                    let path: Vec<String> = cid.path[1..].to_vec();
                    let path_opt = if path.is_empty() { None } else { Some(path) };
                    Some((path_opt, Some(node.value.clone())))
                })
                .unwrap_or((None, None));

            (idx, fp, fv)
        });

        // Use panel origin with some offset for keyboard-triggered menu
        let position = Point {
            x: self.panel_origin.x + px(100.0),
            y: self.panel_origin.y + px(100.0),
        };

        self.context_menu = Some(TableContextMenu {
            is_document_view: true,
            doc_field_path: field_path,
            doc_field_value: field_value,
            ..TableContextMenu::new(doc_index, 0, position)
        });

        self.focus.context_menu_focus.focus(window, cx);
        cx.emit(DataGridEvent::Focused);
        cx.notify();
    }

    fn filter_backend(&self, cx: &App) -> Option<FilterBackend> {
        match &self.source {
            DataSource::Table { profile_id, .. } => {
                let is_sql = self
                    .app_state
                    .read(cx)
                    .connections()
                    .get(profile_id)
                    .map(|c| {
                        c.connection.metadata().query_language == dbflux_core::QueryLanguage::Sql
                    })
                    .unwrap_or(false);
                is_sql.then_some(FilterBackend::Sql)
            }
            DataSource::Collection { .. } => Some(FilterBackend::Mongo),
            _ => None,
        }
    }

    fn has_filter_submenu(
        &self,
        backend: Option<FilterBackend>,
        is_document_view: bool,
        cx: &App,
    ) -> bool {
        match backend {
            Some(FilterBackend::Sql) => !is_document_view,
            Some(FilterBackend::Mongo) => {
                is_document_view && self.mongo_filter_field_info(cx).is_some()
            }
            None => false,
        }
    }

    fn mongo_filter_field_info(&self, _cx: &App) -> Option<(String, Value)> {
        use dbflux_components::components::document_tree::NodeValue;

        let menu = self.context_menu.as_ref()?;
        let path = menu.doc_field_path.as_ref()?;
        if path.is_empty() {
            return None;
        }

        let field = path.join(".");
        let value = match menu.doc_field_value.as_ref()? {
            NodeValue::Scalar(v) => v.clone(),
            _ => return None,
        };

        if matches!(value, Value::Bytes(_)) {
            return None;
        }

        Some((field, value))
    }

    fn sql_filter_operators(type_name: &str, value: &Value) -> Vec<FilterOperator> {
        if !Self::is_value_filterable(value) {
            return Vec::new();
        }

        if Self::is_sql_json_type(type_name, value) {
            return vec![FilterOperator::Eq, FilterOperator::NotEq];
        }

        if Self::is_sql_bool_type(type_name, value) {
            return vec![FilterOperator::Eq, FilterOperator::NotEq];
        }

        if Self::is_sql_comparable_type(type_name, value) {
            return vec![
                FilterOperator::Eq,
                FilterOperator::NotEq,
                FilterOperator::Gt,
                FilterOperator::Gte,
                FilterOperator::Lt,
                FilterOperator::Lte,
            ];
        }

        if Self::is_sql_text_type(type_name, value) {
            return vec![
                FilterOperator::Eq,
                FilterOperator::NotEq,
                FilterOperator::Like,
            ];
        }

        vec![FilterOperator::Eq, FilterOperator::NotEq]
    }

    fn mongo_filter_operators(value: &Value) -> Vec<FilterOperator> {
        if !Self::is_value_filterable(value) {
            return Vec::new();
        }

        if Self::is_mongo_comparable(value) {
            return vec![
                FilterOperator::Eq,
                FilterOperator::NotEq,
                FilterOperator::Gt,
                FilterOperator::Gte,
                FilterOperator::Lt,
                FilterOperator::Lte,
            ];
        }

        vec![FilterOperator::Eq, FilterOperator::NotEq]
    }

    fn is_mongo_comparable(value: &Value) -> bool {
        matches!(
            value,
            Value::Int(_)
                | Value::Float(_)
                | Value::Decimal(_)
                | Value::DateTime(_)
                | Value::Date(_)
                | Value::Time(_)
        )
    }

    fn is_sql_json_type(type_name: &str, value: &Value) -> bool {
        type_name.contains("json") || type_name.contains("bson") || matches!(value, Value::Json(_))
    }

    fn is_sql_bool_type(type_name: &str, value: &Value) -> bool {
        type_name.contains("bool") || matches!(value, Value::Bool(_))
    }

    fn is_sql_comparable_type(type_name: &str, value: &Value) -> bool {
        if matches!(
            value,
            Value::Int(_)
                | Value::Float(_)
                | Value::Decimal(_)
                | Value::DateTime(_)
                | Value::Date(_)
                | Value::Time(_)
        ) {
            return true;
        }

        type_name.contains("int")
            || type_name.contains("serial")
            || type_name.contains("float")
            || type_name.contains("double")
            || type_name.contains("real")
            || type_name.contains("numeric")
            || type_name.contains("decimal")
            || type_name.contains("number")
            || type_name.contains("money")
            || type_name.contains("date")
            || type_name.contains("time")
            || type_name.contains("timestamp")
            || type_name.contains("datetime")
            || type_name.contains("year")
    }

    fn is_sql_text_type(type_name: &str, value: &Value) -> bool {
        if matches!(value, Value::Text(_) | Value::ObjectId(_)) {
            return true;
        }

        type_name.contains("text")
            || type_name.contains("char")
            || type_name.contains("string")
            || type_name.contains("clob")
            || type_name.contains("uuid")
            || type_name.contains("citext")
            || type_name.contains("tsvector")
            || type_name.contains("tsquery")
            || type_name.contains("enum")
            || type_name.contains("set")
    }

    fn sql_operator_symbol(operator: FilterOperator) -> &'static str {
        match operator {
            FilterOperator::Eq => "=",
            FilterOperator::NotEq => "<>",
            FilterOperator::Gt => ">",
            FilterOperator::Gte => ">=",
            FilterOperator::Lt => "<",
            FilterOperator::Lte => "<=",
            FilterOperator::Like => "LIKE",
        }
    }

    fn mongo_operator_symbol(operator: FilterOperator) -> &'static str {
        match operator {
            FilterOperator::Eq => "=",
            FilterOperator::NotEq => "!=",
            FilterOperator::Gt => ">",
            FilterOperator::Gte => ">=",
            FilterOperator::Lt => "<",
            FilterOperator::Lte => "<=",
            FilterOperator::Like => "LIKE",
        }
    }

    fn mongo_value_display_preview(value: &Value) -> String {
        match value {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Decimal(d) => d.clone(),
            Value::Text(s) => {
                let sanitized = Self::sanitize_for_label(s);
                format!("\"{}\"", sanitized)
            }
            Value::Json(j) => Self::truncate_for_label(&Self::sanitize_for_label(j), 20),
            Value::ObjectId(oid) => {
                format!("ObjectId(\"{}\")", Self::truncate_for_label(oid, 12))
            }
            Value::DateTime(dt) => format!("\"{}\"", dt.to_rfc3339()),
            Value::Date(d) => format!("\"{}\"", d),
            Value::Time(t) => format!("\"{}\"", t),
            Value::Unsupported(type_name) => {
                format!("<unsupported:{}>", Self::truncate_for_label(type_name, 20))
            }
            Value::Bytes(b) => format!("[{} bytes]", b.len()),
            Value::Array(_) | Value::Document(_) => "...".to_string(),
        }
    }

    fn build_filter_items(
        &self,
        menu: &TableContextMenu,
        backend: Option<FilterBackend>,
        cx: &App,
    ) -> (String, usize, Vec<(String, ContextMenuAction)>, usize) {
        match backend {
            Some(FilterBackend::Sql) => self.build_sql_filter_items(menu, cx),
            Some(FilterBackend::Mongo) => self.build_mongo_filter_items(cx),
            None => (String::new(), 0, Vec::new(), 0),
        }
    }

    fn build_sql_filter_items(
        &self,
        menu: &TableContextMenu,
        cx: &App,
    ) -> (String, usize, Vec<(String, ContextMenuAction)>, usize) {
        let (col_name, col_type_name) = self
            .result
            .columns
            .get(menu.col)
            .map(|column| (column.name.clone(), column.type_name.to_ascii_lowercase()))
            .unwrap_or_default();

        let cell_value = self.resolve_cell_value(menu.row, menu.col, cx);

        let mut items: Vec<(String, ContextMenuAction)> = Vec::new();
        let mut value_ops_count = 0;

        if let Some(ref value) = cell_value {
            let display = cell_value
                .as_ref()
                .map(Self::value_display_preview)
                .unwrap_or_default();
            let short = Self::truncate_for_label(&display, 20);

            let operators = Self::sql_filter_operators(&col_type_name, value);
            value_ops_count = operators.len();

            for operator in operators {
                let op = Self::sql_operator_symbol(operator);
                items.push((
                    format!("{} {} {}", col_name, op, short),
                    ContextMenuAction::FilterByValue(operator),
                ));
            }
        }

        items.push((
            format!("{} IS NULL", col_name),
            ContextMenuAction::FilterIsNull,
        ));
        items.push((
            format!("{} IS NOT NULL", col_name),
            ContextMenuAction::FilterIsNotNull,
        ));
        items.push((
            dbflux_i18n::t!("document.data.context_menu.toolbar.clear_filter"),
            ContextMenuAction::RemoveFilter,
        ));

        let count = items.len();
        (col_name, count, items, value_ops_count)
    }

    fn build_mongo_filter_items(
        &self,
        cx: &App,
    ) -> (String, usize, Vec<(String, ContextMenuAction)>, usize) {
        let Some((field, ref val)) = self.mongo_filter_field_info(cx) else {
            return (String::new(), 0, Vec::new(), 0);
        };

        let display = Self::mongo_value_display_preview(val);
        let short = Self::truncate_for_label(&display, 20);

        let mut items: Vec<(String, ContextMenuAction)> = Vec::new();
        let operators = Self::mongo_filter_operators(val);
        let value_ops_count = operators.len();

        for operator in operators {
            let op = Self::mongo_operator_symbol(operator);
            items.push((
                format!("{} {} {}", field, op, short),
                ContextMenuAction::FilterByValue(operator),
            ));
        }

        items.push((
            format!("{} IS NULL", field),
            ContextMenuAction::FilterIsNull,
        ));
        items.push((
            format!("{} IS NOT NULL", field),
            ContextMenuAction::FilterIsNotNull,
        ));
        items.push((
            dbflux_i18n::t!("document.data.context_menu.toolbar.clear_filter"),
            ContextMenuAction::RemoveFilter,
        ));

        let count = items.len();
        (field, count, items, value_ops_count)
    }

    /// Returns true if the data grid is editable (has primary key info).
    pub(super) fn check_is_editable(&self, cx: &App) -> bool {
        self.grid_table
            .table_state
            .as_ref()
            .map(|ts| ts.read(cx).is_editable())
            .unwrap_or(false)
    }

    /// Returns the active context for keyboard handling.
    pub fn active_context(&self, cx: &App) -> ContextId {
        if self.document_view.cell_editor.read(cx).is_visible()
            || self
                .document_view
                .document_preview_modal
                .read(cx)
                .is_visible()
        {
            return ContextId::TextInput;
        }

        // The value panel's editor is a plain text buffer sitting next to the
        // grid. Without this the results keymap would claim every bare letter
        // the user types into it as a grid command.
        if self.inspector.value_panel_open
            && let Some(panel) = self.inspector.value_panel.as_ref()
            && panel.read(cx).editor_has_focus()
        {
            return ContextId::TextInput;
        }

        if let Some(context) = self.builder_rail_context(cx) {
            return context;
        }

        if self.focused_side_island(cx).is_some() {
            return ContextId::Inspector;
        }

        let inline_text_input_active = self
            .grid_table
            .table_state
            .as_ref()
            .map(|ts| ts.read(cx).is_editing_text_input())
            .unwrap_or(false);

        if self.context_menu.is_some()
            || self.column_chooser.is_some()
            || self.chrome.export_menu_open
            || self.collection.history_open
        {
            ContextId::ContextMenu
        } else if inline_text_input_active || self.focus.edit_state == EditState::Editing {
            ContextId::TextInput
        } else {
            ContextId::Results
        }
    }

    fn has_context_menu_row_target(&self, row: usize, is_document_view: bool, cx: &App) -> bool {
        if is_document_view {
            return self
                .document_view
                .document_tree_state
                .as_ref()
                .and_then(|state| state.read(cx).get_raw_document(row))
                .is_some();
        }

        self.grid_table
            .table_state
            .as_ref()
            .and_then(|state| state.read(cx).edit_buffer().visual_row_source(row))
            .is_some()
    }

    fn context_menu_action_requires_row_target(action: ContextMenuAction) -> bool {
        matches!(
            action,
            ContextMenuAction::Edit
                | ContextMenuAction::EditAsExpression
                | ContextMenuAction::GenerateId(_)
                | ContextMenuAction::EditInModal
                | ContextMenuAction::ViewValue
                | ContextMenuAction::AddToFilter
                | ContextMenuAction::SetDefault
                | ContextMenuAction::SetNull
                | ContextMenuAction::UnsetField
                | ContextMenuAction::RevertCell
                | ContextMenuAction::FindSimilarRows
                | ContextMenuAction::DuplicateRow
                | ContextMenuAction::DeleteRow
                | ContextMenuAction::GenerateSelectWhere
                | ContextMenuAction::GenerateInsert
                | ContextMenuAction::GenerateUpdate
                | ContextMenuAction::GenerateDelete
                | ContextMenuAction::CopyAsInsert
                | ContextMenuAction::CopyAsUpdate
                | ContextMenuAction::CopyAsDelete
                | ContextMenuAction::FilterByValue(_)
        )
    }

    // === Export ===

    /// Opens the export menu with the keyboard in it, or closes it when it is
    /// already open (`Command::ExportResults`, the Export button).
    pub fn export_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.export_menu_open {
            self.close_export_menu(window, cx);
            return;
        }

        if self.result.rows.is_empty()
            && self.result.text_body.is_none()
            && self.result.raw_bytes.is_none()
        {
            let message = dbflux_i18n::t!("document.data.context_menu.error.no_results_to_export");
            Toast::error(message.clone())
                .meta_right(now_hms())
                .action(copy_action(message))
                .push(cx);
            return;
        }

        self.load_export_options(cx);
        self.chrome.export_menu_open = true;
        // The keyboard starts on the first format, so Ctrl+E Enter still
        // saves at once; the options sit above it.
        self.chrome.export_menu_selected = self
            .export_menu_entries()
            .iter()
            .position(|entry| !matches!(entry, ExportMenuEntry::Option(_)))
            .unwrap_or(0);
        self.focus.export_menu_focus.focus(window, cx);
        cx.emit(DataGridEvent::Focused);
        cx.notify();
    }

    /// Closes the export menu and hands the keyboard back to the grid.
    pub(super) fn close_export_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.export_menu_open = false;

        let is_document_view = self.view_config.mode == crate::DataViewMode::Document;
        self.restore_focus_after_context_menu(is_document_view, window, cx);
        cx.notify();
    }

    /// The rows of the export menu, in the order it draws them: the option
    /// rows, then a save row and a copy row per format, or, when the Rows
    /// option is All rows, an export-all row per streaming format.
    pub(crate) fn export_menu_entries(&self) -> Vec<ExportMenuEntry> {
        let options = self
            .export_option_rows()
            .into_iter()
            .map(ExportMenuEntry::Option);

        if self.exports_all_rows() {
            return options
                .chain(
                    dbflux_transfer::FileFormat::ALL
                        .iter()
                        .map(|&format| ExportMenuEntry::SaveAll(format)),
                )
                .collect();
        }

        let formats = dbflux_export::available_formats(&self.result.shape);
        options
            .chain(formats.iter().map(|&format| ExportMenuEntry::Save(format)))
            .chain(formats.iter().map(|&format| ExportMenuEntry::Copy(format)))
            .collect()
    }

    /// Handles the context-menu keys while the export menu is open: move,
    /// run the highlighted row, or close. Every other command is refused, as
    /// the cell menu refuses them.
    /// Keeps the highlighted export row in view in a menu that scrolls.
    fn scroll_export_selection_into_view(&self, entries: &[ExportMenuEntry]) {
        self.focus
            .export_menu_scroll
            .scroll_to_item(super::render::export_menu_child_index(
                entries,
                self.chrome.export_menu_selected,
            ));
    }

    pub(super) fn dispatch_export_menu_command(
        &mut self,
        cmd: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let entries = self.export_menu_entries();

        if let Some(&entry) = entries.get(self.chrome.export_menu_selected)
            && let Some(handled) = self.dispatch_export_option_command(entry, cmd, cx)
        {
            return handled;
        }

        match cmd {
            Command::MenuDown | Command::SelectNext => {
                self.chrome.export_menu_selected =
                    step_export_selection(&entries, self.chrome.export_menu_selected, true);
                self.scroll_export_selection_into_view(&entries);
                cx.notify();
                true
            }
            Command::MenuUp | Command::SelectPrev => {
                self.chrome.export_menu_selected =
                    step_export_selection(&entries, self.chrome.export_menu_selected, false);
                self.scroll_export_selection_into_view(&entries);
                cx.notify();
                true
            }
            Command::MenuSelect | Command::Execute => {
                if let Some(&entry) = entries.get(self.chrome.export_menu_selected)
                    && entry.is_enabled()
                {
                    self.run_export_menu_entry(entry, window, cx);
                }
                true
            }
            Command::MenuBack | Command::Cancel | Command::ExportResults => {
                self.close_export_menu(window, cx);
                true
            }
            _ => false,
        }
    }

    pub(super) fn run_export_menu_entry(
        &mut self,
        entry: ExportMenuEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match entry {
            ExportMenuEntry::Option(row) => self.activate_export_option(row, cx),
            ExportMenuEntry::Save(format) => self.export_with_format(format, window, cx),
            ExportMenuEntry::Copy(format) => self.copy_to_clipboard_with_format(format, window, cx),
            ExportMenuEntry::SaveAll(format) => self.export_all_rows(format, window, cx),
        }
    }

    pub fn export_with_format(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.export_menu_open {
            self.close_export_menu(window, cx);
        }

        let (result, scope_label) = self.export_menu_payload(cx);
        let csv_options = self.export_menu_csv_options();
        let base_name = self.export_base_name();
        let extension = format.extension();
        let suggested_name =
            dbflux_core::local_files::SafeFileName::new(&format!("{}.{}", base_name, extension));
        let format_name = format.name();

        let entity = cx.entity().clone();
        let audit_service = self.app_state.read(cx).audit_service().clone();
        let save_target_override = self.app_state.read(cx).save_target_override();

        cx.spawn(async move |_this, cx| {
            let outcome = dbflux_ui_base::file_dialog::resolve_save_target(
                save_target_override,
                dbflux_ui_base::SaveTargetRequest {
                    suggested_name: &suggested_name,
                    language_name: format_name,
                    default_extension: extension,
                },
                async {
                    let file_handle = rfd::AsyncFileDialog::new()
                        .set_title(crate::labels::context_menu_export_dialog_title(format_name))
                        .set_file_name(suggested_name.as_str())
                        .add_filter(format_name, &[extension])
                        .save_file()
                        .await;

                    file_handle.map(|handle| handle.path().to_path_buf())
                },
            )
            .await;

            let (target_path, used_fallback) = match outcome {
                SaveTargetOutcome::Selected {
                    path,
                    used_fallback,
                } => (path, used_fallback),
                SaveTargetOutcome::Cancelled => {
                    // Native dialog was available and the user cancelled — no
                    // toast, no audit. Cancellations are not failures.
                    return;
                }
                SaveTargetOutcome::Failed(err) => {
                    record_export_audit(
                        &audit_service,
                        format_name,
                        None,
                        true,
                        false,
                        Some(err.as_str()),
                    );
                    let message =
                        crate::labels::context_menu_export_dialog_fallback_failed_error(&err);
                    cx.update(|cx| {
                        entity.update(cx, |panel, cx| {
                            panel.pending.toast = Some(PendingToast {
                                message,
                                is_error: true,
                            });
                            cx.notify();
                        });
                    });
                    return;
                }
            };

            let export_result = (|| {
                let file = File::create(&target_path)?;
                let mut writer = BufWriter::new(file);
                dbflux_export::export_with_csv_options(&result, format, csv_options, &mut writer)?;
                Ok::<_, dbflux_export::ExportError>(())
            })();

            let path_text = target_path.display().to_string();
            let (message, is_error) = match (&export_result, scope_label.as_deref()) {
                (Ok(()), Some(scope)) if used_fallback => (
                    crate::labels::context_menu_export_native_picker_fallback_scope_toast(
                        scope, &path_text,
                    ),
                    false,
                ),
                (Ok(()), None) if used_fallback => (
                    crate::labels::context_menu_export_native_picker_fallback_toast(&path_text),
                    false,
                ),
                (Ok(()), Some(scope)) => (
                    crate::labels::context_menu_export_exported_scope_toast(scope, &path_text),
                    false,
                ),
                (Ok(()), None) => (
                    crate::labels::context_menu_export_exported_toast(&path_text),
                    false,
                ),
                (Err(e), _) => (
                    crate::labels::context_menu_export_failed_error(&e.to_string()),
                    true,
                ),
            };

            record_export_audit(
                &audit_service,
                format_name,
                Some(&target_path),
                is_error,
                used_fallback,
                export_result
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .as_deref(),
            );

            cx.update(|cx| {
                entity.update(cx, |panel, cx| {
                    panel.pending.toast = Some(PendingToast { message, is_error });
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn copy_to_clipboard_with_format(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.export_menu_open {
            self.close_export_menu(window, cx);
        }

        if matches!(format, ExportFormat::Binary) {
            self.pending.toast = Some(PendingToast {
                message: dbflux_i18n::t!(
                    "document.data.context_menu.clipboard.error.binary_unsupported"
                ),
                is_error: true,
            });
            cx.notify();
            return;
        }

        let (result, scope_label) = self.export_menu_payload(cx);
        let mut buffer: Vec<u8> = Vec::new();
        let export_result = dbflux_export::export_with_csv_options(
            &result,
            format,
            self.export_menu_csv_options(),
            &mut buffer,
        );
        let byte_len = buffer.len();
        let row_count = scope_label.is_some().then_some(result.rows.len());
        self.finish_clipboard_copy(export_result, buffer, format, row_count, cx);

        // The shared copy report words a row count; the menu states the rows
        // and columns it chose instead.
        if let Some(scope) = scope_label
            && let Some(toast) = self.pending.toast.as_mut()
            && !toast.is_error
        {
            toast.message = crate::labels::context_menu_clipboard_copied_scope_toast(
                &scope,
                format.name(),
                byte_len,
            );
        }
    }

    /// Context menu Copy as CSV / Copy as JSON: the selected cells through the
    /// same exporter as Copy loaded rows, so the two agree on quoting and value
    /// types.
    pub(super) fn copy_selection_with_format(
        &mut self,
        format: ExportFormat,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self.selection_result(cx) else {
            return;
        };
        let csv_options = dbflux_export::CsvOptions {
            protect_formulas: self
                .app_state
                .read(cx)
                .general_settings()
                .grid_copy_protect_formulas,
            ..dbflux_export::CsvOptions::default()
        };
        let mut buffer: Vec<u8> = Vec::new();
        let export_result =
            dbflux_export::export_with_csv_options(&selected, format, csv_options, &mut buffer);
        let row_count = Some(selected.rows.len());
        self.finish_clipboard_copy(export_result, buffer, format, row_count, cx);
    }

    /// The part of the result the grid selection covers: every row that holds
    /// a selected cell and every column that does, in the order the grid
    /// shows them. A cell in that grid but outside the selection is `Null`,
    /// where the plain copy leaves it empty.
    ///
    /// The selection names visual rows, so each is resolved through the edit
    /// buffer: a pending insert copies its values, and a base row copies the
    /// result's value or, for a cell with an unsaved edit, the edited one.
    fn selection_result(&self, cx: &App) -> Option<dbflux_core::QueryResult> {
        self.selection_values(cx, true)
    }

    /// [`Self::selection_result`], with every cell of the covered rows and
    /// columns as the grid shows it when `mask_unselected` is false (Copy as
    /// INSERT statements writes whole rows of the selected columns).
    fn selection_values(
        &self,
        cx: &App,
        mask_unselected: bool,
    ) -> Option<dbflux_core::QueryResult> {
        use dbflux_components::components::data_table::selection::CellCoord;

        let table_state = self.grid_table.table_state.as_ref()?;
        let state = table_state.read(cx);
        let selection = state.selection();
        let buffer = state.edit_buffer();
        let visual_order = buffer.compute_visual_order();
        let (rows, cols) =
            selection_grid(selection, visual_order.len(), self.result.columns.len())?;

        let columns = cols
            .iter()
            .filter_map(|&col| self.result.columns.get(col).cloned())
            .collect();
        let shown_value =
            |row: usize, col: usize| shown_value(&self.result, buffer, &visual_order, row, col);
        let rows = rows
            .iter()
            .map(|&row| {
                cols.iter()
                    .map(|&col| {
                        if !mask_unselected || selection.is_selected(CellCoord::new(row, col)) {
                            shown_value(row, col)
                        } else {
                            Value::Null
                        }
                    })
                    .collect()
            })
            .collect();

        Some(dbflux_core::QueryResult::table(
            columns,
            rows,
            None,
            std::time::Duration::ZERO,
        ))
    }

    /// For each row and column of `selected` (a [`Self::selection_values`]
    /// result), whether the cell is staged as a SQL expression.
    pub(in crate::data_grid_panel) fn selection_expressions(
        &self,
        cx: &App,
        selected: &dbflux_core::QueryResult,
    ) -> Vec<Vec<bool>> {
        use dbflux_components::components::data_table::model::{CellValue, VisualRowSource};

        let Some(table_state) = self.grid_table.table_state.as_ref() else {
            return Vec::new();
        };
        let state = table_state.read(cx);
        let buffer = state.edit_buffer();
        let visual_order = buffer.compute_visual_order();
        let Some((rows, cols)) = selection_grid(
            state.selection(),
            visual_order.len(),
            self.result.columns.len(),
        ) else {
            return Vec::new();
        };
        if rows.len() != selected.rows.len() || cols.len() != selected.columns.len() {
            return Vec::new();
        }

        rows.iter()
            .map(|&row| {
                cols.iter()
                    .map(|&col| match visual_order.get(row).copied() {
                        Some(VisualRowSource::Base(base_row)) => {
                            buffer.is_cell_dirty(base_row, col)
                                && buffer
                                    .get_cell(base_row, col, &CellValue::null())
                                    .is_expression()
                        }
                        Some(VisualRowSource::Insert(insert_idx)) => buffer
                            .get_pending_insert_by_idx(insert_idx)
                            .and_then(|cells| cells.get(col))
                            .is_some_and(|cell| cell.is_expression()),
                        None => false,
                    })
                    .collect()
            })
            .collect()
    }

    /// Every column of the given visual rows as the grid shows them, the way
    /// [`Self::selection_result`] reads its cells.
    pub(in crate::data_grid_panel) fn shown_row_values(
        &self,
        visual_rows: &[usize],
        cx: &App,
    ) -> Vec<Vec<Value>> {
        let Some(table_state) = self.grid_table.table_state.as_ref() else {
            return Vec::new();
        };
        let buffer = table_state.read(cx).edit_buffer();
        let visual_order = buffer.compute_visual_order();
        let col_count = self.result.columns.len();

        visual_rows
            .iter()
            .filter(|&&row| row < visual_order.len())
            .map(|&row| {
                (0..col_count)
                    .map(|col| shown_value(&self.result, buffer, &visual_order, row, col))
                    .collect()
            })
            .collect()
    }

    /// Writes an export to the clipboard and reports it: the toast, and the
    /// audit event for a copy that leaves the application.
    fn finish_clipboard_copy(
        &mut self,
        export_result: Result<(), dbflux_export::ExportError>,
        buffer: Vec<u8>,
        format: ExportFormat,
        row_count: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.finish_named_clipboard_copy(export_result, buffer, format.name(), row_count, cx);
    }

    /// `finish_clipboard_copy` for text built outside the exporter, so the
    /// copy extractors report and audit the same way.
    fn finish_clipboard_text(
        &mut self,
        text: String,
        format_name: &str,
        row_count: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.finish_named_clipboard_copy(Ok(()), text.into_bytes(), format_name, row_count, cx);
    }

    fn finish_named_clipboard_copy(
        &mut self,
        export_result: Result<(), dbflux_export::ExportError>,
        buffer: Vec<u8>,
        format_name: &str,
        row_count: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let audit_service = self.app_state.read(cx).audit_service().clone();

        match export_result {
            Ok(()) => match String::from_utf8(buffer) {
                Ok(text) => {
                    let byte_len = text.len();
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    record_clipboard_audit(&audit_service, format_name, Some(byte_len), None);
                    let message = match row_count {
                        Some(count) => crate::labels::context_menu_clipboard_copied_rows_toast(
                            count,
                            format_name,
                            byte_len,
                        ),
                        None => crate::labels::context_menu_clipboard_copied_toast(
                            format_name,
                            byte_len,
                        ),
                    };
                    self.pending.toast = Some(PendingToast {
                        message,
                        is_error: false,
                    });
                    cx.notify();
                }
                Err(e) => {
                    let err_text = e.to_string();
                    record_clipboard_audit(&audit_service, format_name, None, Some(&err_text));
                    self.pending.toast = Some(PendingToast {
                        message: crate::labels::context_menu_clipboard_non_utf8_error(&err_text),
                        is_error: true,
                    });
                    cx.notify();
                }
            },
            Err(e) => {
                let err_text = e.to_string();
                record_clipboard_audit(&audit_service, format_name, None, Some(&err_text));
                self.pending.toast = Some(PendingToast {
                    message: crate::labels::context_menu_clipboard_copy_failed_error(&err_text),
                    is_error: true,
                });
                cx.notify();
            }
        }
    }

    pub(super) fn export_base_name(&self) -> String {
        match &self.source {
            DataSource::Table { table, .. } => table.name.clone(),
            DataSource::Collection { collection, .. } => collection.name.clone(),
            DataSource::QueryResult { .. } => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("result_{}", timestamp)
            }
        }
    }

    pub(super) fn render_delete_confirm_modal(
        &self,
        _theme: &gpui_component::theme::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = self
            .pending_delete_confirm
            .as_ref()
            .map(|c| c.row_indices.len())
            .unwrap_or(1);

        let (title, description) = crate::labels::delete_confirm_copy(count);

        let footer = div()
            .flex()
            .gap(Spacing::SM)
            .child(
                Button::new(
                    "delete-cancel-btn",
                    dbflux_i18n::t!("document.data.context_menu.delete_confirm.cancel"),
                )
                .icon(AppIcon::X)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.cancel_delete(window, cx);
                })),
            )
            .child(
                Button::new(
                    "delete-confirm-btn",
                    dbflux_i18n::t!("document.data.context_menu.delete_confirm.delete"),
                )
                .danger()
                .icon(AppIcon::Delete)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.confirm_delete(window, cx);
                })),
            );

        Modal::new(title)
            .id("delete-modal-overlay")
            .danger()
            .icon(AppIcon::TriangleAlert)
            .width(px(420.0))
            .body(Text::body(description))
            .footer(footer)
    }

    pub(super) fn handle_context_menu_action(
        &mut self,
        action: ContextMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = match self.context_menu.take() {
            Some(m) => m,
            None => return,
        };

        let is_document_view = menu.is_document_view;
        let backend = self.filter_backend(cx);
        let has_row_target = self.has_context_menu_row_target(menu.row, menu.is_document_view, cx);

        if Self::context_menu_action_requires_row_target(action) && !has_row_target {
            self.restore_focus_after_context_menu(is_document_view, window, cx);
            cx.notify();
            return;
        }

        match action {
            ContextMenuAction::Copy => {
                if menu.is_document_view {
                    self.handle_copy_document(menu.row, cx);
                } else {
                    self.handle_copy(window, cx);
                }
            }
            ContextMenuAction::CopyAsCsv => {
                self.copy_selection_with_format(ExportFormat::Csv, window, cx)
            }
            ContextMenuAction::CopyAsJson => {
                self.copy_selection_with_format(ExportFormat::JsonPretty, window, cx)
            }
            ContextMenuAction::CopyAsSqlInList => self.copy_selection_as_in_list(cx),
            ContextMenuAction::CopyAsInsertStatements => self.copy_selection_as_inserts(cx),
            ContextMenuAction::CopyAsMarkdown => self.copy_selection_as_markdown(cx),
            ContextMenuAction::Paste => self.handle_paste(window, cx),
            ContextMenuAction::Edit => self.handle_edit(menu.row, menu.col, window, cx),
            ContextMenuAction::GenerateId(kind) => {
                self.handle_generate_id(menu.row, menu.col, kind, cx)
            }
            ContextMenuAction::EditAsExpression => {
                // The editor takes focus, so the focus restore below must run
                // before it opens rather than after.
                self.restore_focus_after_context_menu(is_document_view, window, cx);
                self.handle_edit_as_expression(menu.row, menu.col, window, cx);
                cx.notify();
                return;
            }
            ContextMenuAction::ViewValue => {
                self.request_value_panel(menu.row, menu.col, cx);
            }
            ContextMenuAction::AddToFilter => {
                self.add_selection_to_filter(false, window, cx);
            }
            ContextMenuAction::EditInModal => {
                if menu.is_document_view {
                    self.handle_view_document(menu.row, cx);
                } else {
                    self.handle_edit_in_modal(menu.row, menu.col, cx);
                }
            }
            ContextMenuAction::SetDefault => self.handle_set_default(menu.row, menu.col, cx),
            ContextMenuAction::UnsetField => self.handle_unset_field(menu.row, menu.col, cx),
            ContextMenuAction::RevertCell => self.handle_revert_cell(menu.row, menu.col, cx),
            ContextMenuAction::SetNull => self.handle_set_null(menu.row, menu.col, cx),
            ContextMenuAction::AddRow => self.handle_add_row(menu.row, is_document_view, cx),
            ContextMenuAction::DuplicateRow => {
                self.handle_duplicate_row(menu.row, is_document_view, cx)
            }
            ContextMenuAction::DeleteRow => {
                if menu.is_document_view {
                    self.pending_delete_confirm = Some(PendingDeleteConfirm {
                        row_indices: vec![menu.row],
                        is_table: false,
                    });
                    cx.notify();
                } else {
                    self.handle_delete_row(menu.row, cx);
                }
            }
            ContextMenuAction::GenerateSelectWhere => {
                self.handle_generate_sql(menu.row, SqlGenerateKind::SelectWhere, cx)
            }
            ContextMenuAction::GenerateInsert => {
                self.handle_generate_sql(menu.row, SqlGenerateKind::Insert, cx)
            }
            ContextMenuAction::GenerateUpdate => {
                self.handle_generate_sql(menu.row, SqlGenerateKind::Update, cx)
            }
            ContextMenuAction::GenerateDelete => {
                self.handle_generate_sql(menu.row, SqlGenerateKind::Delete, cx)
            }
            ContextMenuAction::CopyAsInsert
            | ContextMenuAction::CopyAsUpdate
            | ContextMenuAction::CopyAsDelete => {
                self.handle_copy_as_query(menu.row, action, cx);
            }
            ContextMenuAction::FilterByValue(op) => match backend {
                Some(FilterBackend::Mongo) => {
                    self.handle_mongo_filter_by_value(
                        &menu.doc_field_path,
                        &menu.doc_field_value,
                        op,
                        window,
                        cx,
                    );
                }
                _ => {
                    self.handle_filter_by_value(menu.row, menu.col, op, window, cx);
                }
            },
            ContextMenuAction::FilterIsNull => match backend {
                Some(FilterBackend::Mongo) => {
                    self.handle_mongo_filter_null(&menu.doc_field_path, false, window, cx);
                }
                _ => {
                    self.handle_filter_is_null(menu.col, false, window, cx);
                }
            },
            ContextMenuAction::FilterIsNotNull => match backend {
                Some(FilterBackend::Mongo) => {
                    self.handle_mongo_filter_null(&menu.doc_field_path, true, window, cx);
                }
                _ => {
                    self.handle_filter_is_null(menu.col, true, window, cx);
                }
            },
            ContextMenuAction::RemoveFilter => {
                self.handle_remove_filter(window, cx);
            }
            ContextMenuAction::Order(direction) => {
                self.handle_sort_request(menu.col, direction, cx);
            }
            ContextMenuAction::AddToSort(direction) => {
                self.handle_add_sort_key(menu.col, direction, cx);
            }
            ContextMenuAction::RemoveOrdering => {
                self.handle_sort_clear(cx);
            }
            ContextMenuAction::InspectRow => {
                self.open_row_inspector(menu.row, menu.col, cx);
            }
            ContextMenuAction::FindSimilarRows => {
                self.find_similar_rows_at(menu.row, menu.col, cx);
            }
            // The chooser takes the keyboard, so focus stays with it rather
            // than going back to the grid.
            ContextMenuAction::ChooseColumns => {
                self.open_column_chooser(menu.position, window, cx);
                return;
            }
            ContextMenuAction::FitColumnWidth
            | ContextMenuAction::HideColumn
            | ContextMenuAction::MoveColumnLeft
            | ContextMenuAction::MoveColumnRight
            | ContextMenuAction::PinColumn
            | ContextMenuAction::UnpinColumn => {
                self.handle_column_menu_action(action, menu.col, cx);
            }
            ContextMenuAction::ChartThisQuery => {
                let query = self.chart_host_current_query(cx);
                let connection_id = self.chart_host_connection_id(cx);

                if let Some(query) = query {
                    cx.emit(DataGridEvent::ChartThisQuery {
                        query,
                        connection_id,
                    });
                }
            }
        }

        // Restore focus to the active view after action
        self.restore_focus_after_context_menu(is_document_view, window, cx);
        cx.notify();
    }

    /// Build an `InspectorSnapshot` from the given row/col and emit
    /// `DataGridEvent::OpenInspector` so the workspace mounts the content.
    pub(super) fn open_row_inspector(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        use super::row_inspector::{
            InspectorCell, InspectorSnapshot, RowInspectorContent, column_type_label, row_key_label,
        };

        if self.collection.raw.is_some() && self.is_document_collection(cx) {
            self.open_document_inspector(row, col, cx);
            return;
        }

        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        let state = table_state.read(cx);
        let model = state.model();

        // The remembered row may now be out of bounds after a refresh shrinks
        // the result. Drop the cached state and hide the rail rather than
        // showing a phantom row of nulls.
        if row >= model.row_count() {
            self.inspector.follow_selection = false;
            self.inspector.pinned = false;
            self.inspector.inspector_row = None;
            self.inspector.row_inspector_content = None;
            self.inspector.incoming_references.cancel();
            self.inspector.outgoing_labels = None;
            cx.emit(DataGridEvent::CloseInspector);
            return;
        }

        let pk_cols: std::collections::HashSet<usize> =
            state.pk_columns().iter().copied().collect();
        let fk_cols = state.fk_columns().clone();
        let can_edit = state.is_editable() && !self.is_grouped_result();
        let referenced_tables = self.foreign_key_targets(cx);

        let cells: Vec<InspectorCell> = model
            .columns
            .iter()
            .enumerate()
            .map(|(ix, spec)| {
                let value = model
                    .cell(row, ix)
                    .map(|c| self.cell_to_value(c))
                    .unwrap_or(dbflux_core::Value::Null);
                let name = spec.title.to_string();
                let type_label = column_type_label(
                    &spec.type_name,
                    referenced_tables.get(&name).map(String::as_str),
                );

                InspectorCell {
                    name,
                    value,
                    type_label,
                    is_primary_key: pk_cols.contains(&ix),
                    is_foreign_key: fk_cols.contains(&ix),
                }
            })
            .collect();

        let table_name = match &self.source {
            DataSource::Table { table, .. } => Some(table.name.clone()),
            DataSource::Collection { .. } | DataSource::QueryResult { .. } => None,
        };

        let snapshot = InspectorSnapshot {
            row_number: row + 1,
            row_key: row_key_label(table_name.as_deref(), &cells),
            cells: cells.clone(),
            can_edit,
        };

        let outgoing_references = self.build_fk_references(&cells, cx);

        // Reuse the existing content entity or create a new one.
        let content = match &self.inspector.row_inspector_content {
            Some(existing) => {
                existing.update(cx, |c, cx| c.open(snapshot, cx));
                existing.clone()
            }
            None => {
                let new_content = cx.new(|cx| RowInspectorContent::new(snapshot, cx));
                self.inspector._row_inspector_subscription =
                    Some(cx.subscribe(&new_content, |this, _, event, cx| {
                        this.handle_row_inspector_event(*event, cx);
                    }));
                self.inspector.row_inspector_content = Some(new_content.clone());
                new_content
            }
        };

        let pinned = self.inspector.pinned;
        content.update(cx, |c, cx| {
            c.set_pinned(pinned, cx);
            c.set_outgoing_references(outgoing_references.clone(), cx);
        });

        self.load_outgoing_reference_labels(&outgoing_references, content.clone(), cx);
        self.load_incoming_references(&cells, content.clone(), cx);

        // Remember the active coordinates so refresh / tab activation /
        // selection navigation can rebuild the snapshot from fresh data.
        self.inspector.follow_selection = true;
        self.inspector.inspector_row = Some((row, col));

        // Tell the workspace to mount/replace the inspector rail. The row
        // inspector draws its own header, so the rail skips its title bar.
        let title = SharedString::from(crate::labels::row_inspector_title(row + 1));
        cx.emit(DataGridEvent::OpenInspector {
            title,
            content: AnyView::from(content),
            content_has_header: true,
        });
        cx.notify();
    }

    /// Carry out a request from the row inspector's buttons.
    ///
    /// Row actions need a `Window`, so they are queued for the next render;
    /// pin and close only touch the grid's inspector state.
    pub(in crate::data_grid_panel) fn handle_row_inspector_event(
        &mut self,
        event: super::row_inspector::RowInspectorContentEvent,
        cx: &mut Context<Self>,
    ) {
        use super::row_inspector::RowInspectorContentEvent;

        match event {
            RowInspectorContentEvent::Close => {
                self.clear_inspector_state(cx);
                cx.emit(DataGridEvent::CloseInspector);
            }
            RowInspectorContentEvent::TogglePin => {
                self.inspector.pinned = !self.inspector.pinned;
                let pinned = self.inspector.pinned;
                if let Some(content) = &self.inspector.row_inspector_content {
                    content.update(cx, |c, cx| c.set_pinned(pinned, cx));
                }
            }
            RowInspectorContentEvent::NavigateReference { index } => {
                let reference = self
                    .inspector
                    .row_inspector_content
                    .as_ref()
                    .and_then(|content| content.read(cx).reference(index).cloned());

                if let Some(target) = reference
                    .as_ref()
                    .and_then(|reference| self.reference_target(reference))
                {
                    self.navigate_to_reference(target, cx);
                }
            }
            RowInspectorContentEvent::Edit
            | RowInspectorContentEvent::Duplicate
            | RowInspectorContentEvent::Delete => {
                if let Some((row, col)) = self.inspector.inspector_row {
                    self.pending.row_inspector_action = Some((event, row, col));
                }
            }
        }

        cx.notify();
    }

    /// Run a row action queued by the row inspector's footer.
    pub(super) fn apply_row_inspector_action(
        &mut self,
        (event, row, col): (super::row_inspector::RowInspectorContentEvent, usize, usize),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use super::row_inspector::RowInspectorContentEvent;

        if !self.has_context_menu_row_target(row, false, cx) {
            return;
        }

        match event {
            RowInspectorContentEvent::Edit => self.handle_edit(row, col, window, cx),
            RowInspectorContentEvent::Duplicate => self.handle_duplicate_row(row, false, cx),
            RowInspectorContentEvent::Delete => self.handle_delete_row(row, cx),
            RowInspectorContentEvent::Close
            | RowInspectorContentEvent::TogglePin
            | RowInspectorContentEvent::NavigateReference { .. } => {}
        }
    }

    /// The table each single-column foreign key of the browsed table points
    /// at, keyed by its column, for the inspector's type labels.
    fn foreign_key_targets(&self, cx: &Context<Self>) -> std::collections::HashMap<String, String> {
        let Some(foreign_keys) = self
            .table_details_for(cx)
            .and_then(|table_info| table_info.foreign_keys.as_deref())
        else {
            return std::collections::HashMap::new();
        };

        foreign_keys
            .iter()
            .filter(|fk| fk.columns.len() == 1)
            .filter_map(|fk| {
                fk.columns
                    .first()
                    .map(|column| (column.clone(), fk.referenced_table.clone()))
            })
            .collect()
    }

    /// The outgoing references of the current row: one per single-column
    /// foreign key of the browsed table whose value is not null.
    fn build_fk_references(
        &self,
        cells: &[super::row_inspector::InspectorCell],
        cx: &Context<Self>,
    ) -> Vec<super::row_inspector::FkReference> {
        use super::row_inspector::{FkReference, ReferenceKind};

        let Some(table_info) = self.table_details_for(cx) else {
            return Vec::new();
        };

        let fk_list = match table_info.foreign_keys.as_deref() {
            Some(fks) if !fks.is_empty() => fks,
            _ => return Vec::new(),
        };

        let mut references = Vec::new();

        for fk in fk_list {
            let ([local_col], [ref_col]) =
                (fk.columns.as_slice(), fk.referenced_columns.as_slice())
            else {
                continue;
            };

            let Some(cell) = cells.iter().find(|c| &c.name == local_col) else {
                continue;
            };

            if cell.value.is_null() {
                continue;
            }

            references.push(FkReference {
                column: local_col.clone(),
                target_schema: fk.referenced_schema.clone(),
                target_table: fk.referenced_table.clone(),
                target_pk: ref_col.clone(),
                value: cell.value.clone(),
                // The grid resolves the same referenced rows for its FK
                // cells; a cache hit names the row without a query. The
                // referenced column is part of the cache identity, so two
                // foreign keys into one table never share a label.
                display_label: self.fk_display.cached_label(
                    fk.referenced_schema.as_deref(),
                    &fk.referenced_table,
                    ref_col,
                    &cell.value,
                ),
                kind: ReferenceKind::Outgoing,
            });
        }

        references
    }

    /// Where the browsed table lives. `None` for other sources.
    fn reference_lookup_target(&self, cx: &Context<Self>) -> Option<ReferenceLookupTarget> {
        let DataSource::Table {
            profile_id,
            database,
            table,
            ..
        } = &self.source
        else {
            return None;
        };

        let state = self.app_state.read(cx);
        let connected = state.connections().get(profile_id)?;
        let database = database
            .clone()
            .or_else(|| connected.active_database.clone())
            .unwrap_or_else(|| "default".to_string());
        let connection = connected.connection_for_database(&database);

        Some(ReferenceLookupTarget {
            profile_id: *profile_id,
            database,
            schema: table.schema.clone(),
            table: table.name.clone(),
            connection,
        })
    }

    /// Finds the tables whose foreign keys point at the inspected row and
    /// counts the rows of each that do, in the background, once the cursor
    /// rests on the row (see `IncomingReferencesLoader`). Foreign keys come
    /// from the schema's foreign-key cache, fetched once when missing; the
    /// counts go through the driver's `count_table`, one query per table and
    /// at most `MAX_CONCURRENT_REFERENCE_COUNTS` at a time.
    fn load_incoming_references(
        &mut self,
        cells: &[super::row_inspector::InspectorCell],
        content: Entity<super::row_inspector::RowInspectorContent>,
        cx: &mut Context<Self>,
    ) {
        let generation = content.read(cx).generation();

        let Some(ReferenceLookupTarget {
            profile_id,
            database,
            schema,
            table: table_name,
            connection,
        }) = self.reference_lookup_target(cx)
        else {
            self.inspector.incoming_references.cancel();
            content.update(cx, |content, cx| {
                content.add_incoming_references(generation, Vec::new(), cx);
            });
            return;
        };

        let values: std::collections::HashMap<String, Value> = cells
            .iter()
            .map(|cell| (cell.name.clone(), cell.value.clone()))
            .collect();
        let app_state = self.app_state.clone();

        let job = async move |cx: &mut AsyncApp| {
            let foreign_keys = match cached_or_fetched_schema_foreign_keys(
                &app_state,
                profile_id,
                &database,
                schema.as_deref(),
                cx,
            )
            .await
            {
                Ok(foreign_keys) => foreign_keys,
                Err(error) => {
                    log::debug!("row inspector could not read the schema's foreign keys: {error}");
                    Vec::new()
                }
            };

            let references = super::row_inspector::incoming_references(
                &foreign_keys,
                &table_name,
                schema.as_deref(),
                &values,
            );

            let first = cx.update(|cx| {
                content.update(cx, |content, cx| {
                    content.add_incoming_references(generation, references.clone(), cx)
                })
            });

            let Some(first) = first else {
                return;
            };

            super::row_inspector::count_incoming_references(
                &content,
                generation,
                first,
                &references,
                |reference, executor| {
                    let connection = connection.clone();
                    let request = dbflux_core::TableCountRequest::new(dbflux_core::TableRef {
                        schema: schema.clone(),
                        name: reference.target_table.clone(),
                    })
                    .with_database(Some(database.clone()))
                    .with_semantic_filter(
                        dbflux_core::SemanticFilter::compare(
                            reference.column.as_str(),
                            dbflux_core::WhereOperator::Eq,
                            reference.value.clone(),
                        ),
                    );

                    executor.spawn(async move {
                        connection
                            .count_table(&request)
                            .map_err(|error| error.to_string())
                    })
                },
                cx,
            )
            .await;
        };

        self.inspector.incoming_references.schedule(job, cx);
    }

    /// Resolves the readable label behind each outgoing REFERENCES entry
    /// the panel's FK display cache could not seed: the referenced row
    /// loads through `fetch_row_by_pk` in the background and is named by
    /// its table's display column (cached table details, fetched when
    /// missing). A row that no longer answers, or a failed fetch, keeps
    /// the raw id.
    ///
    /// The lookup is held in `inspector.outgoing_labels`: opening another
    /// row (or closing the inspector) drops it, cancelling its display
    /// column and label fetches before they start; the generation check in
    /// `resolve_outgoing_labels` only stops fetches that already began.
    fn load_outgoing_reference_labels(
        &mut self,
        references: &[super::row_inspector::FkReference],
        content: Entity<super::row_inspector::RowInspectorContent>,
        cx: &mut Context<Self>,
    ) {
        let is_unresolved = |reference: &super::row_inspector::FkReference| {
            matches!(
                reference.kind,
                super::row_inspector::ReferenceKind::Outgoing
            ) && reference.display_label.is_none()
        };
        if !references.iter().any(is_unresolved) {
            self.inspector.outgoing_labels = None;
            return;
        }

        let Some(ReferenceLookupTarget {
            profile_id,
            database,
            connection,
            ..
        }) = self.reference_lookup_target(cx)
        else {
            self.inspector.outgoing_labels = None;
            return;
        };

        let generation = content.read(cx).generation();
        let app_state = self.app_state.clone();
        let references: Vec<super::row_inspector::FkReference> = references.to_vec();

        let mut tables: Vec<(Option<String>, String)> = Vec::new();
        for reference in references
            .iter()
            .filter(|reference| is_unresolved(reference))
        {
            let table = (
                reference.target_schema.clone(),
                reference.target_table.clone(),
            );
            if !tables.contains(&table) {
                tables.push(table);
            }
        }

        let task = cx.spawn(async move |_, cx| {
            let display_columns = reference_display_columns(
                &app_state,
                profile_id,
                &database,
                &tables,
                &connection,
                cx,
            )
            .await;

            super::row_inspector::resolve_outgoing_labels(
                &content,
                generation,
                &references,
                &display_columns,
                |reference, executor| {
                    let connection = connection.clone();
                    let database = database.clone();
                    let schema = reference.target_schema.clone();
                    let table = reference.target_table.clone();
                    let pk_column = reference.target_pk.clone();
                    let value = reference.value.clone();

                    executor.spawn(async move {
                        connection
                            .fetch_row_by_pk(
                                &database,
                                schema.as_deref().unwrap_or(""),
                                &table,
                                &pk_column,
                                &value,
                            )
                            .map_err(|error| error.to_string())
                    })
                },
                cx,
            )
            .await;
        });

        self.inspector.outgoing_labels = Some(task);
    }

    /// Convert a `CellValue` to a `dbflux_core::Value` for the inspector.
    fn cell_to_value(
        &self,
        cell: &dbflux_components::components::data_table::model::CellValue,
    ) -> dbflux_core::Value {
        use dbflux_components::components::data_table::model::CellKind;

        match &cell.kind {
            CellKind::Null => dbflux_core::Value::Null,
            CellKind::Bool(b) => dbflux_core::Value::Bool(*b),
            CellKind::Int(i) => dbflux_core::Value::Int(*i),
            CellKind::Float(f) => dbflux_core::Value::Float(*f),
            CellKind::Text(s) | CellKind::Json(s) | CellKind::Expression(s) => {
                dbflux_core::Value::Text(s.to_string())
            }
            CellKind::Bytes(len) => dbflux_core::Value::Bytes(vec![0u8; *len]),
            CellKind::AutoGenerated(s) => dbflux_core::Value::Text(s.to_string()),
            CellKind::Unsupported(s) => dbflux_core::Value::Text(s.to_string()),
            CellKind::Missing => dbflux_core::Value::Null,
            CellKind::Nested { .. } => dbflux_core::Value::Text(cell.display_text().to_string()),
        }
    }

    pub(super) fn handle_copy(&self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(table_state) = &self.grid_table.table_state {
            let text = table_state.read(cx).copy_selection();
            if let Some(text) = text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    /// Copy entire document as JSON (for document view).
    pub(super) fn handle_copy_document(&self, doc_index: usize, cx: &mut Context<Self>) {
        let Some(tree_state) = &self.document_view.document_tree_state else {
            return;
        };

        if let Some(raw_doc) = tree_state.read(cx).get_raw_document(doc_index) {
            let json_value = value_to_json(raw_doc);
            if let Ok(json_str) = serde_json::to_string_pretty(&json_value) {
                cx.write_to_clipboard(ClipboardItem::new_string(json_str));
            }
        }
    }

    /// Open document preview modal for viewing/editing (for document view).
    pub(super) fn handle_view_document(&mut self, doc_index: usize, cx: &mut Context<Self>) {
        let Some(tree_state) = &self.document_view.document_tree_state else {
            return;
        };

        if let Some(raw_doc) = tree_state.read(cx).get_raw_document(doc_index) {
            let json_value = value_to_json(raw_doc);
            let json_str =
                serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| "{}".to_string());

            self.pending.document_preview = Some(PendingDocumentPreview {
                doc_index,
                document_json: json_str,
            });
            cx.notify();
        }
    }

    /// Copy entire row as TSV (tab-separated values).
    pub(super) fn handle_copy_row(&self, row: usize, cx: &mut Context<Self>) {
        use dbflux_components::components::data_table::model::VisualRowSource;

        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        let state = table_state.read(cx);
        let buffer = state.edit_buffer();
        let visual_order = buffer.compute_visual_order();

        // Get row data based on visual row source
        let row_values: Vec<String> = match visual_order.get(row).copied() {
            Some(VisualRowSource::Base(base_idx)) => self
                .result
                .rows
                .get(base_idx)
                .map(|r| {
                    r.iter()
                        .map(|val| {
                            dbflux_components::components::data_table::clipboard::format_cell(
                                &dbflux_components::components::data_table::model::CellValue::from(
                                    val,
                                ),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Some(VisualRowSource::Insert(insert_idx)) => buffer
                .get_pending_insert_by_idx(insert_idx)
                .map(|cells| {
                    cells
                        .iter()
                        .map(dbflux_components::components::data_table::clipboard::format_cell)
                        .collect()
                })
                .unwrap_or_default(),
            None => return,
        };

        if !row_values.is_empty() {
            let text = row_values.join("\t");
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn handle_paste(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        let clipboard_text = cx
            .read_from_clipboard()
            .and_then(|item| item.text().map(|s| s.to_string()));

        let Some(text) = clipboard_text else {
            return;
        };

        table_state.update(cx, |state, cx| {
            let Some(coord) = state.selection().active else {
                return;
            };

            state.stage_pasted_text(coord.row, coord.col, &text);

            cx.notify();
        });
    }

    pub(super) fn handle_edit(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                let coord =
                    dbflux_components::components::data_table::selection::CellCoord::new(row, col);
                state.start_editing(coord, window, cx);
            });
        }
    }

    pub(super) fn handle_edit_in_modal(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        use dbflux_components::components::data_table::model::{ColumnKind, VisualRowSource};

        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        let state = table_state.read(cx);
        if !state.is_editable() {
            return;
        }

        let is_json = state
            .model()
            .columns
            .get(col)
            .map(|c| c.kind == ColumnKind::Json)
            .unwrap_or(false);

        let visual_order = state.edit_buffer().compute_visual_order();
        let null_cell = dbflux_components::components::data_table::model::CellValue::null();

        let value = match visual_order.get(row).copied() {
            Some(VisualRowSource::Base(base_idx)) => {
                let base_cell = state.model().cell(base_idx, col);
                let base = base_cell.unwrap_or(&null_cell);
                let cell = state.edit_buffer().get_cell(base_idx, col, base);
                cell.edit_text()
            }
            Some(VisualRowSource::Insert(insert_idx)) => {
                if let Some(insert_data) = state.edit_buffer().get_pending_insert_by_idx(insert_idx)
                {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "col < insert_data.len() is checked directly above"
                    )]
                    if col < insert_data.len() {
                        insert_data[col].edit_text()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            }
            None => return,
        };

        self.pending.modal_open = Some(PendingModalOpen {
            row,
            col,
            value,
            is_json,
        });
        cx.notify();
    }

    pub(super) fn handle_set_default(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        // Get column default value from table details
        let default_value = self.get_column_default(col, cx);

        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        table_state.update(cx, |state, cx| {
            let cell_value = if let Some(default) = default_value {
                dbflux_components::components::data_table::model::CellValue::text(&default)
            } else {
                dbflux_components::components::data_table::model::CellValue::null()
            };

            state.stage_cell_value(row, col, cell_value);

            cx.notify();
        });
    }

    /// Stages the removal of a document field (`$unset` on commit).
    pub(super) fn handle_unset_field(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        table_state.update(cx, |state, cx| {
            state.stage_cell_value(
                row,
                col,
                dbflux_components::components::data_table::model::CellValue::missing(),
            );
            cx.notify();
        });
    }

    /// Drops the staged edit of one cell, restoring the loaded value.
    pub(super) fn handle_revert_cell(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        table_state.update(cx, |state, cx| {
            use dbflux_components::components::data_table::model::VisualRowSource;

            if let Some(VisualRowSource::Base(base_idx)) =
                state.edit_buffer().visual_row_source(row)
            {
                state.edit_buffer_mut().clear_cell(base_idx, col);
                cx.notify();
            }
        });
    }

    pub(super) fn handle_set_null(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        table_state.update(cx, |state, cx| {
            state.stage_cell_value(
                row,
                col,
                dbflux_components::components::data_table::model::CellValue::null(),
            );

            cx.notify();
        });
    }

    pub(super) fn handle_cell_editor_save(
        &mut self,
        row: usize,
        col: usize,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.write_cell_value(row, col, value, cx);
        self.focus_table(window, cx);
    }

    /// Write `value` into the edit buffer at the given visual coordinates.
    ///
    /// Split out of `handle_cell_editor_save` because the value panel saves
    /// without dismissing itself: pulling focus back to the table after every
    /// save would eject the user from the editor they are still typing in.
    pub(super) fn write_cell_value(
        &mut self,
        row: usize,
        col: usize,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        table_state.update(cx, |state, cx| {
            state.stage_cell_value(
                row,
                col,
                dbflux_components::components::data_table::model::CellValue::text(value),
            );

            cx.notify();
        });
    }

    pub(super) fn handle_document_preview_save(
        &mut self,
        doc_index: usize,
        document_json: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use dbflux_components::modals::document_preview::DOC_INDEX_NEW;

        let new_doc: serde_json::Value = match serde_json::from_str(document_json) {
            Ok(v) => v,
            Err(e) => {
                let toast_body = e.to_string();
                let title = dbflux_i18n::t!("document.data.context_menu.error.invalid_json");
                Toast::error(title.clone())
                    .meta_right(now_hms())
                    .body(toast_body.clone())
                    .action(copy_action(crate::labels::error_with_detail_clipboard(
                        &title,
                        &toast_body,
                    )))
                    .push(cx);
                return;
            }
        };

        let DataSource::Collection {
            profile_id,
            collection,
            ..
        } = &self.source
        else {
            return;
        };

        // Insert mode: opened via "Add Document" or "Duplicate Document".
        if doc_index == DOC_INDEX_NEW {
            let conn = self
                .app_state
                .read(cx)
                .connections()
                .get(profile_id)
                .map(|connected| connected.connection.clone());

            let Some(conn) = conn else {
                let message = dbflux_i18n::t!("document.data.grid.error.connection_not_available");
                Toast::error(message.clone())
                    .meta_right(now_hms())
                    .action(copy_action(message))
                    .push(cx);
                return;
            };

            let doc_map = match new_doc {
                serde_json::Value::Object(m) => m,
                _ => {
                    let message = dbflux_i18n::t!(
                        "document.data.context_menu.error.document_must_be_json_object"
                    );
                    Toast::error(message.clone())
                        .meta_right(now_hms())
                        .action(copy_action(message))
                        .push(cx);
                    return;
                }
            };

            let insert = DocumentInsert::one(collection.name.clone(), doc_map.into())
                .with_database(collection.database.clone());

            let entity = cx.entity().clone();
            let task = self.spawn_grid_call(
                self.interactive_work(*profile_id)
                    .described(format!("Insert into {}", collection.name)),
                async move { conn.insert_document(&insert) },
                cx,
            );

            cx.spawn(async move |_this, cx| {
                let result = task.await;

                cx.update(|cx| {
                    entity.update(cx, |panel, cx| {
                        match result {
                            Ok(mut crud_result) => {
                                crate::result_warnings::handoff_crud_returning_result(
                                    &mut crud_result,
                                    |warning| dbflux_ui_base::user_error::report_error(warning, cx),
                                );
                                panel.pending.toast = Some(PendingToast {
                                    message: dbflux_i18n::t!(
                                        "document.data.context_menu.document.toast.inserted"
                                    ),
                                    is_error: false,
                                });
                                panel.queue_reload_after_mutation(cx);
                            }
                            Err(e) => {
                                panel.pending.toast = Some(PendingToast {
                                    message:
                                        crate::labels::context_menu_document_insert_failed_error(
                                            &e.to_string(),
                                        ),
                                    is_error: true,
                                });
                            }
                        }
                        cx.notify();
                    });
                });
            })
            .detach();

            return;
        }

        // Update mode on a driver with field patches: the edited document is
        // diffed against the loaded copy and written as a minimal patch, with
        // the server-change check.
        if self.commits_document_patches(cx) {
            self.commit_document_preview_edit(doc_index, document_json, cx);
            return;
        }

        // Update mode: build filter from PK columns in result metadata
        let pk_columns = extract_pk_columns(&self.result);

        let filter = if pk_columns.is_empty() {
            // MongoDB fallback: use _id from the edited document
            match new_doc.get("_id") {
                Some(id) => DocumentFilter::new(serde_json::json!({"_id": id})),
                None => {
                    let message =
                        dbflux_i18n::t!("document.data.context_menu.error.document_missing_id");
                    Toast::error(message.clone())
                        .meta_right(now_hms())
                        .action(copy_action(message))
                        .push(cx);
                    return;
                }
            }
        } else {
            // Extract PK values from the current row
            let Some(table_state) = &self.grid_table.table_state else {
                let message =
                    dbflux_i18n::t!("document.data.context_menu.error.table_state_not_available");
                Toast::error(message.clone())
                    .meta_right(now_hms())
                    .action(copy_action(message))
                    .push(cx);
                return;
            };

            let state = table_state.read(cx);
            let model = state.model();
            let mut filter_obj = serde_json::Map::new();

            for (col_idx, col_name) in &pk_columns {
                if let Some(cell) = model.cell(doc_index, *col_idx) {
                    filter_obj.insert(col_name.clone(), value_to_json(&cell.to_value()));
                }
            }

            if filter_obj.is_empty() {
                let message =
                    dbflux_i18n::t!("document.data.context_menu.error.primary_key_not_determined");
                Toast::error(message.clone())
                    .meta_right(now_hms())
                    .action(copy_action(message))
                    .push(cx);
                return;
            }

            DocumentFilter::new(serde_json::Value::Object(filter_obj))
        };

        // Build $set update (skip PK fields)
        let pk_names: std::collections::HashSet<&str> =
            pk_columns.iter().map(|(_, name)| name.as_str()).collect();

        let mut set_fields = serde_json::Map::new();
        if let serde_json::Value::Object(doc_map) = &new_doc {
            for (key, value) in doc_map {
                if !pk_names.contains(key.as_str()) {
                    set_fields.insert(key.clone(), value.clone());
                }
            }
        }

        let update_doc = serde_json::json!({ "$set": set_fields });

        let conn = self
            .app_state
            .read(cx)
            .connections()
            .get(profile_id)
            .map(|connected| connected.connection.clone());

        let Some(conn) = conn else {
            let message = dbflux_i18n::t!("document.data.grid.error.connection_not_available");
            Toast::error(message.clone())
                .meta_right(now_hms())
                .action(copy_action(message))
                .push(cx);
            return;
        };

        let update = DocumentUpdate::new(collection.name.clone(), filter, update_doc)
            .with_database(collection.database.clone());

        let entity = cx.entity().clone();
        let task = self.spawn_grid_call(
            QueryWork::interactive(*profile_id).described(format!("Update {}", collection.name)),
            async move { conn.update_document(&update) },
            cx,
        );

        cx.spawn(async move |_this, cx| {
            let result = task.await;

            cx.update(|cx| {
                entity.update(cx, |panel, cx| {
                    match result {
                        Ok(mut crud_result) => {
                            crate::result_warnings::handoff_crud_returning_result(
                                &mut crud_result,
                                |warning| dbflux_ui_base::user_error::report_error(warning, cx),
                            );
                            panel.pending.toast = Some(PendingToast {
                                message: dbflux_i18n::t!(
                                    "document.data.context_menu.document.toast.updated"
                                ),
                                is_error: false,
                            });
                            panel.queue_reload_after_mutation(cx);
                        }
                        Err(e) => {
                            panel.pending.toast = Some(PendingToast {
                                message: crate::labels::context_menu_document_update_failed_error(
                                    &e.to_string(),
                                ),
                                is_error: true,
                            });
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn handle_add_row(
        &mut self,
        after_visual_row: usize,
        is_document_view: bool,
        cx: &mut Context<Self>,
    ) {
        use dbflux_components::components::data_table::model::VisualRowSource;
        use dbflux_components::modals::document_preview::DOC_INDEX_NEW;

        let is_table = matches!(self.source, DataSource::Table { .. });
        let is_collection = matches!(self.source, DataSource::Collection { .. });

        if !is_table && !is_collection {
            return;
        }

        // In document view, open the modal with a pre-seeded document so the user
        // can fill in all fields and confirm. The modal saves via DOC_INDEX_NEW,
        // which routes to insert_document instead of update_document.
        if is_document_view && is_collection {
            let new_doc = self.build_new_document_template();
            self.pending.document_preview = Some(PendingDocumentPreview {
                doc_index: DOC_INDEX_NEW,
                document_json: new_doc,
            });
            cx.notify();
            return;
        }

        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        let insert_after_base = {
            let state = table_state.read(cx);
            let buffer = state.edit_buffer();
            let visual_order = buffer.compute_visual_order();

            match visual_order.get(after_visual_row).copied() {
                Some(VisualRowSource::Base(base_idx)) => base_idx,
                Some(VisualRowSource::Insert(insert_idx)) => buffer
                    .pending_inserts()
                    .get(insert_idx)
                    .and_then(|pi| pi.insert_after())
                    .unwrap_or(self.result.rows.len().saturating_sub(1)),
                None => self.result.rows.len().saturating_sub(1),
            }
        };

        let new_row: Vec<dbflux_components::components::data_table::model::CellValue> =
            if is_collection {
                self.result
                    .columns
                    .iter()
                    .map(|col| {
                        if col.is_primary_key {
                            let new_id = self.generate_new_id_for_column(&col.name);
                            dbflux_components::components::data_table::model::CellValue::text(
                                &new_id,
                            )
                        } else {
                            dbflux_components::components::data_table::model::CellValue::null()
                        }
                    })
                    .collect()
            } else {
                let column_defaults = self.get_all_column_defaults(cx);
                self.result
                .columns
                .iter()
                .enumerate()
                .map(|(idx, _)| {
                    if let Some(default_expr) = column_defaults.get(idx).and_then(|d| d.as_ref()) {
                        dbflux_components::components::data_table::model::CellValue::auto_generated(
                            default_expr,
                        )
                    } else {
                        dbflux_components::components::data_table::model::CellValue::null()
                    }
                })
                .collect()
            };

        table_state.update(cx, |state, cx| {
            let buffer = state.edit_buffer_mut();
            buffer.set_base_row_count(self.result.rows.len());
            buffer.add_pending_insert_after(insert_after_base, new_row);
            cx.notify();
        });
    }

    /// Build an empty document JSON template pre-seeded with generated PK values.
    fn build_new_document_template(&self) -> String {
        let mut doc = serde_json::Map::new();

        for col in &self.result.columns {
            if col.is_primary_key {
                doc.insert(
                    col.name.clone(),
                    serde_json::Value::String(self.generate_new_id_for_column(&col.name)),
                );
            }
        }

        serde_json::to_string_pretty(&serde_json::Value::Object(doc))
            .unwrap_or_else(|_| "{}".to_string())
    }

    /// Generate a new ID: 24-char hex for `_id` (MongoDB ObjectId), full UUID otherwise.
    fn generate_new_id_for_column(&self, col_name: &str) -> String {
        if col_name == "_id" {
            uuid::Uuid::new_v4().to_string().replace("-", "")[..24].to_string()
        } else {
            uuid::Uuid::new_v4().to_string()
        }
    }

    pub(super) fn handle_duplicate_row(
        &mut self,
        visual_row: usize,
        is_document_view: bool,
        cx: &mut Context<Self>,
    ) {
        use dbflux_components::components::data_table::model::VisualRowSource;
        use dbflux_components::modals::document_preview::DOC_INDEX_NEW;

        let is_table = matches!(self.source, DataSource::Table { .. });
        let is_collection = matches!(self.source, DataSource::Collection { .. });

        if !is_table && !is_collection {
            return;
        }

        // In document view, open the modal with the source document pre-filled but
        // with a fresh PK so the user can review and confirm before inserting.
        if is_document_view && is_collection {
            if let Some(tree_state) = &self.document_view.document_tree_state
                && let Some(raw_doc) = tree_state.read(cx).get_raw_document(visual_row)
            {
                let mut doc_json = value_to_json(raw_doc);

                // Replace PK values with freshly generated IDs.
                if let serde_json::Value::Object(ref mut map) = doc_json {
                    for col in &self.result.columns {
                        if col.is_primary_key {
                            map.insert(
                                col.name.clone(),
                                serde_json::Value::String(
                                    self.generate_new_id_for_column(&col.name),
                                ),
                            );
                        }
                    }
                }

                let json_str =
                    serde_json::to_string_pretty(&doc_json).unwrap_or_else(|_| "{}".to_string());

                self.pending.document_preview = Some(PendingDocumentPreview {
                    doc_index: DOC_INDEX_NEW,
                    document_json: json_str,
                });
                cx.notify();
            }
            return;
        }

        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        // For collections and tables, find PK columns using is_primary_key metadata
        let pk_indices: std::collections::HashSet<usize> = self
            .result
            .columns
            .iter()
            .enumerate()
            .filter(|(_, col)| col.is_primary_key)
            .map(|(idx, _)| idx)
            .collect();

        let column_defaults = if is_table {
            self.get_all_column_defaults(cx)
        } else {
            vec![]
        };

        // Get source row data and determine insert position
        let base_row_count = self.result.rows.len();
        let state = table_state.read(cx);
        let buffer = state.edit_buffer();
        let visual_order = buffer.compute_visual_order();

        // Generate new ID helper: MongoDB-style for "_id", UUID for others
        let new_id_for_column = |col_name: &str| {
            if col_name == "_id" {
                uuid::Uuid::new_v4().to_string().replace("-", "")[..24].to_string()
            } else {
                uuid::Uuid::new_v4().to_string()
            }
        };

        let (source_values, insert_after_base): (
            Vec<dbflux_components::components::data_table::model::CellValue>,
            usize,
        ) = match visual_order.get(visual_row).copied() {
            Some(VisualRowSource::Base(base_idx)) => {
                let values = self
                    .result
                    .rows
                    .get(base_idx)
                    .map(|r| {
                        r.iter()
                            .enumerate()
                            .map(|(idx, val)| {
                                // Generate new ID for primary key columns
                                if pk_indices.contains(&idx) {
                                    let col_name = self.result.columns.get(idx).map(|c| c.name.as_str()).unwrap_or("");
                                    if is_table {
                                        // For tables, use default or null
                                        if let Some(default_expr) = column_defaults.get(idx).and_then(|d| d.as_ref()) {
                                            dbflux_components::components::data_table::model::CellValue::auto_generated(default_expr)
                                        } else {
                                            dbflux_components::components::data_table::model::CellValue::null()
                                        }
                                    } else {
                                        // For collections, generate new ID
                                        dbflux_components::components::data_table::model::CellValue::text(&new_id_for_column(col_name))
                                    }
                                } else {
                                    dbflux_components::components::data_table::model::CellValue::from(val)
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (values, base_idx)
            }
            Some(VisualRowSource::Insert(insert_idx)) => {
                let insert_after = buffer
                    .pending_inserts()
                    .get(insert_idx)
                    .and_then(|pi| pi.insert_after())
                    .unwrap_or(base_row_count.saturating_sub(1));

                let values = buffer
                    .get_pending_insert_by_idx(insert_idx)
                    .map(|insert_data| {
                        insert_data
                            .iter()
                            .enumerate()
                            .map(|(idx, val)| {
                                // Generate new ID for primary key columns
                                if pk_indices.contains(&idx) {
                                    let col_name = self.result.columns.get(idx).map(|c| c.name.as_str()).unwrap_or("");
                                    if is_table {
                                        // For tables, use default or null
                                        if let Some(default_expr) = column_defaults.get(idx).and_then(|d| d.as_ref()) {
                                            dbflux_components::components::data_table::model::CellValue::auto_generated(default_expr)
                                        } else {
                                            dbflux_components::components::data_table::model::CellValue::null()
                                        }
                                    } else {
                                        // For collections, generate new ID
                                        dbflux_components::components::data_table::model::CellValue::text(&new_id_for_column(col_name))
                                    }
                                } else {
                                    val.clone()
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (values, insert_after)
            }
            None => return,
        };

        if source_values.is_empty() {
            return;
        }

        table_state.update(cx, |state, cx| {
            let buffer = state.edit_buffer_mut();
            buffer.set_base_row_count(base_row_count);
            buffer.add_pending_insert_after(insert_after_base, source_values);
            cx.notify();
        });
    }

    /// Rows (visual) that hold a selected cell, ascending.
    pub(in crate::data_grid_panel) fn selected_visual_rows(&self, cx: &App) -> Vec<usize> {
        let Some(table_state) = &self.grid_table.table_state else {
            return Vec::new();
        };
        let state = table_state.read(cx);
        let row_count = state.edit_buffer().compute_visual_order().len();
        state.selection().selected_rows(row_count)
    }

    /// Marks every row with a selected cell for delete (the `−` button of
    /// the edit bar and the Delete key), or `fallback` when no cell is
    /// selected. Rows already marked stay as they are. Returns how many
    /// rows it went through.
    pub(in crate::data_grid_panel) fn delete_selected_rows(
        &mut self,
        fallback: Option<usize>,
        cx: &mut Context<Self>,
    ) -> usize {
        let mut rows = self.selected_visual_rows(cx);
        if rows.is_empty() {
            rows.extend(fallback);
        }

        // From the bottom up: dropping an unsaved row moves the rows below
        // it up by one, which would shift every later index.
        for &row in rows.iter().rev() {
            let already_marked = self.grid_table.table_state.as_ref().is_some_and(|state| {
                let buffer = state.read(cx).edit_buffer();
                matches!(
                    buffer.visual_row_source(row),
                    Some(dbflux_components::components::data_table::model::VisualRowSource::Base(base))
                        if buffer.is_pending_delete(base)
                )
            });
            if !already_marked {
                self.handle_delete_row(row, cx);
            }
        }
        rows.len()
    }

    pub(super) fn handle_delete_row(&mut self, row: usize, cx: &mut Context<Self>) {
        use dbflux_components::components::data_table::model::VisualRowSource;

        let is_table = matches!(self.source, DataSource::Table { .. });
        let is_collection = matches!(self.source, DataSource::Collection { .. });

        if !is_table && !is_collection {
            return;
        }

        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        let base_row_count = self.result.rows.len();

        let mut removed_insert = None;

        table_state.update(cx, |state, cx| {
            let buffer = state.edit_buffer_mut();
            buffer.set_base_row_count(base_row_count);

            let visual_order = buffer.compute_visual_order();

            match visual_order.get(row).copied() {
                Some(VisualRowSource::Base(base_idx)) => {
                    buffer.mark_for_delete(base_idx);
                }
                Some(VisualRowSource::Insert(insert_idx)) => {
                    buffer.remove_pending_insert_by_idx(insert_idx);
                    removed_insert = Some(insert_idx);
                }
                None => {}
            }

            cx.notify();
        });

        if let Some(insert_idx) = removed_insert {
            self.renumber_queued_inserts_after_removal(insert_idx);
        }
    }

    // === Filter / Order from context menu ===

    /// Resolves the original `Value` for a cell from the result set.
    pub(super) fn resolve_cell_value(
        &self,
        visual_row: usize,
        col: usize,
        cx: &App,
    ) -> Option<Value> {
        use dbflux_components::components::data_table::model::VisualRowSource;

        let table_state = self.grid_table.table_state.as_ref()?;
        let ts = table_state.read(cx);
        let buffer = ts.edit_buffer();
        let visual_order = buffer.compute_visual_order();

        match visual_order.get(visual_row).copied() {
            Some(VisualRowSource::Base(base_idx)) => self
                .result
                .rows
                .get(base_idx)
                .and_then(|r| r.get(col).cloned()),
            Some(VisualRowSource::Insert(insert_idx)) => buffer
                .get_pending_insert_by_idx(insert_idx)
                .and_then(|cells| cells.get(col).map(|c| self.cell_value_to_value(c))),
            None => None,
        }
    }

    /// Narrows the WHERE filter with `expr` and reloads. The current filter
    /// is kept as an `AND` term, in parentheses when it has a top-level `OR`;
    /// a condition it already holds is not added again and reads nothing.
    pub(in crate::data_grid_panel) fn apply_filter_expression(
        &mut self,
        expr: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.filter_bar.filter_input.read(cx).value().to_string();

        let Some(new_filter) = super::filter_bar::and_filter(&current, expr) else {
            return;
        };

        self.replace_filter_and_reload(&new_filter, window, cx);
    }

    fn handle_filter_by_value(
        &mut self,
        visual_row: usize,
        col: usize,
        operator: FilterOperator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(expr) = self.sql_value_filter_expr(visual_row, col, operator, cx) {
            self.apply_filter_expression(&expr, window, cx);
        }
    }

    /// The SQL condition comparing column `col` with the value of the cell
    /// at `visual_row`, in the connection's dialect.
    fn sql_value_filter_expr(
        &self,
        visual_row: usize,
        col: usize,
        operator: FilterOperator,
        cx: &App,
    ) -> Option<String> {
        let profile_id = match &self.source {
            DataSource::Table { profile_id, .. } => *profile_id,
            _ => return None,
        };

        let conn = self
            .app_state
            .read(cx)
            .connections()
            .get(&profile_id)
            .map(|c| c.connection.clone())?;
        let dialect = conn.dialect();

        let column = self.result.columns.get(col)?;
        let col_name = dialect.quote_identifier(&column.name);
        let col_type_name = column.type_name.to_ascii_lowercase();

        let cell_value = self.resolve_cell_value(visual_row, col, cx)?;

        let literal = dialect.value_to_literal(&cell_value);

        let op_str = Self::sql_operator_symbol(operator);

        let comparable_column = dialect.comparison_column_expr(&col_name, &col_type_name);

        let expr = if operator == FilterOperator::Like {
            let raw = match &cell_value {
                Value::Text(text) => text.clone(),
                Value::ObjectId(id) => id.clone(),
                _ => return None,
            };

            like_contains_filter(dialect, &comparable_column, &raw)
        } else if matches!(cell_value, Value::Json(_)) {
            dialect.json_filter_expr(&col_name, op_str, &literal, &col_type_name)
        } else {
            format!("{} {} {}", comparable_column, op_str, literal)
        };

        Some(expr)
    }

    fn handle_filter_is_null(
        &mut self,
        col: usize,
        is_not_null: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(expr) = self.sql_is_null_expr(col, is_not_null, cx) {
            self.apply_filter_expression(&expr, window, cx);
        }
    }

    /// `col IS NULL` (or `IS NOT NULL`) in the connection's dialect.
    fn sql_is_null_expr(&self, col: usize, is_not_null: bool, cx: &App) -> Option<String> {
        let profile_id = match &self.source {
            DataSource::Table { profile_id, .. } => *profile_id,
            _ => return None,
        };

        let conn = self
            .app_state
            .read(cx)
            .connections()
            .get(&profile_id)
            .map(|c| c.connection.clone())?;
        let col_name = conn
            .dialect()
            .quote_identifier(&self.result.columns.get(col)?.name);

        Some(if is_not_null {
            format!("{} IS NOT NULL", col_name)
        } else {
            format!("{} IS NULL", col_name)
        })
    }

    fn handle_remove_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_filter_and_reload("", window, cx);
    }

    // === MongoDB filter handlers ===

    fn handle_mongo_filter_by_value(
        &mut self,
        field_path: &Option<Vec<String>>,
        field_value: &Option<dbflux_components::components::document_tree::NodeValue>,
        operator: FilterOperator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use dbflux_components::components::document_tree::NodeValue;

        let Some(path) = field_path else { return };
        if path.is_empty() {
            return;
        }
        let field_dot = path.join(".");

        let scalar = match field_value {
            Some(NodeValue::Scalar(v)) => v,
            _ => return,
        };

        let json_val = value_to_json(scalar);

        let filter_obj = match operator {
            FilterOperator::Eq => serde_json::json!({ &field_dot: json_val }),
            FilterOperator::NotEq => serde_json::json!({ &field_dot: { "$ne": json_val } }),
            FilterOperator::Gt => serde_json::json!({ &field_dot: { "$gt": json_val } }),
            FilterOperator::Gte => serde_json::json!({ &field_dot: { "$gte": json_val } }),
            FilterOperator::Lt => serde_json::json!({ &field_dot: { "$lt": json_val } }),
            FilterOperator::Lte => serde_json::json!({ &field_dot: { "$lte": json_val } }),
            FilterOperator::Like => return,
        };

        self.apply_mongo_filter(&filter_obj, window, cx);
    }

    fn handle_mongo_filter_null(
        &mut self,
        field_path: &Option<Vec<String>>,
        is_not_null: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = field_path else { return };
        if path.is_empty() {
            return;
        }
        let field_dot = path.join(".");

        let filter_obj = if is_not_null {
            serde_json::json!({
                "$and": [
                    { &field_dot: { "$ne": null } },
                    { &field_dot: { "$exists": true } }
                ]
            })
        } else {
            serde_json::json!({
                "$and": [
                    { &field_dot: null },
                    { &field_dot: { "$exists": true } }
                ]
            })
        };

        self.apply_mongo_filter(&filter_obj, window, cx);
    }

    fn apply_mongo_filter(
        &mut self,
        new_filter: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.filter_bar.filter_input.read(cx).value().to_string();
        let current_trimmed = current.trim();

        let composed = if current_trimmed.is_empty() {
            new_filter.clone()
        } else {
            match serde_json::from_str::<serde_json::Value>(current_trimmed) {
                Ok(existing) => Self::compose_mongo_and(&existing, new_filter),
                Err(_) => new_filter.clone(),
            }
        };

        let serialized = serde_json::to_string(&composed).unwrap_or_default();

        self.replace_filter_and_reload(&serialized, window, cx);
    }

    fn compose_mongo_and(
        existing: &serde_json::Value,
        new_clause: &serde_json::Value,
    ) -> serde_json::Value {
        if let Some(obj) = existing.as_object()
            && obj.len() == 1
            && let Some(existing_and) = obj.get("$and")
            && let Some(arr) = existing_and.as_array()
        {
            let mut clauses = arr.clone();
            clauses.push(new_clause.clone());
            return serde_json::json!({ "$and": clauses });
        }

        serde_json::json!({ "$and": [existing, new_clause] })
    }

    fn sanitize_for_label(s: &str) -> String {
        s.chars()
            .map(|c| if c.is_ascii_control() { ' ' } else { c })
            .collect()
    }

    fn truncate_for_label(s: &str, max_len: usize) -> String {
        if s.len() <= max_len {
            s.to_string()
        } else {
            let truncated: String = s.chars().take(max_len).collect();
            format!("{}...", truncated)
        }
    }

    fn value_display_preview(value: &Value) -> String {
        match value {
            Value::Null => "NULL".to_string(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Decimal(s) => s.clone(),
            Value::Text(s) => format!("'{}'", Self::sanitize_for_label(s)),
            Value::Json(s) => format!("'{}'", Self::sanitize_for_label(s)),
            Value::ObjectId(id) => format!("'{}'", id),
            Value::DateTime(dt) => format!("'{}'", dt.format("%Y-%m-%d %H:%M:%S")),
            Value::Date(d) => format!("'{}'", d.format("%Y-%m-%d")),
            Value::Time(t) => format!("'{}'", t.format("%H:%M:%S")),
            Value::Unsupported(type_name) => format!("UNSUPPORTED<{}>", type_name),
            Value::Bytes(b) => format!("[{} bytes]", b.len()),
            Value::Array(_) | Value::Document(_) => "'...'".to_string(),
        }
    }

    /// NULL, bytes, complex structures, NaN, and Infinity don't support value operators;
    /// only IS NULL / IS NOT NULL applies.
    fn is_value_filterable(value: &Value) -> bool {
        match value {
            Value::Null
            | Value::Bytes(_)
            | Value::Array(_)
            | Value::Document(_)
            | Value::Unsupported(_) => false,
            Value::Float(f) if f.is_nan() || f.is_infinite() => false,
            _ => true,
        }
    }

    pub(super) fn handle_generate_sql(
        &mut self,
        visual_row: usize,
        kind: SqlGenerateKind,
        cx: &mut Context<Self>,
    ) {
        use dbflux_components::components::data_table::model::VisualRowSource;
        use dbflux_components::{SqlGenerationType, SqlPreviewContext};

        let (profile_id, table_ref) = match &self.source {
            DataSource::Table {
                profile_id, table, ..
            } => (*profile_id, table.clone()),
            DataSource::Collection { .. } => return,
            DataSource::QueryResult { .. } => return,
        };

        let Some(table_state) = &self.grid_table.table_state else {
            return;
        };

        // Get column info including primary keys
        let columns_info = self
            .table_details_for(cx)
            .and_then(|table_info| table_info.columns.as_deref());

        let col_names: Vec<String> = self.result.columns.iter().map(|c| c.name.clone()).collect();
        let ts = table_state.read(cx);
        let buffer = ts.edit_buffer();
        let visual_order = buffer.compute_visual_order();

        let row_values: Vec<Value> = match visual_order.get(visual_row).copied() {
            Some(VisualRowSource::Base(base_idx)) => {
                self.result.rows.get(base_idx).cloned().unwrap_or_default()
            }
            Some(VisualRowSource::Insert(insert_idx)) => buffer
                .get_pending_insert_by_idx(insert_idx)
                .map(|cells| cells.iter().map(|c| self.cell_value_to_value(c)).collect())
                .unwrap_or_default(),
            None => return,
        };

        if row_values.is_empty() || col_names.len() != row_values.len() {
            return;
        }

        // Find primary key columns
        let pk_indices: Vec<usize> = if let Some(cols) = columns_info {
            col_names
                .iter()
                .enumerate()
                .filter_map(|(idx, name)| {
                    cols.iter()
                        .find(|c| c.name == *name && c.is_primary_key)
                        .map(|_| idx)
                })
                .collect()
        } else {
            vec![]
        };

        // Convert SqlGenerateKind to SqlGenerationType
        let generation_type = match kind {
            SqlGenerateKind::SelectWhere => SqlGenerationType::SelectWhere,
            SqlGenerateKind::Insert => SqlGenerationType::Insert,
            SqlGenerateKind::Update => SqlGenerationType::Update,
            SqlGenerateKind::Delete => SqlGenerationType::Delete,
        };

        let context = match generation_type {
            SqlGenerationType::Insert | SqlGenerationType::Update | SqlGenerationType::Delete => {
                let action = match generation_type {
                    SqlGenerationType::Insert => ContextMenuAction::CopyAsInsert,
                    SqlGenerationType::Update => ContextMenuAction::CopyAsUpdate,
                    SqlGenerationType::Delete => ContextMenuAction::CopyAsDelete,
                    _ => unreachable!(),
                };

                let Some(mutation) = self.build_sql_mutation(visual_row, &table_ref, action, cx)
                else {
                    return;
                };

                SqlPreviewContext::DataMutation {
                    profile_id,
                    mutation,
                }
            }
            SqlGenerationType::SelectWhere => SqlPreviewContext::DataTableRow {
                profile_id,
                schema_name: table_ref.schema.clone(),
                table_name: table_ref.name.clone(),
                column_names: col_names,
                row_values,
                pk_indices,
            },
            SqlGenerationType::SelectAll => return,
            SqlGenerationType::CreateTable
            | SqlGenerationType::Truncate
            | SqlGenerationType::DropTable => return,
        };

        cx.emit(DataGridEvent::RequestSqlPreview {
            context: Box::new(context),
            generation_type,
        });
    }

    // -- Copy as Query --

    fn copy_query_submenu_label(&self, cx: &App) -> String {
        let profile_id = match &self.source {
            DataSource::Table { profile_id, .. } => profile_id,
            DataSource::Collection { profile_id, .. } => profile_id,
            DataSource::QueryResult { .. } => {
                return crate::labels::copy_query_language_label(None);
            }
        };

        let language = self
            .app_state
            .read(cx)
            .connections()
            .get(profile_id)
            .map(|c| c.connection.metadata().query_language.clone());

        crate::labels::copy_query_language_label(language)
    }

    fn has_copy_query_support(&self) -> bool {
        self.offers_table_query_actions()
            && matches!(
                self.source,
                DataSource::Table { .. } | DataSource::Collection { .. }
            )
    }

    /// Returns true when a "Chart this query" context menu item should be shown.
    ///
    /// The item only makes sense when the panel has a non-empty original query (i.e., a
    /// `QueryResult` source) AND `detect_chart_columns` on the current result returns Ok.
    fn can_chart_from_context_menu(&self, _cx: &App) -> bool {
        let has_query = matches!(
            &self.source,
            DataSource::QueryResult { original_query, .. } if !original_query.is_empty()
        );

        if !has_query {
            return false;
        }

        matches!(
            detect_chart_columns(&self.result),
            dbflux_components::chart::ChartDetection::Ok { .. }
        )
    }

    pub(super) fn handle_copy_as_query(
        &mut self,
        visual_row: usize,
        action: ContextMenuAction,
        cx: &mut Context<Self>,
    ) {
        use dbflux_components::components::data_table::model::VisualRowSource;

        let profile_id = match &self.source {
            DataSource::Table { profile_id, .. } => *profile_id,
            DataSource::Collection { profile_id, .. } => *profile_id,
            DataSource::QueryResult { .. } => return,
        };

        let conn = self
            .app_state
            .read(cx)
            .connections()
            .get(&profile_id)
            .map(|c| c.connection.clone());

        let Some(conn) = conn else {
            return;
        };

        let Some(generator) = conn.query_generator() else {
            return;
        };

        let mutation = match &self.source {
            DataSource::Table { table, .. } => {
                self.build_sql_mutation(visual_row, table, action, cx)
            }
            DataSource::Collection { collection, .. } => {
                self.build_document_mutation(visual_row, collection, action, cx)
            }
            DataSource::QueryResult { .. } => None,
        };

        let Some(mutation) = mutation else {
            return;
        };

        if let Some(generated) = generator.generate_mutation(&mutation) {
            cx.write_to_clipboard(ClipboardItem::new_string(generated.text));
        }
    }

    fn build_sql_mutation(
        &self,
        visual_row: usize,
        table: &dbflux_core::TableRef,
        action: ContextMenuAction,
        cx: &App,
    ) -> Option<MutationRequest> {
        use dbflux_components::components::data_table::model::VisualRowSource;

        let table_state = self.grid_table.table_state.as_ref()?;
        let state = table_state.read(cx);
        let model = state.model();
        let buffer = state.edit_buffer();
        let visual_order = buffer.compute_visual_order();

        let col_names: Vec<String> = self.result.columns.iter().map(|c| c.name.clone()).collect();
        let col_types: Vec<String> = self
            .result
            .columns
            .iter()
            .map(|c| c.type_name.clone())
            .collect();

        let row_values: Vec<Value> = match visual_order.get(visual_row).copied() {
            Some(VisualRowSource::Base(base_idx)) => {
                self.result.rows.get(base_idx).cloned().unwrap_or_default()
            }
            Some(VisualRowSource::Insert(insert_idx)) => buffer
                .get_pending_insert_by_idx(insert_idx)
                .map(|cells| cells.iter().map(|c| self.cell_value_to_value(c)).collect())
                .unwrap_or_default(),
            None => return None,
        };

        if row_values.is_empty() || col_names.len() != row_values.len() {
            return None;
        }

        let has_unsupported = row_values
            .iter()
            .any(|value| matches!(value, Value::Unsupported(_)));

        let pk_indices = state.pk_columns();

        match action {
            ContextMenuAction::CopyAsInsert => {
                if has_unsupported {
                    return None;
                }

                let assignments: Vec<dbflux_core::ColumnAssignment> = col_names
                    .iter()
                    .zip(row_values.iter())
                    .zip(col_types.iter())
                    .map(|((name, value), type_name)| dbflux_core::ColumnAssignment {
                        name: name.clone(),
                        value: value.clone(),
                        type_name: Some(type_name.clone()),
                        is_expression: false,
                    })
                    .collect();

                let insert = RowInsert::with_typed_assignments(
                    table.name.clone(),
                    table.schema.clone(),
                    assignments,
                );
                Some(MutationRequest::SqlInsert(insert))
            }

            ContextMenuAction::CopyAsUpdate => {
                if has_unsupported {
                    return None;
                }

                if pk_indices.is_empty() {
                    return None;
                }

                let pk_columns: Vec<String> = pk_indices
                    .iter()
                    .filter_map(|&idx| model.columns.get(idx).map(|c| c.title.to_string()))
                    .collect();

                let pk_values: Vec<Value> = pk_indices
                    .iter()
                    .filter_map(|&idx| row_values.get(idx).cloned())
                    .collect();

                if pk_values
                    .iter()
                    .any(|value| matches!(value, Value::Unsupported(_)))
                {
                    return None;
                }

                let identity = RowIdentity::new(pk_columns, pk_values);

                let changes: Vec<dbflux_core::ColumnAssignment> = col_names
                    .into_iter()
                    .zip(row_values)
                    .zip(col_types)
                    .enumerate()
                    .filter(|(idx, _)| !pk_indices.contains(idx))
                    .map(
                        |(_, ((name, value), type_name))| dbflux_core::ColumnAssignment {
                            name,
                            value,
                            type_name: Some(type_name),
                            is_expression: false,
                        },
                    )
                    .collect();

                let patch = RowPatch::with_typed_changes(
                    identity,
                    table.name.clone(),
                    table.schema.clone(),
                    changes,
                );
                Some(MutationRequest::SqlUpdate(patch))
            }

            ContextMenuAction::CopyAsDelete => {
                if pk_indices.is_empty() {
                    return None;
                }

                let pk_columns: Vec<String> = pk_indices
                    .iter()
                    .filter_map(|&idx| model.columns.get(idx).map(|c| c.title.to_string()))
                    .collect();

                let pk_values: Vec<Value> = pk_indices
                    .iter()
                    .filter_map(|&idx| row_values.get(idx).cloned())
                    .collect();

                let identity = RowIdentity::new(pk_columns, pk_values);
                let delete = RowDelete::new(identity, table.name.clone(), table.schema.clone());
                Some(MutationRequest::SqlDelete(delete))
            }

            _ => None,
        }
    }

    /// Extracts primary key columns from result using `is_primary_key` flag.
    /// Falls back to `_id` if no PK columns are found.
    fn extract_pk_filter_for_document(&self, row_values: &[Value]) -> Option<DocumentFilter> {
        // Try to find PK columns from ColumnMeta
        let pk_columns: Vec<(usize, &str)> = self
            .result
            .columns
            .iter()
            .enumerate()
            .filter(|(_, col)| col.is_primary_key)
            .map(|(idx, col)| (idx, col.name.as_str()))
            .collect();

        if !pk_columns.is_empty() {
            // Build filter from PK columns
            let mut filter_obj = serde_json::Map::new();
            for (idx, col_name) in pk_columns {
                if let Some(value) = row_values.get(idx) {
                    let json_val = match value {
                        Value::ObjectId(oid) => serde_json::json!({"$oid": oid}),
                        Value::Text(s) => serde_json::json!(s),
                        Value::Int(i) => serde_json::json!(i),
                        Value::Float(f) => serde_json::json!(f),
                        Value::Bool(b) => serde_json::json!(b),
                        _ => continue,
                    };
                    filter_obj.insert(col_name.to_string(), json_val);
                }
            }

            if !filter_obj.is_empty() {
                return Some(DocumentFilter::new(serde_json::Value::Object(filter_obj)));
            }
        }

        // Fallback to _id column if no PK columns found
        let id_col_idx = self
            .result
            .columns
            .iter()
            .position(|c| c.name == "_id")
            .unwrap_or(0);

        let id_value = row_values.get(id_col_idx).cloned().unwrap_or(Value::Null);

        match &id_value {
            Value::ObjectId(oid) => Some(DocumentFilter::new(
                serde_json::json!({"_id": {"$oid": oid}}),
            )),
            Value::Text(s) => Some(DocumentFilter::new(serde_json::json!({"_id": s}))),
            _ => None,
        }
    }

    fn build_document_mutation(
        &self,
        visual_row: usize,
        collection: &dbflux_core::CollectionRef,
        action: ContextMenuAction,
        cx: &App,
    ) -> Option<MutationRequest> {
        use dbflux_components::components::data_table::model::VisualRowSource;

        let table_state = self.grid_table.table_state.as_ref()?;
        let state = table_state.read(cx);
        let buffer = state.edit_buffer();
        let visual_order = buffer.compute_visual_order();

        let row_values: Vec<Value> = match visual_order.get(visual_row).copied() {
            Some(VisualRowSource::Base(base_idx)) => {
                self.result.rows.get(base_idx).cloned().unwrap_or_default()
            }
            Some(VisualRowSource::Insert(insert_idx)) => buffer
                .get_pending_insert_by_idx(insert_idx)
                .map(|cells| cells.iter().map(|c| self.cell_value_to_value(c)).collect())
                .unwrap_or_default(),
            None => return None,
        };

        if row_values.is_empty() {
            return None;
        }

        let has_unsupported = row_values
            .iter()
            .any(|value| matches!(value, Value::Unsupported(_)));

        // Extract filter dynamically from PK columns or fallback to _id
        let filter = self.extract_pk_filter_for_document(&row_values)?;

        match action {
            ContextMenuAction::CopyAsInsert => {
                if has_unsupported {
                    return None;
                }

                let mut doc = serde_json::Map::new();
                for (col_idx, val) in row_values.iter().enumerate() {
                    if let Some(col) = self.result.columns.get(col_idx)
                        && !matches!(val, Value::Null)
                    {
                        doc.insert(col.name.clone(), value_to_json(val));
                    }
                }

                let insert = DocumentInsert::one(collection.name.clone(), doc.into())
                    .with_database(collection.database.clone());
                Some(MutationRequest::DocumentInsert(insert))
            }

            ContextMenuAction::CopyAsUpdate => {
                if has_unsupported {
                    return None;
                }

                // Get PK column indices to exclude from update
                let pk_indices: std::collections::HashSet<usize> = self
                    .result
                    .columns
                    .iter()
                    .enumerate()
                    .filter(|(_, col)| col.is_primary_key)
                    .map(|(idx, _)| idx)
                    .collect();

                // Fallback to _id if no PK columns
                let id_col_idx = if pk_indices.is_empty() {
                    Some(
                        self.result
                            .columns
                            .iter()
                            .position(|c| c.name == "_id")
                            .unwrap_or(0),
                    )
                } else {
                    None
                };

                let mut set_fields = serde_json::Map::new();
                for (col_idx, val) in row_values.iter().enumerate() {
                    // Skip PK columns or _id column
                    if pk_indices.contains(&col_idx) || Some(col_idx) == id_col_idx {
                        continue;
                    }
                    if let Some(col) = self.result.columns.get(col_idx) {
                        set_fields.insert(col.name.clone(), value_to_json(val));
                    }
                }

                let update_doc = serde_json::json!({"$set": set_fields});
                let update = DocumentUpdate::new(collection.name.clone(), filter, update_doc)
                    .with_database(collection.database.clone());
                Some(MutationRequest::DocumentUpdate(update))
            }

            ContextMenuAction::CopyAsDelete => {
                let delete = DocumentDelete::new(collection.name.clone(), filter)
                    .with_database(collection.database.clone());
                Some(MutationRequest::DocumentDelete(delete))
            }

            _ => None,
        }
    }

    pub(super) fn cell_value_to_value(
        &self,
        cell: &dbflux_components::components::data_table::model::CellValue,
    ) -> Value {
        use dbflux_components::components::data_table::model::CellKind;

        match &cell.kind {
            CellKind::Null => Value::Null,
            CellKind::Bool(b) => Value::Bool(*b),
            CellKind::Int(i) => Value::Int(*i),
            CellKind::Float(f) => Value::Float(*f),
            CellKind::Text(s) => Value::Text(s.to_string()),
            CellKind::Json(s) => Value::Json(s.to_string()),
            CellKind::Bytes(len) => Value::Bytes(vec![0u8; *len]),
            CellKind::Unsupported(type_name) => Value::Unsupported(type_name.to_string()),
            CellKind::AutoGenerated(expr) => Value::Text(format!("DEFAULT({})", expr)),
            CellKind::Expression(sql) => Value::Text(sql.to_string()),
            CellKind::Missing => Value::Null,
            CellKind::Nested { .. } => Value::Text(cell.display_text().to_string()),
        }
    }
}

/// `column LIKE '%raw%'` that matches `raw` literally.
///
/// The escape character is `!`, not a backslash: `ESCAPE '\\'` is an
/// unterminated string on MySQL's default `sql_mode` (and on PostgreSQL with
/// `standard_conforming_strings` off), and a bare backslash in the pattern is
/// already LIKE's default escape on MySQL and PostgreSQL. Declaring `!` makes
/// a backslash an ordinary character on every dialect.
fn like_contains_filter(dialect: &dyn dbflux_core::SqlDialect, column: &str, raw: &str) -> String {
    let needs_escape = raw.contains(['\\', '!', '%', '_']);

    if !needs_escape {
        let pattern = dialect.value_to_literal(&Value::Text(format!("%{raw}%")));
        return format!("{column} LIKE {pattern}");
    }

    let escaped = raw.replace('!', "!!").replace('%', "!%").replace('_', "!_");
    let pattern = dialect.value_to_literal(&Value::Text(format!("%{escaped}%")));
    format!("{column} LIKE {pattern} ESCAPE '!'")
}

pub(super) fn record_export_audit(
    audit_service: &dbflux_audit::AuditService,
    format_name: &str,
    path: Option<&std::path::Path>,
    is_error: bool,
    used_fallback: bool,
    error_text: Option<&str>,
) {
    use dbflux_core::chrono::Utc;
    use dbflux_core::observability::{
        EventCategory, EventOutcome, EventRecord, EventSeverity, EventSink,
    };

    let (severity, outcome) = match (is_error, used_fallback) {
        (true, _) => (EventSeverity::Error, EventOutcome::Failure),
        (false, true) => (EventSeverity::Warn, EventOutcome::Success),
        (false, false) => (EventSeverity::Info, EventOutcome::Success),
    };

    let action = if is_error {
        "result_export_failed"
    } else if used_fallback {
        "result_export_fallback"
    } else {
        "result_export"
    };

    let mut summary = format!("Result export ({})", format_name);
    if used_fallback && !is_error {
        summary.push_str(" — fallback directory used (no native file picker)");
    }
    if let Some(p) = path {
        summary.push_str(&format!(" -> {}", p.display()));
    }
    if let Some(err) = error_text {
        summary.push_str(&format!(": {}", err));
    }

    let event = EventRecord::new(
        Utc::now().timestamp_millis(),
        severity,
        EventCategory::Query,
        outcome,
    )
    .with_action(action.to_string())
    .with_summary(summary)
    .with_actor_id("ui:user");

    if let Err(e) = audit_service.record(event) {
        log::warn!("Failed to record export audit event: {}", e);
    }
}

pub(super) fn record_clipboard_audit(
    audit_service: &dbflux_audit::AuditService,
    format_name: &str,
    byte_len: Option<usize>,
    error_text: Option<&str>,
) {
    use dbflux_core::chrono::Utc;
    use dbflux_core::observability::{
        EventCategory, EventOutcome, EventRecord, EventSeverity, EventSink,
    };

    let is_error = error_text.is_some();
    let severity = if is_error {
        EventSeverity::Error
    } else {
        EventSeverity::Info
    };
    let outcome = if is_error {
        EventOutcome::Failure
    } else {
        EventOutcome::Success
    };

    let mut summary = if is_error {
        format!("Clipboard export failed ({})", format_name)
    } else {
        format!("Clipboard export ({})", format_name)
    };
    if let Some(len) = byte_len {
        summary.push_str(&format!(" — {} bytes", len));
    }
    if let Some(err) = error_text {
        summary.push_str(&format!(": {}", err));
    }

    let event = EventRecord::new(
        Utc::now().timestamp_millis(),
        severity,
        EventCategory::Query,
        outcome,
    )
    .with_action(if is_error {
        "result_clipboard_failed".to_string()
    } else {
        "result_clipboard".to_string()
    })
    .with_summary(summary)
    .with_actor_id("ui:user");

    if let Err(e) = audit_service.record(event) {
        log::warn!("Failed to record clipboard audit event: {}", e);
    }
}

/// The browsed table and the connection that reaches it, for the row
/// inspector's reference lookups.
struct ReferenceLookupTarget {
    profile_id: uuid::Uuid,
    database: String,
    schema: Option<String>,
    table: String,
    connection: std::sync::Arc<dyn dbflux_core::Connection>,
}

/// The foreign keys of one schema: from the connection's cache when it has
/// them, otherwise fetched on the background executor and cached for the
/// next reader.
async fn cached_or_fetched_schema_foreign_keys(
    app_state: &Entity<dbflux_ui_base::AppStateEntity>,
    profile_id: uuid::Uuid,
    database: &str,
    schema: Option<&str>,
    cx: &mut AsyncApp,
) -> Result<Vec<dbflux_core::SchemaForeignKeyInfo>, String> {
    let key = dbflux_core::SchemaCacheKey::new(database, schema);

    let cached = cx.update(|cx| {
        app_state
            .read(cx)
            .connections()
            .get(&profile_id)
            .and_then(|connected| connected.schema_foreign_keys.get(&key))
            .cloned()
    });

    if let Some(foreign_keys) = cached {
        return Ok(foreign_keys);
    }

    let params = cx
        .update(|cx| {
            app_state
                .read(cx)
                .prepare_fetch_schema_foreign_keys(profile_id, database, schema)
        })
        .map_err(|error| error.to_string())?;

    let work = QueryWork::background(profile_id).described(format!("Foreign keys: {database}"));
    let fetched = cx
        .update(|cx| {
            dbflux_ui_base::work_queue::spawn_query(
                app_state,
                work,
                async move { params.execute() },
                cx,
            )
        })
        .await
        .map_err(|_| dbflux_core::DbError::Cancelled.to_string())??;

    let foreign_keys = fetched.foreign_keys.clone();

    cx.update(|cx| {
        app_state.update(cx, |state, _| {
            state.set_schema_foreign_keys(
                fetched.profile_id,
                fetched.database,
                fetched.schema,
                fetched.foreign_keys,
            );
        });
    });

    Ok(foreign_keys)
}

/// The display column of each referenced table, from the connection's
/// cached details when it has them — keyed by the referenced schema first
/// and without one second, the shape MySQL writes — otherwise fetched on
/// the background executor and cached for the next reader. A table whose
/// details cannot be read has no display column and keeps naming its rows
/// by id.
async fn reference_display_columns(
    app_state: &Entity<dbflux_ui_base::AppStateEntity>,
    profile_id: uuid::Uuid,
    database: &str,
    tables: &[(Option<String>, String)],
    connection: &std::sync::Arc<dyn dbflux_core::Connection>,
    cx: &mut AsyncApp,
) -> std::collections::HashMap<(Option<String>, String), Option<String>> {
    let mut display_columns: std::collections::HashMap<(Option<String>, String), Option<String>> =
        std::collections::HashMap::new();

    for (schema, table) in tables {
        let cached = cx.update(|cx| {
            app_state
                .read(cx)
                .connections()
                .get(&profile_id)
                .and_then(|connected| {
                    connected
                        .table_details
                        .get(&(database.to_string(), schema.clone(), table.clone()))
                        .or_else(|| {
                            connected.table_details.get(&(
                                database.to_string(),
                                None,
                                table.clone(),
                            ))
                        })
                        .cloned()
                })
        });

        let column = match cached {
            Some(details) => super::fk_display::pick_display_column(
                details.columns.as_deref().unwrap_or(&[]),
                details.foreign_keys.as_deref().unwrap_or(&[]),
            ),
            None => {
                let request = {
                    let connection = connection.clone();
                    let database = database.to_string();
                    let schema = schema.clone();
                    let table = table.clone();
                    let description = format!("Table details: {table}");
                    dbflux_ui_base::work_queue::spawn_query_db_async(
                        app_state,
                        QueryWork::background(profile_id).described(description),
                        async move { connection.table_details(&database, schema.as_deref(), &table) },
                        cx,
                    )
                };

                match request.await {
                    Ok(details) => {
                        let column = super::fk_display::pick_display_column(
                            details.columns.as_deref().unwrap_or(&[]),
                            details.foreign_keys.as_deref().unwrap_or(&[]),
                        );
                        cx.update(|cx| {
                            app_state.update(cx, |state, _| {
                                state.set_table_details(
                                    profile_id,
                                    database.to_string(),
                                    schema.clone(),
                                    table.clone(),
                                    details,
                                );
                            });
                        });
                        column
                    }
                    Err(error) => {
                        log::debug!(
                            "row inspector: no column metadata for referenced table {table}: {error}"
                        );
                        None
                    }
                }
            }
        };

        display_columns.insert((schema.clone(), table.clone()), column);
    }

    display_columns
}

/// The visual rows (below `row_count`) and the columns (below `col_count`,
/// in the order the grid shows them, as the plain copy takes them) that
/// hold a selected cell; `None` with nothing selected.
fn selection_grid(
    selection: &dbflux_components::components::data_table::selection::SelectionState,
    row_count: usize,
    col_count: usize,
) -> Option<(Vec<usize>, Vec<usize>)> {
    use std::collections::BTreeSet;

    let ranges = selection.ranges();
    if ranges.is_empty() {
        return None;
    }
    let rows: BTreeSet<usize> = ranges
        .iter()
        .flat_map(|range| range.start.row..=range.end.row)
        .filter(|&row| row < row_count)
        .collect();
    let mut cols: Vec<usize> = ranges
        .iter()
        .flat_map(|range| range.start.col..=range.end.col)
        .filter(|&col| col < col_count)
        .collect::<BTreeSet<usize>>()
        .into_iter()
        .collect();
    selection.sort_by_screen_order(&mut cols);

    Some((rows.into_iter().collect(), cols))
}

/// The value the grid shows at visual `row`, `col`: a pending insert's
/// value, an unsaved edit, or the loaded value of `result`.
fn shown_value(
    result: &dbflux_core::QueryResult,
    buffer: &dbflux_components::components::data_table::model::EditBuffer,
    visual_order: &[dbflux_components::components::data_table::model::VisualRowSource],
    row: usize,
    col: usize,
) -> Value {
    use dbflux_components::components::data_table::model::{CellValue, VisualRowSource};

    match visual_order.get(row).copied() {
        Some(VisualRowSource::Base(base_row)) if buffer.is_cell_dirty(base_row, col) => {
            let null = CellValue::null();
            buffer.get_cell(base_row, col, &null).to_value()
        }
        Some(VisualRowSource::Base(base_row)) => result
            .rows
            .get(base_row)
            .and_then(|values| values.get(col))
            .cloned()
            .unwrap_or(Value::Null),
        Some(VisualRowSource::Insert(insert_idx)) => buffer
            .get_pending_insert_by_idx(insert_idx)
            .and_then(|cells| cells.get(col))
            .map(CellValue::to_value)
            .unwrap_or(Value::Null),
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests;
