//! Vim mode in the value editor.

use super::super::{ValuePanelContent, ValuePanelTarget};
use crate::keyboard_test_support::{KeymapHost, host_document, init_keyboard_runtime};
use dbflux_app::keymap::{Command, ContextId};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

/// The panel under a keymap host that routes Cancel the way the grid's
/// inspector does: out of the editor into the panel.
fn open_panel(
    cx: &mut TestAppContext,
    vim: bool,
) -> (Entity<ValuePanelContent>, &mut VisualTestContext) {
    let (_host, panel, window) = open_panel_with_host(cx, vim);
    (panel, window)
}

fn open_panel_with_host(
    cx: &mut TestAppContext,
    vim: bool,
) -> (
    Entity<KeymapHost<ValuePanelContent>>,
    Entity<ValuePanelContent>,
    &mut VisualTestContext,
) {
    init_keyboard_runtime(cx);
    cx.update(|cx| dbflux_components::vim::set_vim_enabled(cx, vim));

    let (host, window) = host_document(
        cx,
        |window, cx| {
            cx.new(|cx| {
                ValuePanelContent::new(
                    ValuePanelTarget {
                        row: 0,
                        col: 0,
                        column_name: "notes".to_string(),
                        value: "ab\ncd".to_string(),
                        bytes: None,
                        editable: true,
                    },
                    window,
                    cx,
                )
            })
        },
        |panel, _cx| {
            if panel.editor_has_focus() {
                ContextId::TextInput
            } else {
                ContextId::Inspector
            }
        },
        |panel, command, window, cx| {
            if command == Command::Cancel && panel.editor_has_focus() {
                panel.leave_editor(window, cx);
                return true;
            }
            false
        },
    );
    let panel = window.update(|_, cx| host.read(cx).document.clone());
    window.update(|window, cx| panel.update(cx, |panel, cx| panel.focus_editor(window, cx)));
    window.run_until_parked();

    (host, panel, window)
}

fn text_and_cursor(
    panel: &Entity<ValuePanelContent>,
    window: &mut VisualTestContext,
) -> (String, usize) {
    window.update(|_, cx| {
        let state = panel.read(cx).input.read(cx);
        (state.value().to_string(), state.cursor())
    })
}

/// With Vim mode on, `j` moves, the first Escape leaves Insert mode and
/// keeps the editor, and Escape in Normal mode leaves the editor.
#[gpui::test]
fn vim_mode_in_the_value_editor(cx: &mut TestAppContext) {
    let (panel, window) = open_panel(cx, true);

    window.simulate_keystrokes("j");
    window.run_until_parked();
    assert_eq!(text_and_cursor(&panel, window), ("ab\ncd".into(), 3));

    window.simulate_keystrokes("i");
    window.simulate_input("X");
    window.simulate_keystrokes("escape");
    window.run_until_parked();
    assert_eq!(text_and_cursor(&panel, window).0, "ab\nXcd");
    assert!(window.update(|_, cx| panel.read(cx).editor_has_focus()));

    window.simulate_keystrokes("escape");
    window.run_until_parked();
    assert!(!window.update(|_, cx| panel.read(cx).editor_has_focus()));
}

/// With Vim mode off, keys type and Escape leaves the editor at once.
#[gpui::test]
fn without_vim_mode_escape_leaves_the_editor(cx: &mut TestAppContext) {
    let (panel, window) = open_panel(cx, false);

    window.simulate_input("j");
    window.simulate_keystrokes("escape");
    window.run_until_parked();

    assert_eq!(text_and_cursor(&panel, window).0, "jab\ncd");
    assert!(!window.update(|_, cx| panel.read(cx).editor_has_focus()));
}

/// The leader works outside the code editor: `space a` in Normal mode
/// reaches the host as the pane-actions command, and Insert mode types
/// the space.
#[gpui::test]
fn leader_sequences_reach_the_host_from_the_value_editor(cx: &mut TestAppContext) {
    let _keymap_state = dbflux_ui_base::keymap::keymap_state_test_guard();
    let (host, panel, window) = open_panel_with_host(cx, true);
    let host_commands =
        |window: &mut VisualTestContext| window.update(|_, cx| host.read(cx).commands.clone());

    window.simulate_keystrokes("space a");
    window.run_until_parked();
    assert_eq!(host_commands(window), vec![Command::OpenPaneActions]);
    assert_eq!(text_and_cursor(&panel, window), ("ab\ncd".into(), 0));

    window.simulate_keystrokes("i space escape");
    window.run_until_parked();
    assert_eq!(text_and_cursor(&panel, window).0, " ab\ncd");
    assert_eq!(host_commands(window).len(), 1);
}
