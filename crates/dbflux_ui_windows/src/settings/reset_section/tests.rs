// Explicit imports: the parent's `gpui::*` glob would make `#[test]` resolve
// to `gpui::test`, whose expansion recurses without bound.
use super::{ConfirmStep, ROWS, ResetRow, ResetSection};
use crate::settings::keyboard_tests::{focus_content, open_settings};
use crate::settings::{ActiveSettingsSection, SettingsCoordinator, SettingsSectionId};
use dbflux_storage::ResetCategory;
use dbflux_ui_base::AppStateEntity;
use gpui::{Entity, TestAppContext, VisualTestContext};

const QUERY_HISTORY: ResetRow = ResetRow::Category(ResetCategory::QueryHistory);
const AUDIT_LOG: ResetRow = ResetRow::Category(ResetCategory::AuditLog);

fn open_reset(
    cx: &mut TestAppContext,
) -> (
    Entity<SettingsCoordinator>,
    Entity<ResetSection>,
    Entity<AppStateEntity>,
    &mut VisualTestContext,
) {
    let (settings, window) = open_settings(cx, SettingsSectionId::Reset);
    window.update(|_, cx| cx.set_reduce_motion(true));

    let (section, app_state) = window.update(|_, cx| {
        let settings = settings.read(cx);
        let section = match &settings.active_section_entity {
            ActiveSettingsSection::Reset(section) => section.clone(),
            _ => unreachable!("the reset section is open"),
        };
        (section, settings.app_state.clone())
    });

    (settings, section, app_state, window)
}

/// Two remembered column layouts, three history entries, a remembered cell
/// editor size and two audit events.
fn seed(
    app_state: &Entity<AppStateEntity>,
    section: &Entity<ResetSection>,
    window: &mut VisualTestContext,
) {
    window.update(|_, cx| {
        app_state.update(cx, |state, _| {
            let ui = state.storage_runtime().ui_state();
            ui.set("grid_column_layout.v1:[\"a\"]", "{}").expect("seed");
            ui.set("grid_column_layout.v1:[\"b\"]", "{}").expect("seed");
            ui.set(
                "cell_editor_geometry",
                r#"{"width":1.0,"height":1.0,"maximized":true}"#,
            )
            .expect("seed");

            for sql in ["SELECT 1", "SELECT 2", "SELECT 3"] {
                state.add_history_entry(dbflux_core::HistoryEntry::new(
                    sql.to_string(),
                    None,
                    None,
                    std::time::Duration::from_millis(1),
                    None,
                ));
            }

            state
                .storage_runtime()
                .dbflux_db()
                .execute_batch(
                    "INSERT INTO aud_audit_events (actor_id, tool_id, decision, created_at_epoch_ms)
                     VALUES ('user', 'query', 'allow', 1700000000000),
                            ('user', 'query', 'allow', 1760000000000);",
                )
                .expect("seed audit");
        });
        section.update(cx, |section, cx| {
            section.refresh_stats(cx);
            cx.notify();
        });
    });
    window.run_until_parked();
}

fn stored(
    app_state: &Entity<AppStateEntity>,
    category: ResetCategory,
    window: &mut VisualTestContext,
) -> u64 {
    window.update(|_, cx| {
        app_state
            .read(cx)
            .stored_state_stats(category)
            .expect("stats")
            .count
    })
}

/// Clicks the confirmation dialog's button drawn with `button_id`.
fn click_dialog_button(window: &mut VisualTestContext, button_id: &str) {
    use dbflux_components::accessibility::accessibility_node;
    use dbflux_ui_base::keyboard_coverage::FrameCapture;

    let capture = FrameCapture::observe(window);
    let frame = capture.frame(window);
    let bounds = accessibility_node(&frame, button_id)
        .and_then(|node| node.bounds())
        .unwrap_or_else(|| panic!("the confirmation draws its {button_id} button"));
    let scale = window.update(|window, _| window.scale_factor()) as f64;
    let center = gpui::point(
        gpui::px(((bounds.x0 + bounds.x1) / 2.0 / scale) as f32),
        gpui::px(((bounds.y0 + bounds.y1) / 2.0 / scale) as f32),
    );

    window.simulate_click(center, Default::default());
    window.run_until_parked();
}

fn request(section: &Entity<ResetSection>, row: ResetRow, window: &mut VisualTestContext) {
    window.update(|_, cx| section.update(cx, |section, cx| section.request_reset(row, cx)));
    window.run_until_parked();
}

#[gpui::test]
fn every_row_renders_a_reset_button_with_its_count(cx: &mut TestAppContext) {
    use dbflux_components::accessibility::accessibility_node;
    use dbflux_ui_base::keyboard_coverage::FrameCapture;

    let (_settings, section, app_state, window) = open_reset(cx);
    seed(&app_state, &section, window);

    let capture = FrameCapture::observe(window);
    let frame = capture.frame(window);
    for row in ROWS {
        assert!(
            accessibility_node(&frame, row.button_id().as_ref()).is_some(),
            "{row:?} draws its button"
        );
    }

    window.update(|_, cx| {
        let section = section.read(cx);
        assert_eq!(
            section.count_label(ResetRow::Category(ResetCategory::ColumnLayouts), cx),
            dbflux_i18n::t!("settings.reset.count.many", count = 2)
        );
        assert_eq!(
            section.count_label(QUERY_HISTORY, cx),
            dbflux_i18n::t!("settings.reset.count.many", count = 3)
        );
        assert_eq!(
            section.count_label(ResetRow::Category(ResetCategory::WindowLayout), cx),
            dbflux_i18n::t!("settings.reset.count.one")
        );
        assert_eq!(
            section.count_label(ResetRow::Category(ResetCategory::ExportOptions), cx),
            dbflux_i18n::t!("settings.reset.count.none")
        );
        assert_eq!(
            section.count_label(ResetRow::Category(ResetCategory::GeneralSettings), cx),
            dbflux_i18n::t!("settings.reset.count.defaults")
        );
        // The app records events of its own, so the newest one is today's.
        let audit = section.count_label(AUDIT_LOG, cx);
        let events = section.stats[&ResetCategory::AuditLog].count;
        assert!(events >= 2, "{events}");
        assert!(
            audit.starts_with(&dbflux_i18n::t!(
                "settings.reset.count.events_range",
                count = events,
                oldest = "2023-11-14",
                newest = ""
            )),
            "{audit}"
        );
        // The bulk row counts what it would clear, not the audit log.
        assert_eq!(
            section.count_label(ResetRow::Everything, cx),
            dbflux_i18n::t!("settings.reset.count.many", count = 6)
        );
    });
}

#[gpui::test]
fn cancel_closes_the_confirmation_and_deletes_nothing(cx: &mut TestAppContext) {
    let (_settings, section, app_state, window) = open_reset(cx);
    seed(&app_state, &section, window);

    request(&section, QUERY_HISTORY, window);
    assert!(window.update(|_, cx| section.read(cx).pending.is_some()));

    click_dialog_button(window, "cancel");

    assert!(window.update(|_, cx| section.read(cx).pending.is_none()));
    assert_eq!(stored(&app_state, ResetCategory::QueryHistory, window), 3);
    assert_eq!(
        window.update(|_, cx| app_state.read(cx).history_entries().len()),
        3
    );
}

#[gpui::test]
fn confirming_a_row_clears_only_that_category(cx: &mut TestAppContext) {
    let (_settings, section, app_state, window) = open_reset(cx);
    seed(&app_state, &section, window);

    let audit_before = stored(&app_state, ResetCategory::AuditLog, window);
    request(&section, QUERY_HISTORY, window);
    click_dialog_button(window, "ok");

    assert!(window.update(|_, cx| section.read(cx).pending.is_none()));
    assert_eq!(stored(&app_state, ResetCategory::QueryHistory, window), 0);
    assert!(window.update(|_, cx| app_state.read(cx).history_entries().is_empty()));
    assert_eq!(stored(&app_state, ResetCategory::ColumnLayouts, window), 2);
    assert_eq!(
        stored(&app_state, ResetCategory::AuditLog, window),
        audit_before + 1,
        "the reset is audited"
    );
    window.update(|_, cx| {
        assert_eq!(
            section.read(cx).count_label(QUERY_HISTORY, cx),
            dbflux_i18n::t!("settings.reset.count.none"),
            "the count refreshes after the reset"
        );
    });
}

/// "Everything above" takes a second confirmation and leaves the audit log
/// alone; the audit log takes its own second confirmation.
#[gpui::test]
fn the_bulk_and_audit_resets_take_two_confirmations(cx: &mut TestAppContext) {
    let (_settings, section, app_state, window) = open_reset(cx);
    seed(&app_state, &section, window);

    let audit_before = stored(&app_state, ResetCategory::AuditLog, window);
    request(&section, ResetRow::Everything, window);
    click_dialog_button(window, "ok");

    let step = window.update(|_, cx| section.read(cx).pending.map(|pending| pending.step));
    assert_eq!(
        step,
        Some(ConfirmStep::Final),
        "the first OK only asks again"
    );
    assert_eq!(stored(&app_state, ResetCategory::ColumnLayouts, window), 2);

    click_dialog_button(window, "ok");

    for category in ResetCategory::BULK {
        if category != ResetCategory::GeneralSettings {
            assert_eq!(stored(&app_state, category, window), 0, "{category:?}");
        }
    }
    assert_eq!(
        stored(&app_state, ResetCategory::AuditLog, window),
        audit_before + 1,
        "the bulk reset keeps the audit log"
    );
    assert!(window.update(|_, cx| app_state.read(cx).session_persistence_held()));

    let audit_before = stored(&app_state, ResetCategory::AuditLog, window);
    request(&section, AUDIT_LOG, window);
    click_dialog_button(window, "ok");
    assert_eq!(
        stored(&app_state, ResetCategory::AuditLog, window),
        audit_before,
        "the first OK only asks again"
    );
    click_dialog_button(window, "ok");

    // What remains was written after the clear: the record of the reset
    // (and any log line the audit log captured meanwhile).
    let audit = window.update(|_, cx| {
        app_state
            .read(cx)
            .stored_state_stats(ResetCategory::AuditLog)
            .expect("stats")
    });
    assert!(audit.count >= 1 && audit.count < audit_before, "{audit:?}");
    let (oldest, _) = audit.range.expect("the record of the reset");
    assert!(
        oldest.as_str() > "2025-10-09T08:53:20Z",
        "the seeded events are gone: {oldest}"
    );
}

#[gpui::test]
fn the_keyboard_moves_over_the_rows_and_opens_a_confirmation(cx: &mut TestAppContext) {
    let (settings, section, app_state, window) = open_reset(cx);
    seed(&app_state, &section, window);
    focus_content(&settings, window);

    window.simulate_keystrokes("j");
    assert_eq!(window.update(|_, cx| section.read(cx).cursor), 1);

    window.simulate_keystrokes("enter");
    let pending = window.update(|_, cx| section.read(cx).pending.map(|pending| pending.row));
    assert_eq!(
        pending,
        Some(ResetRow::Category(ResetCategory::ColumnLayouts))
    );

    window.simulate_keystrokes("escape");
    assert!(window.update(|_, cx| section.read(cx).pending.is_none()));
    assert_eq!(stored(&app_state, ResetCategory::ColumnLayouts, window), 2);
}

#[gpui::test]
fn the_reset_buttons_are_on_the_keyboard_coverage_registry(cx: &mut TestAppContext) {
    use crate::keyboard_coverage::SETTINGS;
    use dbflux_ui_base::keyboard_coverage::{Coverage, FrameCapture};

    let (_settings, section, app_state, window) = open_reset(cx);
    seed(&app_state, &section, window);
    let capture = FrameCapture::observe(window);

    let checked = Coverage::new(SETTINGS).assert_covered(&capture.frame(window));
    assert!(
        checked.iter().any(|id| id == "reset-query-history"),
        "{checked:?}"
    );

    request(&section, AUDIT_LOG, window);
    let checked = Coverage::new(SETTINGS).assert_covered(&capture.frame(window));
    for button in ["ok", "cancel"] {
        assert!(
            checked.iter().any(|id| id == button),
            "{button}: {checked:?}"
        );
    }
}

#[test]
fn the_reset_copy_resolves_in_every_locale() {
    let mut keys: Vec<String> = [
        "settings.nav.reset",
        "settings.reset.title",
        "settings.reset.subtitle",
        "settings.reset.intro",
        "settings.reset.button",
        "settings.reset.error",
        "settings.reset.groups.interface",
        "settings.reset.groups.history",
        "settings.reset.groups.settings",
        "settings.reset.groups.everything",
        "settings.reset.groups.audit",
        "settings.reset.count.none",
        "settings.reset.count.one",
        "settings.reset.count.many",
        "settings.reset.count.customized",
        "settings.reset.count.defaults",
        "settings.reset.count.events_range",
        "settings.reset.confirm.title",
        "settings.reset.confirm.final_title",
        "settings.reset.confirm.includes",
        "settings.reset.confirm.current",
        "settings.reset.confirm.untouched",
        "settings.reset.confirm.final_body",
        "settings.reset.confirm.irreversible",
        "settings.reset.confirm.continue",
        "settings.reset.confirm.reset",
        "settings.reset.confirm.reset_permanently",
        "settings.reset.confirm.cancel",
        "settings.reset.toast.done",
        "settings.reset.toast.restart_note",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    for row in ROWS {
        keys.push(format!("settings.reset.rows.{}.name", row.key()));
        keys.push(format!("settings.reset.rows.{}.description", row.key()));
    }

    for key in &keys {
        for locale in ["en", "es", "ko", "pt_BR", "zh_Hans"] {
            let value = dbflux_i18n::t!(key, locale = locale);

            assert!(!value.is_empty(), "{key} resolved empty for {locale}");
            assert_ne!(
                value,
                format!("{locale}.{key}"),
                "{key} missing in {locale}"
            );
            assert_ne!(value, *key, "{key} missing in {locale}");
        }
    }
}
