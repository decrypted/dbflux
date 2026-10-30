// Explicit imports rather than the parent glob: combining `use super::*`
// with `#[gpui::test]` sends the gpui_macros expansion into unbounded
// recursion.
use super::super::keyboard_coverage_tests::open_manager;
use super::super::{ActiveTab, ConnectionManagerWindow, EditState, FormFocus, MainExtraStop};
use super::same_driver_param_stop;
use crate::keyboard_coverage::CONNECTION_MANAGER;
use dbflux_core::{CompressionChoice, DriverParam};
use dbflux_ui_base::keyboard_coverage::{Coverage, FrameCapture};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

fn select_driver(
    manager: &Entity<ConnectionManagerWindow>,
    window: &mut VisualTestContext,
    driver_id: &str,
) {
    window.update(|window, cx| {
        manager.update(cx, |manager, cx| {
            manager.select_driver(driver_id, window, cx);
            manager.active_tab = ActiveTab::Main;
            manager.edit_state = EditState::Navigating;
            window.focus(&manager.focus_handle, cx);
        })
    });
    window.run_until_parked();
}

fn type_into_editor(
    manager: &Entity<ConnectionManagerWindow>,
    window: &mut VisualTestContext,
    key: &str,
    value: &str,
) {
    window.update(|window, cx| {
        manager.update(cx, |manager, cx| {
            manager
                .form
                .driver_param_key_input
                .update(cx, |state, cx| state.set_value(key.to_string(), window, cx));
            manager
                .form
                .driver_param_value_input
                .update(cx, |state, cx| {
                    state.set_value(value.to_string(), window, cx)
                });
        })
    });
}

fn params(
    manager: &Entity<ConnectionManagerWindow>,
    window: &mut VisualTestContext,
) -> Vec<DriverParam> {
    window.update(|_, cx| manager.read(cx).form.driver_params.clone())
}

fn focus_stop(
    manager: &Entity<ConnectionManagerWindow>,
    window: &mut VisualTestContext,
    stop: MainExtraStop,
) {
    window.update(|window, cx| {
        manager.update(cx, |manager, cx| {
            manager.form_focus = manager
                .main_extra_focus_where(|candidate| same_driver_param_stop(candidate, &stop))
                .expect("the stop is on the ring");
            manager.edit_state = EditState::Navigating;
            window.focus(&manager.focus_handle, cx);
        })
    });
    window.run_until_parked();
}

#[gpui::test]
fn rows_are_added_edited_and_removed(cx: &mut TestAppContext) {
    let (manager, window) = open_manager(cx);
    select_driver(&manager, window, "mysql");

    type_into_editor(&manager, window, "stmt_cache_size", "64");
    focus_stop(&manager, window, MainExtraStop::DriverParamApply);
    window.simulate_keystrokes("enter");
    window.run_until_parked();
    assert_eq!(
        params(&manager, window),
        vec![DriverParam::new("stmt_cache_size", "64")],
        "Enter on Add adds the editor's row"
    );
    assert!(
        window.update(|_, cx| manager
            .read(cx)
            .form
            .driver_param_key_input
            .read(cx)
            .value()
            .is_empty()),
        "the editor is cleared for the next row"
    );

    focus_stop(&manager, window, MainExtraStop::DriverParamEdit(0));
    window.simulate_keystrokes("enter");
    window.run_until_parked();
    let (editing, loaded_value, focus, edit_state) = window.update(|_, cx| {
        let manager = manager.read(cx);
        (
            manager.form.driver_param_editing,
            manager
                .form
                .driver_param_value_input
                .read(cx)
                .value()
                .to_string(),
            manager.form_focus,
            manager.edit_state,
        )
    });
    assert_eq!(editing, Some(0));
    assert_eq!(loaded_value, "64", "Edit loads the row into the editor");
    assert_eq!(
        edit_state,
        EditState::Editing,
        "Edit starts typing in the key"
    );
    assert!(matches!(focus, FormFocus::MainExtra(_)));

    type_into_editor(&manager, window, "stmt_cache_size", "128");
    focus_stop(&manager, window, MainExtraStop::DriverParamApply);
    window.simulate_keystrokes("enter");
    window.run_until_parked();
    assert_eq!(
        params(&manager, window),
        vec![DriverParam::new("stmt_cache_size", "128")],
        "Update writes the row back in place"
    );

    focus_stop(&manager, window, MainExtraStop::DriverParamRemove(0));
    window.simulate_keystrokes("enter");
    window.run_until_parked();
    assert!(params(&manager, window).is_empty(), "Remove drops the row");
}

#[gpui::test]
fn an_unknown_key_is_reported_and_blocks_saving(cx: &mut TestAppContext) {
    let (manager, window) = open_manager(cx);
    select_driver(&manager, window, "mysql");

    type_into_editor(&manager, window, "compresss", "true");
    window.update(|window, cx| {
        manager.update(cx, |manager, cx| manager.apply_driver_param(window, cx))
    });

    let (row_error, valid, errors) = window.update(|_, cx| {
        manager.update(cx, |manager, cx| {
            let row_error = manager.form.driver_param_error.clone();
            let valid = manager.validate_form(false, cx);
            (row_error, valid, manager.validation_errors.clone())
        })
    });

    assert!(
        row_error.is_some_and(|error| error.contains("`compresss`")),
        "the editor names the refused key"
    );
    assert!(!valid);
    assert!(
        errors.iter().any(|error| error.contains("`compresss`")),
        "{errors:?}"
    );
}

#[gpui::test]
fn the_compression_switch_writes_and_reads_the_compress_parameter(cx: &mut TestAppContext) {
    let (manager, window) = open_manager(cx);
    select_driver(&manager, window, "mysql");
    let choice = |window: &mut VisualTestContext| {
        window.update(|_, cx| manager.read(cx).compression_choice())
    };

    assert_eq!(choice(window), Some(CompressionChoice::Auto));
    assert!(params(&manager, window).is_empty(), "Auto is no parameter");

    focus_stop(&manager, window, MainExtraStop::Compression);
    window.simulate_keystrokes("right");
    window.run_until_parked();
    assert_eq!(choice(window), Some(CompressionChoice::On));
    assert_eq!(
        params(&manager, window),
        vec![DriverParam::new("compress", "true")]
    );

    window.simulate_keystrokes("right");
    window.run_until_parked();
    assert_eq!(choice(window), Some(CompressionChoice::Off));
    assert_eq!(
        params(&manager, window),
        vec![DriverParam::new("compress", "false")]
    );

    window.simulate_keystrokes("right");
    window.run_until_parked();
    assert_eq!(choice(window), Some(CompressionChoice::Auto));
    assert!(params(&manager, window).is_empty());

    // A row typed into the list moves the switch too.
    type_into_editor(&manager, window, "compress", "best");
    window.update(|window, cx| {
        manager.update(cx, |manager, cx| manager.apply_driver_param(window, cx))
    });
    assert_eq!(choice(window), Some(CompressionChoice::On));

    let saved = window.update(|_, cx| {
        manager.update(cx, |manager, cx| {
            manager
                .build_profile(cx)
                .map(|profile| profile.driver_params)
        })
    });
    assert_eq!(saved, Some(vec![DriverParam::new("compress", "best")]));
}

#[gpui::test]
fn postgres_has_parameters_but_no_compression_switch(cx: &mut TestAppContext) {
    let (manager, window) = open_manager(cx);
    select_driver(&manager, window, "postgres");

    let (choice, stops) = window.update(|_, cx| {
        let manager = manager.read(cx);
        (manager.compression_choice(), manager.main_extra_stops())
    });

    assert_eq!(choice, None);
    assert!(
        !stops
            .iter()
            .any(|stop| matches!(stop, MainExtraStop::Compression))
    );
    assert!(
        stops
            .iter()
            .any(|stop| matches!(stop, MainExtraStop::DriverParamKey))
    );
}

#[gpui::test]
fn opening_a_profile_loads_its_parameters(cx: &mut TestAppContext) {
    let (manager, window) = open_manager(cx);
    select_driver(&manager, window, "mysql");
    type_into_editor(&manager, window, "compress", "false");
    window.update(|window, cx| {
        manager.update(cx, |manager, cx| manager.apply_driver_param(window, cx))
    });
    let profile = window
        .update(|_, cx| manager.update(cx, |manager, cx| manager.build_profile(cx)))
        .expect("a MySQL profile builds");
    let app_state = window.update(|_, cx| manager.read(cx).app_state.clone());

    let reopened = window.update(|window, cx| {
        cx.new(|cx| ConnectionManagerWindow::new_for_edit(app_state, &profile, window, cx))
    });

    let (loaded, choice) = window.update(|_, cx| {
        let manager = reopened.read(cx);
        (
            manager.form.driver_params.clone(),
            manager.compression_choice(),
        )
    });
    assert_eq!(loaded, vec![DriverParam::new("compress", "false")]);
    assert_eq!(choice, Some(CompressionChoice::Off));
}

/// The rows, the editor in both modes and the Compression switch are
/// covered by the connection manager's keyboard registry.
#[gpui::test]
fn the_driver_parameter_controls_are_covered(cx: &mut TestAppContext) {
    let (manager, window) = open_manager(cx);
    select_driver(&manager, window, "mysql");
    type_into_editor(&manager, window, "stmt_cache_size", "64");
    window.update(|window, cx| {
        manager.update(cx, |manager, cx| {
            manager.apply_driver_param(window, cx);
            manager.edit_driver_param(0, window, cx);
        })
    });
    window.run_until_parked();

    let capture = FrameCapture::observe(window);
    let checked = Coverage::new(CONNECTION_MANAGER).assert_covered(&capture.frame(window));

    for expected in [
        "cm-driver-param-edit-0",
        "cm-driver-param-remove-0",
        "cm-driver-param-apply",
        "cm-driver-param-cancel",
        "segmented-compression-",
    ] {
        assert!(
            checked.iter().any(|id| id.starts_with(expected)),
            "{expected} in {checked:?}"
        );
    }
}
