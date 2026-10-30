use std::cell::Cell;
use std::rc::Rc;

use super::tab_manager::TabManager;
use super::types::{DocumentId, DocumentMetaSnapshot, DocumentState};
use dbflux_components::composites::{MenuItem, document_tab, document_tab_bar, document_tab_title};
use dbflux_components::controls::Button;
use dbflux_components::icons::AppIcon;
use dbflux_components::primitives::{Icon, Status, StatusIndicator};
use dbflux_components::tokens::{ChromeColors, TabMetrics};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::tooltip::Tooltip;

#[allow(dead_code)]
pub struct TabBar {
    tab_manager: Entity<TabManager>,
    focus_handle: FocusHandle,

    context_menu: Option<TabContextMenu>,

    /// Center X of the active tab, updated each render via canvas measurement.
    active_tab_center_x: Rc<Cell<Pixels>>,

    // Drag state (for future drag & drop support)
    dragging_tab: Option<DocumentId>,
    drop_target_index: Option<usize>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct TabContextMenu {
    pub tab_id: DocumentId,
    pub tab_index: usize,
    /// X position from the mouse click (window-absolute).
    pub position_x: Pixels,
    pub selected_index: usize,
}

pub const TAB_MENU_CLOSE: usize = 0;
pub const TAB_MENU_CLOSE_OTHERS: usize = 1;
pub const TAB_MENU_CLOSE_ALL: usize = 2;
#[allow(dead_code)]
pub const TAB_MENU_SEPARATOR: usize = 3;
pub const TAB_MENU_CLOSE_LEFT: usize = 4;
pub const TAB_MENU_CLOSE_RIGHT: usize = 5;

impl TabBar {
    pub fn new(tab_manager: Entity<TabManager>, cx: &mut Context<Self>) -> Self {
        Self {
            tab_manager,
            focus_handle: cx.focus_handle(),
            context_menu: None,
            active_tab_center_x: Rc::new(Cell::new(px(0.0))),
            dragging_tab: None,
            drop_target_index: None,
        }
    }

    pub fn context_menu_state(&self) -> Option<&TabContextMenu> {
        self.context_menu.as_ref()
    }

    pub fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        cx.notify();
    }

    pub fn context_menu_hover_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(ref mut menu) = self.context_menu
            && menu.selected_index != index
        {
            menu.selected_index = index;
            cx.notify();
        }
    }

    pub fn context_menu_execute_at(&mut self, action_index: usize, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };

        let tab_id = menu.tab_id;

        match action_index {
            TAB_MENU_CLOSE => cx.emit(TabBarEvent::CloseTab(tab_id)),
            TAB_MENU_CLOSE_OTHERS => cx.emit(TabBarEvent::CloseOtherTabs(tab_id)),
            TAB_MENU_CLOSE_ALL => cx.emit(TabBarEvent::CloseAllTabs),
            TAB_MENU_CLOSE_LEFT => cx.emit(TabBarEvent::CloseTabsToLeft(tab_id)),
            TAB_MENU_CLOSE_RIGHT => cx.emit(TabBarEvent::CloseTabsToRight(tab_id)),
            _ => {}
        }

        cx.notify();
    }

    pub fn build_tab_menu_items() -> Vec<MenuItem> {
        vec![
            MenuItem::new(dbflux_i18n::t!("document.tabs.menu.close")).icon(AppIcon::X),
            MenuItem::new(dbflux_i18n::t!("document.tabs.menu.close_others")).icon(AppIcon::X),
            MenuItem::new(dbflux_i18n::t!("document.tabs.menu.close_all")).icon(AppIcon::X),
            MenuItem::separator(),
            MenuItem::new(dbflux_i18n::t!("document.tabs.menu.close_left"))
                .icon(AppIcon::ChevronLeft),
            MenuItem::new(dbflux_i18n::t!("document.tabs.menu.close_right"))
                .icon(AppIcon::ChevronRight),
        ]
    }

    pub fn has_context_menu_open(&self) -> bool {
        self.context_menu.is_some()
    }

    /// Requests a close for one tab through the tab bar's single close route.
    ///
    /// The close button, a middle-click, and the context menu's Close item all
    /// emit `TabBarEvent::CloseTab`, so the workspace applies one close policy
    /// to every gesture instead of removing the tab from the bar directly.
    pub fn request_close(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        cx.emit(TabBarEvent::CloseTab(id));
    }

    /// Commits the input the active document still holds in an open editor,
    /// before a right-click on a tab opens the tab menu.
    ///
    /// The click moves focus out of the document, and an inline editor that
    /// loses focus drops what was typed into it before any menu item can run.
    /// Every item of this menu closes tabs, the active one included, so the
    /// value is committed first and the close it leads to asks about it. Input
    /// the document cannot commit stays where it is, and the close refuses it.
    fn commit_active_pending_input(&mut self, cx: &mut Context<Self>) {
        self.tab_manager.update(cx, |manager, cx| {
            if let Some(tab) = manager.active_id().and_then(|id| manager.document(id)) {
                tab.as_pane().commit_pending_input(cx);
            }
        });
    }

    pub fn open_context_menu_for_active(&mut self, cx: &mut Context<Self>) {
        let manager = self.tab_manager.read(cx);
        let Some(active_id) = manager.active_id() else {
            return;
        };

        let active_index = manager
            .documents()
            .iter()
            .position(|d| d.id() == active_id)
            .unwrap_or(0);

        self.context_menu = Some(TabContextMenu {
            tab_id: active_id,
            tab_index: active_index,
            position_x: self.active_tab_center_x.get(),
            selected_index: 0,
        });
        cx.notify();
    }

    pub fn context_menu_select_next(&mut self, cx: &mut Context<Self>) {
        let Some(ref mut menu) = self.context_menu else {
            return;
        };

        let items = Self::build_tab_menu_items();
        menu.selected_index = next_actionable_index(menu.selected_index, &items);
        cx.notify();
    }

    pub fn context_menu_select_prev(&mut self, cx: &mut Context<Self>) {
        let Some(ref mut menu) = self.context_menu else {
            return;
        };

        let items = Self::build_tab_menu_items();
        menu.selected_index = prev_actionable_index(menu.selected_index, &items);
        cx.notify();
    }

    pub fn context_menu_execute(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = &self.context_menu else {
            return;
        };

        self.context_menu_execute_at(menu.selected_index, cx);
    }
}

/// Returns the next non-separator index after `current`, or `current` if at the end.
#[expect(
    clippy::indexing_slicing,
    reason = "items[idx] is reached only while the loop condition holds idx < items.len()"
)]
pub fn next_actionable_index(current: usize, items: &[MenuItem]) -> usize {
    let mut idx = current + 1;
    while idx < items.len() {
        if !items[idx].is_separator {
            return idx;
        }
        idx += 1;
    }
    current
}

/// Returns the previous non-separator index before `current`, or `current` if at the start.
// current is the context menu's selected_index: 0 on open, then written by
// next/prev_actionable_index and by context_menu_hover_at with a rendered row
// index, so it stays below items.len(). This helper does not re-check that
// bound; current == items.len() still works, a larger value panics at
// items[idx].
#[expect(
    clippy::indexing_slicing,
    reason = "current is the menu's selected_index: 0 on open, then a next/prev_actionable_index result or a context_menu_hover_at index taken from the rendered build_tab_menu_items() rows, all below items.len(); this helper does not re-check the upper bound"
)]
pub fn prev_actionable_index(current: usize, items: &[MenuItem]) -> usize {
    if current == 0 {
        return current;
    }

    let mut idx = current - 1;
    loop {
        if !items[idx].is_separator {
            return idx;
        }
        if idx == 0 {
            return current;
        }
        idx -= 1;
    }
}

impl Render for TabBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let manager = self.tab_manager.read(cx);
        let active_id = manager.active_id();
        let drop_target_index = self.drop_target_index;

        let tab_data: Vec<_> = manager
            .documents()
            .iter()
            .map(|doc| {
                (
                    doc.meta_snapshot(cx),
                    doc.change_summary(cx),
                    doc.tab_tooltip(cx),
                )
            })
            .collect();

        let mut tabs: Vec<AnyElement> = Vec::with_capacity(tab_data.len());
        for (idx, (meta, change_summary, tooltip)) in tab_data.into_iter().enumerate() {
            tabs.push(
                self.render_tab(
                    meta,
                    change_summary,
                    tooltip,
                    idx,
                    active_id,
                    drop_target_index,
                    cx,
                )
                .into_any_element(),
            );
        }

        let new_tab_btn = self.render_new_tab_button(cx).into_any_element();

        document_tab_bar(cx).id("tab-bar").w_full().min_w_0().child(
            div()
                .id("document-tab-list")
                .role(Role::TabList)
                .flex()
                .min_w_0()
                .items_center()
                .overflow_x_scroll()
                .gap(TabMetrics::DOCUMENT_BAR_GAP)
                .children(tabs)
                .child(new_tab_btn),
        )
    }
}

impl TabBar {
    #[allow(clippy::too_many_arguments)]
    fn render_tab(
        &self,
        meta: DocumentMetaSnapshot,
        change_summary: Option<String>,
        tooltip: Option<SharedString>,
        idx: usize,
        active_id: Option<DocumentId>,
        drop_target_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = meta.id;
        let is_active = active_id == Some(id);
        let is_executing = meta.state == DocumentState::Executing;
        let is_dirty = meta.state == DocumentState::Modified;
        let is_drop_target = drop_target_index == Some(idx);

        let title = meta.title.clone();

        let tab_manager = self.tab_manager.clone();

        let icon = tab_icon(meta.icon);

        let center_x = self.active_tab_center_x.clone();

        let theme = cx.theme();
        let tint = ChromeColors::tint(theme);
        let icon_color = if is_active {
            tint
        } else {
            theme.muted_foreground
        };
        // One weight for every tab: a heavier active title is wider, so each
        // activation would resize two tabs and shift every tab after them.
        // The active tab stands out by its chip fill, strong text color and
        // tinted icon instead.
        let title_element =
            document_tab_title(title.as_str(), is_active, cx).font_weight(FontWeight::NORMAL);
        let hover_group: SharedString = format!("tab-group-{}", id.0).into();

        document_tab(
            ElementId::Name(format!("tab-{}", id.0).into()),
            is_active,
            cx,
        )
        .group(hover_group.clone())
        .debug_selector(|| format!("tab-{}", id.0))
        .role(Role::Tab)
        .aria_selected(is_active)
        .min_w(TabMetrics::DOCUMENT_TAB_MIN_WIDTH)
        .max_w(TabMetrics::DOCUMENT_TAB_MAX_WIDTH)
        .when(is_active, |el| {
            el.child(
                canvas(
                    move |bounds: Bounds<Pixels>, _, _| {
                        center_x.set(bounds.center().x);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
        })
        .when(is_drop_target, |el| el.border_l_2().border_color(tint))
        .when_some(tooltip, |el, tooltip| {
            el.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        })
        // Click to activate
        .on_click({
            let tab_manager = tab_manager.clone();
            cx.listener(move |_this, _event, _window, cx| {
                tab_manager.update(cx, |mgr, cx| {
                    mgr.activate(id, cx);
                });
            })
        })
        // Middle-click to close: routed through the workspace, like every
        // other close gesture, so pending edits are persisted first.
        .on_mouse_down(
            MouseButton::Middle,
            cx.listener(move |this, _event, _window, cx| {
                this.request_close(id, cx);
            }),
        )
        // Right-click for context menu
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.commit_active_pending_input(cx);
                this.context_menu = Some(TabContextMenu {
                    tab_id: id,
                    tab_index: idx,
                    position_x: event.position.x,
                    selected_index: 0,
                });
                cx.notify();
            }),
        )
        .child(Icon::new(icon).size(TabMetrics::ICON).color(icon_color))
        .child(
            div()
                .debug_selector(|| format!("tab-title-{}", id.0))
                .flex_1()
                .min_w_0()
                .truncate()
                .child(title_element),
        )
        .child(Self::render_dirty_indicator(id, is_dirty, change_summary))
        // A spinner and the close button take turns in one fixed slot, so a
        // query starting or finishing does not resize the tab either.
        .child(
            div()
                .debug_selector(|| format!("tab-action-{}", id.0))
                .flex()
                .flex_shrink_0()
                .justify_center()
                .w(TabMetrics::CLOSE_ICON)
                .child(self.render_tab_action(
                    id,
                    is_executing,
                    is_active,
                    hover_group,
                    &title,
                    cx,
                )),
        )
    }

    /// The tint diamond of a document with unsaved changes, with the change
    /// summary as its tooltip.
    ///
    /// A clean tab keeps the diamond's slot with nothing drawn in it, so a
    /// document turning dirty or being saved does not resize its tab and push
    /// the tabs after it.
    fn render_dirty_indicator(
        id: DocumentId,
        is_dirty: bool,
        change_summary: Option<String>,
    ) -> impl IntoElement {
        let slot = div()
            .id(ElementId::Name(format!("dirty-dot-{}", id.0).into()))
            .debug_selector(|| format!("dirty-dot-{}", id.0))
            .flex_shrink_0()
            .child(StatusIndicator::new(Status::Busy).compact());

        if !is_dirty {
            return slot.invisible();
        }

        let tooltip_text: SharedString = change_summary
            .unwrap_or_else(|| dbflux_i18n::t!("document.tabs.unsaved_changes"))
            .into();

        slot.tooltip(move |window, cx| Tooltip::new(tooltip_text.clone()).build(window, cx))
    }

    fn render_tab_action(
        &self,
        id: DocumentId,
        is_executing: bool,
        is_active: bool,
        hover_group: SharedString,
        title: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if is_executing {
            return Icon::new(AppIcon::Loader)
                .size(TabMetrics::CLOSE_ICON)
                .color(ChromeColors::tint(cx.theme()))
                .into_any_element();
        }

        let hover = cx.theme().secondary;

        div()
            .id(ElementId::Name(format!("tab-close-{}", id.0).into()))
            .debug_selector(|| format!("tab-close-{}", id.0))
            .role(Role::Button)
            .aria_label(dbflux_i18n::t!("document.tabs.close_named", title = title))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |el| el.bg(hover))
            // Inactive tabs show the button only while hovered. It is hidden
            // rather than left out, so it keeps its slot and is not clickable
            // while hidden.
            .when(!is_active, |el| {
                el.invisible()
                    .group_hover(hover_group, |style| style.visible())
            })
            .child(
                Icon::new(AppIcon::CircleX)
                    .size(TabMetrics::CLOSE_ICON)
                    .muted(),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.request_close(id, cx);
            }))
            .into_any_element()
    }

    fn render_new_tab_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .ml(TabMetrics::NEW_TAB_MARGIN_LEFT)
            .flex()
            .flex_shrink_0()
            .items_center()
            .child(
                Button::new("new-tab-btn", dbflux_i18n::t!("document.tabs.new"))
                    .ghost()
                    .icon(AppIcon::Plus)
                    .icon_size(TabMetrics::ICON)
                    .icon_only()
                    .on_click(cx.listener(|_this, _event, _window, cx| {
                        cx.emit(TabBarEvent::NewTabRequested);
                    })),
            )
    }
}

impl EventEmitter<TabBarEvent> for TabBar {}

#[derive(Clone, Debug)]
pub enum TabBarEvent {
    NewTabRequested,
    CloseTab(DocumentId),
    CloseOtherTabs(DocumentId),
    CloseAllTabs,
    CloseTabsToLeft(DocumentId),
    CloseTabsToRight(DocumentId),
}

#[cfg(test)]
mod tests {
    use super::{
        TAB_MENU_CLOSE, TAB_MENU_CLOSE_ALL, TAB_MENU_CLOSE_LEFT, TAB_MENU_CLOSE_OTHERS,
        TAB_MENU_CLOSE_RIGHT, TAB_MENU_SEPARATOR, TabBar, TabBarEvent, next_actionable_index,
        prev_actionable_index,
    };
    use crate::code::CodeDocument;
    use crate::tab_manager::Tab;
    use crate::tab_manager::TabManager;
    use crate::types::DocumentId;
    use dbflux_components::composites::document_tab_title;
    use dbflux_components::primitives::TextVariant;
    use dbflux_components::theme;
    use dbflux_components::typography::AppFonts;
    use dbflux_core::QueryLanguage;
    use dbflux_storage::bootstrap::StorageRuntime;
    use dbflux_ui_base::AppStateEntity;
    use dbflux_ui_base::toast::{ToastGlobal, ToastHost};
    use gpui::{
        AccessibilityFrame, AppContext as _, FontWeight, FrameObserver, Role, TestAppContext,
        VisualTestContext,
    };
    use gpui_component::theme::Theme;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    #[test]
    fn build_tab_menu_items_returns_correct_structure() {
        let items = TabBar::build_tab_menu_items();

        assert_eq!(items.len(), 6);
        assert_eq!(
            items[TAB_MENU_CLOSE].label.as_ref(),
            dbflux_i18n::t!("document.tabs.menu.close", locale = "en")
        );
        assert_eq!(
            items[TAB_MENU_CLOSE_OTHERS].label.as_ref(),
            dbflux_i18n::t!("document.tabs.menu.close_others", locale = "en")
        );
        assert_eq!(
            items[TAB_MENU_CLOSE_ALL].label.as_ref(),
            dbflux_i18n::t!("document.tabs.menu.close_all", locale = "en")
        );
        assert!(items[TAB_MENU_SEPARATOR].is_separator);
        assert_eq!(
            items[TAB_MENU_CLOSE_LEFT].label.as_ref(),
            dbflux_i18n::t!("document.tabs.menu.close_left", locale = "en")
        );
        assert_eq!(
            items[TAB_MENU_CLOSE_RIGHT].label.as_ref(),
            dbflux_i18n::t!("document.tabs.menu.close_right", locale = "en")
        );
    }

    #[test]
    fn tab_menu_keys_resolve_in_both_locales() {
        let keys = [
            "document.tabs.menu.close",
            "document.tabs.menu.close_others",
            "document.tabs.menu.close_all",
            "document.tabs.menu.close_left",
            "document.tabs.menu.close_right",
            "document.tabs.unsaved_changes",
        ];

        for key in keys {
            for locale in ["en", "es"] {
                let value = dbflux_i18n::t!(key, locale = locale);
                assert!(!value.is_empty(), "{locale}.{key} resolved empty");
                assert_ne!(value, key, "{locale}.{key} resolved to the raw key");
                assert_ne!(
                    value,
                    format!("{locale}.{key}"),
                    "{locale}.{key} resolved to the missing-translation sentinel"
                );
            }
        }
    }

    #[test]
    fn tab_menu_close_differs_between_locales() {
        let english = dbflux_i18n::t!("document.tabs.menu.close", locale = "en");
        let spanish = dbflux_i18n::t!("document.tabs.menu.close", locale = "es");

        assert_ne!(english, spanish);
    }

    #[test]
    fn build_tab_menu_items_have_icons() {
        let items = TabBar::build_tab_menu_items();

        for (idx, item) in items.iter().enumerate() {
            if item.is_separator {
                assert!(item.icon.is_none(), "separator should have no icon");
            } else {
                assert!(item.icon.is_some(), "item {} should have an icon", idx);
            }
        }
    }

    #[test]
    fn no_tab_menu_items_are_danger_or_submenu() {
        let items = TabBar::build_tab_menu_items();

        for item in &items {
            assert!(!item.is_danger);
            assert!(!item.has_submenu);
        }
    }

    #[test]
    fn next_actionable_skips_separator() {
        let items = TabBar::build_tab_menu_items();

        // 0 -> 1 -> 2 -> 4 (skip separator at 3) -> 5
        assert_eq!(next_actionable_index(0, &items), 1);
        assert_eq!(next_actionable_index(1, &items), 2);
        assert_eq!(next_actionable_index(2, &items), 4);
        assert_eq!(next_actionable_index(4, &items), 5);
    }

    #[test]
    fn next_actionable_stays_at_end() {
        let items = TabBar::build_tab_menu_items();
        assert_eq!(next_actionable_index(5, &items), 5);
    }

    #[test]
    fn prev_actionable_skips_separator() {
        let items = TabBar::build_tab_menu_items();

        // 5 -> 4 -> 2 (skip separator at 3) -> 1 -> 0
        assert_eq!(prev_actionable_index(5, &items), 4);
        assert_eq!(prev_actionable_index(4, &items), 2);
        assert_eq!(prev_actionable_index(2, &items), 1);
        assert_eq!(prev_actionable_index(1, &items), 0);
    }

    #[test]
    fn prev_actionable_stays_at_start() {
        let items = TabBar::build_tab_menu_items();
        assert_eq!(prev_actionable_index(0, &items), 0);
    }

    #[gpui::test]
    fn tab_titles_are_strong_when_active_and_muted_otherwise(cx: &mut TestAppContext) {
        cx.update(theme::init);

        let (active, inactive) = cx.update(|cx| {
            (
                document_tab_title("query.sql", true, cx).inspect(),
                document_tab_title("table/users", false, cx).inspect(),
            )
        });

        for inspection in [active, inactive] {
            assert_eq!(inspection.variant, TextVariant::Body);
            assert_eq!(inspection.family, AppFonts::INTERFACE);
        }

        assert!(active.has_custom_color_override);
        assert_eq!(active.weight_override, Some(FontWeight::SEMIBOLD));
        assert!(inactive.uses_muted_foreground_override);
    }

    /// The close button, a middle-click, and the context menu all emit the same
    /// `TabBarEvent::CloseTab`, so the workspace applies one close policy to
    /// every gesture instead of the bar removing tabs directly.
    #[gpui::test]
    fn every_close_gesture_requests_close_through_one_event(cx: &mut TestAppContext) {
        cx.update(theme::init);

        let manager = cx.update(|cx| cx.new(|_| TabManager::new()));
        let bar = cx.update(|cx| cx.new(|cx| TabBar::new(manager, cx)));
        let id = DocumentId::new();

        let events: Rc<RefCell<Vec<TabBarEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        cx.update(|cx| {
            cx.subscribe(&bar, move |_, event: &TabBarEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
        });

        cx.update(|cx| {
            bar.update(cx, |bar, cx| bar.request_close(id, cx));
        });
        cx.run_until_parked();

        let recorded = events.borrow().clone();
        assert!(
            matches!(recorded.as_slice(), [TabBarEvent::CloseTab(got)] if *got == id),
            "every close gesture shares one close request, got {recorded:?}"
        );
    }

    /// Keeps the latest rendered accessibility frame of the window it observes.
    #[derive(Default)]
    struct FrameCapture(Mutex<Option<AccessibilityFrame>>);

    impl FrameObserver for FrameCapture {
        fn accessibility_updated(&self, frame: &AccessibilityFrame) {
            *self.0.lock().expect("frame capture lock") = Some(frame.clone());
        }
    }

    /// Document tabs are exposed as tabs inside one tab list, and only the
    /// active tab reports itself selected.
    #[gpui::test]
    fn tabs_are_exposed_as_selectable_tabs_in_a_tab_list(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        cx.update(theme::init);
        cx.update(|cx| {
            let host = cx.new(|_| ToastHost::new());
            cx.set_global(ToastGlobal { host });
        });

        let app_state = cx.update(|cx| {
            cx.new(|_| {
                AppStateEntity::new_with_storage_runtime(
                    StorageRuntime::in_memory().expect("isolated storage runtime"),
                )
                .expect("test storage setup")
            })
        });
        let manager = cx.update(|cx| cx.new(|_| TabManager::new()));

        let capture = Arc::new(FrameCapture::default());
        let (_bar, window) = cx.add_window_view({
            let manager = manager.clone();
            let capture = capture.clone();
            move |window, cx| {
                window.observe_frames(&capture);
                TabBar::new(manager, cx)
            }
        });

        let open_tab = |window: &mut VisualTestContext| {
            let app_state = app_state.clone();
            let manager = manager.clone();
            window.update(|window, cx| {
                let document = cx.new(|cx| {
                    CodeDocument::new_with_language(app_state, None, QueryLanguage::Sql, window, cx)
                });
                let document_id = document.read(cx).id();
                let pane = CodeDocument::into_pane(document, cx);
                manager.update(cx, |manager, cx| {
                    manager.open(Tab::Pane(Box::new(pane)), cx)
                });
                document_id
            })
        };
        let background = open_tab(window);
        let active = open_tab(window);

        window.update(|window, _| window.refresh());
        window.run_until_parked();

        let frame = capture
            .0
            .lock()
            .expect("frame capture lock")
            .clone()
            .expect("the window rendered a frame");
        let node = |id: &str| {
            frame
                .nodes()
                .find(|(_, node)| node.id() == id)
                .and_then(|(_, node)| frame.accessibility_node(node))
                .unwrap_or_else(|| panic!("no accessible node {id}"))
        };

        assert_eq!(node("document-tab-list").role(), Role::TabList);

        for (id, selected) in [(active, true), (background, false)] {
            let tab = node(&format!("tab-{}", id.0));
            assert_eq!(tab.role(), Role::Tab);
            assert_eq!(tab.is_selected(), Some(selected), "tab {id:?}");
        }

        // The close button draws only an icon, so it is named after the tab it
        // closes.
        let tab_title =
            dbflux_components::accessibility::accessible_name(&frame, &format!("tab-{}", active.0))
                .expect("the tab is named by its title");
        let close = node(&format!("tab-close-{}", active.0));

        assert_eq!(close.role(), Role::Button);
        assert_eq!(
            close.label(),
            Some(
                dbflux_i18n::t!("document.tabs.close_named", title = tab_title)
                    .to_string()
                    .as_str()
            )
        );
    }
}

/// The icon a tab shows for its document kind, matching the icon the sidebar
/// gives the same object (a collection's box, a key-value database's key).
fn tab_icon(icon: super::types::DocumentIcon) -> AppIcon {
    use super::types::DocumentIcon;

    match icon {
        DocumentIcon::Sql => AppIcon::Code,
        DocumentIcon::Table => AppIcon::Table,
        DocumentIcon::Redis => AppIcon::KeyRound,
        DocumentIcon::RedisKey => AppIcon::Hash,
        DocumentIcon::Terminal => AppIcon::SquareTerminal,
        DocumentIcon::Mongo => AppIcon::Database,
        DocumentIcon::Collection => AppIcon::Box,
        DocumentIcon::Script => AppIcon::ScrollText,
        DocumentIcon::Audit => AppIcon::ScrollText,
        DocumentIcon::SchemaViz => AppIcon::Layers,
        DocumentIcon::Chart => AppIcon::ChartSpline,
        DocumentIcon::Dashboard => AppIcon::ChartSpline,
        DocumentIcon::Buckets => AppIcon::Box,
        DocumentIcon::ObjectBrowser => AppIcon::Folder,
        DocumentIcon::DumpAnalysis => AppIcon::HardDrive,
        DocumentIcon::McpApprovals => AppIcon::Bot,
        DocumentIcon::Migrate => AppIcon::ArrowUpDown,
        DocumentIcon::SecretStore => AppIcon::Lock,
    }
}

#[cfg(test)]
mod tab_icon_tests {
    use super::tab_icon;
    use crate::types::DocumentIcon;
    use dbflux_components::icons::AppIcon;

    #[test]
    fn collection_and_key_value_tabs_use_their_sidebar_icons() {
        assert_eq!(tab_icon(DocumentIcon::Collection), AppIcon::Box);
        assert_eq!(tab_icon(DocumentIcon::Redis), AppIcon::KeyRound);
        assert_eq!(tab_icon(DocumentIcon::SchemaViz), AppIcon::Layers);
    }
}

/// A tab's width must not depend on whether it is active or hovered, or every
/// tab switch moves the tabs after it.
#[cfg(test)]
mod tab_layout_tests {
    use super::TabBar;
    use crate::code::CodeDocument;
    use crate::tab_manager::{Tab, TabManager};
    use crate::types::DocumentId;
    use dbflux_components::theme;
    use dbflux_components::tokens::TabMetrics;
    use dbflux_core::QueryLanguage;
    use dbflux_storage::bootstrap::StorageRuntime;
    use dbflux_ui_base::AppStateEntity;
    use dbflux_ui_base::toast::{ToastGlobal, ToastHost};
    use gpui::{
        AppContext as _, Bounds, DevicePixels, Entity, Font, FontId, FontMetrics, FontRun, GlyphId,
        LineLayout, Modifiers, Pixels, PlatformTextSystem, RenderGlyphParams, ShapedGlyph,
        ShapedRun, Size, TestAppContext, TestDispatcher, TextRenderingMode, VisualTestContext,
        point, px, size,
    };
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    const UNITS_PER_EM: f32 = 1000.;

    thread_local! {
        /// Every line `WeightedTextSystem` shapes, with the font id (the
        /// weight) of each of its runs.
        static SHAPED_LINES: RefCell<Vec<(String, FontId)>> = const { RefCell::new(Vec::new()) };
    }

    /// The weights `prefix`-led lines were shaped in so far.
    fn weights_shaped_for(prefix: &str) -> BTreeSet<usize> {
        SHAPED_LINES.with(|lines| {
            lines
                .borrow()
                .iter()
                .filter(|(text, _)| text.starts_with(prefix))
                .map(|(_, font_id)| font_id.0)
                .collect()
        })
    }

    /// Shapes every glyph with an advance that grows with the font weight, as
    /// real fonts do. The default test text system gives all weights the same
    /// advance, which would hide a width change caused by a weight change.
    struct WeightedTextSystem;

    impl WeightedTextSystem {
        /// The font id is the weight, so `layout_line` can read it back.
        fn advance_units(font_id: FontId) -> f32 {
            400. + font_id.0 as f32 / 2.
        }
    }

    impl PlatformTextSystem for WeightedTextSystem {
        fn add_fonts(&self, _fonts: Vec<Cow<'static, [u8]>>) -> anyhow::Result<()> {
            Ok(())
        }

        fn all_font_names(&self) -> Vec<String> {
            Vec::new()
        }

        fn font_id(&self, descriptor: &Font) -> anyhow::Result<FontId> {
            Ok(FontId(descriptor.weight.0 as usize))
        }

        fn font_metrics(&self, _font_id: FontId) -> FontMetrics {
            FontMetrics {
                units_per_em: UNITS_PER_EM as u32,
                ascent: 800.,
                descent: -200.,
                line_gap: 0.,
                underline_position: -100.,
                underline_thickness: 50.,
                cap_height: 700.,
                x_height: 500.,
                bounding_box: Bounds {
                    origin: point(0., -200.),
                    size: size(1000., 1000.),
                },
            }
        }

        fn typographic_bounds(
            &self,
            font_id: FontId,
            _glyph_id: GlyphId,
        ) -> anyhow::Result<Bounds<f32>> {
            Ok(Bounds {
                origin: point(0., 0.),
                size: size(Self::advance_units(font_id), 700.),
            })
        }

        fn advance(&self, font_id: FontId, _glyph_id: GlyphId) -> anyhow::Result<Size<f32>> {
            Ok(size(Self::advance_units(font_id), 0.))
        }

        fn glyph_for_char(&self, _font_id: FontId, ch: char) -> Option<GlyphId> {
            Some(GlyphId(ch as u32))
        }

        fn glyph_raster_bounds(
            &self,
            _params: &RenderGlyphParams,
        ) -> anyhow::Result<Bounds<DevicePixels>> {
            Ok(Bounds::default())
        }

        fn rasterize_glyph(
            &self,
            _params: &RenderGlyphParams,
            raster_bounds: Bounds<DevicePixels>,
        ) -> anyhow::Result<(Size<DevicePixels>, Vec<u8>)> {
            Ok((raster_bounds.size, Vec::new()))
        }

        fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
            let mut position = px(0.);
            let mut shaped_runs = Vec::new();
            let mut run_start = 0;
            for run in runs {
                let run_end = (run_start + run.len).min(text.len());
                SHAPED_LINES.with(|lines| {
                    lines.borrow_mut().push((text.to_string(), run.font_id));
                });
                let advance = font_size * (Self::advance_units(run.font_id) / UNITS_PER_EM);
                let mut glyphs = Vec::new();
                for (ix, ch) in text.get(run_start..run_end).unwrap_or("").char_indices() {
                    glyphs.push(ShapedGlyph {
                        id: GlyphId(ch as u32),
                        position: point(position, px(0.)),
                        index: run_start + ix,
                        is_emoji: false,
                    });
                    position += advance;
                }
                shaped_runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs,
                });
                run_start = run_end;
            }

            LineLayout {
                font_size,
                width: position,
                ascent: font_size * 0.8,
                descent: font_size * 0.2,
                runs: shaped_runs,
                len: text.len(),
            }
        }

        fn recommended_rendering_mode(
            &self,
            _font_id: FontId,
            _font_size: Pixels,
        ) -> TextRenderingMode {
            TextRenderingMode::Grayscale
        }
    }

    /// Short, medium and long titles; the long one is cut at the widest a tab
    /// gets.
    const TITLES: [&str; 3] = [
        "Query 1.sql",
        "f3base.api_call_log",
        "f3base.balance_adjustments_by_operator_and_currency",
    ];

    struct Fixture {
        manager: Entity<TabManager>,
        tabs: Vec<DocumentId>,
    }

    fn weighted_text_cx() -> TestAppContext {
        TestAppContext::build_with_text_system(
            TestDispatcher::new(0),
            None,
            Arc::new(WeightedTextSystem),
        )
    }

    /// Opens a tab bar with one code tab per title; the last one is active.
    fn open_tab_bar(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
        cx.update(gpui_component::init);
        cx.update(theme::init);
        cx.update(|cx| {
            let host = cx.new(|_| ToastHost::new());
            cx.set_global(ToastGlobal { host });
        });

        let app_state = cx.update(|cx| {
            cx.new(|_| {
                AppStateEntity::new_with_storage_runtime(
                    StorageRuntime::in_memory().expect("isolated storage runtime"),
                )
                .expect("test storage setup")
            })
        });
        let manager = cx.update(|cx| cx.new(|_| TabManager::new()));

        let (_bar, window) = cx.add_window_view({
            let manager = manager.clone();
            move |_, cx| TabBar::new(manager, cx)
        });

        let tabs = TITLES
            .iter()
            .map(|title| {
                let app_state = app_state.clone();
                let manager = manager.clone();
                window.update(|window, cx| {
                    let document = cx.new(|cx| {
                        CodeDocument::new_with_language(
                            app_state,
                            None,
                            QueryLanguage::Sql,
                            window,
                            cx,
                        )
                        .with_title((*title).to_string())
                    });
                    let document_id = document.read(cx).id();
                    let pane = CodeDocument::into_pane(document, cx);
                    manager.update(cx, |manager, cx| {
                        manager.open(Tab::Pane(Box::new(pane)), cx)
                    });
                    document_id
                })
            })
            .collect();

        (Fixture { manager, tabs }, window)
    }

    /// Bounds of the element `{part}-{id}` of every tab, from a fresh frame.
    fn bounds_of(
        window: &mut VisualTestContext,
        fixture: &Fixture,
        part: &str,
    ) -> Vec<Bounds<Pixels>> {
        window.update(|window, _| window.refresh());
        window.run_until_parked();

        fixture
            .tabs
            .iter()
            .map(|id| {
                let selector: &'static str = Box::leak(format!("{part}-{}", id.0).into_boxed_str());
                window
                    .debug_bounds(selector)
                    .unwrap_or_else(|| panic!("no element {selector} was drawn"))
            })
            .collect()
    }

    fn activate(window: &mut VisualTestContext, fixture: &Fixture, index: usize) {
        let id = fixture.tabs[index];
        let manager = fixture.manager.clone();
        window.update(|_, cx| manager.update(cx, |manager, cx| manager.activate(id, cx)));
        window.run_until_parked();
    }

    #[test]
    fn activating_a_tab_keeps_every_tab_in_place() {
        let mut cx = weighted_text_cx();
        let (fixture, window) = open_tab_bar(&mut cx);

        let initial = bounds_of(window, &fixture, "tab");

        for index in 0..fixture.tabs.len() {
            activate(window, &fixture, index);

            assert_eq!(
                bounds_of(window, &fixture, "tab"),
                initial,
                "activating tab {index} moved or resized a tab"
            );
        }
    }

    #[test]
    fn hovering_a_tab_keeps_every_tab_in_place() {
        let mut cx = weighted_text_cx();
        let (fixture, window) = open_tab_bar(&mut cx);

        let initial = bounds_of(window, &fixture, "tab");

        for (index, tab) in initial.iter().enumerate() {
            window.simulate_mouse_move(tab.center(), None, Modifiers::none());

            assert_eq!(
                bounds_of(window, &fixture, "tab"),
                initial,
                "hovering tab {index} moved or resized a tab"
            );
        }
    }

    #[test]
    fn a_long_title_is_cut_the_same_whether_its_tab_is_active_or_not() {
        let mut cx = weighted_text_cx();
        let (fixture, window) = open_tab_bar(&mut cx);
        let long = TITLES.len() - 1;
        let max_width = window
            .update(|window, _| TabMetrics::DOCUMENT_TAB_MAX_WIDTH.to_pixels(window.rem_size()));

        let active_tab = bounds_of(window, &fixture, "tab")[long];
        let active_title = bounds_of(window, &fixture, "tab-title")[long];

        activate(window, &fixture, 0);

        let inactive_tab = bounds_of(window, &fixture, "tab")[long];
        let inactive_title = bounds_of(window, &fixture, "tab-title")[long];

        assert_eq!(
            active_tab.size.width, max_width,
            "the long title fills the widest tab"
        );
        assert_eq!(inactive_tab.size, active_tab.size);
        assert_eq!(
            inactive_title.size, active_title.size,
            "the title gets the same room active or not"
        );
        // Same room and same glyph widths cut the title at the same character.
        assert_eq!(
            weights_shaped_for("f3base.balance").len(),
            1,
            "the long title was shaped in more than one weight"
        );
    }

    /// Clean tabs keep the dirty diamond's slot, so a document turning dirty
    /// does not widen its tab.
    #[test]
    fn every_tab_reserves_the_dirty_indicator_and_action_slots() {
        let mut cx = weighted_text_cx();
        let (fixture, window) = open_tab_bar(&mut cx);

        for part in ["dirty-dot", "tab-action"] {
            let slots = bounds_of(window, &fixture, part);
            let width = slots[0].size.width;

            assert!(width > px(0.), "{part} has no width on a clean tab");
            assert!(
                slots.iter().all(|slot| slot.size.width == width),
                "{part} differs between tabs: {slots:?}"
            );
        }
    }
}
