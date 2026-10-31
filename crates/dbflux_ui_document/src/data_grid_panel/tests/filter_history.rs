//! The WHERE field's filter history, through the real read path.

use super::*;
use crate::keyboard_test_support::{KeymapHost, host_document, init_keyboard_runtime};
use dbflux_core::{DbError, TableBrowseRequest};
use gpui::Focusable as _;
use std::sync::Mutex;

/// Connection that records the filter of every table read and fails the
/// reads whose filter is `failing`.
pub(super) struct FilterRecordingConnection {
    pub filters: Arc<Mutex<Vec<Option<String>>>>,
    pub failing: Option<String>,
    pub rows: QueryResult,
}

impl dbflux_core::Connection for FilterRecordingConnection {
    fn collection_facets(&self, _: &dbflux_core::CollectionRef) -> Vec<dbflux_core::ObjectFacet> {
        dbflux_core::category_collection_facets(self.metadata().category)
    }

    fn metadata(&self) -> &dbflux_core::DriverMetadata {
        use dbflux_core::{
            DatabaseCategory, DriverCapabilities, DriverMetadata, Icon as CoreIcon, QueryLanguage,
            TransferFamily,
        };

        static META: std::sync::OnceLock<DriverMetadata> = std::sync::OnceLock::new();
        META.get_or_init(|| DriverMetadata {
            id: "stub-filter".to_string(),
            display_name: "Stub".to_string(),
            description: "test".to_string(),
            category: DatabaseCategory::Relational,
            transfer_family: TransferFamily::Sql,
            deployment_class: None,
            query_language: QueryLanguage::Sql,
            capabilities: DriverCapabilities::empty(),
            default_port: None,
            uri_scheme: "stub".to_string(),
            icon: CoreIcon::Database,
            syntax: None,
            query: None,
            mutation: None,
            ddl: None,
            transactions: None,
            limits: None,
            ssl_modes: None,
            ssl_cert_fields: None,
            classification_override: None,
            default_chunk_size: None,
            supports_lock_timeout: false,
            editor_profile: None,
        })
    }

    fn kind(&self) -> dbflux_core::DbKind {
        dbflux_core::DbKind::SQLite
    }

    fn schema_loading_strategy(&self) -> dbflux_core::SchemaLoadingStrategy {
        dbflux_core::SchemaLoadingStrategy::SingleDatabase
    }

    fn dialect(&self) -> &dyn dbflux_core::SqlDialect {
        static DIALECT: dbflux_core::DefaultSqlDialect = dbflux_core::DefaultSqlDialect;
        &DIALECT
    }

    fn ping(&self) -> Result<(), DbError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }

    fn execute(&self, _: &dbflux_core::QueryRequest) -> Result<QueryResult, DbError> {
        Err(DbError::NotSupported("stub".to_string()))
    }

    fn browse_table(&self, request: &TableBrowseRequest) -> Result<QueryResult, DbError> {
        let mut filters = self
            .filters
            .lock()
            .map_err(|_| DbError::NotSupported("poisoned".to_string()))?;
        filters.push(request.filter.clone());

        if request.filter.is_some() && request.filter == self.failing {
            return Err(DbError::NotSupported("syntax error".to_string()));
        }
        Ok(self.rows.clone())
    }

    fn cancel(&self, _: &dbflux_core::QueryHandle) -> Result<(), DbError> {
        Ok(())
    }

    fn schema(&self) -> Result<dbflux_core::SchemaSnapshot, DbError> {
        Ok(dbflux_core::SchemaSnapshot::default())
    }
}

/// The reads a [`FilterRecordingConnection`] answered, by filter.
pub(super) type ReadLog = Arc<Mutex<Vec<Option<String>>>>;

pub(super) fn reads(log: &ReadLog) -> Vec<Option<String>> {
    log.lock().map(|reads| reads.clone()).unwrap_or_default()
}

/// Registers a connected profile answering table reads with `rows`.
pub(super) fn register_filter_stub(
    app_state: &gpui::Entity<AppStateEntity>,
    rows: QueryResult,
    failing: Option<&str>,
    cx: &mut gpui::App,
) -> (Uuid, ReadLog) {
    let profile_id = Uuid::new_v4();
    let filters: ReadLog = Arc::default();

    app_state.update(cx, |app, _cx| {
        let profile = dbflux_core::ConnectionProfile::new(
            "test",
            dbflux_core::DbConfig::SQLite {
                path: std::path::PathBuf::from(":memory:"),
                connection_id: None,
            },
        );
        let connected = dbflux_core::ConnectedProfile {
            profile,
            connection: Arc::new(FilterRecordingConnection {
                filters: filters.clone(),
                failing: failing.map(str::to_string),
                rows,
            }),
            schema: None,
            mutation_policy: dbflux_core::MutationPolicy::default(),
            read_only_reason: None,
            database_schemas: Default::default(),
            table_details: Default::default(),
            collection_children: Default::default(),
            schema_types: Default::default(),
            schema_columns: Default::default(),
            schema_indexes: Default::default(),
            schema_foreign_keys: Default::default(),
            schema_routines: Default::default(),
            dependents_cache: Default::default(),
            active_database: None,
            redis_key_cache: Default::default(),
            database_connections: Default::default(),
            proxy_tunnel: None,
        };
        app.connections_mut().insert(profile_id, connected);
    });

    (profile_id, filters)
}

/// Two text columns, `id` (the key) and `method`.
pub(super) fn method_rows() -> QueryResult {
    let column = |name: &str, is_primary_key: bool| ColumnMeta {
        name: name.to_string(),
        type_name: "varchar".to_string(),
        kind: ColumnKind::Text,
        nullable: true,
        is_primary_key,
    };

    QueryResult::table(
        vec![column("id", true), column("method", false)],
        vec![vec![
            dbflux_core::Value::Text("1".to_string()),
            dbflux_core::Value::Text("getbets".to_string()),
        ]],
        None,
        Duration::ZERO,
    )
}

/// A table grid on `table` of `profile_id`, in a window that routes the
/// keymap's commands to it.
pub(super) fn hosted_table<'a>(
    cx: &'a mut TestAppContext,
    app_state: gpui::Entity<AppStateEntity>,
    profile_id: Uuid,
    table: &'static str,
) -> (
    gpui::Entity<KeymapHost<DataGridPanel>>,
    gpui::Entity<DataGridPanel>,
    &'a mut VisualTestContext,
) {
    let (host, window) = host_document(
        cx,
        move |window, cx| {
            cx.new(|cx| {
                let source = DataSource::Table {
                    profile_id,
                    database: Some("app".to_string()),
                    table: TableRef::with_schema("public", table),
                    pagination: Pagination::default(),
                    order_by: Vec::new(),
                    total_rows: Some(1),
                };

                let mut panel = DataGridPanel::new_internal(
                    source,
                    app_state.clone(),
                    vec!["id".to_string()],
                    window,
                    cx,
                );
                panel.set_result(method_rows(), cx);
                panel
            })
        },
        |panel, cx| panel.active_context(cx),
        DataGridPanel::dispatch_command,
    );
    let panel = window.update(|_, cx| host.read(cx).document.clone());
    window.run_until_parked();

    (host, panel, window)
}

/// Types `filter` into the WHERE field and applies it, as Enter does.
pub(super) fn apply_filter(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
    filter: &str,
) {
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .filter_bar
                .filter_input
                .update(cx, |input, cx| input.set_value(filter, window, cx));
            panel.apply_filter_input(window, cx);
        });
    });
    window.run_until_parked();
}

pub(super) fn filter_text(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> String {
    window.update(|_, cx| {
        panel
            .read(cx)
            .filter_bar
            .filter_input
            .read(cx)
            .value()
            .to_string()
    })
}

fn history(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> Vec<String> {
    window.update(|_, cx| {
        let panel = panel.read(cx);
        let key = panel.filter_history_key().expect("a table has a key");
        crate::data_grid_panel::filter_history::load_history(&panel.app_state, &key, cx)
    })
}

fn app_state_with_stub(
    cx: &mut TestAppContext,
    failing: Option<&str>,
) -> (gpui::Entity<AppStateEntity>, Uuid, ReadLog) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let (profile_id, log) =
        cx.update(|cx| register_filter_stub(&app_state, method_rows(), failing, cx));
    (app_state, profile_id, log)
}

#[gpui::test]
fn only_filters_whose_read_succeeded_are_kept(cx: &mut TestAppContext) {
    let (app_state, profile_id, log) = app_state_with_stub(cx, Some("bogus syntax"));
    let (_host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");

    apply_filter(&panel, window, "  method = 'getbets' ");
    apply_filter(&panel, window, "bogus syntax");

    assert_eq!(
        reads(&log),
        vec![
            Some("  method = 'getbets' ".to_string()),
            Some("bogus syntax".to_string())
        ],
        "both filters were read"
    );
    assert_eq!(
        history(&panel, window),
        vec!["method = 'getbets'".to_string()],
        "the rejected filter is not kept, the kept one is trimmed"
    );
}

#[gpui::test]
fn a_reused_filter_moves_first_without_a_second_copy(cx: &mut TestAppContext) {
    let (app_state, profile_id, _log) = app_state_with_stub(cx, None);
    let (_host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");

    for filter in ["status = 500", "method = 'x'", "status = 500", ""] {
        apply_filter(&panel, window, filter);
    }

    assert_eq!(
        history(&panel, window),
        vec!["status = 500".to_string(), "method = 'x'".to_string()],
        "newest first, deduplicated, and an empty filter is not history"
    );
}

#[gpui::test]
fn the_history_belongs_to_its_table_and_outlives_the_grid(cx: &mut TestAppContext) {
    let (app_state, profile_id, _log) = app_state_with_stub(cx, None);
    let (_host, panel, window) = hosted_table(cx, app_state.clone(), profile_id, "requests");
    apply_filter(&panel, window, "status = 500");

    // A grid opened later on the same table reads it from the store.
    let (_host, reopened, window) = hosted_table(cx, app_state.clone(), profile_id, "requests");
    window.update(|window, cx| {
        reopened.update(cx, |panel, cx| panel.open_filter_history(window, cx));
    });
    assert_eq!(
        window.update(|_, cx| reopened.read(cx).filter_bar.history.entries.clone()),
        vec!["status = 500".to_string()]
    );

    let (_host, other, window) = hosted_table(cx, app_state, profile_id, "sessions");
    assert!(
        history(&other, window).is_empty(),
        "another table starts with its own, empty history"
    );
}

#[gpui::test]
fn up_in_the_empty_field_walks_the_history_and_enter_applies_it(cx: &mut TestAppContext) {
    let (app_state, profile_id, log) = app_state_with_stub(cx, None);
    let (_host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");
    apply_filter(&panel, window, "status = 404");
    apply_filter(&panel, window, "status = 500");
    apply_filter(&panel, window, "");

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| panel.focus_filter_input(window, cx));
    });
    window.run_until_parked();

    window.simulate_keystrokes("up");
    window.run_until_parked();
    assert!(window.update(|_, cx| panel.read(cx).filter_bar.history.open));
    assert_eq!(
        window.update(|_, cx| panel.read(cx).filter_bar.history.selected),
        Some(0),
        "Up opens the popup on the newest filter"
    );
    assert!(search_focused(&panel, window));

    window.simulate_keystrokes("down");
    window.run_until_parked();
    let reads_before = reads(&log).len();

    window.simulate_keystrokes("enter");
    window.run_until_parked();

    assert_eq!(filter_text(&panel, window), "status = 404");
    assert_eq!(
        reads(&log)[reads_before..],
        [Some("status = 404".to_string())],
        "Enter reads the table once with the picked filter"
    );
    assert!(!window.update(|_, cx| panel.read(cx).filter_bar.history.open));
}

/// The entries the popup lists, in order.
fn shown(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> Vec<String> {
    window.update(|_, cx| {
        let popup = &panel.read(cx).filter_bar.history;
        popup
            .shown()
            .into_iter()
            .map(|index| popup.entries[index].clone())
            .collect()
    })
}

fn popup_open(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> bool {
    window.update(|_, cx| panel.read(cx).filter_bar.history.open)
}

fn search_focused(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> bool {
    window.update(|window, cx| panel.read(cx).filter_history_search_focused(window, cx))
}

fn search_text(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> String {
    window.update(|_, cx| {
        panel
            .read(cx)
            .filter_bar
            .history_search
            .read(cx)
            .value()
            .to_string()
    })
}

fn filter_focused(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) -> bool {
    window.update(|window, cx| {
        panel
            .read(cx)
            .filter_bar
            .filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    })
}

/// The shortcut that opens the history from the WHERE field.
const SEARCH_KEYS: &str = if cfg!(target_os = "macos") {
    "cmd-r"
} else {
    "ctrl-r"
};

/// A grid on `requests` whose history holds `filters` (the last one is
/// newest) with an empty WHERE field that has the keyboard.
fn table_with_history<'a>(
    cx: &'a mut TestAppContext,
    filters: &[&str],
) -> (
    gpui::Entity<KeymapHost<DataGridPanel>>,
    gpui::Entity<DataGridPanel>,
    &'a mut VisualTestContext,
    ReadLog,
) {
    let (app_state, profile_id, log) = app_state_with_stub(cx, None);
    let (host, panel, window) = hosted_table(cx, app_state, profile_id, "requests");
    for filter in filters.iter().chain([&""]) {
        apply_filter(&panel, window, filter);
    }

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| panel.focus_filter_input(window, cx));
    });
    window.run_until_parked();

    (host, panel, window, log)
}

#[gpui::test]
fn opening_the_history_puts_the_keyboard_in_an_empty_search(cx: &mut TestAppContext) {
    let (_host, panel, window, _log) = table_with_history(cx, &["status = 404", "status = 500"]);

    window.simulate_keystrokes("up");
    window.run_until_parked();
    assert!(popup_open(&panel, window), "Up in the empty field opens it");
    assert!(
        search_focused(&panel, window),
        "the search field has the keyboard"
    );
    assert_eq!(search_text(&panel, window), "");

    // Typing goes to the search, not to the WHERE field.
    window.simulate_input("404");
    window.run_until_parked();
    assert_eq!(filter_text(&panel, window), "");
    assert_eq!(shown(&panel, window), vec!["status = 404".to_string()]);

    // Opened again, from the Toolbar menu entry, the search starts empty.
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.close_filter_history(cx);
            panel.run_toolbar_action(
                crate::data_grid_panel::context_menu::toolbar::ToolbarAction::FilterHistory,
                window,
                cx,
            );
        });
    });
    window.run_until_parked();
    assert!(popup_open(&panel, window));
    assert!(search_focused(&panel, window));
    assert_eq!(search_text(&panel, window), "");
    assert_eq!(shown(&panel, window).len(), 2);
}

#[gpui::test]
fn the_shortcut_opens_the_history_with_the_search_focused(cx: &mut TestAppContext) {
    let (_host, panel, window, _log) = table_with_history(cx, &["status = 404", "status = 500"]);
    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .filter_bar
                .filter_input
                .update(cx, |input, cx| input.set_value("id > 1", window, cx));
        });
    });
    window.run_until_parked();

    window.simulate_keystrokes(SEARCH_KEYS);
    window.run_until_parked();
    assert!(popup_open(&panel, window), "the caret is not at the start");
    assert!(search_focused(&panel, window));
    assert_eq!(
        window.update(|_, cx| panel.read(cx).filter_bar.history.selected),
        Some(0)
    );
    assert_eq!(
        filter_text(&panel, window),
        "id > 1",
        "the field keeps its text"
    );

    // Pressed again, it moves to the next older match.
    window.simulate_keystrokes(SEARCH_KEYS);
    window.run_until_parked();
    assert_eq!(
        window.update(|_, cx| panel.read(cx).filter_bar.history.selected),
        Some(1)
    );
}

#[gpui::test]
fn the_search_matches_every_word_in_any_order_and_enter_applies_the_highlight(
    cx: &mut TestAppContext,
) {
    let (_host, panel, window, log) = table_with_history(
        cx,
        &[
            "status = 500 AND method = 'x'",
            "status = 404",
            "method = 'getbets' AND Status = 500",
        ],
    );

    window.simulate_keystrokes(SEARCH_KEYS);
    window.simulate_input("500  STATUS");
    window.run_until_parked();
    assert_eq!(
        shown(&panel, window),
        vec![
            "method = 'getbets' AND Status = 500".to_string(),
            "status = 500 AND method = 'x'".to_string(),
        ],
        "every word, in any order, ignoring case"
    );
    assert_eq!(
        window.update(|_, cx| panel.read(cx).filter_bar.history.selected),
        Some(0),
        "the newest match is highlighted"
    );

    window.simulate_keystrokes("down");
    let reads_before = reads(&log).len();
    window.simulate_keystrokes("enter");
    window.run_until_parked();

    assert_eq!(filter_text(&panel, window), "status = 500 AND method = 'x'");
    assert_eq!(
        reads(&log)[reads_before..],
        [Some("status = 500 AND method = 'x'".to_string())],
        "Enter reads the table once with the highlighted match"
    );
    assert!(!popup_open(&panel, window));
}

#[gpui::test]
fn escape_clears_the_search_then_closes_the_history(cx: &mut TestAppContext) {
    let (_host, panel, window, _log) = table_with_history(cx, &["status = 404", "status = 500"]);

    window.simulate_keystrokes(SEARCH_KEYS);
    window.simulate_input("404");
    window.run_until_parked();
    assert_eq!(shown(&panel, window).len(), 1);

    window.simulate_keystrokes("escape");
    window.run_until_parked();
    assert!(popup_open(&panel, window), "the first Escape keeps it open");
    assert_eq!(search_text(&panel, window), "");
    assert_eq!(shown(&panel, window).len(), 2, "and lists everything again");
    assert!(search_focused(&panel, window));

    window.simulate_keystrokes("escape");
    window.run_until_parked();
    assert!(!popup_open(&panel, window), "the second Escape closes it");
    assert!(
        filter_focused(&panel, window),
        "the keyboard goes back to the WHERE field"
    );
}

#[gpui::test]
fn the_search_narrows_the_list_and_shift_delete_removes_the_match(cx: &mut TestAppContext) {
    let (_host, panel, window, _log) =
        table_with_history(cx, &["status = 404", "method = 'getbets'", "status = 500"]);

    window.simulate_keystrokes("up");
    window.simulate_keystrokes("s t a t u s");
    window.run_until_parked();
    assert_eq!(
        shown(&panel, window),
        vec!["status = 500".to_string(), "status = 404".to_string()],
        "the search narrows the list"
    );

    // A platform reports the held Shift before the key, as here.
    window.simulate_modifiers_change(gpui::Modifiers::shift());
    window.simulate_keystrokes("shift-delete");
    window.simulate_modifiers_change(gpui::Modifiers::default());
    window.run_until_parked();
    assert_eq!(shown(&panel, window), vec!["status = 404".to_string()]);
    assert_eq!(
        history(&panel, window),
        vec!["method = 'getbets'".to_string(), "status = 404".to_string()],
        "Shift+Delete removes the highlighted match from the store"
    );
    assert_eq!(search_text(&panel, window), "status", "the search stays");

    // The row after the entries clears the table's history.
    window.simulate_keystrokes("down enter");
    window.run_until_parked();
    assert!(history(&panel, window).is_empty());
    assert!(!popup_open(&panel, window));
}

#[gpui::test]
fn the_open_history_is_covered_by_the_keyboard(cx: &mut TestAppContext) {
    use crate::keyboard_coverage::DATA_GRID;
    use dbflux_ui_base::keyboard_coverage::{Coverage, FrameCapture};

    let (_host, panel, window, _log) = table_with_history(cx, &["status = 404", "status = 500"]);
    let capture = FrameCapture::observe(window);

    window.simulate_keystrokes(SEARCH_KEYS);
    window.simulate_input("status");
    window.update(|window, _| window.refresh());
    window.run_until_parked();

    let menu: Vec<String> = window.update(|_, cx| {
        panel
            .read(cx)
            .toolbar_actions(cx)
            .into_iter()
            .map(|action| action.id().to_string())
            .collect()
    });
    let frame = capture.frame(window);
    let checked = Coverage::new(DATA_GRID)
        .with_menu_entries(menu)
        .assert_covered(&frame);

    assert!(
        checked
            .iter()
            .any(|id| id.starts_with("filter-history-entry-")),
        "{checked:?}"
    );
}

/// A document collection's query history uses the same store, so a
/// collection opened again lists the queries run on it before.
#[gpui::test]
fn a_collection_opened_again_lists_its_earlier_queries(cx: &mut TestAppContext) {
    use crate::data_grid_panel::documents::QueryHistoryEntry;

    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);
    let profile_id = Uuid::new_v4();
    let window = cx.add_empty_window();

    let open = |window: &mut VisualTestContext| {
        let app_state = app_state.clone();
        window.update(|window, cx| {
            cx.new(|cx| {
                let source = DataSource::Collection {
                    profile_id,
                    collection: CollectionRef::new("shop", "orders"),
                    pagination: Pagination::default(),
                    total_docs: None,
                };
                DataGridPanel::new_internal(source, app_state, Vec::new(), window, cx)
            })
        })
    };

    let entry = QueryHistoryEntry {
        filter: r#"{"status": "paid"}"#.to_string(),
        projection: String::new(),
        sort: r#"{"_id": -1}"#.to_string(),
        limit: "50".to_string(),
    };

    let first = open(window);
    window.update(|_, cx| {
        first.update(cx, |panel, cx| {
            panel.collection.history = vec![entry.clone()];
            panel.save_collection_query_history(cx);
        });
    });

    let reopened = open(window);
    assert_eq!(
        window.update(|_, cx| reopened.read(cx).collection.history.clone()),
        vec![entry]
    );
}
