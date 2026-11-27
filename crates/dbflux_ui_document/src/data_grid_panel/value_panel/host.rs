//! The grid's side of the value panel: opening it on a cell, following the
//! cursor, and carrying out what the panel asks of the grid.

use super::{CellBytes, ImageKind, ValuePanelContent, ValuePanelEvent, ValuePanelTarget};
use crate::data_grid_panel::context_menu::{record_clipboard_audit, record_export_audit};
use crate::data_grid_panel::{DataGridEvent, DataGridPanel};
use dbflux_ui_base::SaveTargetOutcome;
use dbflux_ui_base::toast::PendingToast;
use gpui::*;
use std::sync::Arc;

impl DataGridPanel {
    /// Whether the value panel currently owns the shared inspector rail.
    pub fn value_panel_is_open(&self) -> bool {
        self.inspector.value_panel_open
    }

    /// Open the value panel on the active cell, or close it if already open.
    pub fn toggle_value_panel(&mut self, cx: &mut Context<Self>) {
        if self.inspector.value_panel_open {
            self.close_value_panel(cx);
            return;
        }

        let active = self
            .grid_table
            .table_state
            .as_ref()
            .and_then(|state| state.read(cx).selection().active);

        if let Some(coord) = active {
            self.request_value_panel(coord.row, coord.col, cx);
        }
    }

    /// Queue the value panel to open on `(row, col)`.
    ///
    /// The builder and the row inspector share this rail, so taking it means
    /// the row inspector must stop following the cursor — otherwise the two
    /// would replace each other on every selection change.
    pub(crate) fn request_value_panel(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        let Some(target) = self.value_panel_target(row, col, cx) else {
            return;
        };

        self.inspector.follow_selection = false;
        self.inspector.inspector_row = None;
        self.inspector.value_panel_open = true;
        self.mark_document_builder_closed();
        self.pending.value_panel = Some(target);
        cx.notify();
    }

    /// Carry the value panel's open state onto this tab.
    ///
    /// Called when the tab becomes active. Opening re-reads *this* grid's
    /// selection, so the rail shows the new table's cell instead of whatever
    /// the previous tab left there.
    pub fn set_value_panel_open(&mut self, open: bool, _cx: &mut Context<Self>) {
        if open && !self.builder.builder_open {
            self.inspector.value_panel_open = true;
        } else if !open {
            self.inspector.value_panel_open = false;
            self.pending.value_panel = None;
        }
    }

    /// Point the value panel at the active cell of this grid.
    ///
    /// Returns false when there is nothing to show yet — a tab whose grid has
    /// not loaded, or one with no selection.
    pub(in crate::data_grid_panel) fn mount_value_panel_for_active_cell(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let active = self
            .grid_table
            .table_state
            .as_ref()
            .and_then(|state| state.read(cx).selection().active);

        let Some(coord) = active else {
            return false;
        };

        let Some(target) = self.value_panel_target(coord.row, coord.col, cx) else {
            return false;
        };

        self.pending.value_panel = Some(target);
        cx.notify();
        true
    }

    /// The value panel when it is open and holds an unsaved edit.
    pub(crate) fn value_panel_pending_save(&self, cx: &App) -> Option<Entity<ValuePanelContent>> {
        if !self.inspector.value_panel_open {
            return None;
        }

        let panel = self.inspector.value_panel.as_ref()?;
        panel.read(cx).is_modified(cx).then(|| panel.clone())
    }

    fn close_value_panel(&mut self, cx: &mut Context<Self>) {
        self.inspector.value_panel_open = false;
        self.pending.value_panel = None;
        cx.emit(DataGridEvent::CloseInspector);
        cx.notify();
    }

    /// Read the cell's current edit-buffer text and editability for the panel.
    fn value_panel_target(&self, row: usize, col: usize, cx: &App) -> Option<ValuePanelTarget> {
        use dbflux_components::components::data_table::model::{CellValue, VisualRowSource};

        let table_state = self.grid_table.table_state.as_ref()?;
        let state = table_state.read(cx);
        let model = state.model();
        let column = model.columns.get(col)?;

        let edit_buffer = state.edit_buffer();
        let null_cell = CellValue::null();

        let (value, bytes) = match edit_buffer.compute_visual_order().get(row).copied()? {
            VisualRowSource::Base(base_idx) => {
                let base = model.cell(base_idx, col).unwrap_or(&null_cell);
                let cell = edit_buffer.get_cell(base_idx, col, base);
                (cell.edit_text(), self.stored_bytes(base_idx, col, cell))
            }
            VisualRowSource::Insert(insert_idx) => (
                edit_buffer
                    .get_pending_insert_by_idx(insert_idx)
                    .and_then(|data| data.get(col))
                    .map(|cell| cell.edit_text())
                    .unwrap_or_default(),
                None,
            ),
        };

        Some(ValuePanelTarget {
            row,
            col,
            column_name: column.title.to_string(),
            value,
            editable: bytes.is_none()
                && state.is_editable()
                && !state.readonly_columns().contains(&col),
            bytes,
        })
    }

    /// The bytes behind a binary cell, read from the result the grid was
    /// built from: the grid model keeps only their length.
    ///
    /// The cell's length must match, so a grid whose columns do not map
    /// one to one onto the result (a flattened document) shows nothing
    /// rather than another field's bytes.
    fn stored_bytes(
        &self,
        base_idx: usize,
        col: usize,
        cell: &dbflux_components::components::data_table::model::CellValue,
    ) -> Option<CellBytes> {
        use dbflux_components::components::data_table::model::CellKind;

        let CellKind::Bytes(len) = cell.kind else {
            return None;
        };

        match self.result.rows.get(base_idx)?.get(col)? {
            dbflux_core::Value::Bytes(bytes) if bytes.len() == len => Some(CellBytes::new(bytes)),
            _ => None,
        }
    }

    /// Mount or update the value panel. Called from render, where a `Window`
    /// is available to build the editor.
    pub(crate) fn apply_pending_value_panel(
        &mut self,
        target: ValuePanelTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = match &self.inspector.value_panel {
            Some(existing) => {
                existing.update(cx, |panel, cx| panel.open(target, window, cx));
                existing.clone()
            }
            None => {
                let content = cx.new(|cx| ValuePanelContent::new(target, window, cx));
                self.inspector._value_panel_subscription = Some(cx.subscribe(
                    &content,
                    |this, _, event: &ValuePanelEvent, cx| match event {
                        ValuePanelEvent::Save { row, col, value } => {
                            // Save in the panel means "write this value to the
                            // database", not "stage it": having to confirm
                            // again in the toolbar after pressing Save reads as
                            // the panel having done nothing. The commit goes
                            // through the normal row-save path, so mutation
                            // policy, approval, and the task list all still
                            // apply.
                            this.write_cell_value(*row, *col, value, cx);

                            if let Some(table_state) = this.grid_table.table_state.clone() {
                                table_state.update(cx, |state, cx| {
                                    state.request_save_row_at(*row, cx);
                                });
                            }
                        }
                        ValuePanelEvent::SaveImage {
                            kind,
                            bytes,
                            column,
                        } => this.save_cell_image(*kind, bytes.clone(), column, cx),
                        ValuePanelEvent::CopyImage { kind, bytes } => {
                            this.copy_cell_image(*kind, bytes, cx)
                        }
                    },
                ));
                self.inspector.value_panel = Some(content.clone());
                content
            }
        };

        cx.emit(DataGridEvent::OpenInspector {
            title: SharedString::from(dbflux_i18n::t!("components.value_panel.title")),
            content: AnyView::from(content),
            content_has_header: false,
        });
    }

    /// Whether the panel may be re-pointed at `(row, col)`.
    ///
    /// An edited-but-unsaved panel stays pinned to its cell; following the
    /// cursor there would throw the user's typing away without asking.
    pub(in crate::data_grid_panel) fn value_panel_can_follow(
        &self,
        row: usize,
        col: usize,
        cx: &App,
    ) -> bool {
        let Some(panel) = self.inspector.value_panel.as_ref() else {
            return true;
        };

        let panel = panel.read(cx);
        !panel.is_modified(cx) && panel.target_cell() != (row, col)
    }
}

impl DataGridPanel {
    /// Writes an image the value panel shows to a file the user picks,
    /// through the same save prompt, fallback directory and audit trail as
    /// an export.
    fn save_cell_image(
        &mut self,
        kind: ImageKind,
        bytes: Arc<[u8]>,
        column: &str,
        cx: &mut Context<Self>,
    ) {
        let suggested_name = format!(
            "{}_{}.{}",
            self.export_base_name(),
            file_name_part(column),
            kind.extension()
        );
        let format_name = format!("Image ({})", kind.label());
        let extension = kind.extension();
        let entity = cx.entity().clone();
        let audit_service = self.app_state.read(cx).audit_service().clone();
        let save_target_override = self.app_state.read(cx).save_target_override();

        cx.spawn(async move |_this, cx| {
            let outcome = dbflux_ui_base::file_dialog::resolve_save_target(
                save_target_override,
                dbflux_ui_base::SaveTargetRequest {
                    suggested_name: &suggested_name,
                    language_name: &format_name,
                    default_extension: extension,
                },
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(dbflux_i18n::t!("components.value_panel.image.save_title"))
                        .set_file_name(&suggested_name)
                        .add_filter(kind.label(), &[extension])
                        .save_file()
                        .await
                        .map(|handle| handle.path().to_path_buf())
                },
            )
            .await;

            let (path, used_fallback) = match outcome {
                SaveTargetOutcome::Selected {
                    path,
                    used_fallback,
                } => (path, used_fallback),
                SaveTargetOutcome::Cancelled => return,
                SaveTargetOutcome::Failed(error) => {
                    record_export_audit(
                        &audit_service,
                        &format_name,
                        None,
                        true,
                        false,
                        Some(&error),
                    );
                    let message =
                        crate::labels::context_menu_export_dialog_fallback_failed_error(&error);
                    show_toast(&entity, message, true, cx);
                    return;
                }
            };

            let written = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::write(&path, &bytes) }
                })
                .await;

            let path_text = path.display().to_string();
            let error = written.as_ref().err().map(|error| error.to_string());
            record_export_audit(
                &audit_service,
                &format_name,
                Some(&path),
                error.is_some(),
                used_fallback,
                error.as_deref(),
            );

            let (message, is_error) = match error {
                Some(error) => (
                    crate::labels::context_menu_export_failed_error(&error),
                    true,
                ),
                None if used_fallback => (
                    crate::labels::context_menu_export_native_picker_fallback_toast(&path_text),
                    false,
                ),
                None => (
                    crate::labels::context_menu_export_exported_toast(&path_text),
                    false,
                ),
            };
            show_toast(&entity, message, is_error, cx);
        })
        .detach();
    }

    /// Puts the encoded image on the clipboard as an image, not as text.
    fn copy_cell_image(&mut self, kind: ImageKind, bytes: &[u8], cx: &mut Context<Self>) {
        let image = Image::from_bytes(kind.gpui_format(), bytes.to_vec());
        cx.write_to_clipboard(ClipboardItem::new_image(&image));

        let format_name = format!("Image ({})", kind.label());
        let audit_service = self.app_state.read(cx).audit_service().clone();
        record_clipboard_audit(&audit_service, &format_name, Some(bytes.len()), None);

        self.pending.toast = Some(PendingToast {
            message: crate::labels::context_menu_clipboard_copied_toast(kind.label(), bytes.len()),
            is_error: false,
        });
        cx.notify();
    }
}

/// A column name made safe to put in a suggested file name.
fn file_name_part(column: &str) -> String {
    column
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn show_toast(entity: &Entity<DataGridPanel>, message: String, is_error: bool, cx: &mut AsyncApp) {
    cx.update(|cx| {
        entity.update(cx, |panel, cx| {
            panel.pending.toast = Some(PendingToast { message, is_error });
            cx.notify();
        });
    });
}
