use crate::controls::Button;
use crate::icons::AppIcon;
use crate::modals::modal::{Modal, ModalFocus, ModalVariant};
use crate::modals::parts::{modal_field, modal_hint, modal_lead};
use crate::primitives::{BannerBlock, BannerVariant, SurfaceRole, surface};
use crate::tokens::{FontSizes, Spacing};
use dbflux_core::LogErr;
use gpui::prelude::*;
use gpui::{Context, EventEmitter, Window, div, px};
use gpui_component::ActiveTheme;

/// Debug selector of the fingerprint box, for tests.
pub const HOST_KEY_FINGERPRINT_SELECTOR: &str = "host-key-fingerprint";

/// What the modal shows about a host key no source knows yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyConfirmRequest {
    /// `host:port` as dialled.
    pub host: String,
    /// e.g. `ssh-ed25519`.
    pub key_type: String,
    /// `SHA256:<base64>`.
    pub fingerprint: String,
    /// Key types known_hosts already records for the host.
    pub other_key_types: Vec<String>,
}

/// Outcome emitted when the user resolves the modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyConfirmOutcome {
    Trusted,
    Cancelled,
}

/// Asks whether to trust an SSH host key seen for the first time.
///
/// Enter does not confirm: trusting a key must be a deliberate click after
/// comparing the fingerprint. Closing the modal any other way cancels.
pub struct ModalHostKeyConfirm {
    request: Option<HostKeyConfirmRequest>,
    visible: bool,
    focus: ModalFocus,
}

impl ModalHostKeyConfirm {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            request: None,
            visible: false,
            focus: ModalFocus::new(cx),
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn request(&self) -> Option<&HostKeyConfirmRequest> {
        self.request.as_ref()
    }

    pub fn open(&mut self, request: HostKeyConfirmRequest, cx: &mut Context<Self>) {
        self.request = Some(request);
        self.visible = true;
        self.focus.focus_on_next_render();
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.request = None;
        self.focus.restore(cx);
        cx.notify();
    }

    pub fn trust(&mut self, cx: &mut Context<Self>) {
        cx.emit(HostKeyConfirmOutcome::Trusted);
        self.close(cx);
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(HostKeyConfirmOutcome::Cancelled);
        self.close(cx);
    }
}

impl EventEmitter<HostKeyConfirmOutcome> for ModalHostKeyConfirm {}

/// The warning shown when known_hosts has keys of other types for the host.
pub fn host_key_other_types_warning(types: &[String]) -> String {
    dbflux_i18n::t!(
        "modals.host_key_confirm.other_types_warning",
        types = types.join(", ")
    )
}

impl Render for ModalHostKeyConfirm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        self.focus.apply_pending(window, cx);

        let Some(request) = self.request.clone() else {
            return div().into_any_element();
        };

        let mono = crate::fonts::editor_family(cx);
        let value_box = |text: String, cx: &gpui::App| {
            surface(SurfaceRole::Raised, cx)
                .w_full()
                .px(Spacing::SM)
                .py(Spacing::XS)
                .child(
                    div()
                        .text_size(FontSizes::SM)
                        .font_family(mono.clone())
                        .text_color(cx.theme().foreground)
                        .child(text),
                )
        };

        let mut body = div()
            .flex()
            .flex_col()
            .gap(Spacing::MD)
            .child(modal_lead(
                dbflux_i18n::t!("modals.host_key_confirm.lead"),
                cx,
            ))
            .child(modal_field(
                dbflux_i18n::t!("modals.host_key_confirm.host"),
                value_box(request.host.clone(), cx),
                cx,
            ))
            .child(modal_field(
                dbflux_i18n::t!("modals.host_key_confirm.fingerprint"),
                value_box(format!("{} {}", request.key_type, request.fingerprint), cx)
                    .debug_selector(|| HOST_KEY_FINGERPRINT_SELECTOR.to_string()),
                cx,
            ));

        if !request.other_key_types.is_empty() {
            body = body.child(BannerBlock::new(
                BannerVariant::Warning,
                host_key_other_types_warning(&request.other_key_types),
            ));
        }

        let body = body.child(modal_hint(
            dbflux_i18n::t!("modals.host_key_confirm.hint"),
            cx,
        ));

        let on_cancel = cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
            this.cancel(cx);
        });
        let on_trust = cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
            this.trust(cx);
        });

        let footer = div()
            .flex()
            .items_center()
            .gap(Spacing::SM)
            .child(
                Button::new(
                    "host-key-cancel",
                    dbflux_i18n::t!("modals.host_key_confirm.cancel"),
                )
                .on_click(on_cancel),
            )
            .child(
                Button::new(
                    "host-key-trust",
                    dbflux_i18n::t!("modals.host_key_confirm.trust"),
                )
                .danger()
                .on_click(on_trust),
            );

        Modal::new(dbflux_i18n::t!("modals.host_key_confirm.title"))
            .body(body)
            .footer(footer)
            .icon(AppIcon::TriangleAlert)
            .variant(ModalVariant::Danger)
            .width(px(520.0))
            .focus_handle(self.focus.handle())
            .on_close({
                let entity = cx.entity().downgrade();
                move |_, cx| {
                    entity.update(cx, |this, cx| this.cancel(cx)).log_err();
                }
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    const KEYS: &[&str] = &[
        "modals.host_key_confirm.title",
        "modals.host_key_confirm.lead",
        "modals.host_key_confirm.host",
        "modals.host_key_confirm.fingerprint",
        "modals.host_key_confirm.other_types_warning",
        "modals.host_key_confirm.hint",
        "modals.host_key_confirm.cancel",
        "modals.host_key_confirm.trust",
    ];

    #[test]
    fn host_key_confirm_keys_resolve_in_every_locale() {
        for locale in ["en", "es", "ko", "pt_BR", "zh_Hans"] {
            for key in KEYS {
                let text = dbflux_i18n::t!(*key, locale = locale);
                assert!(!text.is_empty() && text != *key, "{locale} missing {key}");
            }
        }
    }

    #[test]
    fn the_other_types_warning_names_the_types() {
        let text = super::host_key_other_types_warning(&[
            "ssh-ed25519".to_string(),
            "ssh-rsa".to_string(),
        ]);
        assert!(text.contains("ssh-ed25519, ssh-rsa"), "{text}");
    }
}
