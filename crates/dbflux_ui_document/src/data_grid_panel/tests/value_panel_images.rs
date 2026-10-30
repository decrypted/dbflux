//! The value panel opened from the grid on a binary column.

use super::rail_keys::{host_in_rail, keys};
use super::*;
use crate::keyboard_test_support::init_keyboard_runtime;
use dbflux_core::Value;

/// A two-row table whose first column is binary, beside its inspector rail.
fn host_binary_grid(
    cx: &mut TestAppContext,
    photo: Vec<u8>,
) -> (gpui::Entity<DataGridPanel>, &mut VisualTestContext) {
    init_keyboard_runtime(cx);
    let app_state = isolated_test_app_state(cx);

    host_in_rail(cx, move |window, cx| {
        let source = DataSource::Table {
            profile_id: Uuid::nil(),
            database: Some("app".to_string()),
            table: TableRef::with_schema("public", "users"),
            pagination: Pagination::default(),
            order_by: Vec::new(),
            total_rows: Some(2),
        };
        let mut columns = reload_columns(&["photo", "id"]);
        columns[0].type_name = "bytea".to_string();
        let rows = ["1", "2"]
            .iter()
            .map(|id| vec![Value::Bytes(photo.clone()), Value::Text((*id).to_string())])
            .collect();
        let result = QueryResult::table(columns, rows, None, Duration::ZERO);

        cx.new(|cx| {
            let mut panel =
                DataGridPanel::new_internal(source, app_state, vec!["id".to_string()], window, cx);
            panel.set_result(result, cx);
            panel
        })
    })
}

fn png() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(5, 4)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("a PNG encodes");
    bytes.into_inner()
}

fn value_panel(
    panel: &gpui::Entity<DataGridPanel>,
    window: &mut VisualTestContext,
) -> gpui::Entity<super::super::value_panel::ValuePanelContent> {
    window.update(|_, cx| {
        panel
            .read(cx)
            .inspector
            .value_panel
            .clone()
            .expect("the value panel is open")
    })
}

/// A binary cell showed an empty, editable text editor; it now shows its
/// bytes as a hex dump the user cannot type over.
#[gpui::test]
fn a_binary_cell_opens_as_its_hex_dump(cx: &mut TestAppContext) {
    let (panel, window) = host_binary_grid(cx, b"\x00\x01binary\xfe\xff".to_vec());
    keys(window, "j v");

    let value_panel = value_panel(&panel, window);
    let text = window.update(|_, cx| value_panel.read(cx).editor_text(cx));
    assert!(text.starts_with("00000000  00 01 62 69"), "{text:?}");
}

#[gpui::test]
fn a_png_cell_opens_as_its_picture(cx: &mut TestAppContext) {
    let (panel, window) = host_binary_grid(cx, png());
    keys(window, "j v");

    let value_panel = value_panel(&panel, window);
    window.update(|_, cx| {
        let value_panel = value_panel.read(cx);
        assert!(value_panel.shows_image());
        let decoded = value_panel.decoded_image().expect("the PNG decodes");
        assert_eq!((decoded.width, decoded.height), (5, 4));
    });
}

/// The picture's view switch and its Save and Copy buttons run from the
/// Toolbar submenu of the table menu.
#[gpui::test]
fn the_picture_buttons_are_covered(cx: &mut TestAppContext) {
    use crate::keyboard_coverage::DATA_GRID;
    use dbflux_ui_base::keyboard_coverage::{Coverage, FrameCapture};

    let (panel, window) = host_binary_grid(cx, png());
    let capture = FrameCapture::observe(window);
    keys(window, "j v");
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

    for id in [
        "value-panel-view-hex",
        "value-panel-save-image",
        "value-panel-copy-image",
    ] {
        assert!(
            checked.iter().any(|checked| checked == id),
            "{id}: {checked:?}"
        );
    }
}
