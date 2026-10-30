//! The driver parameters of the Main tab: an editable `key = value` list and,
//! for a driver that declares one, the Compression switch of the Transport
//! section. Both edit `FormState::driver_params`, so the switch and the list
//! cannot disagree.

use super::*;
use crate::settings::layout;
use crate::tokens::ConnectionFormMetrics;
use dbflux_components::controls::{Button, Input};
use dbflux_components::icons::AppIcon;
use dbflux_components::primitives::{SegmentedControl, SegmentedItem, Text};
use dbflux_core::{CompressionChoice, CompressionParam, DriverParam, DriverParamsSpec};
use gpui::prelude::FluentBuilder;
use gpui_component::ActiveTheme;

impl ConnectionManagerWindow {
    /// The driver parameter spec of the selected driver, if it takes any.
    pub(super) fn driver_params_spec(&self) -> Option<&'static DriverParamsSpec> {
        self.form
            .selected_driver
            .as_ref()
            .and_then(|driver| driver.driver_params_spec())
    }

    pub(super) fn compression_param(&self) -> Option<CompressionParam> {
        self.driver_params_spec().and_then(|spec| spec.compression)
    }

    /// The Main-tab stops of the parameter list, in drawing order: Edit and
    /// Remove of every row, then the editor's key, value, Add (or Update)
    /// and, while a row is edited, Cancel.
    pub(super) fn driver_param_stops(&self) -> Vec<MainExtraStop> {
        if self.driver_params_spec().is_none() {
            return Vec::new();
        }

        let mut stops = Vec::new();
        for index in 0..self.form.driver_params.len() {
            stops.push(MainExtraStop::DriverParamEdit(index));
            stops.push(MainExtraStop::DriverParamRemove(index));
        }
        stops.push(MainExtraStop::DriverParamKey);
        stops.push(MainExtraStop::DriverParamValue);
        stops.push(MainExtraStop::DriverParamApply);
        if self.form.driver_param_editing.is_some() {
            stops.push(MainExtraStop::DriverParamCancel);
        }
        stops
    }

    /// The list a save or test uses: the stored rows plus what the editor
    /// holds, so a typed but not yet added row is not lost.
    pub(super) fn effective_driver_params(&self, cx: &App) -> Vec<DriverParam> {
        let mut params = self.form.driver_params.clone();
        if let Some(pending) = self.pending_driver_param(cx) {
            write_driver_param(&mut params, self.form.driver_param_editing, pending);
        }
        params
    }

    fn pending_driver_param(&self, cx: &App) -> Option<DriverParam> {
        let key = self
            .form
            .driver_param_key_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        if key.is_empty() {
            return None;
        }
        let value = self
            .form
            .driver_param_value_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        Some(DriverParam::new(key, value))
    }

    /// Replaces the parameter list, as when a profile is opened for editing.
    pub(super) fn load_driver_params(
        &mut self,
        params: Vec<DriverParam>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.form.driver_params = params;
        self.clear_driver_param_editor(window, cx);
    }

    fn clear_driver_param_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form.driver_param_editing = None;
        self.form.driver_param_error = None;
        for input in [
            &self.form.driver_param_key_input,
            &self.form.driver_param_value_input,
        ] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    /// Loads row `index` into the editor for changing it.
    pub(super) fn edit_driver_param(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(param) = self.form.driver_params.get(index).cloned() else {
            return;
        };
        self.form.driver_param_editing = Some(index);
        self.form.driver_param_error = None;
        self.form
            .driver_param_key_input
            .update(cx, |state, cx| state.set_value(param.key, window, cx));
        self.form
            .driver_param_value_input
            .update(cx, |state, cx| state.set_value(param.value, window, cx));
        cx.notify();
    }

    pub(super) fn remove_driver_param(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.form.driver_params.len() {
            return;
        }
        self.form.driver_params.remove(index);
        match self.form.driver_param_editing {
            Some(editing) if editing == index => self.clear_driver_param_editor(window, cx),
            Some(editing) if editing > index => {
                self.form.driver_param_editing = Some(editing - 1);
            }
            _ => {}
        }
        self.refresh_driver_param_error();
        cx.notify();
    }

    /// Adds the editor's row, or writes it back over the row being edited.
    pub(super) fn apply_driver_param(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(param) = self.pending_driver_param(cx) else {
            self.form.driver_param_error = Some(dbflux_i18n::t!(
                "connection_manager.driver_params.key_required"
            ));
            cx.notify();
            return;
        };

        let editing = self.form.driver_param_editing;
        write_driver_param(&mut self.form.driver_params, editing, param);

        self.clear_driver_param_editor(window, cx);
        self.refresh_driver_param_error();
        cx.notify();
    }

    pub(super) fn cancel_driver_param_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_driver_param_editor(window, cx);
        self.refresh_driver_param_error();
        cx.notify();
    }

    /// Runs the driver's own checks on the list, so an unknown key or a bad
    /// value shows next to the list as soon as it is added.
    fn refresh_driver_param_error(&mut self) {
        self.form.driver_param_error = self.form.selected_driver.as_ref().and_then(|driver| {
            driver
                .validate_driver_params(&self.form.driver_params)
                .err()
                .map(|error| error.to_string())
        });
    }

    pub(super) fn compression_choice(&self) -> Option<CompressionChoice> {
        self.compression_param()
            .map(|param| param.choice(&self.form.driver_params))
    }

    pub(super) fn set_compression_choice(&mut self, choice: CompressionChoice) {
        if let Some(param) = self.compression_param() {
            param.apply(&mut self.form.driver_params, choice);
            if self.form.driver_param_editing.is_some() {
                // The row under edit may have moved or vanished.
                self.form.driver_param_editing = None;
            }
        }
    }

    /// Moves the Compression switch one step, wrapping at either end.
    pub(super) fn step_compression_choice(&mut self, forward: bool) {
        let Some(current) = self.compression_choice() else {
            return;
        };
        let choices = CompressionChoice::ALL;
        let position = choices
            .iter()
            .position(|choice| *choice == current)
            .unwrap_or(0);
        let next = if forward {
            (position + 1) % choices.len()
        } else {
            (position + choices.len() - 1) % choices.len()
        };
        if let Some(choice) = choices.get(next) {
            self.set_compression_choice(*choice);
        }
    }

    fn driver_param_stop_focused(&self, stop: &MainExtraStop) -> bool {
        self.edit_state == EditState::Navigating
            && self.active_tab == ActiveTab::Main
            && self.main_extra_focus_where(|candidate| same_driver_param_stop(candidate, stop))
                == Some(self.form_focus)
    }

    pub(super) fn render_compression_row(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let current = self.compression_choice()?;
        let entity = cx.entity().clone();

        let items = CompressionChoice::ALL
            .into_iter()
            .map(|choice| SegmentedItem::new(choice.as_str(), compression_label(choice)))
            .collect();

        let control = SegmentedControl::new(
            items,
            current.as_str(),
            move |selected: &SharedString, _window, cx| {
                let Some(choice) = CompressionChoice::from_id(selected) else {
                    return;
                };
                entity.update(cx, |this, cx| {
                    this.set_compression_choice(choice);
                    cx.notify();
                });
            },
        )
        .group("compression")
        .focused(self.driver_param_stop_focused(&MainExtraStop::Compression));

        Some(
            Self::field_row_cm(
                dbflux_i18n::t!("connection_manager.field.compression"),
                false,
                div()
                    .flex()
                    .items_center()
                    .child(control)
                    .child(div().flex_1()),
                Some(dbflux_i18n::t!("connection_manager.field.compression_help")),
                cx,
            )
            .into_any_element(),
        )
    }

    pub(super) fn render_driver_params_section(
        &mut self,
        spec: &'static DriverParamsSpec,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .form
            .driver_params
            .clone()
            .into_iter()
            .enumerate()
            .map(|(index, param)| self.render_driver_param_row(index, param, cx))
            .collect();

        let empty = rows.is_empty().then(|| {
            Text::caption(dbflux_i18n::t!("connection_manager.driver_params.empty"))
                .into_any_element()
        });

        let list = div()
            .id("cm-driver-param-list")
            .flex()
            .flex_col()
            .gap_1()
            .children(rows)
            .children(empty);

        let supported = spec
            .keys
            .iter()
            .map(|key| format!("{} ({})", key.key, key.example))
            .collect::<Vec<_>>()
            .join(", ");

        let help = format!(
            "{} {}",
            dbflux_i18n::t!("connection_manager.driver_params.help"),
            dbflux_i18n::t!(
                "connection_manager.driver_params.supported",
                keys = supported
            ),
        );

        div()
            .flex()
            .flex_col()
            .child(dbflux_components::composites::section_header(
                dbflux_i18n::t!("connection_manager.section.driver_params"),
                Some(AppIcon::Settings.into()),
                cx,
            ))
            .child(Self::field_row_cm(
                dbflux_i18n::t!("connection_manager.driver_params.label"),
                false,
                list,
                Some(help),
                cx,
            ))
            .child(self.render_driver_param_editor(cx))
            .into_any_element()
    }

    fn render_driver_param_row(
        &self,
        index: usize,
        param: DriverParam,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editing = self.form.driver_param_editing == Some(index);
        let text = format!("{} = {}", param.key, param.value);

        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w_0().child(if editing {
                Text::code(text).color(cx.theme().accent)
            } else {
                Text::code(text)
            }))
            .child(
                Button::new(
                    SharedString::from(format!("cm-driver-param-edit-{index}")),
                    dbflux_i18n::t!("connection_manager.driver_params.edit"),
                )
                .ghost()
                .inline()
                .icon(AppIcon::Pencil)
                .aria_label(dbflux_i18n::t!(
                    "connection_manager.driver_params.edit_named",
                    key = param.key.clone()
                ))
                .focused(self.driver_param_stop_focused(&MainExtraStop::DriverParamEdit(index)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.edit_driver_param(index, window, cx);
                })),
            )
            .child(
                Button::new(
                    SharedString::from(format!("cm-driver-param-remove-{index}")),
                    dbflux_i18n::t!("connection_manager.driver_params.remove"),
                )
                .ghost()
                .inline()
                .icon(AppIcon::X)
                .aria_label(dbflux_i18n::t!(
                    "connection_manager.driver_params.remove_named",
                    key = param.key
                ))
                .focused(self.driver_param_stop_focused(&MainExtraStop::DriverParamRemove(index)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.remove_driver_param(index, window, cx);
                })),
            )
            .into_any_element()
    }

    fn render_driver_param_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let key_focus = self.main_extra_focus_where(|stop| {
            same_driver_param_stop(stop, &MainExtraStop::DriverParamKey)
        });
        let value_focus = self.main_extra_focus_where(|stop| {
            same_driver_param_stop(stop, &MainExtraStop::DriverParamValue)
        });
        let editing = self.form.driver_param_editing.is_some();

        let apply_label = if editing {
            dbflux_i18n::t!("connection_manager.driver_params.update")
        } else {
            dbflux_i18n::t!("connection_manager.driver_params.add")
        };

        let controls = layout::inline_controls()
            .child(
                self.cm_control_frame(
                    self.driver_param_stop_focused(&MainExtraStop::DriverParamKey),
                    key_focus,
                    true,
                    Some(ConnectionFormMetrics::SOURCE_SELECT_WIDTH),
                    true,
                    Input::new(&self.form.driver_param_key_input)
                        .id("cm-driver-param-key")
                        .aria_label(dbflux_i18n::t!(
                            "connection_manager.driver_params.key_label"
                        )),
                    cx,
                ),
            )
            .child(
                self.cm_control_frame(
                    self.driver_param_stop_focused(&MainExtraStop::DriverParamValue),
                    value_focus,
                    true,
                    None,
                    true,
                    Input::new(&self.form.driver_param_value_input)
                        .id("cm-driver-param-value")
                        .aria_label(dbflux_i18n::t!(
                            "connection_manager.driver_params.value_label"
                        )),
                    cx,
                ),
            )
            .child(
                Button::new("cm-driver-param-apply", apply_label)
                    .secondary()
                    .icon(if editing {
                        AppIcon::Check
                    } else {
                        AppIcon::Plus
                    })
                    .focused(self.driver_param_stop_focused(&MainExtraStop::DriverParamApply))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.apply_driver_param(window, cx);
                    })),
            )
            .when(editing, |controls| {
                controls.child(
                    Button::new(
                        "cm-driver-param-cancel",
                        dbflux_i18n::t!("connection_manager.driver_params.cancel"),
                    )
                    .ghost()
                    .focused(self.driver_param_stop_focused(&MainExtraStop::DriverParamCancel))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cancel_driver_param_edit(window, cx);
                    })),
                )
            });

        let help = self
            .form
            .driver_param_error
            .clone()
            .unwrap_or_else(|| dbflux_i18n::t!("connection_manager.driver_params.editor_help"));

        Self::field_row_cm(
            dbflux_i18n::t!("connection_manager.driver_params.editor_label"),
            false,
            controls,
            Some(help),
            cx,
        )
        .into_any_element()
    }
}

/// Writes `param` over row `editing` when it exists, otherwise appends it.
fn write_driver_param(params: &mut Vec<DriverParam>, editing: Option<usize>, param: DriverParam) {
    match editing.and_then(|index| params.get_mut(index)) {
        Some(row) => *row = param,
        None => params.push(param),
    }
}

/// Whether two stops are the same Compression or parameter-list stop.
/// `MainExtraStop` has no `PartialEq` because it carries field definitions.
pub(super) fn same_driver_param_stop(left: &MainExtraStop, right: &MainExtraStop) -> bool {
    use MainExtraStop::*;

    match (left, right) {
        (DriverParamEdit(a), DriverParamEdit(b)) | (DriverParamRemove(a), DriverParamRemove(b)) => {
            a == b
        }
        (Compression, Compression)
        | (DriverParamKey, DriverParamKey)
        | (DriverParamValue, DriverParamValue)
        | (DriverParamApply, DriverParamApply)
        | (DriverParamCancel, DriverParamCancel) => true,
        _ => false,
    }
}

fn compression_label(choice: CompressionChoice) -> String {
    match choice {
        CompressionChoice::Auto => dbflux_i18n::t!("connection_manager.compression.option.auto"),
        CompressionChoice::On => dbflux_i18n::t!("connection_manager.compression.option.enabled"),
        CompressionChoice::Off => dbflux_i18n::t!("connection_manager.compression.option.disabled"),
    }
}

#[cfg(test)]
mod tests;
