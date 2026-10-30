//! What a table grid carries across a restart: the read it last applied, and
//! a grid that names a restored source before its connection is up.

use super::*;
use serde::{Deserialize, Serialize};

/// The read a restored table tab replays: its filter, page size and sort.
/// Rows are never part of it; the tab reads them again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSessionView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Empty when the rows were in their default (primary-key) order, so a
    /// key, or a default direction, that changes later is not pinned by an
    /// old session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_by: Vec<OrderByColumn>,
}

impl TableSessionView {
    /// Reads stored view state. State this build cannot read (written by
    /// another version, or damaged) reopens the table with the defaults
    /// instead of losing the tab.
    pub fn from_json(json: Option<&str>) -> Self {
        json.and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default()
    }

    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }
}

impl DataGridPanel {
    /// A table grid that names its source and reads nothing. A restored tab
    /// holds one while its connection is not up: the tab keeps its place,
    /// title and dedup key, and no read is issued that could only fail.
    pub fn new_for_pending_table(
        profile_id: Uuid,
        table: TableRef,
        database: Option<String>,
        app_state: gpui::Entity<AppStateEntity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = DataSource::Table {
            profile_id,
            database,
            table,
            pagination: Pagination::default(),
            order_by: Vec::new(),
            total_rows: None,
        };

        Self::new_internal(source, app_state, Vec::new(), window, cx)
    }

    /// The collection counterpart of [`Self::new_for_pending_table`].
    pub fn new_for_pending_collection(
        profile_id: Uuid,
        collection: CollectionRef,
        app_state: gpui::Entity<AppStateEntity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = DataSource::Collection {
            profile_id,
            collection,
            pagination: Pagination::default(),
            total_docs: None,
        };

        Self::new_internal(source, app_state, vec!["_id".to_string()], window, cx)
    }

    /// The table read this grid last applied, for the session to replay.
    ///
    /// Taken from the last browse that landed rather than from the filter
    /// input, so a half-typed filter is not replayed. A builder or relational
    /// query is not a plain browse and yields `None`: such a tab reopens on
    /// the table itself.
    pub fn session_view(&self) -> Option<TableSessionView> {
        let Some(dbflux_transfer::ReplayableRead::Table(request)) = &self.last_replayable_read
        else {
            return None;
        };

        Some(TableSessionView {
            filter: request.filter.clone(),
            limit: Some(request.pagination.limit()),
            order_by: session_order_by(&request.order_by, &self.default_order_by),
        })
    }

    /// Puts a restored filter and page size into the inputs the first read
    /// takes them from.
    pub(super) fn apply_session_view(
        &mut self,
        view: &TableSessionView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(filter) = view.filter.as_deref() {
            self.filter_bar
                .filter_input
                .update(cx, |input, cx| input.set_value(filter, window, cx));
        }

        if let Some(limit) = view.limit.filter(|limit| *limit > 0) {
            self.filter_bar.limit_input.update(cx, |input, cx| {
                input.set_value(limit.to_string(), window, cx)
            });
        }
    }
}

/// The sort a session saves for a read in `applied` order: none when it is
/// the table's default order, which is computed again on the next launch.
fn session_order_by(
    applied: &[OrderByColumn],
    default_order: &[OrderByColumn],
) -> Vec<OrderByColumn> {
    if applied == default_order {
        Vec::new()
    } else {
        applied.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::session_order_by;
    use dbflux_core::OrderByColumn;

    #[test]
    fn the_default_order_is_not_saved_in_either_direction() {
        let ascending = vec![OrderByColumn::asc("id")];
        let descending = vec![OrderByColumn::desc("id")];

        assert!(session_order_by(&ascending, &ascending).is_empty());
        assert!(
            session_order_by(&descending, &descending).is_empty(),
            "a table opened newest first saves no sort, so the next launch follows the setting"
        );
    }

    #[test]
    fn a_sort_that_differs_from_the_default_is_saved() {
        let ascending = vec![OrderByColumn::asc("id")];
        let descending = vec![OrderByColumn::desc("id")];

        assert_eq!(session_order_by(&ascending, &descending), ascending);
        assert_eq!(session_order_by(&descending, &ascending), descending);
        assert_eq!(
            session_order_by(&[OrderByColumn::desc("created_at")], &ascending),
            vec![OrderByColumn::desc("created_at")]
        );
    }

    #[test]
    fn without_a_known_default_only_an_unordered_read_saves_no_sort() {
        assert!(session_order_by(&[], &[]).is_empty());
        assert_eq!(
            session_order_by(&[OrderByColumn::asc("id")], &[]),
            vec![OrderByColumn::asc("id")]
        );
    }
}
