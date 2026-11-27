//! The value panel's picture of a cell that holds an image: a binary cell
//! whose bytes are one, or text that spells one out in base64.
//!
//! The decode runs on the background executor (see `picture::decode_image`);
//! until it lands the panel says so, and a refused image falls back to the
//! hex or text view under a notice that says why.

use super::picture::{CellBytes, DecodedImage, ImageKind, ImageRefusal, check_size, decode_image};
use super::{ValuePanelContent, ValuePanelEvent};
use dbflux_components::controls::{Button, ButtonVariant};
use dbflux_components::icons::AppIcon;
use dbflux_components::primitives::{Icon, Text};
use dbflux_components::tokens::{FontSizes, Heights, Radii, Spacing};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme;
use std::sync::Arc;

/// How the panel presents the value: as a picture, or in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ValueView {
    Image,
    /// The hex dump of a binary cell.
    Hex,
    /// The base64 text an image was read from.
    Text,
}

impl ValueView {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Hex => "hex",
            Self::Text => "text",
        }
    }

    fn label(self) -> String {
        match self {
            Self::Image => dbflux_i18n::t!("components.value_panel.view.image"),
            Self::Hex => dbflux_i18n::t!("components.value_panel.view.hex"),
            Self::Text => dbflux_i18n::t!("components.value_panel.view.text"),
        }
    }
}

pub(super) enum ImageState {
    Decoding,
    Ready(DecodedImage),
    Refused(ImageRefusal),
}

/// The image a cell holds, and how far the panel got with it.
pub(super) struct ImageView {
    kind: ImageKind,
    /// The encoded bytes, kept for Save image as and Copy image.
    bytes: Arc<[u8]>,
    byte_len: usize,
    state: ImageState,
    /// Dropping it discards a decode the user has moved away from.
    _decode: Option<Task<()>>,
}

impl ValuePanelContent {
    /// Whether the value is shown as a picture rather than in the editor.
    pub(crate) fn shows_image(&self) -> bool {
        self.show_image
            && self
                .image
                .as_ref()
                .is_some_and(|image| !matches!(image.state, ImageState::Refused(_)))
    }

    /// The decoded picture once it is ready.
    pub(crate) fn decoded_image(&self) -> Option<&DecodedImage> {
        match &self.image.as_ref()?.state {
            ImageState::Ready(decoded) => Some(decoded),
            _ => None,
        }
    }

    /// Why a value that starts like an image is shown as bytes instead.
    pub(crate) fn image_notice(&self) -> Option<String> {
        let image = self.image.as_ref()?;
        match &image.state {
            ImageState::Refused(refusal) => Some(refusal.message(image.kind)),
            _ => None,
        }
    }

    /// The editor view the value switches to from the picture.
    pub(super) fn editor_view(&self) -> ValueView {
        if self.target.bytes.is_some() {
            ValueView::Hex
        } else {
            ValueView::Text
        }
    }

    /// The views offered beside the one shown: the picture, unless it was
    /// refused, and the editor.
    pub(super) fn other_views(&self) -> Vec<ValueView> {
        if self.image.is_none() || self.image_notice().is_some() {
            return Vec::new();
        }

        if self.shows_image() {
            vec![self.editor_view()]
        } else {
            vec![ValueView::Image]
        }
    }

    pub(super) fn set_view(&mut self, view: ValueView, cx: &mut Context<Self>) {
        self.show_image = view == ValueView::Image;
        cx.notify();
    }

    /// Reads the target as an image when it starts like one, and starts
    /// decoding it. Opens on the picture, as DataGrip does.
    pub(super) fn load_image(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.release_image(window, cx);

        let source = match &self.target.bytes {
            Some(cell) => Some(cell.clone()),
            None => super::picture::image_from_base64_text(&self.target.value),
        };
        let Some(CellBytes { bytes, total_len }) = source else {
            return;
        };
        let Some(kind) = ImageKind::sniff(&bytes) else {
            return;
        };

        let (state, decode) = match check_size(total_len) {
            Err(refusal) => (ImageState::Refused(refusal), None),
            Ok(()) => {
                let encoded = bytes.clone();
                let decode = cx.spawn(async move |this, cx| {
                    let outcome = cx
                        .background_spawn(async move { decode_image(kind, &encoded) })
                        .await;
                    this.update(cx, |panel, cx| panel.finish_decode(outcome, cx))
                        .ok();
                });
                (ImageState::Decoding, Some(decode))
            }
        };

        self.show_image = matches!(state, ImageState::Decoding);
        self.image = Some(ImageView {
            kind,
            bytes,
            byte_len: total_len,
            state,
            _decode: decode,
        });
    }

    fn finish_decode(
        &mut self,
        outcome: Result<DecodedImage, ImageRefusal>,
        cx: &mut Context<Self>,
    ) {
        let Some(image) = self.image.as_mut() else {
            return;
        };

        image.state = match outcome {
            Ok(decoded) => ImageState::Ready(decoded),
            Err(refusal) => {
                self.show_image = false;
                ImageState::Refused(refusal)
            }
        };
        image._decode = None;
        cx.notify();
    }

    /// Frees the texture of the picture shown so far. A `RenderImage` is
    /// not cached by gpui, so its atlas entry lives until dropped here.
    pub(super) fn release_image(&mut self, window: Option<&mut Window>, cx: &mut App) {
        if let Some(ImageView {
            state: ImageState::Ready(decoded),
            ..
        }) = self.image.take()
        {
            cx.drop_image(decoded.image, window);
        }
        self.show_image = false;
    }

    /// Hands the image bytes to the grid, which owns the save prompt, the
    /// audit trail and the toasts.
    pub(super) fn request_save_image(&self, cx: &mut Context<Self>) {
        if let Some(image) = self.ready_image() {
            cx.emit(ValuePanelEvent::SaveImage {
                kind: image.kind,
                bytes: image.bytes.clone(),
                column: self.target.column_name.clone(),
            });
        }
    }

    pub(super) fn request_copy_image(&self, cx: &mut Context<Self>) {
        if let Some(image) = self.ready_image() {
            cx.emit(ValuePanelEvent::CopyImage {
                kind: image.kind,
                bytes: image.bytes.clone(),
            });
        }
    }

    fn ready_image(&self) -> Option<&ImageView> {
        self.image
            .as_ref()
            .filter(|image| matches!(image.state, ImageState::Ready(_)))
    }

    /// The Image / Hex (or Text) switch of the header.
    pub(super) fn render_view_chips(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.other_views().is_empty() {
            return None;
        }

        let theme = cx.theme().clone();
        let current = if self.shows_image() {
            ValueView::Image
        } else {
            self.editor_view()
        };
        let views = [ValueView::Image, self.editor_view()];

        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .children(views.map(|view| {
                    let is_active = view == current;
                    div()
                        .id(SharedString::from(format!(
                            "value-panel-view-{}",
                            view.id()
                        )))
                        .px(Spacing::XS)
                        .rounded(Radii::SM)
                        .cursor_pointer()
                        .when(is_active, |d| d.bg(theme.accent.opacity(0.15)))
                        .when(!is_active, |d| d.hover(|d| d.bg(theme.secondary)))
                        .child(Text::body(view.label()).font_size(FontSizes::XS).color(
                            if is_active {
                                theme.foreground
                            } else {
                                theme.muted_foreground
                            },
                        ))
                        .on_click(cx.listener(move |this, _, _window, cx| this.set_view(view, cx)))
                })),
        )
    }

    /// The picture, scaled down to fit the panel but never up, over its
    /// stored size and byte count.
    pub(super) fn render_image_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(image) = self.image.as_ref() else {
            return div().into_any_element();
        };

        let (picture, meta) = match &image.state {
            ImageState::Ready(decoded) => (
                img(decoded.image.clone())
                    .max_w_full()
                    .max_h_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                dbflux_i18n::t!(
                    "components.value_panel.image.meta",
                    width = decoded.width,
                    height = decoded.height,
                    format = image.kind.label(),
                    size = crate::buckets_table::format_bytes(image.byte_len as u64)
                ),
            ),
            _ => (
                Text::caption(dbflux_i18n::t!(
                    "components.value_panel.image.decoding",
                    format = image.kind.label()
                ))
                .color(theme.muted_foreground)
                .into_any_element(),
                crate::buckets_table::format_bytes(image.byte_len as u64),
            ),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .bg(theme.secondary)
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .p(Spacing::SM)
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .child(picture),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .py(Spacing::XS)
                    .child(Text::caption(meta).color(theme.muted_foreground)),
            )
            .into_any_element()
    }

    /// Save image as and Copy image, under the picture.
    pub(super) fn render_image_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ready = self.ready_image().is_some();

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap(Spacing::XS)
            .px(Spacing::SM)
            .py(Spacing::XS)
            .border_t_1()
            .border_color(theme.border)
            .child(
                Button::new(
                    "value-panel-copy-image",
                    dbflux_i18n::t!("components.value_panel.image.copy"),
                )
                .variant(ButtonVariant::Ghost)
                .disabled(!ready)
                .on_click(cx.listener(|this, _, _window, cx| this.request_copy_image(cx))),
            )
            .child(
                Button::new(
                    "value-panel-save-image",
                    dbflux_i18n::t!("components.value_panel.image.save"),
                )
                .variant(ButtonVariant::Ghost)
                .disabled(!ready)
                .on_click(cx.listener(|this, _, _window, cx| this.request_save_image(cx))),
            )
    }

    /// Why the picture is not shown, or that the hex view stops short.
    pub(super) fn render_notice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let hex_cut = self
            .target
            .bytes
            .as_ref()
            .filter(|cell| cell.hex_is_cut() && !self.shows_image())
            .map(|cell| {
                dbflux_i18n::t!(
                    "components.value_panel.hex_cut",
                    shown =
                        crate::buckets_table::format_bytes(super::picture::MAX_HEX_BYTES as u64),
                    size = crate::buckets_table::format_bytes(cell.total_len as u64)
                )
            });
        let lines: Vec<String> = self.image_notice().into_iter().chain(hex_cut).collect();
        if lines.is_empty() {
            return None;
        }

        let theme = cx.theme().clone();
        Some(
            div()
                .px(Spacing::SM)
                .py(Spacing::XS)
                .flex()
                .items_start()
                .gap(Spacing::XS)
                .bg(theme.warning.opacity(0.1))
                .border_b_1()
                .border_color(theme.warning.opacity(0.3))
                .child(
                    Icon::new(AppIcon::TriangleAlert)
                        .size(Heights::ICON_SM)
                        .warning(),
                )
                .child(
                    div().flex().flex_col().min_w_0().children(
                        lines
                            .into_iter()
                            .map(|line| Text::caption(line).font_size(FontSizes::XS)),
                    ),
                ),
        )
    }
}
