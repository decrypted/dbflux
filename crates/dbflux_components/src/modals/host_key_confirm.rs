use crate::controls::Button;
use crate::icons::AppIcon;
use crate::modals::modal::{Modal, ModalFocus, ModalVariant};
use crate::modals::parts::{modal_field, modal_hint, modal_lead};
use crate::primitives::{BannerBlock, BannerVariant, SurfaceRole, surface};
use crate::tokens::{FontSizes, Spacing};
use dbflux_core::LogErr;
use gpui::prelude::*;
use gpui::{Context, EventEmitter, FocusHandle, KeyDownEvent, Subscription, Task, Window, div, px};
use gpui_component::ActiveTheme;
use std::time::Duration;

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

/// The shortcut for Trust and remember, as the button's keycap shows it.
pub const HOST_KEY_TRUST_SHORTCUT: &str = "Alt+T";

/// How long Trust stays disabled after the prompt opens, as browsers do for
/// download and permission prompts: keys typed for something else when the
/// prompt appears (Tab then Enter in an editor) cannot reach it.
pub const HOST_KEY_TRUST_ARM_DELAY: Duration = Duration::from_millis(1000);

/// Asks whether to trust an SSH host key seen for the first time.
///
/// The prompt opens when a connect is refused, often while the user is still
/// pressing keys meant for something else, so Enter must never trust: focus
/// starts on Cancel, and Enter or Space activate the focused button. Trust is
/// reached with Tab, or with Alt+T, and only once it is armed:
/// [`HOST_KEY_TRUST_ARM_DELAY`] after the prompt is first drawn in the active
/// window, again for each new request and after the window was inactive.
/// Escape and Cancel work at once.
pub struct ModalHostKeyConfirm {
    request: Option<HostKeyConfirmRequest>,
    visible: bool,
    focus: ModalFocus,
    cancel_focus: FocusHandle,
    trust_focus: FocusHandle,
    focus_cancel_on_render: bool,
    trust_armed: bool,
    /// Start the arming delay at the next render in an active window.
    arm_on_render: bool,
    _arm_task: Option<Task<()>>,
    _activation: Option<Subscription>,
}

impl ModalHostKeyConfirm {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            request: None,
            visible: false,
            focus: ModalFocus::new(cx),
            cancel_focus: cx.focus_handle(),
            trust_focus: cx.focus_handle(),
            focus_cancel_on_render: false,
            trust_armed: false,
            arm_on_render: false,
            _arm_task: None,
            _activation: None,
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
        self.focus_cancel_on_render = true;
        self.disarm_until_seen();
        cx.notify();
    }

    /// Disarms Trust until the prompt is drawn in the active window again;
    /// dropping the task cancels a running timer.
    fn disarm_until_seen(&mut self) {
        self.trust_armed = false;
        self._arm_task = None;
        self.arm_on_render = true;
    }

    /// Arms Trust after the delay.
    fn arm_trust_later(&mut self, cx: &mut Context<Self>) {
        self.trust_armed = false;
        let entity = cx.entity().downgrade();
        self._arm_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(HOST_KEY_TRUST_ARM_DELAY)
                .await;
            entity
                .update(cx, |this, cx| {
                    this.trust_armed = true;
                    cx.notify();
                })
                .log_err();
        }));
    }

    pub fn is_trust_armed(&self) -> bool {
        self.trust_armed
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.request = None;
        self.trust_armed = false;
        self.arm_on_render = false;
        self._arm_task = None;
        self.focus.restore(cx);
        cx.notify();
    }

    /// Does nothing until Trust is armed.
    pub fn trust(&mut self, cx: &mut Context<Self>) {
        if !self.trust_armed {
            return;
        }
        cx.emit(HostKeyConfirmOutcome::Trusted);
        self.close(cx);
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(HostKeyConfirmOutcome::Cancelled);
        self.close(cx);
    }
}

impl EventEmitter<HostKeyConfirmOutcome> for ModalHostKeyConfirm {}

/// Alt+T, pressed once (a held key does not repeat into a trust).
fn is_trust_shortcut(event: &KeyDownEvent) -> bool {
    let modifiers = &event.keystroke.modifiers;
    !event.is_held
        && event.keystroke.key == "t"
        && modifiers.alt
        && !modifiers.control
        && !modifiers.platform
        && !modifiers.shift
        && !modifiers.function
}

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

        if std::mem::take(&mut self.focus_cancel_on_render) {
            self.focus.focus(Some(&self.cancel_focus), window, cx);
        }
        if self._activation.is_none() {
            // A prompt that opened while the user was in another app, or that
            // they left, is armed only once seen again.
            self._activation = Some(cx.observe_window_activation(window, |this, window, cx| {
                if this.visible && !window.is_window_active() {
                    this.disarm_until_seen();
                }
                cx.notify();
            }));
        }
        if self.arm_on_render && window.is_window_active() {
            self.arm_on_render = false;
            self.arm_trust_later(cx);
        }

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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if is_trust_shortcut(event) {
                    cx.stop_propagation();
                    this.trust(cx);
                }
            }))
            .child(
                Button::new(
                    "host-key-cancel",
                    dbflux_i18n::t!("modals.host_key_confirm.cancel"),
                )
                .focus_handle(&self.cancel_focus)
                .on_click(on_cancel),
            )
            .child(
                Button::new(
                    "host-key-trust",
                    dbflux_i18n::t!("modals.host_key_confirm.trust"),
                )
                .kbd(HOST_KEY_TRUST_SHORTCUT)
                .danger()
                .disabled(!self.trust_armed)
                .focus_handle(&self.trust_focus)
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
            // Enter that no focused button takes stops here instead of
            // reaching the window behind; it never trusts.
            .on_confirm(|_, _| {})
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

    use super::{
        HOST_KEY_TRUST_ARM_DELAY, HostKeyConfirmOutcome, HostKeyConfirmRequest, ModalHostKeyConfirm,
    };
    use gpui::{AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _};
    use gpui::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Host {
        modal: Entity<ModalHostKeyConfirm>,
    }

    impl Render for Host {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            gpui::div().size_full().child(self.modal.clone())
        }
    }

    type Outcomes = Rc<RefCell<Vec<HostKeyConfirmOutcome>>>;

    /// Lets the test clock pass the arming delay.
    fn wait_until_armed(window: &mut VisualTestContext) {
        window
            .executor()
            .advance_clock(HOST_KEY_TRUST_ARM_DELAY + std::time::Duration::from_millis(50));
        window.run_until_parked();
    }

    /// A full key press: a focused button activates on the release.
    fn press(window: &mut VisualTestContext, key: &str) {
        let keystroke = gpui::Keystroke::parse(key).expect("valid keystroke");
        window.simulate_event(gpui::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        window.simulate_event(gpui::KeyUpEvent { keystroke });
        window.run_until_parked();
    }

    fn open(
        cx: &mut TestAppContext,
    ) -> (
        Entity<ModalHostKeyConfirm>,
        Outcomes,
        &mut VisualTestContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::init(cx);
        });
        crate::modals::modal::bind_modal_keys_for_tests(cx);

        let outcomes: Outcomes = Rc::default();
        // Root carries GPUI-component's Tab navigation, as in the app.
        let slot: Rc<RefCell<Option<Entity<ModalHostKeyConfirm>>>> = Rc::default();
        let (_, window) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let modal = cx.new(ModalHostKeyConfirm::new);
                slot.replace(Some(modal.clone()));
                let host = cx.new(|_| Host { modal });
                gpui_component::Root::new(host, window, cx)
            }
        });
        let modal = slot.borrow().clone().expect("modal built");
        window.update(|_, cx| {
            let outcomes = outcomes.clone();
            cx.subscribe(&modal, move |_, outcome: &HostKeyConfirmOutcome, _| {
                outcomes.borrow_mut().push(outcome.clone());
            })
            .detach();
            modal.update(cx, |modal, cx| {
                modal.open(
                    HostKeyConfirmRequest {
                        host: "bastion.example:22".to_string(),
                        key_type: "ssh-ed25519".to_string(),
                        fingerprint: "SHA256:abc".to_string(),
                        other_key_types: Vec::new(),
                    },
                    cx,
                );
            });
        });
        window.update(|window, _| {
            window.activate_window();
            window.refresh();
        });
        window.run_until_parked();
        (modal, outcomes, window)
    }

    /// The prompt can open under a key press meant for something else (a
    /// second Enter to retry a connect, a newline in an editor): Enter right
    /// after opening must cancel, never trust.
    #[gpui::test]
    fn a_bare_enter_on_open_never_trusts(cx: &mut TestAppContext) {
        let (modal, outcomes, window) = open(cx);

        press(window, "enter");

        assert_eq!(*outcomes.borrow(), vec![HostKeyConfirmOutcome::Cancelled]);
        assert!(!window.update(|_, cx| modal.read(cx).is_visible()));
    }

    /// Enter that reaches the dialog itself (focus on no button, for example
    /// after a click on its text) does nothing and goes nowhere else.
    #[gpui::test]
    fn enter_on_the_dialog_itself_does_nothing(cx: &mut TestAppContext) {
        let (modal, outcomes, window) = open(cx);
        wait_until_armed(window);
        window.update(|window, cx| {
            let handle = modal.read(cx).focus.handle().clone();
            handle.focus(window, cx);
        });
        window.run_until_parked();

        press(window, "enter");

        assert!(outcomes.borrow().is_empty());
        assert!(window.update(|_, cx| modal.read(cx).is_visible()));
    }

    /// Tab then Enter, or Alt+T, typed for something else as the prompt
    /// appears must not trust: Trust is disabled until the delay passed.
    #[gpui::test]
    fn trust_is_disarmed_right_after_opening(cx: &mut TestAppContext) {
        let (modal, outcomes, window) = open(cx);
        assert!(!window.update(|_, cx| modal.read(cx).is_trust_armed()));

        window.simulate_keystrokes("alt-t");
        window.run_until_parked();
        press(window, "tab");
        press(window, "enter");

        assert!(
            !outcomes.borrow().contains(&HostKeyConfirmOutcome::Trusted),
            "{:?}",
            outcomes.borrow()
        );
    }

    /// The delay runs only while the prompt can be seen: leaving the window
    /// disarms Trust, and it arms again a delay after coming back.
    #[gpui::test]
    fn trust_arms_only_while_the_window_is_active(cx: &mut TestAppContext) {
        let (modal, _, window) = open(cx);
        wait_until_armed(window);
        assert!(window.update(|_, cx| modal.read(cx).is_trust_armed()));

        window.deactivate_window();
        window.run_until_parked();
        assert!(!window.update(|_, cx| modal.read(cx).is_trust_armed()));

        wait_until_armed(window);
        assert!(
            !window.update(|_, cx| modal.read(cx).is_trust_armed()),
            "no arming while the window is inactive"
        );

        window.update(|window, _| {
            window.activate_window();
            window.refresh();
        });
        window.run_until_parked();
        wait_until_armed(window);
        assert!(window.update(|_, cx| modal.read(cx).is_trust_armed()));
    }

    #[gpui::test]
    fn escape_works_before_trust_is_armed(cx: &mut TestAppContext) {
        let (_, outcomes, window) = open(cx);

        window.simulate_keystrokes("escape");
        window.run_until_parked();

        assert_eq!(*outcomes.borrow(), vec![HostKeyConfirmOutcome::Cancelled]);
    }

    #[gpui::test]
    fn a_new_request_disarms_trust_again(cx: &mut TestAppContext) {
        let (modal, outcomes, window) = open(cx);
        wait_until_armed(window);
        assert!(window.update(|_, cx| modal.read(cx).is_trust_armed()));

        window.update(|_, cx| {
            modal.update(cx, |modal, cx| {
                modal.open(
                    HostKeyConfirmRequest {
                        host: "other.example:22".to_string(),
                        key_type: "ssh-ed25519".to_string(),
                        fingerprint: "SHA256:def".to_string(),
                        other_key_types: Vec::new(),
                    },
                    cx,
                );
            });
        });
        window.run_until_parked();

        assert!(!window.update(|_, cx| modal.read(cx).is_trust_armed()));
        window.simulate_keystrokes("alt-t");
        window.run_until_parked();
        assert!(outcomes.borrow().is_empty());
    }

    #[gpui::test]
    fn tab_then_enter_trusts(cx: &mut TestAppContext) {
        let (_, outcomes, window) = open(cx);
        wait_until_armed(window);

        press(window, "tab");
        press(window, "enter");

        assert_eq!(*outcomes.borrow(), vec![HostKeyConfirmOutcome::Trusted]);
    }

    #[gpui::test]
    fn alt_t_trusts(cx: &mut TestAppContext) {
        let (_, outcomes, window) = open(cx);
        wait_until_armed(window);

        window.simulate_keystrokes("alt-t");
        window.run_until_parked();

        assert_eq!(*outcomes.borrow(), vec![HostKeyConfirmOutcome::Trusted]);
    }

    #[gpui::test]
    fn escape_cancels(cx: &mut TestAppContext) {
        let (_, outcomes, window) = open(cx);

        window.simulate_keystrokes("escape");
        window.run_until_parked();

        assert_eq!(*outcomes.borrow(), vec![HostKeyConfirmOutcome::Cancelled]);
    }
}
