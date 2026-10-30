//! The row UPDATE, INSERT and DELETE requests a table grid's save sends.
//!
//! The save path and the change review both build them here, so the review
//! shows the requests the save will make, not a reconstruction of them.

use dbflux_components::components::data_table::DataTableState;
use dbflux_components::components::data_table::model::CellValue;
use dbflux_core::{ColumnAssignment, RowDelete, RowIdentity, RowInsert, RowPatch, TableRef, Value};

use super::cell_expressions::column_assignment;

/// Why a staged row edit cannot become an UPDATE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PatchProblem {
    /// A key column holds a value of a type the grid cannot send back.
    UnsupportedKey,
    /// The row lacks part of its key.
    MissingIdentity,
    /// A changed column holds a value of a type the grid cannot send.
    UnsupportedValues,
}

impl PatchProblem {
    pub(super) fn message(self) -> String {
        match self {
            Self::UnsupportedKey => {
                dbflux_i18n::t!("document.data.mutation.error.save_row_unsupported_pk")
            }
            Self::MissingIdentity => {
                dbflux_i18n::t!("document.data.mutation.error.save_row_identity_failed")
            }
            Self::UnsupportedValues => {
                dbflux_i18n::t!("document.data.mutation.error.save_row_unsupported_values")
            }
        }
    }
}

/// The key of base row `row_idx` as the database knows it: the loaded
/// values of the key columns, never staged edits to them. `None` when a key
/// column or its value is missing.
pub(super) fn row_identity(state: &DataTableState, row_idx: usize) -> Option<RowIdentity> {
    let pk_indices = state.pk_columns();
    let model = state.model();

    if pk_indices.is_empty() {
        return None;
    }

    let mut columns = Vec::with_capacity(pk_indices.len());
    let mut values = Vec::with_capacity(pk_indices.len());
    for &col_idx in pk_indices {
        columns.push(model.columns.get(col_idx)?.title.to_string());
        values.push(model.cell(row_idx, col_idx)?.to_value());
    }

    Some(RowIdentity::new(columns, values))
}

/// The UPDATE of base row `row_idx` writing `changes`.
pub(super) fn row_patch(
    state: &DataTableState,
    row_idx: usize,
    changes: &[(usize, &CellValue)],
    table: &TableRef,
) -> Result<RowPatch, PatchProblem> {
    let identity = row_identity(state, row_idx);

    if identity
        .as_ref()
        .is_some_and(|identity| identity.values().iter().any(is_unsupported))
    {
        return Err(PatchProblem::UnsupportedKey);
    }
    let identity = identity.ok_or(PatchProblem::MissingIdentity)?;

    let model = state.model();
    let changes: Vec<ColumnAssignment> = changes
        .iter()
        .filter_map(|&(col_idx, cell)| {
            model
                .columns
                .get(col_idx)
                .map(|column| column_assignment(column, cell))
        })
        .collect();

    if changes
        .iter()
        .any(|assignment| is_unsupported(&assignment.value))
    {
        return Err(PatchProblem::UnsupportedValues);
    }

    Ok(RowPatch::with_typed_changes(
        identity,
        table.name.clone(),
        table.schema.clone(),
        changes,
    ))
}

/// The UPDATE of base row `row_idx` writing every cell staged in it.
pub(super) fn staged_row_patch(
    state: &DataTableState,
    row_idx: usize,
    table: &TableRef,
) -> Result<RowPatch, PatchProblem> {
    let changes = state.edit_buffer().row_changes(row_idx);
    row_patch(state, row_idx, &changes, table)
}

/// The INSERT of pending insert `insert_idx`, or `None` when there is no
/// such insert. The insert names no value for a null or column-default
/// cell, leaving those to the database, and has no assignments when every
/// cell is one of them.
pub(super) fn row_insert(
    state: &DataTableState,
    insert_idx: usize,
    table: &TableRef,
) -> Option<RowInsert> {
    let cells = state.edit_buffer().get_pending_insert_by_idx(insert_idx)?;
    let model = state.model();

    let assignments = cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| !matches!(cell.to_value(), Value::Null))
        .filter_map(|(col_idx, cell)| {
            model
                .columns
                .get(col_idx)
                .map(|column| column_assignment(column, cell))
        })
        .collect();

    Some(RowInsert::with_typed_assignments(
        table.name.clone(),
        table.schema.clone(),
        assignments,
    ))
}

/// The DELETE of base row `row_idx`, or `None` without its key.
pub(super) fn row_delete(
    state: &DataTableState,
    row_idx: usize,
    table: &TableRef,
) -> Option<RowDelete> {
    row_identity(state, row_idx)
        .map(|identity| RowDelete::new(identity, table.name.clone(), table.schema.clone()))
}

fn is_unsupported(value: &Value) -> bool {
    matches!(value, Value::Unsupported(_))
}
