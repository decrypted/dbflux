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
        Ok(TableInfo {
            name: table.to_string(),
            schema: schema.map(str::to_string),
            columns: Some(vec![ColumnInfo {
                name: "id".to_string(),
                type_name: "int4".to_string(),
                nullable: false,
                is_primary_key: self.primary_key,
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
        })
    }
}

/// Open `users` through `new_for_table` on a cold details cache and return
/// the SQL of every page it read (the row count is a query of its own).
fn open_users(
    cx: &mut TestAppContext,
    primary_key: Option<bool>,
    newest_first: bool,
) -> Vec<String> {
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
        Root::new(panel, window, cx)
    });
    window.run_until_parked();

    driver
        .stats()
        .executed_requests
        .into_iter()
        .map(|request| request.sql)
        .filter(|sql| sql.contains("users") && !sql.contains("COUNT("))
        .collect()
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
