use super::*;
use dbflux_core::{GeneratedIdKind, Value};
use dbflux_test_support::fake_driver::FakeDriver;

fn column(name: &str, type_name: &str, is_primary_key: bool) -> ColumnMeta {
    ColumnMeta {
        name: name.to_string(),
        type_name: type_name.to_string(),
        kind: ColumnKind::Text,
        nullable: !is_primary_key,
        is_primary_key,
    }
}

/// `tokens(id uuid, ksuid char(27), label varchar(20), payload binary(16),
/// note text)` with one row.
fn tokens_grid(cx: &mut TestAppContext) -> (gpui::Entity<DataGridPanel>, &mut VisualTestContext) {
    let driver = FakeDriver::new(dbflux_core::DbKind::SQLite);
    let (panel, window) = super::cell_expressions::events_grid(cx, &driver);

    let result = QueryResult::table(
        vec![
            column("id", "uuid", true),
            column("ksuid", "char(27)", false),
            column("label", "varchar(20)", false),
            column("payload", "binary(16)", false),
            column("note", "text", false),
        ],
        vec![vec![
            Value::Text("00000000-0000-4000-8000-000000000000".to_string()),
            Value::Text("x".to_string()),
            Value::Text("x".to_string()),
            Value::Null,
            Value::Text("x".to_string()),
        ]],
        None,
        Duration::ZERO,
    );
    window.update(|_, cx| panel.update(cx, |panel, cx| panel.set_result(result, cx)));
    window.run_until_parked();
    (panel, window)
}

#[gpui::test]
fn columns_are_offered_the_ids_their_type_can_store(cx: &mut TestAppContext) {
    use GeneratedIdKind::*;

    let (panel, window) = tokens_grid(cx);

    window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        let state = table.read(cx);
        assert_eq!(state.generated_id_kinds(0), [UuidV4, UuidV7]);
        assert_eq!(
            state.generated_id_kinds(1),
            [Ksuid],
            "char(27) fits a KSUID only"
        );
        assert_eq!(state.generated_id_kinds(2), [], "varchar(20) is too short");
        assert_eq!(
            state.generated_id_kinds(3),
            [],
            "binary(16) needs raw bytes"
        );
        assert_eq!(state.generated_id_kinds(4), [UuidV4, UuidV7, Ksuid, Ulid]);
    });
}

#[gpui::test]
fn the_menu_entry_writes_a_generated_id(cx: &mut TestAppContext) {
    let (panel, window) = tokens_grid(cx);

    window.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.handle_generate_id(0, 1, GeneratedIdKind::Ksuid, cx)
        })
    });

    let staged = window.update(|_, cx| {
        let table = panel
            .read(cx)
            .grid_table
            .table_state
            .clone()
            .expect("table");
        let state = table.read(cx);
        let base = state.model().cell(0, 1).cloned().expect("cell");
        state.edit_buffer().get_cell(0, 1, &base).edit_text()
    });
    assert_eq!(staged.len(), 27);
    assert!(dbflux_core::ksuid_unix_secs(&staged).is_some(), "{staged}");
}
