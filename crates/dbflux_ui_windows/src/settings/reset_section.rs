//! Settings › Reset: clears categories of stored state (remembered sizes,
//! layouts, histories, the session, the general settings, the audit log)
//! after a confirmation, without touching connections or their profiles.

use super::section_trait::SectionFocusEvent;
use super::{SettingsSection, SettingsSectionId};
use dbflux_app::keymap::Modifiers;
use dbflux_components::controls::Button;
use dbflux_components::icons::AppIcon;
use dbflux_components::primitives::Text;
use dbflux_components::tokens::{FontSizes, Spacing, ui};
use dbflux_core::GeneralSettings;
use dbflux_storage::{ResetCategory, ResetStats};
use dbflux_ui_base::keymap::key_chord_from_gpui;
use dbflux_ui_base::toast::{Toast, now_hms};
use dbflux_ui_base::user_error::{ErrorKind, UserFacingError, report_error};
use dbflux_ui_base::{AppStateChanged, AppStateEntity};
use gpui::prelude::*;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::dialog::{AlertDialog, DialogButtonProps};
use gpui_component::scroll::ScrollableElement;
use std::collections::HashMap;

/// Width of the name column of a row.
const ROW_LABEL_WIDTH: Rems = ui(200.0);

/// Vertical padding of a row.
const ROW_PADDING_Y: Pixels = Spacing::SM;

/// One row of the page: a category, or every category of the bulk reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResetRow {
    Category(ResetCategory),
    Everything,
}

/// The rows in visual order, which is also the keyboard ring. The audit log
/// comes after "Everything above" because that reset leaves it alone.
pub(super) const ROWS: [ResetRow; 11] = [
    ResetRow::Category(ResetCategory::WindowLayout),
    ResetRow::Category(ResetCategory::ColumnLayouts),
    ResetRow::Category(ResetCategory::FilterHistory),
    ResetRow::Category(ResetCategory::ExportOptions),
    ResetRow::Category(ResetCategory::Session),
    ResetRow::Category(ResetCategory::QueryHistory),
    ResetRow::Category(ResetCategory::RecentFiles),
    ResetRow::Category(ResetCategory::SchemaSnapshots),
    ResetRow::Category(ResetCategory::GeneralSettings),
    ResetRow::Everything,
    ResetRow::Category(ResetCategory::AuditLog),
];

impl ResetRow {
    /// The categories the row clears.
    pub(super) fn categories(self) -> &'static [ResetCategory] {
        match self {
            ResetRow::Everything => &ResetCategory::BULK,
            ResetRow::Category(category) => match category {
                ResetCategory::WindowLayout => &[ResetCategory::WindowLayout],
                ResetCategory::ColumnLayouts => &[ResetCategory::ColumnLayouts],
                ResetCategory::FilterHistory => &[ResetCategory::FilterHistory],
                ResetCategory::ExportOptions => &[ResetCategory::ExportOptions],
                ResetCategory::Session => &[ResetCategory::Session],
                ResetCategory::QueryHistory => &[ResetCategory::QueryHistory],
                ResetCategory::RecentFiles => &[ResetCategory::RecentFiles],
                ResetCategory::SchemaSnapshots => &[ResetCategory::SchemaSnapshots],
                ResetCategory::GeneralSettings => &[ResetCategory::GeneralSettings],
                ResetCategory::AuditLog => &[ResetCategory::AuditLog],
            },
        }
    }

    /// Rows whose reset takes a second confirmation: everything at once, and
    /// the audit log, which records what happened and is cleared on its own.
    pub(super) fn needs_final_confirmation(self) -> bool {
        matches!(
            self,
            ResetRow::Everything | ResetRow::Category(ResetCategory::AuditLog)
        )
    }

    /// Element id of the row's button.
    pub(super) fn button_id(self) -> SharedString {
        match self {
            ResetRow::Everything => "reset-everything".into(),
            ResetRow::Category(category) => format!("reset-{}", category.id()).into(),
        }
    }

    fn key(self) -> &'static str {
        match self {
            ResetRow::Everything => "everything",
            ResetRow::Category(category) => match category {
                ResetCategory::WindowLayout => "window_layout",
                ResetCategory::ColumnLayouts => "column_layouts",
                ResetCategory::FilterHistory => "filter_history",
                ResetCategory::ExportOptions => "export_options",
                ResetCategory::Session => "session",
                ResetCategory::QueryHistory => "query_history",
                ResetCategory::RecentFiles => "recent_files",
                ResetCategory::SchemaSnapshots => "schema_snapshots",
                ResetCategory::GeneralSettings => "general_settings",
                ResetCategory::AuditLog => "audit_log",
            },
        }
    }

    pub(super) fn name(self) -> String {
        dbflux_i18n::t!(&format!("settings.reset.rows.{}.name", self.key()))
    }

    pub(super) fn description(self) -> String {
        dbflux_i18n::t!(&format!("settings.reset.rows.{}.description", self.key()))
    }

    /// What applies only after a restart, shown in the toast after the reset.
    fn after_reset_note(self) -> Option<String> {
        let restart = self.categories().iter().any(|category| {
            matches!(
                category,
                ResetCategory::Session | ResetCategory::WindowLayout
            )
        });

        restart.then(|| dbflux_i18n::t!("settings.reset.toast.restart_note"))
    }
}

/// Which step of the confirmation is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfirmStep {
    First,
    Final,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PendingReset {
    pub(super) row: ResetRow,
    pub(super) step: ConfirmStep,
}

pub(super) struct ResetSection {
    app_state: Entity<AppStateEntity>,
    pub(super) stats: HashMap<ResetCategory, ResetStats>,
    pub(super) pending: Option<PendingReset>,
    content_focused: bool,
    pub(super) cursor: usize,
}

impl EventEmitter<SectionFocusEvent> for ResetSection {}

impl ResetSection {
    pub(super) fn new(
        app_state: Entity<AppStateEntity>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut section = Self {
            app_state,
            stats: HashMap::new(),
            pending: None,
            content_focused: false,
            cursor: 0,
        };
        section.refresh_stats(cx);
        section
    }

    pub(super) fn refresh_stats(&mut self, cx: &App) {
        let state = self.app_state.read(cx);

        self.stats = ResetCategory::ALL
            .into_iter()
            .map(|category| {
                let stats = state.stored_state_stats(category).unwrap_or_else(|error| {
                    log::warn!("Could not count the stored {category:?}: {error}");
                    ResetStats::default()
                });
                (category, stats)
            })
            .collect();
    }

    fn total(&self, row: ResetRow) -> u64 {
        row.categories()
            .iter()
            .filter(|category| **category != ResetCategory::GeneralSettings)
            .filter_map(|category| self.stats.get(category))
            .map(|stats| stats.count)
            .sum()
    }

    fn general_settings_customized(&self, cx: &App) -> bool {
        *self.app_state.read(cx).general_settings() != GeneralSettings::default()
    }

    /// The live count of a row, as shown next to its name.
    pub(super) fn count_label(&self, row: ResetRow, cx: &App) -> String {
        match row {
            ResetRow::Category(ResetCategory::GeneralSettings) => {
                if self.general_settings_customized(cx) {
                    dbflux_i18n::t!("settings.reset.count.customized")
                } else {
                    dbflux_i18n::t!("settings.reset.count.defaults")
                }
            }
            ResetRow::Category(ResetCategory::AuditLog) => {
                let stats = self
                    .stats
                    .get(&ResetCategory::AuditLog)
                    .cloned()
                    .unwrap_or_default();

                match (stats.count, stats.range) {
                    (0, _) => dbflux_i18n::t!("settings.reset.count.none"),
                    (count, Some((oldest, newest))) => dbflux_i18n::t!(
                        "settings.reset.count.events_range",
                        count = count,
                        oldest = day_of(&oldest),
                        newest = day_of(&newest)
                    ),
                    (count, None) => count_items(count),
                }
            }
            _ => count_items(self.total(row)),
        }
    }

    /// Whether the row holds anything its reset would remove.
    fn has_anything(&self, row: ResetRow, cx: &App) -> bool {
        self.total(row) > 0
            || (row.categories().contains(&ResetCategory::GeneralSettings)
                && self.general_settings_customized(cx))
    }

    pub(super) fn request_reset(&mut self, row: ResetRow, cx: &mut Context<Self>) {
        self.select(row);
        self.pending = Some(PendingReset {
            row,
            step: ConfirmStep::First,
        });
        cx.notify();
    }

    pub(super) fn cancel_reset(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        cx.notify();
    }

    /// The dialog's Reset button: moves to the final step where the row
    /// takes one, otherwise clears the row's categories.
    pub(super) fn confirm_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending else {
            return;
        };

        if pending.step == ConfirmStep::First && pending.row.needs_final_confirmation() {
            self.pending = Some(PendingReset {
                step: ConfirmStep::Final,
                ..pending
            });
            cx.notify();
            return;
        }

        self.pending = None;
        self.run_reset(pending.row, window, cx);
    }

    fn run_reset(&mut self, row: ResetRow, window: &mut Window, cx: &mut Context<Self>) {
        let categories = row.categories();

        let result = self.app_state.update(cx, |state, cx| {
            let result = state.reset_stored_state(categories);
            cx.emit(AppStateChanged);
            result
        });

        let removed = match result {
            Ok(removed) => removed,
            Err(error) => {
                report_error(
                    UserFacingError::new(
                        ErrorKind::Storage,
                        dbflux_i18n::t!("settings.reset.error", name = row.name()),
                    )
                    .with_cause(error.to_string()),
                    cx,
                );
                self.refresh_stats(cx);
                cx.notify();
                return;
            }
        };

        if categories.contains(&ResetCategory::WindowLayout) {
            // The open app keeps the cell editor's size in a global; without
            // this the next editor would open at the forgotten size again.
            dbflux_components::modals::forget_editor_geometry(cx);
        }

        if categories.contains(&ResetCategory::GeneralSettings) {
            let defaults = GeneralSettings::default();
            dbflux_components::density::set_style(cx, defaults.style);
            dbflux_components::theme::apply_theme(defaults.theme, defaults.style, Some(window), cx);
        }

        self.record_audit_event(row, removed, cx);

        let mut message = dbflux_i18n::t!(
            "settings.reset.toast.done",
            name = row.name(),
            count = removed
        );
        if let Some(note) = row.after_reset_note() {
            message = format!("{message} {note}");
        }
        Toast::success(message).meta_right(now_hms()).push(cx);

        self.refresh_stats(cx);
        cx.notify();
    }

    /// A reset removes what the user may later want to account for, so it
    /// leaves a record; after clearing the audit log this is its first entry.
    fn record_audit_event(&self, row: ResetRow, removed: u64, cx: &App) {
        use dbflux_core::observability::{
            EventCategory, EventOrigin, EventOutcome, EventRecord, EventSeverity, actions,
        };

        let ids: Vec<&str> = row.categories().iter().map(|c| c.id()).collect();
        let event = EventRecord::new(
            dbflux_core::chrono::Utc::now().timestamp_millis(),
            EventSeverity::Info,
            EventCategory::Config,
            EventOutcome::Success,
        )
        .with_typed_action(actions::CONFIG_DELETE)
        .with_summary(format!(
            "Reset stored state: {} ({removed} entries removed)",
            ids.join(", ")
        ))
        .with_origin(EventOrigin::local())
        .with_object_ref("stored_state", ids.join(","));

        if let Err(error) = self.app_state.read(cx).audit_service().record(event) {
            log::warn!("Could not record the stored state reset: {error}");
        }
    }

    fn select(&mut self, row: ResetRow) {
        self.content_focused = true;
        if let Some(position) = ROWS.iter().position(|candidate| *candidate == row) {
            self.cursor = position;
        }
    }

    fn is_at(&self, row: ResetRow) -> bool {
        self.content_focused && ROWS.get(self.cursor).copied() == Some(row)
    }

    fn render_group(&self, title: String, icon: AppIcon, cx: &App) -> AnyElement {
        dbflux_components::composites::section_header(title, Some(icon.into()), cx)
            .into_any_element()
    }

    fn render_row(&self, row: ResetRow, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let has_anything = self.has_anything(row, cx);

        let button = Button::new(row.button_id(), dbflux_i18n::t!("settings.reset.button"))
            .danger()
            .icon(AppIcon::RotateCcw)
            .disabled(!has_anything)
            .focused(self.is_at(row))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select(row);
                this.request_reset(row, cx);
            }));

        div()
            .flex()
            .items_start()
            .gap(Spacing::LG)
            .py(ROW_PADDING_Y)
            .child(
                div()
                    .w(ROW_LABEL_WIDTH)
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(Spacing::XXS)
                    .child(Text::body(row.name()))
                    .child(
                        Text::code(self.count_label(row, cx))
                            .font_size(FontSizes::XS)
                            .text_color(muted),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Text::body_sm(row.description()).text_color(muted)),
            )
            .child(div().flex_shrink_0().child(button))
            .into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col();

        body = body
            .child(
                div().py(Spacing::MD).child(
                    Text::body_sm(dbflux_i18n::t!("settings.reset.intro")).muted_foreground(),
                ),
            )
            .child(self.render_group(
                dbflux_i18n::t!("settings.reset.groups.interface"),
                AppIcon::Layers,
                cx,
            ));

        for (index, row) in ROWS.iter().copied().enumerate() {
            match index {
                4 => {
                    body = body.child(self.render_group(
                        dbflux_i18n::t!("settings.reset.groups.history"),
                        AppIcon::History,
                        cx,
                    ))
                }
                8 => {
                    body = body.child(self.render_group(
                        dbflux_i18n::t!("settings.reset.groups.settings"),
                        AppIcon::Settings,
                        cx,
                    ))
                }
                9 => {
                    body = body.child(self.render_group(
                        dbflux_i18n::t!("settings.reset.groups.everything"),
                        AppIcon::RotateCcw,
                        cx,
                    ))
                }
                10 => {
                    body = body.child(self.render_group(
                        dbflux_i18n::t!("settings.reset.groups.audit"),
                        AppIcon::FingerprintPattern,
                        cx,
                    ))
                }
                _ => {}
            }

            body = body.child(self.render_row(row, cx));
        }

        body.into_any_element()
    }

    /// The lines of the confirmation: what goes, how much, and that it is
    /// final.
    fn confirmation_body(&self, pending: PendingReset, cx: &App) -> Vec<String> {
        let row = pending.row;
        let mut lines = Vec::new();

        match pending.step {
            ConfirmStep::First => {
                lines.push(row.description());
                if row == ResetRow::Everything {
                    let names: Vec<String> = ResetCategory::BULK
                        .into_iter()
                        .map(|category| ResetRow::Category(category).name())
                        .collect();
                    lines.push(dbflux_i18n::t!(
                        "settings.reset.confirm.includes",
                        names = names.join(", ")
                    ));
                }
                lines.push(dbflux_i18n::t!(
                    "settings.reset.confirm.current",
                    count = self.count_label(row, cx)
                ));
                lines.push(dbflux_i18n::t!("settings.reset.confirm.untouched"));
            }
            ConfirmStep::Final => {
                lines.push(dbflux_i18n::t!(
                    "settings.reset.confirm.final_body",
                    name = row.name(),
                    count = self.count_label(row, cx)
                ));
            }
        }

        lines.push(dbflux_i18n::t!("settings.reset.confirm.irreversible"));
        lines
    }

    fn render_confirmation(&self, pending: PendingReset, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity().clone();
        let entity_cancel = entity.clone();
        let row = pending.row;

        let (title, ok_text) = match (pending.step, row.needs_final_confirmation()) {
            (ConfirmStep::First, true) => (
                dbflux_i18n::t!("settings.reset.confirm.title", name = row.name()),
                dbflux_i18n::t!("settings.reset.confirm.continue"),
            ),
            (ConfirmStep::First, false) => (
                dbflux_i18n::t!("settings.reset.confirm.title", name = row.name()),
                dbflux_i18n::t!("settings.reset.confirm.reset"),
            ),
            (ConfirmStep::Final, _) => (
                dbflux_i18n::t!("settings.reset.confirm.final_title", name = row.name()),
                dbflux_i18n::t!("settings.reset.confirm.reset_permanently"),
            ),
        };

        let lines = self.confirmation_body(pending, cx);

        AlertDialog::new(cx)
            .title(title)
            .button_props(
                DialogButtonProps::default()
                    .show_cancel(true)
                    .ok_text(ok_text)
                    .ok_variant(gpui_component::button::ButtonVariant::Danger)
                    .cancel_text(dbflux_i18n::t!("settings.reset.confirm.cancel")),
            )
            .close_button(false)
            .on_ok(move |_, window, cx| {
                entity.update(cx, |section, cx| section.confirm_reset(window, cx));
                true
            })
            .on_cancel(move |_, _, cx| {
                entity_cancel.update(cx, |section, cx| section.cancel_reset(cx));
                true
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(Spacing::SM)
                    .text_sm()
                    .children(lines.into_iter().map(Text::body)),
            )
            .into_any_element()
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let chord = key_chord_from_gpui(&event.keystroke);

        if self.pending.is_some() {
            // The dialog takes Enter and Escape while it holds the keyboard;
            // an Escape that still reaches the page cancels, nothing confirms.
            if chord.key == "escape" && chord.modifiers == Modifiers::none() {
                self.cancel_reset(cx);
            }
            return;
        }

        match (chord.key.as_str(), chord.modifiers) {
            ("j", modifiers) | ("down", modifiers) | ("tab", modifiers)
                if modifiers == Modifiers::none() =>
            {
                self.cursor = (self.cursor + 1).min(ROWS.len() - 1);
                cx.notify();
            }
            ("k", modifiers) | ("up", modifiers) if modifiers == Modifiers::none() => {
                self.cursor = self.cursor.saturating_sub(1);
                cx.notify();
            }
            ("tab", modifiers) if modifiers == Modifiers::shift() => {
                self.cursor = self.cursor.saturating_sub(1);
                cx.notify();
            }
            ("g", modifiers) if modifiers == Modifiers::none() => {
                self.cursor = 0;
                cx.notify();
            }
            ("g", modifiers) if modifiers == Modifiers::shift() => {
                self.cursor = ROWS.len() - 1;
                cx.notify();
            }
            ("enter", modifiers) | ("space", modifiers) if modifiers == Modifiers::none() => {
                if let Some(row) = ROWS.get(self.cursor).copied()
                    && self.has_anything(row, cx)
                {
                    self.request_reset(row, cx);
                }
            }
            ("escape", modifiers) if modifiers == Modifiers::none() => {
                cx.emit(SectionFocusEvent::RequestFocusReturn);
            }
            _ => {}
        }
    }
}

/// The day of an RFC 3339 timestamp.
fn day_of(timestamp: &str) -> String {
    timestamp.get(..10).unwrap_or(timestamp).to_string()
}

fn count_items(count: u64) -> String {
    match count {
        0 => dbflux_i18n::t!("settings.reset.count.none"),
        1 => dbflux_i18n::t!("settings.reset.count.one"),
        count => dbflux_i18n::t!("settings.reset.count.many", count = count),
    }
}

impl SettingsSection for ResetSection {
    fn section_id(&self) -> SettingsSectionId {
        SettingsSectionId::Reset
    }

    fn handle_key_event(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.content_focused {
            return;
        }

        self.handle_key(event, cx);
    }

    fn focus_in(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.content_focused = true;
        self.refresh_stats(cx);
        cx.notify();
    }

    fn focus_out(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.content_focused = false;
        cx.notify();
    }
}

impl Render for ResetSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.render_body(cx);
        let confirmation = self
            .pending
            .map(|pending| self.render_confirmation(pending, cx));

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_shrink_0()
                    .pb(crate::tokens::SettingsMetrics::PAGE_HEAD_PADDING_BOTTOM - Spacing::SM)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(dbflux_components::composites::page_header(
                        dbflux_i18n::t!("settings.reset.title"),
                        dbflux_i18n::t!("settings.reset.subtitle"),
                        cx,
                    )),
            )
            .child(
                div()
                    .id("reset-settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px(crate::tokens::SettingsMetrics::BODY_PADDING_X)
                    .pb(Spacing::XL)
                    .child(body),
            )
            .children(confirmation)
    }
}

#[cfg(test)]
mod tests;
