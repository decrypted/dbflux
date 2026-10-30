//! Identifiers generated on the client for a grid cell (UUID v4 and v7,
//! KSUID, ULID): which columns can take one and the menu entries that write
//! them.

use std::collections::HashMap;

use dbflux_components::components::data_table::selection::CellCoord;
use dbflux_components::components::data_table::{ContextMenuAction, GeneratedIdOffer};
use dbflux_components::icons::AppIcon;
use dbflux_core::{ColumnInfo, GeneratedIdKind, looks_like_id_column};
use gpui::{App, Context};

use super::{ContextMenuItem, DataGridPanel, DataSource, TableContextMenu};

impl DataGridPanel {
    /// The identifier kinds each result column can store as text, keyed by
    /// column index, for a table or collection; a query result saves nothing.
    ///
    /// The type comes from the table's schema when it is loaded, since only
    /// it tells a `char(27)` from a `char(36)`. A binary column takes none:
    /// the grid writes text, and a `binary(16)` uuid needs the raw bytes.
    pub(super) fn generated_id_offers_by_column(
        &self,
        column_details: Option<&[ColumnInfo]>,
    ) -> HashMap<usize, GeneratedIdOffer> {
        if !matches!(
            self.source,
            DataSource::Table { .. } | DataSource::Collection { .. }
        ) {
            return HashMap::new();
        }

        self.result
            .columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| {
                let info = column_details
                    .and_then(|details| details.iter().find(|info| info.name == column.name));
                let type_name = info
                    .map(|info| info.type_name.as_str())
                    .unwrap_or(column.type_name.as_str());
                let is_key = column.is_primary_key || info.is_some_and(|info| info.is_primary_key);

                let kinds = GeneratedIdKind::fitting(type_name);
                (!kinds.is_empty()).then(|| {
                    (
                        index,
                        GeneratedIdOffer {
                            kinds,
                            in_editor: looks_like_id_column(&column.name, type_name, is_key),
                        },
                    )
                })
            })
            .collect()
    }

    /// One "Generate …" entry per identifier kind the cell under the menu
    /// can take.
    pub(super) fn generate_id_items(
        &self,
        menu: &TableContextMenu,
        cx: &App,
    ) -> Vec<ContextMenuItem> {
        if menu.is_document_view {
            return Vec::new();
        }
        let Some(table_state) = &self.grid_table.table_state else {
            return Vec::new();
        };

        table_state
            .read(cx)
            .generated_id_kinds(menu.col)
            .iter()
            .map(|&kind| ContextMenuItem {
                label: dbflux_i18n::t!(
                    "document.data.context_menu.item.generate_id",
                    kind = kind.label()
                )
                .into(),
                action: Some(ContextMenuAction::GenerateId(kind)),
                icon: Some(AppIcon::KeyRound),
                is_separator: false,
                is_danger: false,
                detail: None,
                disabled: false,
            })
            .collect()
    }

    /// Writes a new `kind` identifier into the cell, or one per cell into
    /// every selected cell that can take it.
    pub(super) fn handle_generate_id(
        &mut self,
        row: usize,
        col: usize,
        kind: GeneratedIdKind,
        cx: &mut Context<Self>,
    ) {
        if let Some(table_state) = &self.grid_table.table_state {
            table_state.update(cx, |state, cx| {
                state.generate_ids(kind, CellCoord::new(row, col), cx);
            });
        }
    }
}
