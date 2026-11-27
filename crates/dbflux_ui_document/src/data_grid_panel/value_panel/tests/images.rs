//! The value panel on cells that hold, or almost hold, an image.

use super::super::picture::tests::encoded;
use super::super::picture::{CellBytes, ImageKind, MAX_IMAGE_BYTES};
use super::super::{ValuePanelButton, ValuePanelContent, ValuePanelTarget, ValueView};
use crate::keyboard_test_support::{host_document, init_keyboard_runtime};
use base64::Engine as _;
use dbflux_app::keymap::ContextId;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

fn binary(value: &[u8]) -> ValuePanelTarget {
    ValuePanelTarget {
        row: 0,
        col: 0,
        column_name: "photo".to_string(),
        value: String::new(),
        bytes: Some(CellBytes::new(value)),
        editable: false,
    }
}

/// The panel on `target`, with every background decode finished.
fn open(
    cx: &mut TestAppContext,
    target: ValuePanelTarget,
) -> (Entity<ValuePanelContent>, &mut VisualTestContext) {
    init_keyboard_runtime(cx);

    let (host, window) = host_document(
        cx,
        move |window, cx| cx.new(|cx| ValuePanelContent::new(target, window, cx)),
        |_panel, _cx| ContextId::Inspector,
        |_panel, _command, _window, _cx| false,
    );
    window.run_until_parked();
    let panel = window.update(|_, cx| host.read(cx).document.clone());

    (panel, window)
}

fn editor_text(panel: &Entity<ValuePanelContent>, window: &mut VisualTestContext) -> String {
    window.update(|_, cx| panel.read(cx).input.read(cx).value().to_string())
}

fn buttons(
    panel: &Entity<ValuePanelContent>,
    window: &mut VisualTestContext,
) -> Vec<ValuePanelButton> {
    window.update(|_, cx| panel.read(cx).buttons(cx))
}

#[gpui::test]
fn a_png_cell_opens_on_the_picture(cx: &mut TestAppContext) {
    let (panel, window) = open(cx, binary(&encoded(ImageKind::Png, 3, 2)));

    window.update(|_, cx| {
        let panel = panel.read(cx);
        assert!(panel.shows_image());
        let decoded = panel.decoded_image().expect("the PNG decodes");
        assert_eq!((decoded.width, decoded.height), (3, 2));
        assert_eq!(panel.image_notice(), None);
    });
    assert_eq!(
        buttons(&panel, window),
        vec![
            ValuePanelButton::View(ValueView::Hex),
            ValuePanelButton::SaveImage,
            ValuePanelButton::CopyImage,
        ]
    );

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.press(ValuePanelButton::View(ValueView::Hex), window, cx)
        })
    });
    assert!(!window.update(|_, cx| panel.read(cx).shows_image()));
    assert!(editor_text(&panel, window).starts_with("00000000  89 50 4e 47"));
    assert!(buttons(&panel, window).contains(&ValuePanelButton::View(ValueView::Image)));
}

#[gpui::test]
fn random_bytes_open_as_a_read_only_hex_dump(cx: &mut TestAppContext) {
    let noise: Vec<u8> = (0u32..300).map(|i| (i * 7919 % 251) as u8).collect();
    let (panel, window) = open(cx, binary(&noise));

    window.update(|_, cx| {
        let panel = panel.read(cx);
        assert!(!panel.shows_image());
        assert!(panel.image.is_none());
    });
    assert_eq!(
        editor_text(&panel, window),
        CellBytes::new(&noise).hex_dump()
    );
    // A hex dump is not JSON or XML, and there is no image to switch to.
    assert_eq!(buttons(&panel, window), vec![ValuePanelButton::Wrap]);
}

#[gpui::test]
fn a_cut_image_says_it_is_incomplete_and_shows_the_bytes(cx: &mut TestAppContext) {
    let png = encoded(ImageKind::Png, 16, 16);
    let (panel, window) = open(cx, binary(&png[..png.len() / 2]));

    let notice = window.update(|_, cx| {
        let panel = panel.read(cx);
        assert!(!panel.shows_image());
        panel.image_notice()
    });
    assert_eq!(
        notice,
        Some(dbflux_i18n::t!(
            "components.value_panel.image.incomplete",
            format = "PNG"
        ))
    );
    assert!(!buttons(&panel, window).contains(&ValuePanelButton::View(ValueView::Image)));
}

#[gpui::test]
fn an_image_above_the_size_cap_falls_back_with_a_notice(cx: &mut TestAppContext) {
    let mut huge = encoded(ImageKind::Png, 2, 2);
    huge.resize(MAX_IMAGE_BYTES + 1, 0);
    let (panel, window) = open(cx, binary(&huge));

    let notice = window.update(|_, cx| {
        let panel = panel.read(cx);
        assert!(!panel.shows_image());
        panel.image_notice()
    });
    let notice = notice.expect("the panel says why the image is not shown");
    assert!(notice.contains("PNG"), "{notice}");
    assert!(
        notice.contains(&crate::buckets_table::format_bytes(MAX_IMAGE_BYTES as u64)),
        "{notice}"
    );
    // Only the start of the value reaches the hex view.
    assert!(editor_text(&panel, window).lines().count() <= 64 * 1024 / 16);
}

#[gpui::test]
fn base64_text_holding_an_image_opens_on_the_picture(cx: &mut TestAppContext) {
    let png = encoded(ImageKind::Png, 2, 2);
    let value = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&png)
    );
    let target = ValuePanelTarget {
        row: 0,
        col: 0,
        column_name: "avatar".to_string(),
        value: value.clone(),
        bytes: None,
        editable: true,
    };
    let (panel, window) = open(cx, target);

    assert!(window.update(|_, cx| panel.read(cx).shows_image()));
    assert!(buttons(&panel, window).contains(&ValuePanelButton::View(ValueView::Text)));

    window.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.press(ValuePanelButton::View(ValueView::Text), window, cx)
        })
    });
    assert_eq!(editor_text(&panel, window), value);
}
