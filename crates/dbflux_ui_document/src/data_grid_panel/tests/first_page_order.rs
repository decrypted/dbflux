//! A table opened before its primary key is known reads its first page once,
//! in key order, instead of an unsorted page followed by a sorted one.

use super::*;
use dbflux_core::{ColumnInfo, Connection, TableInfo};
use dbflux_test_support::fake_driver::FakeDriver;

/// The fake driver's connection with a `users` table whose details carry an
/// `id` primary key, or no key at all. The fake itself answers every details
/// request with an error.
struct UsersTable {
    inner: Arc<dyn Connection>,
    primary_key: bool,
}

impl Connection for UsersTable {
    fn metadata(&self) -> &dbflux_core::DriverMetadata {
        self.inner.metadata()
    }

    fn kind(&self) -> dbflux_core::DbKind {
        self.inner.kind()
    }

    fn schema_loading_strategy(&self) -> dbflux_core::SchemaLoadingStrategy {
        self.inner.schema_loading_strategy()
    }

    fn dialect(&self) -> &dyn dbflux_core::SqlDialect {
        self.inner.dialect()
    }

    fn ping(&self) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), dbflux_core::DbError> {
        Ok(())
    }

    fn execute(
        &self,
        request: &dbflux_core::QueryRequest,
    ) -> Result<QueryResult, dbflux_core::DbError> {
        self.inner.execute(request)
    }

    fn cancel(&self, handle: &dbflux_core::QueryHandle) -> Result<(), dbflux_core::DbError> {
        self.inner.cancel(handle)
    }

    fn schema(&self) -> Result<dbflux_core::SchemaSnapshot, dbflux_core::DbError> {
        self.inner.schema()
    }

    fn table_details(
        &self,
        _database: &str,
        schema: Option<&str>,
        table: &str,
    ) -> Result<TableInfo, dbflux_core::DbError> {
        Ok(users_details(table, schema, self.primary_key))
    }
}

fn users_details(table: &str, schema: Option<&str>, primary_key: bool) -> TableInfo {
    TableInfo {
        name: table.to_string(),
        schema: schema.map(str::to_string),
        columns: Some(vec![ColumnInfo {
            name: "id".to_string(),
            type_name: "int4".to_string(),
            nullable: false,
            is_primary_key: primary_key,
            default_value: None,
            enum_values: None,
        }]),
        indexes: None,
        foreign_keys: None,
        constraints: None,
        sample_fields: None,
        presentation: Default::default(),
        child_items: None,
        storage_hints: None,
        pseudo_columns: Box::default(),
    }
}

/// Where the `users` table's key is when it opens.
#[derive(Clone, Copy)]
enum Details {
    /// Not cached; the details read finds this key (`true`) or none.
    Cold(Option<bool>),
    /// Cached before the open, with an `id` primary key.
    Warm,
}

/// Open `users` through `new_for_table` on a cold details cache and return
/// the SQL of every page it read (the row count is a query of its own).
fn open_users(
    cx: &mut TestAppContext,
    primary_key: Option<bool>,
    newest_first: bool,
) -> Vec<String> {
    let (_, _, driver) = open_users_panel(cx, Details::Cold(primary_key), newest_first);
    page_reads(&driver)
}

/// The SQL of every page `driver` read; the row count is a query of its own.
fn page_reads(driver: &FakeDriver) -> Vec<String> {
    driver
        .stats()
        .executed_requests
        .into_iter()
        .map(|request| request.sql)
        .filter(|sql| sql.contains("users") && !sql.contains("COUNT("))
        .collect()
}

/// Open `users` through `new_for_table` and hand back the panel, its window
/// and the driver that counts the reads.
fn open_users_panel(
    cx: &mut TestAppContext,
    details: Details,
    newest_first: bool,
) -> (
    gpui::Entity<DataGridPanel>,
    &mut VisualTestContext,
    FakeDriver,
) {
    let primary_key = match details {
        Details::Cold(primary_key) => primary_key,
        Details::Warm => Some(true),
    };
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite).with_default_result(id_result());
    let profile = dbflux_core::ConnectionProfile::new(
        "first-page",
        dbflux_core::DbConfig::SQLite {
            path: std::path::PathBuf::from(":memory:"),
            connection_id: None,
        },
    );
    let fake = driver
        .connect_arc(&profile)
        .expect("the fake driver connects");
    let connection: Arc<dyn Connection> = match primary_key {
        Some(primary_key) => Arc::new(UsersTable {
            inner: fake,
            primary_key,
        }),
        None => fake,
    };

    let (app_state, profile_id) = register_stub_connection(cx, connection);
    cx.update(|cx| {
        app_state.update(cx, |app, _| {
            let mut settings = app.general_settings().clone();
            settings.grid_pk_sort_desc = newest_first;
            app.update_general_settings(settings);
        });
    });

    if matches!(details, Details::Warm) {
        cx.update(|cx| {
            app_state.update(cx, |app, _| {
                app.set_table_details(
                    profile_id,
                    "main".to_string(),
                    Some("public".to_string()),
                    "users".to_string(),
                    users_details("users", Some("public"), true),
                );
            });
        });
    }

    let panel_holder: Rc<RefCell<Option<gpui::Entity<DataGridPanel>>>> = Rc::default();
    let panel_handle = panel_holder.clone();
    let (_, window) = cx.add_window_view(|window, cx| {
        let panel = cx.new(|cx| {
            DataGridPanel::new_for_table(
                profile_id,
                TableRef::with_schema("public", "users"),
                Some("main".to_string()),
                app_state,
                window,
                cx,
            )
        });
        panel_handle.replace(Some(panel.clone()));
        Root::new(panel, window, cx)
    });
    window.run_until_parked();

    let panel = panel_holder.borrow().clone().expect("the panel is created");
    (panel, window, driver)
}

/// Clicks the `id` header the way the table does: one step of its sort
/// cycle. The read it asks for runs before this returns.
fn click_id_header(panel: &gpui::Entity<DataGridPanel>, window: &mut VisualTestContext) {
    window.update(|window, app| {
        let table_state = panel
            .read(app)
            .grid_table
            .table_state
            .clone()
            .expect("table state");
        table_state.update(app, |state, cx| state.cycle_sort(0, cx));
        panel.update(app, |panel, cx| panel.process_pending_actions(window, cx));
    });
    window.run_until_parked();
}

fn last_read(driver: &FakeDriver) -> String {
    page_reads(driver).pop().unwrap_or_default()
}

fn reads_descending(sql: &str) -> bool {
    sql.contains("ORDER BY") && sql.contains("DESC")
}

fn reads_ascending(sql: &str) -> bool {
    sql.contains("ORDER BY") && !sql.contains("DESC")
}

#[gpui::test]
fn a_cold_table_reads_its_first_page_once_newest_first(cx: &mut TestAppContext) {
    let reads = open_users(cx, Some(true), true);

    assert_eq!(reads.len(), 1, "one read of the first page: {reads:?}");
    assert!(
        reads[0].contains("ORDER BY") && reads[0].contains("DESC"),
        "the page is read newest first: {}",
        reads[0]
    );
}

#[gpui::test]
fn a_cold_table_reads_its_first_page_once_in_key_order(cx: &mut TestAppContext) {
    let reads = open_users(cx, Some(true), false);

    assert_eq!(reads.len(), 1, "one read of the first page: {reads:?}");
    assert!(
        reads[0].contains("ORDER BY") && !reads[0].contains("DESC"),
        "the page is read in ascending key order: {}",
        reads[0]
    );
}

#[gpui::test]
fn a_table_without_a_primary_key_still_reads_its_first_page(cx: &mut TestAppContext) {
    let reads = open_users(cx, Some(false), true);

    assert_eq!(reads.len(), 1, "one read of the first page: {reads:?}");
    assert!(
        !reads[0].contains("ORDER BY"),
        "with no key there is no order: {}",
        reads[0]
    );
}

#[gpui::test]
fn a_failed_details_fetch_still_reads_the_first_page(cx: &mut TestAppContext) {
    let reads = open_users(cx, None, true);

    assert_eq!(reads.len(), 1, "one read of the first page: {reads:?}");
}

/// With the key already cached the first page is read at once, newest first.
#[gpui::test]
fn a_warm_table_reads_its_first_page_once_newest_first(cx: &mut TestAppContext) {
    let (_, _, driver) = open_users_panel(cx, Details::Warm, true);
    let reads = page_reads(&driver);

    assert_eq!(reads.len(), 1, "one read of the first page: {reads:?}");
    assert!(reads_descending(&reads[0]), "newest first: {}", reads[0]);
}

/// A table opened newest first shows the key's arrow descending. A click on
/// that header starts its cycle from there: ascending, then descending, then
/// ascending again, so neither direction is out of reach (clearing the sort
/// would only read the same descending page again).
#[gpui::test]
fn a_header_click_from_the_newest_first_order_sorts_ascending(cx: &mut TestAppContext) {
    let (panel, window, driver) = open_users_panel(cx, Details::Cold(Some(true)), true);
    window.update(|_, app| {
        let sort = panel
            .read(app)
            .grid_table
            .table_state
            .as_ref()
            .expect("table state")
            .read(app)
            .sort()
            .copied();
        assert_eq!(sort, Some(TableSortState::descending(0)));
    });

    click_id_header(&panel, window);
    assert!(
        reads_ascending(&last_read(&driver)),
        "{}",
        last_read(&driver)
    );

    click_id_header(&panel, window);
    assert!(
        reads_descending(&last_read(&driver)),
        "{}",
        last_read(&driver)
    );

    click_id_header(&panel, window);
    assert!(
        reads_ascending(&last_read(&driver)),
        "back in the default order the next click is ascending again: {}",
        last_read(&driver)
    );
    assert_eq!(page_reads(&driver).len(), 4, "{:?}", page_reads(&driver));
}

/// Clearing a column sort returns a newest-first table to its key order
/// descending.
#[gpui::test]
fn clearing_a_sort_returns_to_newest_first(cx: &mut TestAppContext) {
    let (panel, window, driver) = open_users_panel(cx, Details::Cold(Some(true)), true);

    click_id_header(&panel, window);
    assert!(reads_ascending(&last_read(&driver)));

    window.update(|window, app| {
        panel.update(app, |panel, cx| {
            panel.handle_sort_clear(cx);
            panel.process_pending_actions(window, cx);
        });
    });
    window.run_until_parked();

    assert!(
        reads_descending(&last_read(&driver)),
        "{}",
        last_read(&driver)
    );
}

/// With the setting off the header cycle is as before: ascending default,
/// descending, then the cleared sort reads the default ascending order.
#[gpui::test]
fn the_header_cycle_is_unchanged_with_the_setting_off(cx: &mut TestAppContext) {
    let (panel, window, driver) = open_users_panel(cx, Details::Cold(Some(true)), false);
    assert!(reads_ascending(&last_read(&driver)));

    click_id_header(&panel, window);
    assert!(
        reads_descending(&last_read(&driver)),
        "{}",
        last_read(&driver)
    );

    click_id_header(&panel, window);
    assert!(
        reads_ascending(&last_read(&driver)),
        "the cleared sort reads the default order: {}",
        last_read(&driver)
    );
}
