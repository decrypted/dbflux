//! Chart column auto-detection.
//!
//! This module is the boundary between the query result model and the chart engine.
//! It inspects `ColumnKind` values — never `type_name` strings or driver identifiers.

use dbflux_core::{ColumnKind, QueryResult, Value};

/// Outcome of attempting to auto-detect chart-suitable columns from a `QueryResult`.
#[derive(Debug, Clone, PartialEq)]
pub enum ChartDetection {
    /// Detection succeeded.
    Ok {
        /// Index of the leftmost `Timestamp` column (X axis).
        time_col: usize,
        /// Indices of all `Float` and `Integer` columns, in column order, excluding `time_col`.
        numeric_cols: Vec<usize>,
    },
    /// The result has no column with `kind == Timestamp`.
    NoTimeColumn,
    /// A `Timestamp` column was found but no `Float` or `Integer` columns remain.
    NoNumericSeries,
    /// The result has zero data rows; chart cannot be rendered.
    EmptyResult,
}

/// Auto-detect the columns suitable for charting in `result`.
///
/// Detection rules (applied in order):
/// 1. If `result.rows` is empty → `EmptyResult`.
/// 2. Pick the leftmost column with `kind == Timestamp`. If none → `NoTimeColumn`.
/// 3. Collect all columns (excluding the time column) with `kind == Float` or `Integer`.
///    If empty → `NoNumericSeries`.
/// 4. Otherwise → `Ok { time_col, numeric_cols }`.
///
/// The function never inspects `column.type_name`, `column.name`, or any driver identifier.
pub fn detect_chart_columns(result: &QueryResult) -> ChartDetection {
    if result.rows.is_empty() {
        return ChartDetection::EmptyResult;
    }

    let time_col = result
        .columns
        .iter()
        .position(|c| c.kind == ColumnKind::Timestamp);

    let time_col = match time_col {
        Some(idx) => idx,
        None => return ChartDetection::NoTimeColumn,
    };

    let numeric_cols: Vec<usize> = result
        .columns
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            *i != time_col && (c.kind == ColumnKind::Float || c.kind == ColumnKind::Integer)
        })
        .map(|(i, _)| i)
        .collect();

    if numeric_cols.is_empty() {
        return ChartDetection::NoNumericSeries;
    }

    ChartDetection::Ok {
        time_col,
        numeric_cols,
    }
}

/// The columns a chart draws until the user picks, out of the detected
/// `numeric_cols`.
///
/// An identifier plotted next to a measure flattens the measure against its own
/// scale, so identifiers stay out: a primary key, and an integer column that
/// only ever grows or only ever shrinks, as a key does in a table's own order.
/// Query results rarely carry the primary-key flag, which is why the rows
/// decide too. SQL integers are mostly identifiers, flags and foreign keys, so
/// floats win when there are any; the Y picker still lists every column.
///
/// A lone growing column is drawn after all, since a counter looks the same. A
/// result with nothing but flagged keys draws nothing and the user picks.
///
/// Like detection, this reads kinds and values, never names or driver ids.
pub fn default_series_columns(result: &QueryResult, numeric_cols: &[usize]) -> Vec<usize> {
    let is_flagged_key = |column: usize| {
        result
            .columns
            .get(column)
            .is_some_and(|meta| meta.is_primary_key)
    };

    let unflagged: Vec<usize> = numeric_cols
        .iter()
        .copied()
        .filter(|column| !is_flagged_key(*column))
        .collect();

    let measures: Vec<usize> = unflagged
        .iter()
        .copied()
        .filter(|column| !is_integer_sequence(result, *column))
        .collect();

    if measures.is_empty() {
        return unflagged;
    }

    let floats: Vec<usize> = measures
        .iter()
        .copied()
        .filter(|column| {
            result
                .columns
                .get(*column)
                .is_some_and(|meta| meta.kind == ColumnKind::Float)
        })
        .collect();

    if floats.is_empty() { measures } else { floats }
}

/// Whether `column` holds integers that strictly grow or strictly shrink over
/// at least two rows. Stops at the first row that breaks the run.
fn is_integer_sequence(result: &QueryResult, column: usize) -> bool {
    let is_integer = result
        .columns
        .get(column)
        .is_some_and(|meta| meta.kind == ColumnKind::Integer);

    if !is_integer || result.rows.len() < 2 {
        return false;
    }

    let mut previous: Option<i64> = None;
    let mut direction: Option<bool> = None;

    for row in &result.rows {
        let Some(Value::Int(value)) = row.get(column) else {
            return false;
        };

        if let Some(previous) = previous {
            if *value == previous {
                return false;
            }

            let growing = *value > previous;
            if *direction.get_or_insert(growing) != growing {
                return false;
            }
        }

        previous = Some(*value);
    }

    true
}

// ---------------------------------------------------------------------------
// Unit tests (strict TDD: tests were written before the implementation above)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dbflux_core::{ColumnKind, ColumnMeta, QueryResult, Value};
    use std::time::Duration;

    fn make_col(kind: ColumnKind) -> ColumnMeta {
        ColumnMeta {
            name: "col".to_string(),
            type_name: "t".to_string(),
            kind,
            nullable: true,
            is_primary_key: false,
        }
    }

    fn named_col(name: &str, kind: ColumnKind) -> ColumnMeta {
        ColumnMeta {
            name: name.to_string(),
            type_name: "t".to_string(),
            kind,
            nullable: true,
            is_primary_key: false,
        }
    }

    fn one_row() -> Vec<Vec<Value>> {
        vec![vec![Value::Null]]
    }

    #[test]
    fn returns_empty_result_for_no_rows() {
        let result = QueryResult::table(
            vec![make_col(ColumnKind::Timestamp), make_col(ColumnKind::Float)],
            vec![],
            None,
            Duration::ZERO,
        );
        assert_eq!(detect_chart_columns(&result), ChartDetection::EmptyResult);
    }

    #[test]
    fn returns_no_time_column_when_no_timestamp_kind() {
        let result = QueryResult::table(
            vec![make_col(ColumnKind::Text), make_col(ColumnKind::Float)],
            one_row(),
            None,
            Duration::ZERO,
        );
        assert_eq!(detect_chart_columns(&result), ChartDetection::NoTimeColumn);
    }

    #[test]
    fn returns_no_numeric_series_when_only_timestamp_and_text() {
        let result = QueryResult::table(
            vec![make_col(ColumnKind::Timestamp), make_col(ColumnKind::Text)],
            one_row(),
            None,
            Duration::ZERO,
        );
        assert_eq!(
            detect_chart_columns(&result),
            ChartDetection::NoNumericSeries
        );
    }

    #[test]
    fn detects_ok_for_timestamp_plus_floats() {
        let result = QueryResult::table(
            vec![
                make_col(ColumnKind::Timestamp),
                make_col(ColumnKind::Float),
                make_col(ColumnKind::Integer),
            ],
            one_row(),
            None,
            Duration::ZERO,
        );
        assert_eq!(
            detect_chart_columns(&result),
            ChartDetection::Ok {
                time_col: 0,
                numeric_cols: vec![1, 2],
            }
        );
    }

    #[test]
    fn picks_leftmost_timestamp_when_multiple() {
        let result = QueryResult::table(
            vec![
                named_col("a", ColumnKind::Text),
                named_col("t1", ColumnKind::Timestamp),
                named_col("t2", ColumnKind::Timestamp),
                named_col("v", ColumnKind::Float),
            ],
            one_row(),
            None,
            Duration::ZERO,
        );
        match detect_chart_columns(&result) {
            ChartDetection::Ok {
                time_col,
                numeric_cols,
            } => {
                assert_eq!(time_col, 1, "should pick leftmost Timestamp");
                assert_eq!(numeric_cols, vec![3]);
            }
            other => panic!("expected Ok, got {:?}", other),
        }
    }

    fn series_of(columns: Vec<ColumnMeta>, rows: Vec<Vec<Value>>) -> Vec<usize> {
        let result = QueryResult::table(columns, rows, None, Duration::ZERO);
        let ChartDetection::Ok { numeric_cols, .. } = detect_chart_columns(&result) else {
            panic!("the fixture has a time column and a numeric one");
        };

        default_series_columns(&result, &numeric_cols)
    }

    /// `SELECT *` from a log table: the key, a flag and the measure.
    #[test]
    fn the_default_series_is_the_float_and_not_the_id() {
        let series = series_of(
            vec![
                named_col("id", ColumnKind::Integer),
                named_col("created_at", ColumnKind::Timestamp),
                named_col("is_error", ColumnKind::Integer),
                named_col("runtime", ColumnKind::Float),
            ],
            vec![
                vec![
                    Value::Int(1),
                    Value::Int(0),
                    Value::Int(0),
                    Value::Float(0.5),
                ],
                vec![
                    Value::Int(2),
                    Value::Int(1),
                    Value::Int(1),
                    Value::Float(0.7),
                ],
            ],
        );

        assert_eq!(series, vec![3]);
    }

    #[test]
    fn a_growing_integer_is_left_out_next_to_another_integer() {
        let series = series_of(
            vec![
                named_col("ts", ColumnKind::Timestamp),
                named_col("id", ColumnKind::Integer),
                named_col("hits", ColumnKind::Integer),
            ],
            vec![
                vec![Value::Int(0), Value::Int(9), Value::Int(5)],
                vec![Value::Int(1), Value::Int(8), Value::Int(7)],
                vec![Value::Int(2), Value::Int(7), Value::Int(5)],
            ],
        );

        assert_eq!(series, vec![2], "a shrinking id is an id too");
    }

    #[test]
    fn a_lone_growing_integer_is_still_drawn() {
        let series = series_of(
            vec![
                named_col("ts", ColumnKind::Timestamp),
                named_col("total", ColumnKind::Integer),
            ],
            vec![
                vec![Value::Int(0), Value::Int(1)],
                vec![Value::Int(1), Value::Int(2)],
            ],
        );

        assert_eq!(series, vec![1]);
    }

    #[test]
    fn nothing_but_a_flagged_key_draws_nothing() {
        let mut key = named_col("id", ColumnKind::Integer);
        key.is_primary_key = true;

        let series = series_of(
            vec![named_col("ts", ColumnKind::Timestamp), key],
            vec![vec![Value::Int(0), Value::Int(1)]],
        );

        assert!(series.is_empty());
    }
}
