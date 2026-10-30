//! Filter changes as steps of the grid's undo history.
//!
//! The grid keeps one chronological history without numbering its steps:
//! a table read rebuilds the edit buffer, and a new filter is refused while
//! edits are pending, so every edit the buffer can still undo is newer than
//! the last filter change. Undo therefore steps the buffer first and the
//! filters once the buffer has nothing left; Redo the same way, because a
//! filter step's read leaves the buffer with nothing to redo.

#[cfg(test)]
mod tests;

use super::{DataGridPanel, DataSource, TableReload};
use gpui::{Context, Window};

/// Filter steps kept; the oldest go first.
const UNDO_LIMIT: usize = 100;

/// The filters a table was read with, around the one read last.
#[derive(Debug, Default)]
pub(super) struct FilterUndo {
    /// The filter of the last table read issued; `None` before the first,
    /// which is where the history starts rather than a step.
    applied: Option<String>,
    undo: Vec<String>,
    redo: Vec<String>,
}

impl FilterUndo {
    /// Notes the filter a table read is issued with. A filter other than
    /// the last one is a new step: the last one can be undone to, and the
    /// steps undone before are dropped.
    pub(super) fn issued(&mut self, filter: &str) {
        let filter = filter.trim();

        match self.applied.as_deref() {
            Some(applied) if applied == filter => return,
            Some(applied) => {
                self.undo.push(applied.to_string());
                if self.undo.len() > UNDO_LIMIT {
                    self.undo.remove(0);
                }
                self.redo.clear();
            }
            None => {}
        }

        self.applied = Some(filter.to_string());
    }

    pub(super) fn can_step(&self, redo: bool) -> bool {
        if redo {
            !self.redo.is_empty()
        } else {
            !self.undo.is_empty()
        }
    }

    /// The filter one step back (or forward with `redo`), now the applied
    /// one; the filter it replaces moves to the other side.
    pub(super) fn step(&mut self, redo: bool) -> Option<String> {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };

        let target = from.pop()?;
        if let Some(applied) = self.applied.replace(target.clone()) {
            to.push(applied);
        }
        Some(target)
    }
}

impl DataGridPanel {
    /// Whether Undo (or Redo with `redo`) has a filter step to take.
    pub(super) fn can_step_filter(&self, redo: bool) -> bool {
        self.source.is_table() && self.filter_bar.undo.can_step(redo)
    }

    /// Undo or Redo of a filter change, once the edit history has no step
    /// left: puts the previous (or next) filter in the WHERE field and reads
    /// the table with it from the first page. With edits pending it refuses,
    /// as any reload does, so it never drops them. Returns false when there
    /// is no filter step.
    pub(super) fn step_filter(
        &mut self,
        redo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_step_filter(redo) {
            return false;
        }
        if self.reload_blocked_by_pending_edits(cx) {
            return true;
        }
        let Some(filter) = self.filter_bar.undo.step(redo) else {
            return false;
        };

        self.close_filter_history(cx);
        self.filter_bar
            .filter_input
            .update(cx, |input, cx| input.set_value(&filter, window, cx));

        if let DataSource::Table { pagination, .. } = &mut self.source {
            *pagination = pagination.reset_offset();
        }
        self.grid_table.reload = TableReload::ResetRows;
        self.refresh(window, cx);
        true
    }
}
