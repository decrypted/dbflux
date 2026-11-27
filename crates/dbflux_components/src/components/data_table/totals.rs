use std::cell::RefCell;
use std::sync::{Arc, Weak};

use super::model::{CellKind, CellValue, ColumnKind, EditBuffer, TableModel};
use super::selection::{CellCoord, CellRange, SelectionState};

/// Most decimals a sum or average is written with. Past this the figure is
/// float noise (`0.1 + 0.2`) rather than anything the cells hold.
const MAX_DECIMALS: usize = 10;

/// Aggregates over the cells the grid selection covers, as the grid shows
/// them (staged edits and pending inserts included).
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionTotals {
    /// Distinct selected cells; a cell inside two Ctrl+click ranges counts once.
    pub cells: usize,
    /// Selected cells holding NULL.
    pub nulls: usize,
    /// Sum, average, minimum and maximum of the numeric cells, if any.
    pub numeric: Option<NumericTotals>,
}

impl SelectionTotals {
    /// Selected cells that are neither NULL nor a number: they are counted
    /// but take no part in the sum.
    pub fn non_numeric(&self) -> usize {
        let numbers = self.numeric.as_ref().map_or(0, |numeric| numeric.count);
        self.cells.saturating_sub(self.nulls + numbers)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NumericTotals {
    pub count: usize,
    sum: f64,
    /// Exact sum while every number is an integer; `None` once one is not,
    /// or once the sum leaves `i128`.
    int_sum: Option<i128>,
    min: (f64, String),
    max: (f64, String),
    /// Most fractional digits any of the numbers was written with, so a sum
    /// of prices keeps its cents and a sum of integers has none.
    decimals: usize,
}

impl NumericTotals {
    pub fn sum_text(&self) -> String {
        match self.int_sum {
            Some(sum) => sum.to_string(),
            None => format_decimal(self.sum, self.decimals),
        }
    }

    pub fn average_text(&self) -> String {
        let average = self.sum / self.count as f64;
        format_decimal(average, (self.decimals.max(2) + 2).min(MAX_DECIMALS))
    }

    /// The smallest number, written as its cell writes it.
    pub fn min_text(&self) -> &str {
        &self.min.1
    }

    /// The largest number, written as its cell writes it.
    pub fn max_text(&self) -> &str {
        &self.max.1
    }

    fn new(value: f64, integer: Option<i128>, text: String) -> Self {
        Self {
            count: 1,
            sum: value,
            int_sum: integer,
            decimals: fractional_digits(&text),
            min: (value, text.clone()),
            max: (value, text),
        }
    }

    fn add(&mut self, value: f64, integer: Option<i128>, text: String) {
        self.count += 1;
        self.sum += value;
        self.int_sum = match (self.int_sum, integer) {
            (Some(sum), Some(integer)) => sum.checked_add(integer),
            _ => None,
        };
        self.decimals = self.decimals.max(fractional_digits(&text));
        if value < self.min.0 {
            self.min = (value, text.clone());
        }
        if value > self.max.0 {
            self.max = (value, text);
        }
    }
}

/// Totals for `selection`, or `None` when it covers fewer than two cells:
/// one cell's total is the cell itself.
///
/// Runs over the rows already loaded and never asks the database; the cost
/// is one pass over the selected cells.
pub(super) fn selection_totals(
    model: &TableModel,
    edit_buffer: &EditBuffer,
    selection: &SelectionState,
) -> Option<SelectionTotals> {
    let ranges = selection.ranges();
    match ranges.as_slice() {
        [] => return None,
        [only] if only.start == only.end => return None,
        _ => {}
    }

    let visual_order = edit_buffer.compute_visual_order();
    let row_count = visual_order.len();
    let col_count = model.col_count();
    if row_count == 0 || col_count == 0 {
        return None;
    }

    let ranges: Vec<CellRange> = ranges
        .into_iter()
        .filter_map(|range| clamp_range(range, row_count, col_count))
        .collect();

    let mut totals = SelectionTotals {
        cells: 0,
        nulls: 0,
        numeric: None,
    };

    for (index, range) in ranges.iter().enumerate() {
        for coord in range.iter() {
            if ranges.iter().take(index).any(|other| other.contains(coord)) {
                continue;
            }
            let Some(cell) = edit_buffer.displayed_cell(model, &visual_order, coord.row, coord.col)
            else {
                continue;
            };
            let numeric_column = model.columns.get(coord.col).is_some_and(|column| {
                matches!(column.kind, ColumnKind::Integer | ColumnKind::Float)
            });
            add_cell(&mut totals, cell, numeric_column);
        }
    }

    (totals.cells > 1).then_some(totals)
}

fn add_cell(totals: &mut SelectionTotals, cell: &CellValue, numeric_column: bool) {
    totals.cells += 1;
    if cell.is_null() {
        totals.nulls += 1;
        return;
    }
    let Some((value, integer, text)) = number_of(cell, numeric_column) else {
        return;
    };
    match &mut totals.numeric {
        Some(numeric) => numeric.add(value, integer, text),
        None => totals.numeric = Some(NumericTotals::new(value, integer, text)),
    }
}

/// The number a cell holds, with its exact integer value when it is one and
/// the text it is written as in the grid.
///
/// Text counts only in a numeric column: decimals arrive as text, and so
/// does every value typed into a cell, while a number-looking text in a
/// text column (a zip code, an id) is not meant to be summed. Integer text
/// is read as `i128`, so `NUMERIC(38,0)` and unsigned 64-bit keys still sum
/// exactly.
fn number_of(cell: &CellValue, numeric_column: bool) -> Option<(f64, Option<i128>, String)> {
    match &cell.kind {
        CellKind::Int(value) => Some((*value as f64, Some(i128::from(*value)), value.to_string())),
        CellKind::Float(value) if value.is_finite() => {
            Some((*value, None, cell.display_text().to_string()))
        }
        CellKind::Text(text) if numeric_column => {
            let text = text.trim();
            if let Ok(integer) = text.parse::<i128>() {
                return Some((integer as f64, Some(integer), text.to_string()));
            }
            text.parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(|value| (value, None, text.to_string()))
        }
        _ => None,
    }
}

fn clamp_range(range: CellRange, row_count: usize, col_count: usize) -> Option<CellRange> {
    if range.start.row >= row_count || range.start.col >= col_count {
        return None;
    }
    Some(CellRange::new(
        range.start,
        CellCoord::new(
            range.end.row.min(row_count - 1),
            range.end.col.min(col_count - 1),
        ),
    ))
}

fn fractional_digits(text: &str) -> usize {
    if text.contains(['e', 'E']) {
        return MAX_DECIMALS;
    }
    text.split_once('.')
        .map_or(0, |(_, fraction)| fraction.len())
        .min(MAX_DECIMALS)
}

/// `value` with at most `decimals` fractional digits, trailing zeros dropped.
fn format_decimal(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text.as_str()
    };
    match text {
        "-0" => "0".to_string(),
        text => text.to_string(),
    }
}

/// The totals of the current selection, kept while the cells they were
/// worked out from stay the same.
///
/// The host's footer reads them on every frame, scrolling and column resizing
/// included, and summing a whole selected column per frame would cost far
/// more than comparing the key: the model, the edit buffer's revision and the
/// selected ranges.
#[derive(Default)]
pub(super) struct TotalsCache(RefCell<Option<(TotalsKey, Option<SelectionTotals>)>>);

/// What the totals are worked out from. The model is held weakly, which
/// keeps its allocation (not its rows) alive, so a later model can never sit
/// at the same address and pass for it.
pub(super) struct TotalsKey {
    model: Weak<TableModel>,
    edit_revision: u64,
    ranges: Vec<CellRange>,
}

impl TotalsKey {
    pub(super) fn new(
        model: &Arc<TableModel>,
        edit_buffer: &EditBuffer,
        selection: &SelectionState,
    ) -> Self {
        Self {
            model: Arc::downgrade(model),
            edit_revision: edit_buffer.revision(),
            ranges: selection.ranges(),
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.model, &other.model)
            && self.edit_revision == other.edit_revision
            && self.ranges == other.ranges
    }
}

impl TotalsCache {
    pub(super) fn get_or_compute(
        &self,
        key: TotalsKey,
        compute: impl FnOnce() -> Option<SelectionTotals>,
    ) -> Option<SelectionTotals> {
        let mut cached = self.0.borrow_mut();
        if let Some((cached_key, totals)) = cached.as_ref()
            && cached_key.same_as(&key)
        {
            return totals.clone();
        }
        let totals = compute();
        *cached = Some((key, totals.clone()));
        totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::data_table::model::{ColumnSpec, RowData};
    use gpui::TextAlign;

    fn column(title: &str, kind: ColumnKind) -> ColumnSpec {
        ColumnSpec {
            id: title.into(),
            title: title.into(),
            kind,
            align: TextAlign::Left,
            type_name: "".into(),
        }
    }

    /// Columns: `n` integer, `price` numeric (text), `name` text.
    fn model() -> TableModel {
        let rows = [
            (Some(1), Some("1.50"), "a"),
            (Some(5), Some("2.25"), "12"),
            (None, None, "c"),
            (Some(-2), Some("10"), "d"),
        ]
        .into_iter()
        .map(|(n, price, name)| RowData {
            cells: vec![
                n.map_or_else(CellValue::null, CellValue::int),
                price.map_or_else(CellValue::null, CellValue::text),
                CellValue::text(name),
            ],
        })
        .collect();
        TableModel::new(
            vec![
                column("n", ColumnKind::Integer),
                column("price", ColumnKind::Float),
                column("name", ColumnKind::Text),
            ],
            rows,
        )
    }

    fn buffer(model: &TableModel) -> EditBuffer {
        let mut buffer = EditBuffer::new();
        buffer.set_base_row_count(model.row_count());
        buffer
    }

    fn column_selection(col: usize, rows: usize) -> SelectionState {
        let mut selection = SelectionState::new();
        selection.select_column(col, rows);
        selection
    }

    #[test]
    fn a_single_cell_has_no_totals() {
        let model = model();
        let mut selection = SelectionState::new();
        selection.select_cell(CellCoord::new(0, 0));
        assert_eq!(selection_totals(&model, &buffer(&model), &selection), None);
    }

    #[test]
    fn an_integer_column_sums_exactly_and_skips_nulls() {
        let model = model();
        let totals = selection_totals(&model, &buffer(&model), &column_selection(0, 4))
            .expect("a column has totals");
        assert_eq!(totals.cells, 4);
        assert_eq!(totals.nulls, 1);
        let numeric = totals.numeric.expect("integers are numbers");
        assert_eq!(numeric.count, 3);
        assert_eq!(numeric.sum_text(), "4");
        assert_eq!(numeric.average_text(), "1.3333");
        assert_eq!(numeric.min_text(), "-2");
        assert_eq!(numeric.max_text(), "5");
    }

    #[test]
    fn decimals_held_as_text_keep_their_precision() {
        let model = model();
        let numeric = selection_totals(&model, &buffer(&model), &column_selection(1, 4))
            .and_then(|totals| totals.numeric)
            .expect("a numeric column of text is summed");
        assert_eq!(numeric.sum_text(), "13.75");
        assert_eq!(numeric.min_text(), "1.50", "written as the cell writes it");
        assert_eq!(numeric.max_text(), "10");
    }

    #[test]
    fn text_columns_only_count() {
        let model = model();
        let totals = selection_totals(&model, &buffer(&model), &column_selection(2, 4))
            .expect("a column has totals");
        assert_eq!(totals.cells, 4);
        assert_eq!(totals.numeric, None, "a number-looking name is not summed");
        assert_eq!(totals.non_numeric(), 4);
    }

    #[test]
    fn overlapping_ranges_count_each_cell_once() {
        let model = model();
        let mut selection = SelectionState::new();
        selection.select_row(0, 3);
        selection.toggle_column(0, 4);
        let totals = selection_totals(&model, &buffer(&model), &selection).expect("two ranges");
        // Row 0 (3 cells) plus column 0 (4 cells) share cell (0, 0).
        assert_eq!(totals.cells, 6);
        assert_eq!(totals.numeric.expect("numbers").sum_text(), "5.5");
    }

    #[test]
    fn staged_edits_are_summed_as_shown() {
        let model = model();
        let mut buffer = buffer(&model);
        buffer.set_cell(0, 0, CellValue::text("100"));
        let numeric = selection_totals(&model, &buffer, &column_selection(0, 4))
            .and_then(|totals| totals.numeric)
            .expect("numbers");
        assert_eq!(numeric.sum_text(), "103");
        assert_eq!(numeric.max_text(), "100");
    }

    #[test]
    fn a_pending_insert_in_the_selection_is_summed() {
        let model = model();
        let mut buffer = buffer(&model);
        buffer.add_pending_insert(vec![
            CellValue::text("7"),
            CellValue::null(),
            CellValue::text("new"),
        ]);
        // The insert is the fifth visual row, after the four base rows.
        let totals = selection_totals(&model, &buffer, &column_selection(0, 5))
            .expect("a column has totals");
        assert_eq!(totals.cells, 5);
        let numeric = totals.numeric.expect("numbers");
        assert_eq!(numeric.count, 4);
        assert_eq!(numeric.sum_text(), "11");
        assert_eq!(numeric.max_text(), "7");
    }

    fn float_model(cells: Vec<CellValue>) -> TableModel {
        TableModel::new(
            vec![column("x", ColumnKind::Float)],
            cells
                .into_iter()
                .map(|cell| RowData { cells: vec![cell] })
                .collect(),
        )
    }

    #[test]
    fn int_cells_in_a_float_column_sum_with_the_floats() {
        let model = float_model(vec![
            CellValue::int(2),
            CellValue::float(0.5),
            CellValue::float(5.0),
        ]);
        let numeric = selection_totals(&model, &buffer(&model), &column_selection(0, 3))
            .and_then(|totals| totals.numeric)
            .expect("numbers");
        assert_eq!(numeric.count, 3);
        assert_eq!(numeric.sum_text(), "7.5");
        assert_eq!(numeric.min_text(), "0.5");
        assert_eq!(numeric.max_text(), "5.0", "written as the grid writes it");
    }

    #[test]
    fn non_finite_text_in_a_numeric_column_is_skipped() {
        let model = float_model(vec![
            CellValue::text("NaN"),
            CellValue::text("inf"),
            CellValue::text("-infinity"),
            CellValue::text("1.5"),
        ]);
        let totals = selection_totals(&model, &buffer(&model), &column_selection(0, 4))
            .expect("a column has totals");
        assert_eq!(totals.non_numeric(), 3);
        let numeric = totals.numeric.expect("one number");
        assert_eq!(numeric.count, 1);
        assert_eq!(numeric.sum_text(), "1.5");
    }

    #[test]
    fn integers_past_i64_sum_exactly() {
        let model = TableModel::new(
            vec![column("big", ColumnKind::Integer)],
            ["18446744073709551615", "1"]
                .into_iter()
                .map(|text| RowData {
                    cells: vec![CellValue::text(text)],
                })
                .collect(),
        );
        let numeric = selection_totals(&model, &buffer(&model), &column_selection(0, 2))
            .and_then(|totals| totals.numeric)
            .expect("numbers");
        assert_eq!(numeric.sum_text(), "18446744073709551616");
    }

    #[test]
    fn the_cache_recomputes_only_when_its_inputs_change() {
        let model = Arc::new(model());
        let mut buffer = buffer(&model);
        let selection = column_selection(0, 4);
        let cache = TotalsCache::default();
        let computed = std::cell::Cell::new(0);
        let read = |buffer: &EditBuffer, selection: &SelectionState| {
            cache.get_or_compute(TotalsKey::new(&model, buffer, selection), || {
                computed.set(computed.get() + 1);
                selection_totals(&model, buffer, selection)
            })
        };

        let first = read(&buffer, &selection);
        assert_eq!(read(&buffer, &selection), first);
        assert_eq!(computed.get(), 1, "the same inputs reuse the cached totals");

        buffer.set_cell(0, 0, CellValue::text("100"));
        let edited = read(&buffer, &selection).and_then(|totals| totals.numeric);
        assert_eq!(edited.expect("numbers").sum_text(), "103");
        assert_eq!(computed.get(), 2, "an edit recomputes");

        let other = column_selection(1, 4);
        let _ = read(&buffer, &other);
        assert_eq!(computed.get(), 3, "another selection recomputes");

        let reloaded = Arc::new(TableModel::clone(&model));
        let _ = cache.get_or_compute(TotalsKey::new(&reloaded, &buffer, &other), || {
            computed.set(computed.get() + 1);
            None
        });
        assert_eq!(computed.get(), 4, "a new model recomputes");
    }

    #[test]
    fn float_noise_is_not_shown() {
        assert_eq!(format_decimal(0.1 + 0.2, 1), "0.3");
        assert_eq!(format_decimal(-0.0001, 2), "0");
        assert_eq!(format_decimal(2.5, 4), "2.5");
    }
}
