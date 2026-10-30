//! The Rows, Columns and CSV options at the top of the export menu, and the
//! part of the result an export writes under them.
//!
//! The options are one choice per user, not per table: someone who exports
//! without a header or with semicolons does so everywhere. They live in the
//! UI state store and are read again each time the menu opens, so every grid
//! picks up a change made in another one.

use std::collections::HashMap;

use dbflux_app::keymap::Command;
use dbflux_components::components::data_table::selection::SelectionState;
use dbflux_components::composites::{MenuItem, menu_row};
use dbflux_components::icons::AppIcon;
use dbflux_components::primitives::{SegmentedControl, SegmentedItem};
use dbflux_core::{QueryResult, QueryResultShape, TransferColumn, Value};
use dbflux_export::{CsvDelimiter, CsvOptions, ExportFormat};
use dbflux_transfer::PageShape;
use gpui::prelude::FluentBuilder;
use gpui::*;
use serde::{Deserialize, Serialize};

use super::DataGridPanel;
use super::context_menu::ExportMenuEntry;

/// The `st_ui_state` key. The version lets a later format use a new key
/// instead of misreading old rows.
const STORE_KEY: &str = dbflux_storage::ui_state_keys::GRID_EXPORT_OPTIONS;

/// Which rows an export writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportRows {
    /// Every loaded row that holds a selected cell.
    Selected,
    /// Every loaded row, the grid's current page.
    #[default]
    Loaded,
    /// Every row of the table or collection, read again page by page.
    All,
}

impl ExportRows {
    const ORDER: [Self; 3] = [Self::Selected, Self::Loaded, Self::All];

    fn id(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::Loaded => "loaded",
            Self::All => "all",
        }
    }
}

/// Which columns an export writes, and in which order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportColumns {
    /// Every column in result order, hidden ones included.
    All,
    /// The shown columns in screen order: what the grid looks like.
    #[default]
    Visible,
    /// The shown columns that hold a selected cell, in screen order.
    Selected,
}

impl ExportColumns {
    const ORDER: [Self; 3] = [Self::All, Self::Visible, Self::Selected];

    fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Visible => "visible",
            Self::Selected => "selected",
        }
    }
}

/// [`CsvDelimiter`] as the store spells it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportDelimiter {
    #[default]
    Comma,
    Semicolon,
    Tab,
}

impl ExportDelimiter {
    const ORDER: [Self; 3] = [Self::Comma, Self::Semicolon, Self::Tab];

    fn id(self) -> &'static str {
        match self {
            Self::Comma => "comma",
            Self::Semicolon => "semicolon",
            Self::Tab => "tab",
        }
    }

    fn csv(self) -> CsvDelimiter {
        match self {
            Self::Comma => CsvDelimiter::Comma,
            Self::Semicolon => CsvDelimiter::Semicolon,
            Self::Tab => CsvDelimiter::Tab,
        }
    }
}

/// The user's export choices. A field missing from a stored row reads as its
/// default, so a later option does not invalidate what is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ExportOptions {
    pub(crate) rows: ExportRows,
    pub(crate) columns: ExportColumns,
    pub(crate) csv_header: bool,
    pub(crate) csv_delimiter: ExportDelimiter,
}

impl Default for ExportOptions {
    /// What the menu did before it had options: the loaded rows, a header
    /// row and commas. The columns follow the screen, as Copy does.
    fn default() -> Self {
        Self {
            rows: ExportRows::Loaded,
            columns: ExportColumns::Visible,
            csv_header: true,
            csv_delimiter: ExportDelimiter::Comma,
        }
    }
}

impl ExportOptions {
    pub(crate) fn csv(&self) -> CsvOptions {
        CsvOptions {
            include_header: self.csv_header,
            delimiter: self.csv_delimiter.csv(),
        }
    }
}

/// An option row of the export menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportOptionRow {
    Rows,
    Columns,
    CsvHeader,
    CsvDelimiter,
}

/// What the grid shows and has selected, as model indices, for resolving
/// the options without a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GridExportView {
    /// Visual rows (pending inserts at their place) holding a selected
    /// cell, ascending.
    pub(crate) selected_rows: Vec<usize>,
    /// Shown columns holding a selected cell, in screen order.
    pub(crate) selected_columns: Vec<usize>,
    /// Shown columns in screen order.
    pub(crate) visible_columns: Vec<usize>,
    /// Whether the grid can read every row again ("Export all rows").
    pub(crate) can_export_all: bool,
}

impl GridExportView {
    /// The view of a grid with `col_count` columns, every one shown and none
    /// selected.
    pub(crate) fn unselected(col_count: usize, can_export_all: bool) -> Self {
        Self {
            visible_columns: (0..col_count).collect(),
            can_export_all,
            ..Self::default()
        }
    }

    pub(crate) fn from_selection(
        selection: &SelectionState,
        visible_columns: &[usize],
        row_count: usize,
        col_count: usize,
        can_export_all: bool,
    ) -> Self {
        Self {
            selected_rows: selection.selected_rows(row_count),
            selected_columns: selection.selected_columns(col_count),
            visible_columns: visible_columns
                .iter()
                .copied()
                .filter(|col| *col < col_count)
                .collect(),
            can_export_all,
        }
    }

    pub(crate) fn rows_available(&self, rows: ExportRows) -> bool {
        match rows {
            ExportRows::Selected => !self.selected_rows.is_empty(),
            ExportRows::Loaded => true,
            ExportRows::All => self.can_export_all,
        }
    }

    pub(crate) fn columns_available(&self, columns: ExportColumns) -> bool {
        match columns {
            ExportColumns::Selected => !self.selected_columns.is_empty(),
            ExportColumns::All | ExportColumns::Visible => true,
        }
    }
}

/// The rows a choice resolves to: the chosen one, or the loaded rows when it
/// does not apply (nothing selected, or a grid that cannot read again). The
/// stored choice is kept, so it comes back once it applies again.
pub(crate) fn effective_rows(options: &ExportOptions, view: &GridExportView) -> ExportRows {
    if view.rows_available(options.rows) {
        options.rows
    } else {
        ExportRows::Loaded
    }
}

/// The columns a choice resolves to, falling back to the visible ones.
pub(crate) fn effective_columns(options: &ExportOptions, view: &GridExportView) -> ExportColumns {
    if view.columns_available(options.columns) {
        options.columns
    } else {
        ExportColumns::Visible
    }
}

/// What an export writes under the options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExportScope {
    pub(crate) rows: ExportRows,
    /// The loaded rows to write, in result order; `None` for all of them.
    pub(crate) row_indices: Option<Vec<usize>>,
    /// Model columns in the order they are written.
    pub(crate) columns: Vec<usize>,
}

impl ExportScope {
    pub(crate) fn resolve(
        options: &ExportOptions,
        view: &GridExportView,
        col_count: usize,
    ) -> Self {
        let rows = effective_rows(options, view);
        let columns = match effective_columns(options, view) {
            ExportColumns::All => (0..col_count).collect(),
            ExportColumns::Visible if view.visible_columns.is_empty() => (0..col_count).collect(),
            ExportColumns::Visible => view.visible_columns.clone(),
            ExportColumns::Selected => view.selected_columns.clone(),
        };
        let row_indices = (rows == ExportRows::Selected).then(|| view.selected_rows.clone());

        Self {
            rows,
            row_indices,
            columns,
        }
    }

    /// Whether the scope is the whole result in its own column order, so the
    /// result can be written as it is.
    fn is_whole(&self, col_count: usize) -> bool {
        self.row_indices.is_none()
            && self.columns.len() == col_count
            && self
                .columns
                .iter()
                .enumerate()
                .all(|(pos, col)| pos == *col)
    }

    /// The rows and columns of `result` the scope covers, every cell with its
    /// value. A table or document result only: other shapes have no grid.
    pub(crate) fn project(&self, result: &QueryResult) -> QueryResult {
        if self.is_whole(result.columns.len()) {
            return result.clone();
        }

        let columns = self
            .columns
            .iter()
            .filter_map(|col| result.columns.get(*col).cloned())
            .collect();
        let pick = |row: &Vec<Value>| -> Vec<Value> {
            self.columns
                .iter()
                .map(|col| row.get(*col).cloned().unwrap_or(Value::Null))
                .collect()
        };
        let rows = match &self.row_indices {
            Some(indices) => indices
                .iter()
                .filter_map(|row| result.rows.get(*row))
                .map(pick)
                .collect(),
            None => result.rows.iter().map(pick).collect(),
        };

        let mut projected = QueryResult::table(columns, rows, None, std::time::Duration::ZERO);
        projected.shape = result.shape.clone();
        projected
    }

    /// The names of the written columns, for a read that is run again.
    pub(crate) fn column_names(&self, result: &QueryResult) -> Vec<String> {
        self.columns
            .iter()
            .filter_map(|col| result.columns.get(*col))
            .map(|column| column.name.clone())
            .collect()
    }
}

/// The choice after `current` in `order` that `available` accepts, wrapping
/// around; `current` when no other one is.
fn next_choice<T: Copy + PartialEq>(order: &[T], current: T, available: impl Fn(T) -> bool) -> T {
    let start = order
        .iter()
        .position(|choice| *choice == current)
        .unwrap_or(0);

    order
        .iter()
        .cycle()
        .skip(start + 1)
        .take(order.len())
        .copied()
        .find(|choice| available(*choice))
        .unwrap_or(current)
}

/// Writes only the named columns of each page, in that order, for "Export
/// all rows" with a column choice. A name the read no longer has is left
/// out rather than written empty.
pub(super) struct ProjectColumns {
    inner: Box<dyn PageShape>,
    names: Vec<String>,
    /// Position of each written column in the inner shape's columns.
    positions: Vec<usize>,
}

impl ProjectColumns {
    pub(super) fn new(inner: Box<dyn PageShape>, names: Vec<String>) -> Self {
        Self {
            inner,
            names,
            positions: Vec::new(),
        }
    }
}

impl PageShape for ProjectColumns {
    fn columns(&mut self, first_page: &QueryResult) -> Vec<TransferColumn> {
        let columns = self.inner.columns(first_page);
        let by_name: HashMap<&str, usize> = columns
            .iter()
            .enumerate()
            .map(|(index, column)| (column.name.as_str(), index))
            .collect();
        self.positions = self
            .names
            .iter()
            .filter_map(|name| by_name.get(name.as_str()).copied())
            .collect();

        // None of the names left: write the read as it is rather than a file
        // of empty rows.
        if self.positions.is_empty() {
            self.positions = (0..columns.len()).collect();
        }

        self.positions
            .iter()
            .filter_map(|index| columns.get(*index).cloned())
            .collect()
    }

    fn rows(&mut self, page: QueryResult) -> Vec<Vec<Value>> {
        self.inner
            .rows(page)
            .into_iter()
            .map(|row| {
                self.positions
                    .iter()
                    .map(|index| row.get(*index).cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .collect()
    }

    fn dropped_columns(&self) -> Vec<String> {
        self.inner.dropped_columns()
    }

    fn writes_whole_documents(&self) -> bool {
        self.inner.writes_whole_documents()
    }
}

impl DataGridPanel {
    /// Whether the export menu offers the Rows and Columns options: the
    /// result is a grid of rows and columns.
    pub(super) fn offers_export_options(&self) -> bool {
        self.grid_table.table_state.is_some()
            && matches!(
                self.result.shape,
                QueryResultShape::Table | QueryResultShape::Json
            )
    }

    /// Whether the menu offers the CSV options: a CSV format is listed.
    fn offers_csv_options(&self) -> bool {
        dbflux_export::available_formats(&self.result.shape).contains(&ExportFormat::Csv)
    }

    /// The option rows the export menu starts with.
    pub(super) fn export_option_rows(&self) -> Vec<ExportOptionRow> {
        if !self.offers_export_options() {
            return Vec::new();
        }

        let mut rows = vec![ExportOptionRow::Rows, ExportOptionRow::Columns];
        if self.offers_csv_options() {
            rows.push(ExportOptionRow::CsvHeader);
            rows.push(ExportOptionRow::CsvDelimiter);
        }
        rows
    }

    /// Whether the format rows below the options write every row of the
    /// table. Needs no window: only the stored choice and the source count.
    pub(super) fn exports_all_rows(&self) -> bool {
        self.offers_export_options()
            && self.chrome.export_options.rows == ExportRows::All
            && self.can_export_all_rows()
    }

    /// The grid's selection and column layout, as the options read them.
    pub(super) fn grid_export_view(&self, cx: &App) -> GridExportView {
        let col_count = self.result.columns.len();
        let can_export_all = self.can_export_all_rows();

        let Some(table_state) = self.grid_table.table_state.as_ref() else {
            return GridExportView::unselected(col_count, can_export_all);
        };
        // The tree view draws documents, not the table's cells, so its
        // selection says nothing about rows and columns.
        if self.view_config.mode == crate::DataViewMode::Document {
            return GridExportView::unselected(col_count, can_export_all);
        }

        let state = table_state.read(cx);
        GridExportView::from_selection(
            state.selection(),
            state.column_layout().visible_columns(),
            state.row_count(),
            col_count,
            can_export_all,
        )
    }

    /// What an export from the menu writes right now.
    pub(super) fn export_scope(&self, cx: &App) -> ExportScope {
        ExportScope::resolve(
            &self.chrome.export_options,
            &self.grid_export_view(cx),
            self.result.columns.len(),
        )
    }

    /// The scope as the menu and the toasts word it, e.g. "12 selected
    /// rows, 4 columns". `row_count` is `None` while it is unknown (every
    /// row of a table, before the export ran).
    pub(super) fn export_scope_label(
        &self,
        scope: &ExportScope,
        row_count: Option<usize>,
    ) -> String {
        crate::labels::export_scope_label(scope.rows, row_count, Some(scope.columns.len()))
    }

    /// What Save and Copy write: the chosen rows and columns of the loaded
    /// result, and the scope for the toast. A text or binary payload has no
    /// rows or columns, so it is written whole and the toast names no scope.
    pub(super) fn export_menu_payload(&self, cx: &App) -> (QueryResult, Option<String>) {
        let is_payload = self.result.rows.is_empty()
            && (self.result.text_body.is_some() || self.result.raw_bytes.is_some());
        if is_payload {
            return (self.result.clone(), None);
        }

        let col_count = self.result.columns.len();
        let mut scope = if self.offers_export_options() {
            self.export_scope(cx)
        } else {
            ExportScope::resolve(
                &ExportOptions {
                    columns: ExportColumns::All,
                    ..ExportOptions::default()
                },
                &GridExportView::unselected(col_count, false),
                col_count,
            )
        };
        // Save and Copy write what is loaded; every row goes through
        // "Export all rows" instead.
        if scope.rows == ExportRows::All {
            scope.rows = ExportRows::Loaded;
        }

        // Selected rows are the grid's rows as it shows them, as Copy as CSV
        // takes them: unsaved edits applied and added rows at their place.
        let result = match &scope.row_indices {
            Some(visual_rows) => {
                let mut shown = QueryResult::table(
                    self.result.columns.clone(),
                    self.shown_row_values(visual_rows, cx),
                    None,
                    std::time::Duration::ZERO,
                );
                shown.shape = self.result.shape.clone();
                ExportScope {
                    row_indices: None,
                    ..scope.clone()
                }
                .project(&shown)
            }
            None => scope.project(&self.result),
        };
        let label = self.export_scope_label(&scope, Some(result.rows.len()));
        (result, Some(label))
    }

    /// The CSV options Save and Copy use: the menu's, where it shows them.
    pub(super) fn export_menu_csv_options(&self) -> CsvOptions {
        if self.offers_export_options() && self.offers_csv_options() {
            self.chrome.export_options.csv()
        } else {
            CsvOptions::default()
        }
    }

    /// Reads the stored options, keeping the current ones when the store has
    /// none or cannot be read.
    pub(super) fn load_export_options(&mut self, cx: &App) {
        let stored = self
            .app_state
            .read(cx)
            .storage_runtime()
            .ui_state()
            .get(STORE_KEY);

        match stored {
            Ok(Some(json)) => match serde_json::from_str::<ExportOptions>(&json) {
                Ok(options) => self.chrome.export_options = options,
                Err(error) => log::warn!("Ignoring the unreadable export options: {error}"),
            },
            Ok(None) => {}
            Err(error) => log::warn!("Could not read the export options: {error}"),
        }
    }

    fn save_export_options(&self, cx: &App) {
        let json = match serde_json::to_string(&self.chrome.export_options) {
            Ok(json) => json,
            Err(error) => {
                log::error!("Could not encode the export options: {error}");
                return;
            }
        };

        if let Err(error) = self
            .app_state
            .read(cx)
            .storage_runtime()
            .ui_state()
            .set(STORE_KEY, &json)
        {
            log::error!("Could not save the export options: {error}");
        }
    }

    fn update_export_options(
        &mut self,
        change: impl FnOnce(&mut ExportOptions),
        cx: &mut Context<Self>,
    ) {
        let before = self.chrome.export_options;
        change(&mut self.chrome.export_options);
        if self.chrome.export_options != before {
            self.save_export_options(cx);
        }

        // Switching to or from All rows changes the format rows under the
        // options, so a highlight on one of them may be past the end now.
        let entry_count = self.export_menu_entries().len();
        if self.chrome.export_menu_selected >= entry_count {
            self.chrome.export_menu_selected = entry_count.saturating_sub(1);
        }
        cx.notify();
    }

    pub(super) fn set_export_rows(&mut self, rows: ExportRows, cx: &mut Context<Self>) {
        if self.grid_export_view(cx).rows_available(rows) {
            self.update_export_options(|options| options.rows = rows, cx);
        }
    }

    pub(super) fn set_export_columns(&mut self, columns: ExportColumns, cx: &mut Context<Self>) {
        if self.grid_export_view(cx).columns_available(columns) {
            self.update_export_options(|options| options.columns = columns, cx);
        }
    }

    pub(super) fn set_export_delimiter(
        &mut self,
        delimiter: ExportDelimiter,
        cx: &mut Context<Self>,
    ) {
        self.update_export_options(|options| options.csv_delimiter = delimiter, cx);
    }

    /// Enter on an option row: the next choice that applies, or the other
    /// state of the header checkbox. The menu stays open.
    pub(super) fn activate_export_option(&mut self, row: ExportOptionRow, cx: &mut Context<Self>) {
        let view = self.grid_export_view(cx);
        let options = self.chrome.export_options;

        match row {
            ExportOptionRow::Rows => {
                let current = effective_rows(&options, &view);
                let next = next_choice(&ExportRows::ORDER, current, |rows| {
                    view.rows_available(rows)
                });
                self.update_export_options(|options| options.rows = next, cx);
            }
            ExportOptionRow::Columns => {
                let current = effective_columns(&options, &view);
                let next = next_choice(&ExportColumns::ORDER, current, |columns| {
                    view.columns_available(columns)
                });
                self.update_export_options(|options| options.columns = next, cx);
            }
            ExportOptionRow::CsvHeader => {
                self.update_export_options(|options| options.csv_header = !options.csv_header, cx);
            }
            ExportOptionRow::CsvDelimiter => {
                let next = next_choice(&ExportDelimiter::ORDER, options.csv_delimiter, |_| true);
                self.set_export_delimiter(next, cx);
            }
        }
    }

    /// The caption over the option rows: what the formats below write.
    pub(super) fn export_options_title(&self, cx: &App) -> String {
        let scope = self.export_scope(cx);
        let rows = match scope.rows {
            ExportRows::Selected => scope.row_indices.as_ref().map(Vec::len),
            ExportRows::Loaded => Some(self.result.rows.len()),
            ExportRows::All => None,
        };
        crate::labels::export_options_title(&self.export_scope_label(&scope, rows))
    }

    /// One option row: a label with a segmented choice, or the header
    /// checkbox. Every segment and the checkbox act on click, and Enter on
    /// the highlighted row steps the same choice.
    pub(super) fn render_export_option_row(
        &self,
        index: usize,
        row: ExportOptionRow,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = self.grid_export_view(cx);
        let options = self.chrome.export_options;
        let entity = cx.entity().downgrade();

        let hover = cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
            if this.chrome.export_menu_selected != index {
                this.chrome.export_menu_selected = index;
                cx.notify();
            }
        });

        match row {
            ExportOptionRow::CsvHeader => {
                let on = options.csv_header;
                let label = dbflux_i18n::t!("document.data.grid.export.options.header");
                let mut item = MenuItem::new(label.clone());
                if on {
                    item = item.icon(AppIcon::Check);
                }

                menu_row("export-option-header", &item, selected, cx)
                    .role(Role::MenuItemCheckBox)
                    .aria_label(label)
                    .aria_toggled(if on { Toggled::True } else { Toggled::False })
                    .on_mouse_move(hover)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.activate_export_option(ExportOptionRow::CsvHeader, cx);
                    }))
                    .into_any_element()
            }
            ExportOptionRow::Rows => {
                let active = effective_rows(&options, &view);
                let items = ExportRows::ORDER
                    .iter()
                    .filter(|rows| **rows != ExportRows::All || view.can_export_all)
                    .map(|rows| {
                        SegmentedItem::new(
                            rows.id(),
                            crate::labels::export_rows_choice_label(*rows),
                        )
                        .disabled(!view.rows_available(*rows))
                    })
                    .collect();
                let reason = (!view.rows_available(ExportRows::Selected))
                    .then(|| dbflux_i18n::t!("document.data.grid.export.options.no_selected_rows"));

                option_row(
                    "export-option-rows",
                    dbflux_i18n::t!("document.data.grid.export.options.rows"),
                    reason,
                    selected,
                    SegmentedControl::new(items, active.id(), move |id, _, cx| {
                        let Some(rows) = ExportRows::ORDER
                            .iter()
                            .find(|rows| rows.id() == id.as_ref())
                        else {
                            return;
                        };
                        let rows = *rows;
                        entity
                            .update(cx, |this, cx| this.set_export_rows(rows, cx))
                            .ok();
                    })
                    .group("export-rows")
                    .focused(selected),
                    cx,
                )
                .on_mouse_move(hover)
                .into_any_element()
            }
            ExportOptionRow::Columns => {
                let active = effective_columns(&options, &view);
                let items = ExportColumns::ORDER
                    .iter()
                    .map(|columns| {
                        SegmentedItem::new(
                            columns.id(),
                            crate::labels::export_columns_choice_label(*columns),
                        )
                        .disabled(!view.columns_available(*columns))
                    })
                    .collect();
                let reason = (!view.columns_available(ExportColumns::Selected)).then(|| {
                    dbflux_i18n::t!("document.data.grid.export.options.no_selected_columns")
                });

                option_row(
                    "export-option-columns",
                    dbflux_i18n::t!("document.data.grid.export.options.columns"),
                    reason,
                    selected,
                    SegmentedControl::new(items, active.id(), move |id, _, cx| {
                        let Some(columns) = ExportColumns::ORDER
                            .iter()
                            .find(|columns| columns.id() == id.as_ref())
                        else {
                            return;
                        };
                        let columns = *columns;
                        entity
                            .update(cx, |this, cx| this.set_export_columns(columns, cx))
                            .ok();
                    })
                    .group("export-columns")
                    .focused(selected),
                    cx,
                )
                .on_mouse_move(hover)
                .into_any_element()
            }
            ExportOptionRow::CsvDelimiter => {
                let items = ExportDelimiter::ORDER
                    .iter()
                    .map(|delimiter| {
                        SegmentedItem::new(
                            delimiter.id(),
                            crate::labels::export_delimiter_label(*delimiter),
                        )
                    })
                    .collect();

                option_row(
                    "export-option-delimiter",
                    dbflux_i18n::t!("document.data.grid.export.options.delimiter"),
                    None,
                    selected,
                    SegmentedControl::new(items, options.csv_delimiter.id(), move |id, _, cx| {
                        let Some(delimiter) = ExportDelimiter::ORDER
                            .iter()
                            .find(|delimiter| delimiter.id() == id.as_ref())
                        else {
                            return;
                        };
                        let delimiter = *delimiter;
                        entity
                            .update(cx, |this, cx| this.set_export_delimiter(delimiter, cx))
                            .ok();
                    })
                    .group("export-delimiter")
                    .focused(selected),
                    cx,
                )
                .on_mouse_move(hover)
                .into_any_element()
            }
        }
    }

    /// The menu keys on an option row. `None` when the key is not one an
    /// option row answers, so the menu handles it.
    pub(super) fn dispatch_export_option_command(
        &mut self,
        entry: ExportMenuEntry,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let ExportMenuEntry::Option(row) = entry else {
            return None;
        };

        match cmd {
            Command::MenuSelect | Command::Execute | Command::ExpandCollapse => {
                self.activate_export_option(row, cx);
                Some(true)
            }
            _ => None,
        }
    }
}

/// A menu row with a label, an optional muted reason under it, and a
/// segmented choice at its right edge. The row itself takes no click: the
/// segments do.
fn option_row(
    id: &'static str,
    label: String,
    reason: Option<String>,
    selected: bool,
    control: SegmentedControl,
    cx: &App,
) -> Stateful<Div> {
    let mut item = MenuItem::new(label);
    if let Some(reason) = reason {
        item = item.detail(reason);
    }

    menu_row(id, &item, selected, cx).child(div().flex_none().child(control))
}

#[cfg(test)]
mod tests {
    // Not a glob of `super`: it brings in `gpui::test`, which would shadow
    // the standard `#[test]`.
    use super::{
        ExportColumns, ExportDelimiter, ExportOptions, ExportRows, ExportScope, GridExportView,
        ProjectColumns, next_choice,
    };
    use dbflux_components::components::data_table::ColumnLayout;
    use dbflux_components::components::data_table::selection::CellCoord;
    use dbflux_components::components::data_table::selection::SelectionState;
    use dbflux_core::{ColumnKind, ColumnMeta};
    use dbflux_core::{QueryResult, TransferColumn, Value};
    use dbflux_export::{CsvOptions, ExportFormat};
    use dbflux_transfer::PageShape;
    use std::sync::Arc;
    use std::time::Duration;

    fn column(name: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.to_string(),
            type_name: "text".to_string(),
            kind: ColumnKind::Text,
            nullable: true,
            is_primary_key: false,
        }
    }

    /// Columns a, b, c, d; row `n` holds `"<column><n>"`.
    fn result(rows: usize) -> QueryResult {
        let names = ["a", "b", "c", "d"];
        QueryResult::table(
            names.iter().map(|name| column(name)).collect(),
            (0..rows)
                .map(|row| {
                    names
                        .iter()
                        .map(|name| Value::Text(format!("{name}{row}")))
                        .collect()
                })
                .collect(),
            None,
            Duration::ZERO,
        )
    }

    fn csv(result: &QueryResult, options: CsvOptions) -> String {
        let mut buffer = Vec::new();
        dbflux_export::export_with_csv_options(result, ExportFormat::Csv, options, &mut buffer)
            .expect("CSV export");
        String::from_utf8(buffer).expect("UTF-8")
    }

    /// Screen shows d, a, c and hides b: model columns 3, 0, 2.
    fn reordered_layout() -> ColumnLayout {
        ColumnLayout::from_spec(
            &dbflux_components::components::data_table::ColumnLayoutSpec {
                order: vec!["d".into(), "a".into(), "b".into(), "c".into()],
                hidden: vec!["b".into()],
                pinned: Vec::new(),
            },
            &["a", "b", "c", "d"].map(Arc::<str>::from),
        )
    }

    #[test]
    fn selected_rows_with_selected_columns_write_exactly_those_cells_in_screen_order() {
        let layout = reordered_layout();
        assert_eq!(layout.visible_columns(), &[3, 0, 2]);

        let mut selection = SelectionState::new();
        selection.set_column_layout(Some(Arc::new(layout.clone())));
        // Rows 1..=2 from screen column d to screen column a.
        selection.select_cell(CellCoord::new(1, 3));
        selection.extend_to(CellCoord::new(2, 0));

        let view =
            GridExportView::from_selection(&selection, layout.visible_columns(), 6, 4, false);
        let options = ExportOptions {
            rows: ExportRows::Selected,
            columns: ExportColumns::Selected,
            ..ExportOptions::default()
        };
        let scope = ExportScope::resolve(&options, &view, 4);
        assert_eq!(scope.rows, ExportRows::Selected);
        assert_eq!(scope.row_indices.as_deref(), Some([1, 2].as_slice()));
        assert_eq!(scope.columns, [3, 0]);

        let text = csv(&scope.project(&result(6)), CsvOptions::default());
        assert_eq!(text.lines().collect::<Vec<_>>(), ["d,a", "d1,a1", "d2,a2"]);

        // A Ctrl+click range adds its row and column; the export writes the
        // whole grid they span, with values, not just the picked cells.
        selection.toggle_cell(CellCoord::new(4, 2));
        let view =
            GridExportView::from_selection(&selection, layout.visible_columns(), 6, 4, false);
        let scope = ExportScope::resolve(&options, &view, 4);
        let text = csv(&scope.project(&result(6)), CsvOptions::default());
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            ["d,a,c", "d1,a1,c1", "d2,a2,c2", "d4,a4,c4"]
        );
    }

    #[test]
    fn visible_columns_leave_hidden_ones_out_and_all_columns_keep_result_order() {
        let layout = reordered_layout();
        let view = GridExportView::from_selection(
            &SelectionState::new(),
            layout.visible_columns(),
            2,
            4,
            false,
        );

        let visible = ExportScope::resolve(&ExportOptions::default(), &view, 4);
        assert_eq!(visible.rows, ExportRows::Loaded);
        assert_eq!(visible.row_indices, None);
        let text = csv(&visible.project(&result(2)), CsvOptions::default());
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            ["d,a,c", "d0,a0,c0", "d1,a1,c1"]
        );
        assert!(
            !text.contains('b'),
            "the hidden column is not written: {text}"
        );

        let all = ExportScope::resolve(
            &ExportOptions {
                columns: ExportColumns::All,
                ..ExportOptions::default()
            },
            &view,
            4,
        );
        assert_eq!(all.columns, [0, 1, 2, 3]);
        assert_eq!(
            csv(&all.project(&result(2)), CsvOptions::default()),
            csv(&result(2), CsvOptions::default())
        );
    }

    #[test]
    fn csv_without_header_writes_only_the_rows() {
        let view = GridExportView::unselected(4, false);
        let options = ExportOptions {
            csv_header: false,
            csv_delimiter: ExportDelimiter::Semicolon,
            ..ExportOptions::default()
        };
        let scope = ExportScope::resolve(&options, &view, 4);

        let text = csv(&scope.project(&result(2)), options.csv());
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            ["a0;b0;c0;d0", "a1;b1;c1;d1"]
        );
    }

    #[test]
    fn selected_rows_and_columns_are_unavailable_without_a_selection() {
        let view = GridExportView::unselected(4, false);
        assert!(!view.rows_available(ExportRows::Selected));
        assert!(!view.columns_available(ExportColumns::Selected));
        assert!(!view.rows_available(ExportRows::All), "a query result");

        let options = ExportOptions {
            rows: ExportRows::Selected,
            columns: ExportColumns::Selected,
            ..ExportOptions::default()
        };
        let scope = ExportScope::resolve(&options, &view, 4);
        assert_eq!(
            scope.rows,
            ExportRows::Loaded,
            "falls back to the loaded rows"
        );
        assert_eq!(
            scope.columns,
            [0, 1, 2, 3],
            "falls back to the visible columns"
        );
    }

    #[test]
    fn enter_steps_to_the_next_choice_that_applies() {
        let order = ExportRows::ORDER;
        let none_selected = |rows| rows != ExportRows::Selected && rows != ExportRows::All;
        assert_eq!(
            next_choice(&order, ExportRows::Loaded, none_selected),
            ExportRows::Loaded,
            "nothing else applies"
        );
        assert_eq!(
            next_choice(&order, ExportRows::Loaded, |_| true),
            ExportRows::All
        );
        assert_eq!(
            next_choice(&order, ExportRows::All, |_| true),
            ExportRows::Selected,
            "wraps"
        );
        assert_eq!(
            next_choice(&order, ExportRows::All, |rows| rows != ExportRows::Selected),
            ExportRows::Loaded,
            "skips the selection when there is none"
        );
    }

    #[test]
    fn stored_options_round_trip_and_missing_fields_read_as_defaults() {
        let options = ExportOptions {
            rows: ExportRows::Selected,
            columns: ExportColumns::All,
            csv_header: false,
            csv_delimiter: ExportDelimiter::Tab,
        };
        let json = serde_json::to_string(&options).expect("encodes");
        assert_eq!(
            json,
            r#"{"rows":"selected","columns":"all","csv_header":false,"csv_delimiter":"tab"}"#
        );
        assert_eq!(
            serde_json::from_str::<ExportOptions>(&json).ok(),
            Some(options)
        );

        let partial: ExportOptions =
            serde_json::from_str(r#"{"csv_header":false}"#).expect("decodes");
        assert_eq!(
            partial,
            ExportOptions {
                csv_header: false,
                ..ExportOptions::default()
            }
        );
    }

    /// A page shape that hands the first page's columns through.
    struct Passthrough;

    impl PageShape for Passthrough {
        fn columns(&mut self, first_page: &QueryResult) -> Vec<TransferColumn> {
            first_page
                .columns
                .iter()
                .map(dbflux_transfer::to_transfer_column)
                .collect()
        }

        fn rows(&mut self, page: QueryResult) -> Vec<Vec<Value>> {
            page.rows
        }

        fn dropped_columns(&self) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn streamed_pages_keep_the_named_columns_in_order() {
        let mut shape = ProjectColumns::new(
            Box::new(Passthrough),
            vec!["d".to_string(), "gone".to_string(), "a".to_string()],
        );

        let columns: Vec<String> = shape
            .columns(&result(1))
            .into_iter()
            .map(|column| column.name)
            .collect();
        assert_eq!(columns, ["d", "a"], "a name the read lacks is left out");
        assert_eq!(
            shape.rows(result(2)),
            vec![
                vec![Value::Text("d0".into()), Value::Text("a0".into())],
                vec![Value::Text("d1".into()), Value::Text("a1".into())],
            ]
        );
    }
}
