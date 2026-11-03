//! Escape in a chart tab, through the real keymap and close path, and the
//! chart a code editor's button opens.

use crate::ui::document::{CodeDocument, Tab};
use crate::ui::views::workspace::{FocusTarget, Workspace};
use dbflux_core::document_id::DocumentId;
use dbflux_core::{
    ColumnKind, ColumnMeta, ConnectionProfile, DbConfig, DbKind, ExecutionContext, QueryLanguage,
    QueryResult, Value, WritePrivilege,
};
use dbflux_test_support::FakeDriver;
use dbflux_ui_base::AppStateEntity;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::time::Duration;
use uuid::Uuid;

use super::new_workspace;

const QUERY: &str = "select * from api_call_log";

fn column(name: &str, kind: ColumnKind) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        type_name: String::new(),
        kind,
        nullable: true,
        is_primary_key: false,
    }
}

fn chart_result() -> QueryResult {
    QueryResult::table(
        vec![
            column("ts", ColumnKind::Timestamp),
            column("value", ColumnKind::Float),
        ],
        vec![
            vec![Value::Int(0), Value::Float(1.0)],
            vec![Value::Int(1_000), Value::Float(2.0)],
        ],
        None,
        Duration::ZERO,
    )
}

struct Fixture<'a> {
    workspace: Entity<Workspace>,
    app_state: Entity<AppStateEntity>,
    window: &'a mut VisualTestContext,
    driver: FakeDriver,
    profile_id: Uuid,
}

fn connected_workspace(cx: &mut TestAppContext) -> Fixture<'_> {
    let (workspace, app_state, window) = new_workspace(cx);

    let driver = FakeDriver::new(DbKind::MySQL).with_default_result(chart_result());
    let profile = ConnectionProfile::new(
        "test",
        DbConfig::SQLite {
            path: ":memory:".into(),
            connection_id: None,
        },
    );
    let profile_id = profile.id;
    let connection = driver.connect_arc(&profile).expect("fake connection");

    window.update(|window, cx| {
        window.activate_window();
        app_state.update(cx, |state, _| {
            state.apply_connect_profile(
                profile,
                connection,
                None,
                None,
                false,
                WritePrivilege::Unknown,
            );
        });
    });

    Fixture {
        workspace,
        app_state,
        window,
        driver,
        profile_id,
    }
}

impl Fixture<'_> {
    fn open_code_tab(&mut self, content: &str) -> (DocumentId, Entity<CodeDocument>) {
        let app_state = self.app_state.clone();
        let workspace = self.workspace.clone();
        let profile_id = self.profile_id;
        let content = content.to_string();

        let tab = self.window.update(|window, cx| {
            let document = cx.new(|cx| {
                let mut document = CodeDocument::new_with_language(
                    app_state,
                    Some(profile_id),
                    QueryLanguage::Sql,
                    window,
                    cx,
                );
                document.set_content(&content, window, cx);
                document
            });
            let id = document.read(cx).id();

            workspace.update(cx, |workspace, cx| {
                let pane = CodeDocument::into_pane(document.clone(), cx);
                workspace.tab_manager.update(cx, |manager, cx| {
                    manager.open(Tab::Pane(Box::new(pane)), cx);
                });
            });

            (id, document)
        });
        self.settle();

        tab
    }

    /// Opens a chart of [`QUERY`] from `source` and lets its first run land.
    fn open_chart_from(&mut self, source: DocumentId) -> DocumentId {
        let workspace = self.workspace.clone();
        let profile_id = self.profile_id;

        self.window.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_chart_from_query(
                    QUERY.to_string(),
                    Some(profile_id),
                    ExecutionContext::default(),
                    false,
                    source,
                    window,
                    cx,
                );
            });
        });
        self.settle();

        self.active().expect("the chart tab is active")
    }

    /// Lets queued work land and draws until nothing is left to do: a chart
    /// starts its run on its first render and applies the result on the next.
    fn settle(&mut self) {
        for _ in 0..3 {
            self.window.run_until_parked();
            self.window.update(|window, _| window.refresh());
            self.window.run_until_parked();
        }
    }

    fn active(&mut self) -> Option<DocumentId> {
        let workspace = self.workspace.clone();
        self.window
            .update(|_, cx| workspace.read(cx).tab_manager.read(cx).active_id())
    }

    fn is_open(&mut self, id: DocumentId) -> bool {
        let workspace = self.workspace.clone();
        self.window.update(|_, cx| {
            workspace
                .read(cx)
                .tab_manager
                .read(cx)
                .document(id)
                .is_some()
        })
    }

    fn activate(&mut self, id: DocumentId) {
        let workspace = self.workspace.clone();
        self.window.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace
                    .tab_manager
                    .update(cx, |manager, cx| manager.activate(id, cx));
                workspace.set_focus(FocusTarget::Document, window, cx);
            });
        });
        self.window.run_until_parked();
    }

    fn press(&mut self, keys: &str) {
        for key in keys.split(' ') {
            self.window.simulate_keystrokes(key);
            self.window.run_until_parked();
        }
    }
}

/// One press per layer, then the tab closes and the keyboard is back in the
/// tab the chart came from, not in the tab used most recently before it.
#[gpui::test]
fn escape_clears_the_point_then_closes_the_tab_and_returns_to_its_source(cx: &mut TestAppContext) {
    let mut fixture = connected_workspace(cx);
    let (source, document) = fixture.open_code_tab("");
    let (other, _) = fixture.open_code_tab("");

    fixture.activate(source);
    let chart = fixture.open_chart_from(source);
    fixture.activate(other);
    fixture.activate(chart);

    fixture.press("l");
    fixture.press("escape");
    assert!(fixture.is_open(chart), "the first press clears the point");

    fixture.press("escape");
    assert!(!fixture.is_open(chart), "the second closes the tab");
    assert_eq!(fixture.active(), Some(source));

    fixture.settle();
    fixture.window.simulate_input("typed");
    let text = fixture
        .window
        .update(|_, cx| document.read(cx).buffer_text_for_test(cx));
    assert_eq!(text, "typed", "the keyboard is in the source tab's editor");
}

/// Only Escape goes back to the source. The close shortcut and the tab's close
/// button keep the usual order, the most recently used tab.
#[gpui::test]
fn the_close_shortcut_keeps_the_usual_next_tab(cx: &mut TestAppContext) {
    use crate::keymap::{Command, CommandDispatcher as _};

    let mut fixture = connected_workspace(cx);
    let (source, _) = fixture.open_code_tab("");
    let (other, _) = fixture.open_code_tab("");

    fixture.activate(source);
    let chart = fixture.open_chart_from(source);
    fixture.activate(other);
    fixture.activate(chart);

    let workspace = fixture.workspace.clone();
    fixture.window.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.dispatch(Command::CloseCurrentTab, window, cx);
        });
    });
    fixture.window.run_until_parked();

    assert!(!fixture.is_open(chart));
    assert_eq!(fixture.active(), Some(other));
}

/// The source tab may be gone by then: the usual order picks the next tab.
#[gpui::test]
fn escape_without_the_source_tab_goes_to_the_next_tab(cx: &mut TestAppContext) {
    let mut fixture = connected_workspace(cx);
    let (source, _) = fixture.open_code_tab("");
    let (other, _) = fixture.open_code_tab("");
    let chart = fixture.open_chart_from(source);

    let workspace = fixture.workspace.clone();
    fixture.window.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.close_tab(source, window, cx);
        });
    });
    fixture.activate(chart);

    fixture.press("escape");
    assert!(!fixture.is_open(chart));
    assert_eq!(fixture.active(), Some(other));
}

/// Escape reaches the document from anywhere, so the tab closes only while it
/// holds the keyboard.
#[gpui::test]
fn escape_in_the_sidebar_leaves_the_chart_tab_open(cx: &mut TestAppContext) {
    let mut fixture = connected_workspace(cx);
    let (source, _) = fixture.open_code_tab("");
    let chart = fixture.open_chart_from(source);

    let workspace = fixture.workspace.clone();
    fixture.window.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.set_focus(FocusTarget::Sidebar, window, cx);
        });
    });
    fixture.window.run_until_parked();

    fixture.press("escape");
    assert!(fixture.is_open(chart));
}

/// A text field in the chart holds the keyboard: its Escape propagates to the
/// workspace, which only takes the focus off it. The next press closes.
#[gpui::test]
fn a_focused_field_keeps_escape(cx: &mut TestAppContext) {
    use crate::keymap::{Command, CommandDispatcher as _};

    let mut fixture = connected_workspace(cx);
    let (source, _) = fixture.open_code_tab("");
    let chart = fixture.open_chart_from(source);

    let workspace = fixture.workspace.clone();
    let _field = fixture.window.update(|window, cx| {
        let field = cx.focus_handle();
        field.focus(window, cx);
        field
    });
    fixture.window.run_until_parked();

    fixture.window.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.dispatch(Command::Cancel, window, cx));
            assert!(
                workspace.focus_handle.is_focused(window),
                "the field lost the keyboard"
            );
        });
    });
    assert!(fixture.is_open(chart), "the press stopped at the field");

    fixture.window.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.dispatch(Command::Cancel, window, cx);
        });
    });
    fixture.window.run_until_parked();
    assert!(!fixture.is_open(chart));
}

/// A changed setting would be lost with no way back, so the first press warns.
#[gpui::test]
fn escape_over_changed_settings_warns_and_the_second_press_closes(cx: &mut TestAppContext) {
    let mut fixture = connected_workspace(cx);
    let (source, _) = fixture.open_code_tab("");
    let chart = fixture.open_chart_from(source);

    fixture.press("alt-l");
    fixture.press("escape");
    assert!(fixture.is_open(chart), "the first press only warns");

    fixture.press("escape");
    assert!(!fixture.is_open(chart));
}

#[gpui::test]
fn escape_in_the_pane_actions_menu_only_closes_the_menu(cx: &mut TestAppContext) {
    let mut fixture = connected_workspace(cx);
    let (source, _) = fixture.open_code_tab("");
    let chart = fixture.open_chart_from(source);

    fixture.press("m");
    let workspace = fixture.workspace.clone();
    let menu_open = |window: &mut VisualTestContext| {
        window.update(|_, cx| workspace.read(cx).has_pane_actions_menu())
    };
    assert!(menu_open(fixture.window));

    fixture.press("escape");
    assert!(!menu_open(fixture.window));
    assert!(fixture.is_open(chart));
}

/// A tab with no chart keeps what Escape did before.
#[gpui::test]
fn escape_does_not_close_a_code_tab(cx: &mut TestAppContext) {
    let mut fixture = connected_workspace(cx);
    let (tab, _) = fixture.open_code_tab("");

    fixture.activate(tab);
    fixture.press("escape");
    fixture.press("escape");
    assert!(fixture.is_open(tab));
}

/// The button's request crosses the tab manager and the workspace intact: the
/// chart runs the statement, in the database, and closes back to its source.
#[gpui::test]
fn the_button_opens_a_chart_of_the_statement_in_the_consoles_database(cx: &mut TestAppContext) {
    let mut fixture = connected_workspace(cx);
    let (source, document) = fixture.open_code_tab("select 1 from a;\n\nselect 2 from b");

    let app_state = fixture.app_state.clone();
    let profile_id = fixture.profile_id;
    fixture.window.update(|_, cx| {
        app_state.update(cx, |state, _| {
            state.set_active_database(profile_id, Some("analytics".to_string()));
        });
        document.update(cx, |document, cx| document.emit_chart_this_query(cx));
    });
    fixture.settle();

    let chart = fixture.active().expect("a tab is active");
    assert_ne!(chart, source, "the chart tab took over");

    let request = fixture
        .driver
        .stats()
        .executed_requests
        .last()
        .cloned()
        .expect("the chart ran its query");
    assert_eq!(request.sql, "select 1 from a");
    assert_eq!(request.database.as_deref(), Some("analytics"));

    fixture.activate(chart);
    fixture.press("escape");
    assert_eq!(fixture.active(), Some(source));
}
