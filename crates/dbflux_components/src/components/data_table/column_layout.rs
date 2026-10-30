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

/// The screen order, visibility and pinning of a model's columns.
///
/// Pinned columns always lead `order`, so the shown ones are the first
/// [`Self::pinned_count`] entries of [`Self::visible_columns`]: the grid draws
/// them in a strip at its left edge that does not scroll sideways.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnLayout {
    /// Every model column in screen order, hidden ones in their place, so
    /// showing a column again puts it back where it was.
    order: Vec<usize>,
    /// Indexed by model column.
    hidden: Vec<bool>,
    /// Indexed by model column.
    pinned: Vec<bool>,
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
    /// Names of the columns pinned to the left edge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<String>,
}

impl ColumnLayoutSpec {
    /// This spec, written by a grid that reads only some of the table's
    /// columns (a column projection), laid over `stored`, the spec kept for
    /// the whole table. The columns this spec names take the places `stored`
    /// gave them, in this spec's order, and their visibility and pin from
    /// this spec; every other stored column keeps its place and its flags.
    /// Columns new to `stored` follow at the end.
    pub fn merged_over(&self, stored: &ColumnLayoutSpec) -> ColumnLayoutSpec {
        let mut remaining: HashMap<&str, usize> = HashMap::new();
        for name in &self.order {
            *remaining.entry(name.as_str()).or_default() += 1;
        }

        let mut ours = self.order.iter();
        let mut order = Vec::with_capacity(stored.order.len().max(self.order.len()));
        for name in &stored.order {
            match remaining.get_mut(name.as_str()) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    order.extend(ours.next().cloned());
                }
                _ => order.push(name.clone()),
            }
        }
        order.extend(ours.cloned());

        // A flag of a column this spec names comes from this spec, every
        // other one from `stored`.
        let flags = |stored: &[String], ours: &[String]| -> Vec<String> {
            stored
                .iter()
                .filter(|name| !self.order.contains(name))
                .chain(ours)
                .cloned()
                .collect()
        };

        ColumnLayoutSpec {
            order,
            hidden: flags(&stored.hidden, &self.hidden),
            pinned: flags(&stored.pinned, &self.pinned),
        }
    }
}

impl ColumnLayout {
    /// The model's own order with every column shown.
    pub fn identity(col_count: usize) -> Self {
        let mut layout = Self {
            order: (0..col_count).collect(),
            hidden: vec![false; col_count],
            pinned: vec![false; col_count],
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

    pub fn is_pinned(&self, col: usize) -> bool {
        self.pinned.get(col).copied().unwrap_or(false)
    }

    /// How many of the shown columns are pinned; they lead
    /// [`Self::visible_columns`].
    pub fn pinned_count(&self) -> usize {
        self.visible
            .iter()
            .take_while(|col| self.is_pinned(**col))
            .count()
    }

    /// Pins `col` to the left edge, after the columns pinned before it.
    pub(super) fn pin(&mut self, col: usize) -> bool {
        match self.pinned.get_mut(col) {
            Some(pinned) if !*pinned => *pinned = true,
            _ => return false,
        }
        self.place_after_pinned(col);
        true
    }

    /// Unpins `col`; it becomes the first of the scrolling columns.
    pub(super) fn unpin(&mut self, col: usize) -> bool {
        match self.pinned.get_mut(col) {
            Some(pinned) if *pinned => *pinned = false,
            _ => return false,
        }
        self.place_after_pinned(col);
        true
    }

    /// Moves `col` to the boundary between the pinned columns and the
    /// others: the end of the pinned ones for a pinned column, the start of
    /// the others for one that is not.
    fn place_after_pinned(&mut self, col: usize) {
        if let Some(from) = self.order_index(col) {
            self.order.remove(from);
        }
        let boundary = self
            .order
            .iter()
            .take_while(|other| self.is_pinned(**other))
            .count();
        self.order.insert(boundary, col);
        self.derive();
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
    /// before it. Dropped among the pinned columns it is pinned, dropped
    /// among the others it is not.
    pub(super) fn move_to(&mut self, col: usize, target: usize) -> bool {
        let (Some(from), Some(to)) = (self.order_index(col), self.order_index(target)) else {
            return false;
        };
        if from == to {
            return false;
        }

        let pinned = self.is_pinned(target);
        if let Some(slot) = self.pinned.get_mut(col) {
            *slot = pinned;
        }
        self.order.remove(from);
        self.order.insert(to.min(self.order.len()), col);
        self.keep_pinned_first();
        self.derive();
        true
    }

    /// Restores the invariant that pinned columns lead `order`, keeping the
    /// relative order within each group.
    fn keep_pinned_first(&mut self) {
        let (pinned, rest): (Vec<usize>, Vec<usize>) =
            self.order.iter().partition(|col| self.is_pinned(**col));
        self.order = pinned.into_iter().chain(rest).collect();
    }

    /// Moves `col` one shown column to the left (`step` < 0) or right, past
    /// any hidden columns in between. It stays on its side of the pinned
    /// edge: Pin and Unpin cross it.
    pub(super) fn move_by(&mut self, col: usize, step: isize) -> bool {
        let Some(position) = self.view_position(col) else {
            return false;
        };
        let target = position.checked_add_signed(step);
        match target.and_then(|target| self.model_column(target)) {
            Some(target) if self.is_pinned(target) == self.is_pinned(col) => {
                self.move_to(col, target)
            }
            _ => false,
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

        let named = |keep: &dyn Fn(usize) -> bool| -> Vec<String> {
            self.order
                .iter()
                .filter(|col| keep(**col))
                .filter_map(name)
                .collect()
        };

        ColumnLayoutSpec {
            order: named(&|_| true),
            hidden: named(&|col| self.is_hidden(col)),
            pinned: named(&|col| self.is_pinned(col)),
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

        let mut hidden = Self::flags_by_name(&spec.hidden, &order, titles);
        if hidden.iter().all(|hidden| *hidden) {
            hidden.iter_mut().for_each(|hidden| *hidden = false);
        }
        let pinned = Self::flags_by_name(&spec.pinned, &order, titles);

        let mut layout = Self {
            order,
            hidden,
            pinned,
            visible: Vec::new(),
            position: Vec::new(),
        };
        layout.keep_pinned_first();
        layout.derive();
        layout
    }

    /// One flag per model column, set for the columns `names` names,
    /// matched in screen order so repeated names pair up as they were saved.
    fn flags_by_name(names: &[String], order: &[usize], titles: &[Arc<str>]) -> Vec<bool> {
        let mut remaining: HashMap<&str, usize> = HashMap::new();
        for name in names {
            *remaining.entry(name.as_str()).or_default() += 1;
        }

        let mut flags = vec![false; titles.len()];
        for col in order {
            if let Some(title) = titles.get(*col)
                && let Some(count) = remaining.get_mut(title.as_ref())
                && *count > 0
            {
                *count -= 1;
                if let Some(flag) = flags.get_mut(*col) {
                    *flag = true;
                }
            }
        }
        flags
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
    fn pinned_columns_lead_the_screen_and_survive_a_round_trip() {
        let mut layout = ColumnLayout::identity(4);
        assert!(layout.pin(2));
        assert!(layout.pin(0));
        assert_eq!(layout.visible_columns(), &[2, 0, 1, 3]);
        assert_eq!(layout.pinned_count(), 2);

        // Moving by one stays on its side of the pinned edge.
        assert!(!layout.move_by(0, 1));
        assert!(layout.move_by(0, -1));
        assert_eq!(layout.visible_columns(), &[0, 2, 1, 3]);

        // Dropped among the scrolling columns, a pinned column is unpinned.
        assert!(layout.move_to(2, 3));
        assert_eq!(layout.visible_columns(), &[0, 1, 3, 2]);
        assert_eq!(layout.pinned_count(), 1);

        // Unpinning makes it the first scrolling column.
        assert!(layout.pin(3));
        assert!(layout.unpin(0));
        assert_eq!(layout.visible_columns(), &[3, 0, 1, 2]);

        let names = titles(&["id", "msg", "level", "created_at"]);
        let spec = layout.to_spec(&names);
        assert_eq!(spec.pinned, ["created_at"]);
        assert_eq!(ColumnLayout::from_spec(&spec, &names), layout);

        // A spec saved before pinning existed reads with nothing pinned.
        let old: ColumnLayoutSpec =
            serde_json::from_str(r#"{"order":["id"],"hidden":[]}"#).expect("decodes");
        assert!(old.pinned.is_empty());
    }

    #[test]
    fn a_projected_spec_keeps_the_stored_columns_it_does_not_name() {
        let stored = ColumnLayoutSpec {
            order: vec!["email".into(), "id".into(), "name".into(), "note".into()],
            hidden: vec!["note".into()],
            pinned: vec!["name".into()],
        };
        // The grid read only `id` and `email`, and the user put `id` first.
        let projected = ColumnLayoutSpec {
            order: vec!["id".into(), "email".into()],
            hidden: Vec::new(),
            pinned: Vec::new(),
        };

        let merged = projected.merged_over(&stored);
        assert_eq!(merged.order, ["id", "email", "name", "note"]);
        assert_eq!(merged.hidden, ["note"], "`note` stays hidden");
        assert_eq!(merged.pinned, ["name"], "`name` stays pinned");

        // Hiding a projected column, and a column new to the store.
        let projected = ColumnLayoutSpec {
            order: vec!["email".into(), "id".into(), "added".into()],
            hidden: vec!["email".into()],
            pinned: vec!["id".into()],
        };
        let merged = projected.merged_over(&stored);
        assert_eq!(merged.order, ["email", "id", "name", "note", "added"]);
        assert_eq!(merged.hidden, ["note", "email"]);
        assert_eq!(merged.pinned, ["name", "id"]);
    }

    #[test]
    fn a_spec_hiding_every_column_shows_them_all() {
        let spec = ColumnLayoutSpec {
            order: Vec::new(),
            hidden: vec!["a".into(), "b".into()],
            pinned: Vec::new(),
        };
        let layout = ColumnLayout::from_spec(&spec, &titles(&["a", "b"]));
        assert_eq!(layout.visible_columns(), &[0, 1]);
    }
}
