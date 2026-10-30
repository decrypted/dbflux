//! The Columns chooser: every column of the grid with a check mark when it
//! is shown, opened from the column header menu's Columns… entry.
//!
//! It is a menu surface of its own so it can stay open while columns are
//! toggled: Enter or Space (or a click) toggles the highlighted row, Escape
//! closes it. It shares the context menu's focus handle and key context,
//! which are free while it is open since the menu that opened it closed.

use super::{DataGridEvent, DataGridPanel};
use dbflux_app::keymap::Command;
use dbflux_components::composites::{
    MenuItem, menu_frame, menu_row, render_menu_header, render_separator,
};
use dbflux_components::icons::AppIcon;
use dbflux_components::tokens::MenuMetrics;
use gpui::*;

/// Width of the chooser, room for a long column name.
const CHOOSER_WIDTH: Pixels = px(240.0);

/// The open chooser: where it hangs and which row the keyboard is on. Row 0
/// is Show all columns, row `n` the `n`-th column in screen order.
pub(super) struct ColumnChooser {
    pub(super) position: Point<Pixels>,
    pub(super) selected_index: usize,
    /// Scrolls the rows, so a keyboard move keeps the highlighted row in
    /// view on a table with more columns than fit.
    pub(super) scroll_handle: ScrollHandle,
}

impl ColumnChooser {
    /// Scrolls row `index` into view. The scrolled list holds the title
    /// first, then Show all, its separator and the column rows; Show all
    /// reveals the title above it too, so the list returns to its top.
    fn reveal(&self, index: usize) {
        let child = if index == 0 { 0 } else { index + 2 };
        self.scroll_handle.scroll_to_item(child);
    }
}

/// A row of the chooser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChooserRow {
    ShowAll,
    /// A model column.
    Column(usize),
}

impl DataGridPanel {
    pub(super) fn open_column_chooser(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.grid_table.table_state.is_none() {
            return;
        }

        self.column_chooser = Some(ColumnChooser {
            position,
            selected_index: 0,
            scroll_handle: ScrollHandle::new(),
        });
        self.focus.context_menu_focus.focus(window, cx);
        cx.emit(DataGridEvent::Focused);
        cx.notify();
    }

    pub(super) fn close_column_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.column_chooser.take().is_some() {
            self.restore_focus_after_context_menu(false, window, cx);
            cx.notify();
        }
    }

    /// The chooser's rows: Show all, then every column, hidden ones in the
    /// place they keep on screen.
    fn column_chooser_rows(&self, cx: &App) -> Vec<ChooserRow> {
        let Some(table_state) = &self.grid_table.table_state else {
            return Vec::new();
        };

        std::iter::once(ChooserRow::ShowAll)
            .chain(
                table_state
                    .read(cx)
                    .column_layout()
                    .order()
                    .iter()
                    .map(|col| ChooserRow::Column(*col)),
            )
            .collect()
    }

    /// Keys while the chooser is open. Returns whether the key was used.
    pub(super) fn dispatch_column_chooser_command(
        &mut self,
        cmd: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let row_count = self.column_chooser_rows(cx).len().max(1);

        match cmd {
            Command::MenuDown | Command::SelectNext => {
                if let Some(chooser) = self.column_chooser.as_mut() {
                    chooser.selected_index = (chooser.selected_index + 1) % row_count;
                    chooser.reveal(chooser.selected_index);
                }
                cx.notify();
                true
            }
            Command::MenuUp | Command::SelectPrev => {
                if let Some(chooser) = self.column_chooser.as_mut() {
                    chooser.selected_index = (chooser.selected_index + row_count - 1) % row_count;
                    chooser.reveal(chooser.selected_index);
                }
                cx.notify();
                true
            }
            Command::MenuSelect | Command::Execute | Command::ExpandCollapse => {
                let selected = self
                    .column_chooser
                    .as_ref()
                    .map_or(0, |chooser| chooser.selected_index);
                self.activate_column_chooser_row(selected, cx);
                true
            }
            Command::MenuBack | Command::Cancel | Command::OpenContextMenu => {
                self.close_column_chooser(window, cx);
                true
            }
            _ => false,
        }
    }

    /// Runs row `index`: Show all shows every column, a column row shows or
    /// hides its column. The chooser stays open.
    fn activate_column_chooser_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.column_chooser_rows(cx).get(index).copied() else {
            return;
        };
        let Some(table_state) = self.grid_table.table_state.clone() else {
            return;
        };

        table_state.update(cx, |state, cx| match row {
            ChooserRow::ShowAll => {
                state.show_all_columns(cx);
            }
            ChooserRow::Column(col) if state.column_layout().is_hidden(col) => {
                state.show_column(col, cx);
            }
            ChooserRow::Column(col) => {
                state.hide_column(col, cx);
            }
        });

        if let Some(chooser) = self.column_chooser.as_mut() {
            chooser.selected_index = index;
        }
        cx.notify();
    }

    pub(super) fn render_column_chooser(
        &self,
        chooser: &ColumnChooser,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = self.column_chooser_rows(cx);
        let (titles, has_hidden, hidden): (Vec<SharedString>, bool, Vec<bool>) = self
            .grid_table
            .table_state
            .as_ref()
            .map(|state| {
                let state = state.read(cx);
                let layout = state.column_layout();
                (
                    state
                        .model()
                        .columns
                        .iter()
                        .map(|column| SharedString::from(column.title.to_string()))
                        .collect(),
                    layout.has_hidden(),
                    (0..layout.col_count())
                        .map(|col| layout.is_hidden(col))
                        .collect(),
                )
            })
            .unwrap_or_default();

        let mut items: Vec<AnyElement> = vec![
            render_menu_header(
                &MenuItem::header(dbflux_i18n::t!(
                    "document.data.context_menu.column.chooser_title"
                ))
                .icon(AppIcon::Columns),
                cx,
            )
            .into_any_element(),
        ];

        for (index, row) in rows.iter().enumerate() {
            let selected = index == chooser.selected_index;
            let element = match *row {
                ChooserRow::ShowAll => {
                    let mut item = MenuItem::new(dbflux_i18n::t!(
                        "document.data.context_menu.column.show_all"
                    ))
                    .icon(AppIcon::Eye);
                    if !has_hidden {
                        item = item.disabled();
                    }
                    menu_row("column-chooser-show-all", &item, selected, cx)
                }
                ChooserRow::Column(col) => {
                    let shown = !hidden.get(col).copied().unwrap_or(false);
                    let title = titles.get(col).cloned().unwrap_or_default();
                    let mut item = MenuItem::new(title.clone());
                    if shown {
                        item = item.icon(AppIcon::Check);
                    }
                    menu_row(
                        SharedString::from(format!("column-chooser-column-{col}")),
                        &item,
                        selected,
                        cx,
                    )
                    .role(Role::MenuItemCheckBox)
                    .aria_label(title)
                    .aria_toggled(if shown {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                }
            };

            items.push(
                element
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(chooser) = this.column_chooser.as_mut()
                            && chooser.selected_index != index
                        {
                            chooser.selected_index = index;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_column_chooser_row(index, cx);
                    }))
                    .into_any_element(),
            );
            if *row == ChooserRow::ShowAll {
                items.push(render_separator(cx).into_any_element());
            }
        }

        let row_height = dbflux_components::fonts::ui_px(cx, MenuMetrics::ROW_HEIGHT);
        let height = row_height * (items.len() as f32) + MenuMetrics::PADDING_Y * 2.0;
        // The chooser hangs where the menu that opened it was, in window
        // coordinates, kept inside the window.
        let placement = super::context_menu::tree::place_menu(
            chooser.position,
            size(CHOOSER_WIDTH, height),
            viewport,
        );

        deferred(
            div()
                .id("column-chooser-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .track_focus(&self.focus.context_menu_focus)
                .on_action(cx.listener(
                    |this, action: &dbflux_ui_base::keymap::RunCommand, window, cx| {
                        let handled =
                            dbflux_ui_base::keymap::run_command(action).is_some_and(|command| {
                                this.dispatch_column_chooser_command(command, window, cx)
                            });

                        if !handled {
                            cx.propagate();
                        }
                    },
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_column_chooser(window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, window, cx| this.close_column_chooser(window, cx)),
                )
                .child(
                    anchored()
                        .position(placement.origin)
                        .snap_to_window_with_margin(super::context_menu::tree::MENU_EDGE_GAP)
                        .child(
                            menu_frame(cx)
                                .id("column-chooser")
                                .debug_selector(|| "column-chooser".to_string())
                                .w(CHOOSER_WIDTH)
                                .occlude()
                                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                                .child(
                                    div()
                                        .id("column-chooser-rows")
                                        .flex()
                                        .flex_col()
                                        .max_h(
                                            (placement.max_height - MenuMetrics::PADDING_Y * 2.0)
                                                .max(px(0.0)),
                                        )
                                        .overflow_y_scroll()
                                        .track_scroll(&chooser.scroll_handle)
                                        .children(items),
                                ),
                        ),
                ),
        )
        .with_priority(1)
    }
}
