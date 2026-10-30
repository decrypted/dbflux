//! Review changes: the statements Save will run for the staged edits,
//! inserts and deletes of a table grid, shown before they run, as a
//! database IDE's "preview pending changes" does.

use dbflux_components::controls::{Button, ButtonVariant};
use dbflux_components::icons::AppIcon;
use dbflux_components::modals::Modal;
use dbflux_components::primitives::Text;
use dbflux_components::tokens::Spacing;
use dbflux_core::{Connection, DbError, TableRef};
use gpui::prelude::*;
use gpui::{App, ClipboardItem, Context, FocusHandle, Window, div, px};
use gpui_component::ActiveTheme;

use super::row_statements::{row_delete, row_insert, staged_row_patch};
use super::{DataGridPanel, DataSource, PendingBatchRemaining};

/// One statement of a review, in the order the save runs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewedStatement {
    /// The statement as the driver sends it.
    Sql(String),
    /// A change the save will not run, and why.
    Skipped(String),
}

impl ReviewedStatement {
    fn text(&self) -> String {
        match self {
            Self::Sql(sql) => format!("{};", sql.trim_end().trim_end_matches(';')),
            Self::Skipped(reason) => format!("-- {reason}"),
        }
    }
}

/// The staged changes of a table grid as the statements Save runs, and
/// the batch that runs them when the review is confirmed.
pub(crate) struct ChangeReview {
    pub(crate) statements: Vec<ReviewedStatement>,
    deletes: Vec<usize>,
    inserts: Vec<usize>,
    dirty_rows: Vec<usize>,
    focus: FocusHandle,
}

impl ChangeReview {
    /// The whole review as one SQL script, as Copy SQL copies it.
    pub(crate) fn sql(&self) -> String {
        self.statements
            .iter()
            .map(ReviewedStatement::text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl DataGridPanel {
    /// The statements Save would run now, built by the save path's own
    /// builders and rendered by the connection, or `None` when the grid is
    /// not a table with staged changes.
    pub(crate) fn build_change_review(&self, cx: &mut Context<Self>) -> Option<ChangeReview> {
        let DataSource::Table {
            profile_id,
            database,
            table,
            ..
        } = &self.source
        else {
            return None;
        };
        let connection = self
            .app_state
            .read(cx)
            .connections()
            .get(profile_id)
            .and_then(|connected| {
                connected
                    .resolve_connection_for_execution(database.as_deref())
                    .ok()
            });

        let state = self.grid_table.table_state.as_ref()?.read(cx);
        let buffer = state.edit_buffer();

        let deletes = buffer.pending_delete_rows();
        let inserts: Vec<usize> = (0..buffer.pending_inserts().len()).collect();
        let dirty_rows: Vec<usize> = buffer
            .dirty_rows()
            .into_iter()
            .filter(|&row| state.is_editable() && !buffer.row_changes(row).is_empty())
            .collect();
        if deletes.is_empty() && inserts.is_empty() && dirty_rows.is_empty() {
            return None;
        }

        let statements = review_statements(
            state,
            table,
            connection.as_deref(),
            &deletes,
            &inserts,
            &dirty_rows,
        );

        Some(ChangeReview {
            statements,
            deletes,
            inserts,
            dirty_rows,
            focus: cx.focus_handle(),
        })
    }

    /// Opens the review of the staged changes. Returns false when there is
    /// nothing to review.
    pub(crate) fn open_change_review(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| state.commit_pending_edit(cx));
        }

        let Some(review) = self.build_change_review(cx) else {
            return false;
        };

        review.focus.focus(window, cx);
        self.change_review = Some(review);
        cx.notify();
        true
    }

    /// Whether Save shows the review first (Settings > General > Data grid).
    pub(crate) fn reviews_before_saving(&self, cx: &App) -> bool {
        matches!(self.source, DataSource::Table { .. })
            && self
                .app_state
                .read(cx)
                .general_settings()
                .grid_review_before_save
    }

    /// Runs the reviewed statements: the save the review showed, with the
    /// row lists it was built from. The deletes run without the separate
    /// delete confirmation, which the review already was.
    pub(crate) fn confirm_change_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.change_review.take() else {
            return;
        };
        let ChangeReview {
            deletes,
            inserts,
            dirty_rows,
            ..
        } = review;

        if !inserts.is_empty() || !dirty_rows.is_empty() {
            self.pending_batch_remaining = Some(PendingBatchRemaining {
                pending_inserts: inserts,
                dirty_rows,
            });
        }

        match &self.source {
            DataSource::Table {
                profile_id,
                database,
                table,
                ..
            } if !deletes.is_empty() => {
                self.batch_op_in_flight = true;
                self.commit_bulk_deletes_table(
                    *profile_id,
                    database.clone(),
                    table.clone(),
                    deletes,
                    cx,
                );
            }
            _ => {
                if self.pending_batch_remaining.is_some() {
                    self.process_next_batch_op(cx);
                }
            }
        }

        self.focus_active_view(window, cx);
        cx.notify();
    }

    /// Closes the review without running anything.
    pub(crate) fn cancel_change_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.change_review.take().is_some() {
            self.focus_active_view(window, cx);
            cx.notify();
        }
    }

    /// Copies the reviewed statements, keeping the review open.
    pub(crate) fn copy_change_review(&mut self, cx: &mut Context<Self>) {
        if let Some(review) = &self.change_review {
            cx.write_to_clipboard(ClipboardItem::new_string(review.sql()));
        }
    }

    pub(super) fn render_change_review(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(review) = self.change_review.as_ref() else {
            return div().into_any_element();
        };

        let code_background = cx.theme().secondary;
        let runnable = review
            .statements
            .iter()
            .any(|statement| matches!(statement, ReviewedStatement::Sql(_)));

        let body = div()
            .flex()
            .flex_col()
            .gap(Spacing::SM)
            .child(Text::caption(dbflux_i18n::t!(
                "document.data.review.literals_note"
            )))
            .child(
                div()
                    .id("review-changes-sql")
                    .max_h(px(360.0))
                    .overflow_y_scroll()
                    .p(Spacing::SM)
                    .rounded_sm()
                    .bg(code_background)
                    .children(
                        review
                            .statements
                            .iter()
                            .map(|statement| Text::code(statement.text())),
                    ),
            );

        let footer = div()
            .flex()
            .gap(Spacing::SM)
            .child(
                Button::new(
                    "review-changes-cancel",
                    dbflux_i18n::t!("document.data.review.cancel"),
                )
                .icon(AppIcon::X)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.cancel_change_review(window, cx);
                })),
            )
            .child(
                Button::new(
                    "review-changes-copy",
                    dbflux_i18n::t!("document.data.review.copy"),
                )
                .icon(AppIcon::Copy)
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.copy_change_review(cx);
                })),
            )
            .child(
                Button::new(
                    "review-changes-save",
                    dbflux_i18n::t!("document.data.review.save"),
                )
                .variant(ButtonVariant::Primary)
                .icon(AppIcon::Save)
                .disabled(!runnable)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.confirm_change_review(window, cx);
                })),
            );

        let entity = cx.entity().downgrade();
        let confirm_entity = entity.clone();

        Modal::new(dbflux_i18n::t!("document.data.review.title"))
            .id("review-changes-modal")
            .icon(AppIcon::Eye)
            .width(px(720.0))
            .focus_handle(&review.focus)
            .on_close(move |window, cx| {
                if let Some(panel) = entity.upgrade() {
                    panel.update(cx, |panel, cx| panel.cancel_change_review(window, cx));
                }
            })
            .on_confirm(move |window, cx| {
                if let Some(panel) = confirm_entity.upgrade() {
                    panel.update(cx, |panel, cx| panel.confirm_change_review(window, cx));
                }
            })
            .confirm_enabled(runnable)
            .body(body)
            .footer(footer)
            .into_any_element()
    }
}

/// The statements for the deletes, inserts and row edits, in the order the
/// save runs them: deletes first, then inserts, then edits.
fn review_statements(
    state: &dbflux_components::components::data_table::DataTableState,
    table: &TableRef,
    connection: Option<&dyn Connection>,
    deletes: &[usize],
    inserts: &[usize],
    dirty_rows: &[usize],
) -> Vec<ReviewedStatement> {
    let render = |statement: Option<Result<String, DbError>>| match statement {
        Some(Ok(sql)) => ReviewedStatement::Sql(sql),
        Some(Err(error)) => ReviewedStatement::Skipped(error.to_string()),
        None => ReviewedStatement::Skipped(dbflux_i18n::t!("document.data.review.unavailable")),
    };
    let skipped_row = |row: usize, reason: String| {
        ReviewedStatement::Skipped(dbflux_i18n::t!(
            "document.data.review.skipped_row",
            row = row + 1,
            reason = reason
        ))
    };
    let Some(connection) = connection else {
        return vec![ReviewedStatement::Skipped(dbflux_i18n::t!(
            "document.data.review.no_connection"
        ))];
    };

    let mut statements = Vec::new();

    for &row in deletes {
        statements.push(match row_delete(state, row, table) {
            Some(delete) => render(connection.delete_row_statement(&delete)),
            None => skipped_row(
                row,
                dbflux_i18n::t!("document.data.mutation.error.delete_identity_failed"),
            ),
        });
    }

    for &insert_idx in inserts {
        match row_insert(state, insert_idx, table) {
            Some(insert) if insert.is_valid() => {
                statements.push(render(connection.insert_row_statement(&insert)));
            }
            Some(_) => statements.push(ReviewedStatement::Skipped(dbflux_i18n::t!(
                "document.data.mutation.error.insert_no_values"
            ))),
            None => {}
        }
    }

    for &row in dirty_rows {
        statements.push(match staged_row_patch(state, row, table) {
            Ok(patch) => render(connection.update_row_statement(&patch)),
            Err(problem) => skipped_row(row, problem.message()),
        });
    }

    statements
}
