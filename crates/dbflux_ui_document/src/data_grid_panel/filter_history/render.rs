use super::super::DataGridPanel;
use super::{search_words, word_ranges};
use crate::data_grid_panel::utils::single_line;
use dbflux_components::composites::{MenuItem, menu_frame, menu_row, render_separator};
use dbflux_components::controls::GpuiInput;
use dbflux_components::icons::AppIcon;
use dbflux_components::primitives::Icon;
use dbflux_components::tokens::{Fields, Heights, Radii, Spacing};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{ActiveTheme, Sizable};

impl DataGridPanel {
    /// The history toggle drawn where the WHERE field shows its filter icon.
    pub(in crate::data_grid_panel) fn render_filter_history_toggle(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let open = self.filter_bar.history.open;
        let color = if open {
            theme.foreground
        } else {
            theme.muted_foreground
        };

        div()
            .id("filter-history-toggle")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(Heights::ICON_MD)
            .rounded(Radii::SM)
            .cursor_pointer()
            .when(open, |d| d.bg(theme.secondary))
            .hover(|d| d.bg(theme.secondary))
            .aria_label(dbflux_i18n::t!("document.data.grid.filter.history.toggle"))
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                // The press on this toggle already closed the open popup
                // (it is outside the popup); the click must not reopen it.
                let dismissed_here = match event {
                    ClickEvent::Mouse(click) => {
                        this.filter_bar.history.dismiss_press == Some(click.down.position)
                    }
                    _ => false,
                };
                this.filter_bar.history.dismiss_press = None;

                if dismissed_here {
                    return;
                }
                if this.filter_bar.history.open {
                    this.close_filter_history(cx);
                } else {
                    this.open_filter_history(window, cx);
                }
            }))
            .child(
                Icon::new(AppIcon::History)
                    .size(Fields::FILTER_ICON)
                    .color(color),
            )
            .into_any_element()
    }

    /// The search field at the top of the popup. Up / Down / Enter / Escape /
    /// Shift+Delete act on the list while it keeps the keyboard; they are
    /// caught before the input, which would otherwise move its caret or
    /// delete a character.
    fn render_filter_history_search(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_none()
            .px(Spacing::SM)
            .pb(Spacing::XS)
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveUp, _, cx| {
                    this.step_filter_history(false, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::MoveDown, _, cx| {
                    this.step_filter_history(true, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Escape, window, cx| {
                    this.filter_history_escape(window, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Delete, window, cx| {
                    if window.modifiers().shift && this.delete_selected_filter_history(cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                GpuiInput::new(&self.filter_bar.history_search)
                    .small()
                    .aria_label(dbflux_i18n::t!("document.data.grid.filter.history.search")),
            )
            .into_any_element()
    }

    /// The popup under the WHERE field: its search field, then the table's
    /// filters, newest first, narrowed to the search, then "Clear history".
    pub(in crate::data_grid_panel) fn render_filter_history_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let popup = &self.filter_bar.history;
        if !popup.open {
            return None;
        }

        let theme = cx.theme().clone();
        let shown = popup.shown();
        let selected = popup.selected;
        let words = search_words(&popup.query);

        let mut rows: Vec<AnyElement> = shown
            .iter()
            .enumerate()
            .filter_map(|(row, &index)| Some((row, index, popup.entries.get(index)?)))
            .map(|(row, index, entry)| {
                let label = single_line(entry);
                let highlights = word_ranges(&label, &words);
                let item = MenuItem::new(label)
                    .icon(AppIcon::History)
                    .label_highlights(highlights);
                let muted = theme.muted_foreground;
                let hover_bg = theme.secondary;

                menu_row(
                    SharedString::from(format!("filter-history-entry-{index}")),
                    &item,
                    selected == Some(row),
                    cx,
                )
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.filter_bar.history.selected != Some(row) {
                        this.filter_bar.history.selected = Some(row);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_filter_history_entry(index, window, cx);
                }))
                .child(
                    div()
                        .id(SharedString::from(format!("filter-history-delete-{index}")))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(Heights::ICON_MD)
                        .rounded(Radii::SM)
                        .hover(move |d| d.bg(hover_bg))
                        .aria_label(dbflux_i18n::t!("document.data.grid.filter.history.delete"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_filter_history_entry(index, cx);
                        }))
                        .child(Icon::new(AppIcon::X).small().color(muted)),
                )
                .into_any_element()
            })
            .collect();

        if shown.is_empty() && !popup.entries.is_empty() {
            rows.push(
                div()
                    .px(Spacing::MD)
                    .py(Spacing::XS)
                    .text_color(theme.muted_foreground)
                    .child(dbflux_i18n::t!(
                        "document.data.grid.filter.history.no_match"
                    ))
                    .into_any_element(),
            );
        }

        if popup.entries.is_empty() {
            rows.push(
                div()
                    .px(Spacing::MD)
                    .py(Spacing::XS)
                    .text_color(theme.muted_foreground)
                    .child(dbflux_i18n::t!("document.data.grid.filter.history.empty"))
                    .into_any_element(),
            );
        } else {
            let clear_row = shown.len();
            rows.push(render_separator(cx).into_any_element());
            rows.push(
                menu_row(
                    "filter-history-clear",
                    &MenuItem::new(dbflux_i18n::t!("document.data.grid.filter.history.clear"))
                        .icon(AppIcon::Delete)
                        .danger(),
                    selected == Some(clear_row),
                    cx,
                )
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.filter_bar.history.selected != Some(clear_row) {
                        this.filter_bar.history.selected = Some(clear_row);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.clear_filter_history(cx);
                }))
                .into_any_element(),
            );
        }

        Some(
            deferred(
                menu_frame(cx)
                    .id("filter-history-menu")
                    .absolute()
                    .top_full()
                    .left_0()
                    .mt(Spacing::XS)
                    .w(px(520.0)) // guardrail-allow: history popup width, fits a long filter
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.filter_bar.history.dismiss_press = Some(event.position);
                        this.close_filter_history(cx);
                    }))
                    .child(self.render_filter_history_search(cx))
                    .child(
                        div()
                            .id("filter-history-list")
                            .flex()
                            .flex_col()
                            .max_h(px(320.0)) // guardrail-allow: history list height, about 11 rows
                            .overflow_y_scroll()
                            .track_scroll(&popup.scroll)
                            .children(rows),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
