//! Where each column of the model appears on screen.
//!
//! The grid addresses a column by its index in the model everywhere: the
//! selection, editing, copy and the host's events all use model indices.
//! Hiding and reordering columns only changes the screen, so this type is
//! the one place that maps between a model index and a screen position.
//! Code that walks columns geometrically (left/right, a dragged range, the
//! header strip) asks it; code that reads cells does not need to.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The screen order and visibility of a model's columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnLayout {
    /// Every model column in screen order, hidden ones in their place, so
    /// showing a column again puts it back where it was.
    order: Vec<usize>,
    /// Indexed by model column.
    hidden: Vec<bool>,
    /// The shown columns in screen order, derived from `order` and `hidden`.
    visible: Vec<usize>,
    /// Screen position of each model column, `None` while it is hidden.
    position: Vec<Option<usize>>,
}

/// A layout by column name, to keep it across reloads and restarts, where
/// model indices mean nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnLayoutSpec {
    /// Column names in screen order.
    #[serde(default)]
    pub order: Vec<String>,
    /// Names of the hidden columns.
    #[serde(default)]
    pub hidden: Vec<String>,
}

impl ColumnLayout {
    /// The model's own order with every column shown.
    pub fn identity(col_count: usize) -> Self {
        let mut layout = Self {
            order: (0..col_count).collect(),
            hidden: vec![false; col_count],
            visible: Vec::new(),
            position: Vec::new(),
        };
        layout.derive();
        layout
    }

    /// Whether the layout shows every column in model order.
    pub fn is_identity(&self) -> bool {
        self.visible.len() == self.order.len()
            && self
                .visible
                .iter()
                .enumerate()
                .all(|(pos, col)| pos == *col)
    }

    pub fn col_count(&self) -> usize {
        self.order.len()
    }

    /// Shown columns, as model indices, in screen order.
    pub fn visible_columns(&self) -> &[usize] {
        &self.visible
    }

    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    /// The model column at screen position `position`.
    pub fn model_column(&self, position: usize) -> Option<usize> {
        self.visible.get(position).copied()
    }

    /// Screen position of model column `col`; `None` while it is hidden.
    pub fn view_position(&self, col: usize) -> Option<usize> {
        self.position.get(col).copied().flatten()
    }

    pub fn is_hidden(&self, col: usize) -> bool {
        self.hidden.get(col).copied().unwrap_or(false)
    }

    pub fn has_hidden(&self) -> bool {
        self.hidden.iter().any(|hidden| *hidden)
    }

    /// Every model column in screen order, hidden ones included.
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// Hides `col`. The last shown column stays, so the grid never loses
    /// every column (and with them the header a menu opens from).
    pub(super) fn hide(&mut self, col: usize) -> bool {
        if self.is_hidden(col) || col >= self.col_count() || self.visible.len() <= 1 {
            return false;
        }
        if let Some(hidden) = self.hidden.get_mut(col) {
            *hidden = true;
        }
        self.derive();
        true
    }

    pub(super) fn show(&mut self, col: usize) -> bool {
        match self.hidden.get_mut(col) {
            Some(hidden) if *hidden => *hidden = false,
            _ => return false,
        }
        self.derive();
        true
    }

    pub(super) fn show_all(&mut self) -> bool {
        if !self.has_hidden() {
            return false;
        }
        self.hidden.iter_mut().for_each(|hidden| *hidden = false);
        self.derive();
        true
    }

    /// Moves `col` into the place of `target`, as dropping a header onto
    /// another one does: moving right it lands after `target`, moving left
    /// before it.
    pub(super) fn move_to(&mut self, col: usize, target: usize) -> bool {
        let (Some(from), Some(to)) = (self.order_index(col), self.order_index(target)) else {
            return false;
        };
        if from == to {
            return false;
        }

        self.order.remove(from);
        self.order.insert(to.min(self.order.len()), col);
        self.derive();
        true
    }

    /// Moves `col` one shown column to the left (`step` < 0) or right, past
    /// any hidden columns in between.
    pub(super) fn move_by(&mut self, col: usize, step: isize) -> bool {
        let Some(position) = self.view_position(col) else {
            return false;
        };
        let target = position.checked_add_signed(step);
        match target.and_then(|target| self.model_column(target)) {
            Some(target) => self.move_to(col, target),
            None => false,
        }
    }

    fn order_index(&self, col: usize) -> Option<usize> {
        self.order.iter().position(|candidate| *candidate == col)
    }

    fn derive(&mut self) {
        self.visible = self
            .order
            .iter()
            .copied()
            .filter(|col| !self.is_hidden(*col))
            .collect();
        self.position = vec![None; self.order.len()];
        for (position, col) in self.visible.iter().enumerate() {
            if let Some(slot) = self.position.get_mut(*col) {
                *slot = Some(position);
            }
        }
    }

    /// The layout by name, for a model whose columns are named `titles`.
    pub fn to_spec(&self, titles: &[Arc<str>]) -> ColumnLayoutSpec {
        let name = |col: &usize| titles.get(*col).map(|title| title.to_string());

        ColumnLayoutSpec {
            order: self.order.iter().filter_map(name).collect(),
            hidden: self
                .order
                .iter()
                .filter(|col| self.is_hidden(**col))
                .filter_map(name)
                .collect(),
        }
    }

    /// Lays out the columns `titles` as `spec` says. Columns the spec does
    /// not name (new since it was saved) follow the named ones in model
    /// order and are shown; names no column has any more are skipped. Names
    /// repeated in one model are matched in order. A spec that would hide
    /// every column shows them all.
    pub fn from_spec(spec: &ColumnLayoutSpec, titles: &[Arc<str>]) -> Self {
        let mut by_name: HashMap<&str, VecDeque<usize>> = HashMap::new();
        for (col, title) in titles.iter().enumerate() {
            by_name.entry(title.as_ref()).or_default().push_back(col);
        }

        let mut placed = vec![false; titles.len()];
        let mut order = Vec::with_capacity(titles.len());
        for name in &spec.order {
            if let Some(col) = by_name.get_mut(name.as_str()).and_then(VecDeque::pop_front)
                && let Some(slot) = placed.get_mut(col)
            {
                *slot = true;
                order.push(col);
            }
        }
        order.extend((0..titles.len()).filter(|col| !placed.get(*col).copied().unwrap_or(true)));

        let mut hidden_names: HashMap<&str, usize> = HashMap::new();
        for name in &spec.hidden {
            *hidden_names.entry(name.as_str()).or_default() += 1;
        }
        let mut hidden = vec![false; titles.len()];
        for col in &order {
            if let Some(title) = titles.get(*col)
                && let Some(count) = hidden_names.get_mut(title.as_ref())
                && *count > 0
            {
                *count -= 1;
                if let Some(slot) = hidden.get_mut(*col) {
                    *slot = true;
                }
            }
        }
        if hidden.iter().all(|hidden| *hidden) {
            hidden.iter_mut().for_each(|hidden| *hidden = false);
        }

        let mut layout = Self {
            order,
            hidden,
            visible: Vec::new(),
            position: Vec::new(),
        };
        layout.derive();
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(names: &[&str]) -> Vec<Arc<str>> {
        names.iter().map(|name| Arc::from(*name)).collect()
    }

    #[test]
    fn hide_and_move_map_between_model_and_screen() {
        let mut layout = ColumnLayout::identity(4);
        assert!(layout.is_identity());

        assert!(layout.hide(1));
        assert_eq!(layout.visible_columns(), &[0, 2, 3]);
        assert_eq!(layout.view_position(1), None);
        assert_eq!(layout.view_position(2), Some(1));

        // Dropping 3 onto 0 puts it first; dropping 0 onto 3 puts it last.
        assert!(layout.move_to(3, 0));
        assert_eq!(layout.visible_columns(), &[3, 0, 2]);
        assert!(layout.move_to(3, 2));
        assert_eq!(layout.visible_columns(), &[0, 2, 3]);

        // One step right skips the hidden column.
        assert!(layout.move_by(0, 1));
        assert_eq!(layout.visible_columns(), &[2, 0, 3]);
        assert!(!layout.move_by(3, 1), "nothing to the right of the last");

        assert!(layout.show(1));
        // Hidden, `1` kept its place in front of `2`, which moved past it.
        assert_eq!(layout.visible_columns(), &[1, 2, 0, 3], "back in its place");
    }

    #[test]
    fn the_last_shown_column_cannot_be_hidden() {
        let mut layout = ColumnLayout::identity(2);
        assert!(layout.hide(0));
        assert!(!layout.hide(1));
        assert_eq!(layout.visible_columns(), &[1]);
    }

    #[test]
    fn a_spec_round_trips_by_name_and_tolerates_changed_columns() {
        let mut layout = ColumnLayout::identity(3);
        layout.move_to(2, 0);
        layout.hide(1);
        let spec = layout.to_spec(&titles(&["id", "name", "created_at"]));
        assert_eq!(spec.order, ["created_at", "id", "name"]);
        assert_eq!(spec.hidden, ["name"]);

        // The table gained `email` and lost nothing; `name` moved in the model.
        let reloaded =
            ColumnLayout::from_spec(&spec, &titles(&["name", "id", "email", "created_at"]));
        assert_eq!(reloaded.visible_columns(), &[3, 1, 2]);
        assert!(reloaded.is_hidden(0));

        let json = serde_json::to_string(&spec).expect("encodes");
        let decoded: ColumnLayoutSpec = serde_json::from_str(&json).expect("decodes");
        assert_eq!(decoded, spec);
    }

    #[test]
    fn a_spec_hiding_every_column_shows_them_all() {
        let spec = ColumnLayoutSpec {
            order: Vec::new(),
            hidden: vec!["a".into(), "b".into()],
        };
        let layout = ColumnLayout::from_spec(&spec, &titles(&["a", "b"]));
        assert_eq!(layout.visible_columns(), &[0, 1]);
    }
}
