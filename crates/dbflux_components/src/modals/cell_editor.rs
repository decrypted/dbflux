use crate::actions::{
    CompactEditorText, CopyEditorText, CopyEditorTextCompact, FormatEditorText, SaveEdit,
    ToggleDialogMaximized,
};
use crate::components::json_editor_view::JsonEditorView;
use crate::components::value_format::{
    self, EditedValue, ValueFormat, compact_value, edited_value, format_value,
};
use crate::controls::Button;
use crate::icons::AppIcon;
use crate::modals::{Modal, ResizeEdge};
use crate::tokens::ModalMetrics;
use crate::vim::{VimBinding, VimHost};
use dbflux_core::keymap_types::{Command, ContextId};
use gpui::*;
use gpui_component::input::EditorState;
use serde::{Deserialize, Serialize};
use std::rc::Rc;

/// Size the dialog opens with until the user resizes it.
const DEFAULT_SIZE: Size<Pixels> = Size {
    width: px(720.0),
    height: px(560.0),
};

/// The smallest size a drag leaves the dialog at, unless the window itself
/// is smaller.
const MIN_SIZE: Size<Pixels> = Size {
    width: px(560.0),
    height: px(360.0),
};

/// Share of the window a resized dialog may take.
const MAX_WINDOW_SHARE: f32 = 0.95;

/// Share of the window the maximized dialog takes.
const MAXIMIZED_WINDOW_SHARE: f32 = 0.94;

/// Height the editor keeps when the dialog is made small.
const MIN_EDITOR_HEIGHT: Pixels = px(120.0);

/// Event emitted when the modal editor saves.
#[derive(Clone)]
pub struct CellEditorSaveEvent {
    pub row: usize,
    pub col: usize,
    pub value: String,
}

/// Event emitted when the modal editor is closed.
#[derive(Clone)]
pub struct CellEditorClosedEvent;

/// Event emitted when Copy or Copy compact put the editor's text on the
/// clipboard, for the host's toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellEditorCopiedEvent {
    pub compact: bool,
    pub bytes: usize,
}

/// The size of the value editor dialog, shared by every cell editor: the
/// last size the user dragged it to and whether it was maximized.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorGeometry {
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl EditorGeometry {
    fn size(self) -> Size<Pixels> {
        size(px(self.width), px(self.height))
    }

    /// Whether a stored value describes a dialog at all.
    fn is_usable(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

type GeometryStore = Rc<dyn Fn(EditorGeometry) + 'static>;

/// The remembered geometry for this session, and where it is persisted when
/// a host installed a store.
#[derive(Default)]
struct EditorGeometryGlobal {
    geometry: Option<EditorGeometry>,
    store: Option<GeometryStore>,
}

impl Global for EditorGeometryGlobal {}

/// Persists the editor geometry beyond the session: `load` gives the stored
/// one, read once, and `store` is called with every new size. Installing
/// again is a no-op, so every host can call it.
pub fn install_editor_geometry_store(
    load: impl FnOnce() -> Option<EditorGeometry>,
    store: impl Fn(EditorGeometry) + 'static,
    cx: &mut App,
) {
    let global = cx.default_global::<EditorGeometryGlobal>();
    if global.store.is_some() {
        return;
    }

    if global.geometry.is_none() {
        global.geometry = load().filter(|geometry| geometry.is_usable());
    }
    global.store = Some(Rc::new(store));
}

fn remembered_geometry(cx: &App) -> Option<EditorGeometry> {
    cx.try_global::<EditorGeometryGlobal>()
        .and_then(|global| global.geometry)
}

fn remember_geometry(geometry: EditorGeometry, cx: &mut App) {
    let global = cx.default_global::<EditorGeometryGlobal>();
    global.geometry = Some(geometry);

    if let Some(store) = global.store.clone() {
        store(geometry);
    }
}

/// `size` kept between the minimum and the share of the window a dialog
/// may take. A window smaller than the minimum wins over the minimum.
fn clamp_to_window(size: Size<Pixels>, viewport: Size<Pixels>) -> Size<Pixels> {
    let max_width = viewport.width * MAX_WINDOW_SHARE;
    let max_height = viewport.height * MAX_WINDOW_SHARE;

    Size {
        width: size.width.max(MIN_SIZE.width).min(max_width),
        height: size.height.max(MIN_SIZE.height).min(max_height),
    }
}

/// The size a drag of `edge` from `from` to `to` gives a card that was
/// `start` when it began. The card stays centered, so it grows on both sides
/// and the dragged edge follows the pointer at twice the pointer's offset.
fn dragged_size(
    start: Size<Pixels>,
    edge: ResizeEdge,
    from: Point<Pixels>,
    to: Point<Pixels>,
    viewport: Size<Pixels>,
) -> Size<Pixels> {
    let mut size = start;

    if edge.resizes_width() {
        size.width = start.width + (to.x - from.x) * 2.0;
    }
    if edge.resizes_height() {
        size.height = start.height + (to.y - from.y) * 2.0;
    }

    clamp_to_window(size, viewport)
}

/// A resize drag in progress.
#[derive(Clone, Copy, Debug)]
struct ResizeDrag {
    edge: ResizeEdge,
    from: Point<Pixels>,
    start: Size<Pixels>,
}

/// Modal editor for JSON and long text values.
///
/// A JSON column, and a text cell whose content parses as a JSON object or
/// array or as XML (the same detection as the value panel), open in the
/// structured editor: pretty-printed, highlighted, with a status line and
/// Format and Compact. Saving keeps the stored layout (see
/// [`value_format::edited_value`]).
pub struct CellEditorModal {
    visible: bool,
    row: usize,
    col: usize,
    /// The column holds JSON: text that does not parse is refused rather
    /// than stored as text.
    is_json: bool,
    /// How the dialog reads the value.
    format: ValueFormat,
    /// The cell's value as the dialog received it.
    original_value: String,
    /// The text the dialog opened with, to tell an edited value from one that
    /// was only looked at.
    opened_value: String,
    /// Name and type of the edited column, for the title.
    column: Option<(String, String)>,
    input: Entity<EditorState>,
    focus_handle: FocusHandle,
    validation_error: Option<String>,
    /// Shown when Save was pressed on structured text that no longer parses
    /// in a text column; holds the text it was shown for, which the next Save
    /// stores as typed.
    invalid_warning: Option<(String, String)>,
    /// The size the user gave the dialog, kept for the session.
    size: Size<Pixels>,
    maximized: bool,
    drag: Option<ResizeDrag>,
    vim: VimBinding,
    /// Re-renders on every edit so the JSON status line follows the text.
    _input_observation: Subscription,
}

impl CellEditorModal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // soft_wrap defaults to true in 0.6.1, so the old explicit builder is gone.
        let input = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .line_number(true)
        });
        let input_observation = cx.observe(&input, |this: &mut Self, input, cx| {
            // A warning is about the text it was shown for; an edit retires it.
            let edited = this
                .invalid_warning
                .as_ref()
                .is_some_and(|(text, _)| input.read(cx).value().as_ref() != text.as_str());
            if edited {
                this.invalid_warning = None;
            }
            cx.notify();
        });
        let vim = VimBinding::new(input.clone(), window, cx);

        let mut modal = Self {
            visible: false,
            row: 0,
            col: 0,
            is_json: false,
            format: ValueFormat::Text,
            original_value: String::new(),
            opened_value: String::new(),
            column: None,
            input,
            focus_handle: cx.focus_handle(),
            validation_error: None,
            invalid_warning: None,
            size: DEFAULT_SIZE,
            maximized: false,
            drag: None,
            vim,
            _input_observation: input_observation,
        };

        let input = modal.vim.input_id();
        VimBinding::follow_setting(&mut modal, input, cx);
        modal
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// How the dialog reads the value it holds.
    pub fn format(&self) -> ValueFormat {
        self.format
    }

    /// The text the editor holds.
    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Whether the dialog fills most of the window.
    pub fn is_maximized(&self) -> bool {
        self.maximized
    }

    /// The size the dialog is drawn at in a window of `viewport`.
    pub fn card_size(&self, viewport: Size<Pixels>) -> Size<Pixels> {
        if self.maximized {
            return Size {
                width: viewport.width * MAXIMIZED_WINDOW_SHARE,
                height: viewport.height * MAXIMIZED_WINDOW_SHARE,
            };
        }

        clamp_to_window(self.size, viewport)
    }

    /// Opens the editor on a cell. `column` is the column's name and type,
    /// shown in the title ("Edit properties (jsonb)"). `is_json` is for a
    /// JSON column; any other value is read as JSON or XML when it parses as
    /// one.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        row: usize,
        col: usize,
        value: String,
        is_json: bool,
        column: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.row = row;
        self.col = col;
        self.is_json = is_json;
        self.format = if is_json {
            ValueFormat::Json
        } else {
            value_format::detect_format(&value)
        };
        self.column = column;
        self.visible = true;
        self.validation_error = None;
        self.invalid_warning = None;
        self.drag = None;

        if let Some(geometry) = remembered_geometry(cx) {
            self.size = geometry.size();
            self.maximized = geometry.maximized;
        }

        let shown = if self.format.is_structured() && !value.is_empty() {
            format_value(&value, self.format).unwrap_or_else(|_| value.clone())
        } else {
            value.clone()
        };
        self.original_value = value;

        let language = self.format.editor_language();
        self.input.update(cx, |state, cx| {
            state.set_highlighter(language, cx);
            state.set_value(&shown, window, cx);
            state.focus(window, cx);
        });
        // Read back rather than kept from `shown`: the editor may normalize
        // the text it is given, such as its line endings.
        self.opened_value = self.input.read(cx).value().to_string();

        cx.notify();
    }

    /// What Save would store for `edited`: `Ok(None)` when it is no edit,
    /// `Err` with the message the dialog now shows when it is refused or
    /// needs a second Save.
    fn resolve_edit(&mut self, edited: String) -> Result<Option<String>, String> {
        match edited_value(
            &self.original_value,
            &self.opened_value,
            &edited,
            self.format,
        ) {
            EditedValue::Unchanged => Ok(None),
            EditedValue::Write(value) => Ok(Some(value)),
            EditedValue::Invalid { text, error } => {
                if self.is_json {
                    self.validation_error = Some(error.clone());
                    return Err(error);
                }

                if self
                    .invalid_warning
                    .as_ref()
                    .is_some_and(|(warned, _)| *warned == text)
                {
                    return Ok(Some(text));
                }

                let warning = dbflux_i18n::t!(
                    "modals.cell_editor.invalid_warning",
                    format = self.format.label(),
                    error = error
                );
                self.invalid_warning = Some((text, warning.clone()));
                Err(warning)
            }
        }
    }

    /// Hands over the value the dialog holds when it differs from the one it
    /// opened with, and hides the dialog, as Save does.
    ///
    /// For a host that is about to close the document under the dialog, which
    /// stays reachable while it is open. `Ok(None)` when the dialog is closed
    /// or its text is unchanged. A value that Save would not store yet (JSON
    /// that does not parse, or structured text that needs its warning read
    /// first) stays in the open dialog with its message shown, and is
    /// reported as `Err` with that message. No save or close event is
    /// emitted: the host writes the value itself and decides where focus
    /// goes.
    pub fn take_unsaved_edit(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<Option<CellEditorSaveEvent>, String> {
        if !self.visible {
            return Ok(None);
        }

        let edited = self.input.read(cx).value().to_string();
        let value = match self.resolve_edit(edited) {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(None),
            Err(error) => {
                cx.notify();
                return Err(error);
            }
        };

        self.visible = false;
        self.validation_error = None;
        self.invalid_warning = None;
        cx.notify();

        Ok(Some(CellEditorSaveEvent {
            row: self.row,
            col: self.col,
            value,
        }))
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        let was_visible = self.visible;
        self.visible = false;
        self.validation_error = None;
        self.invalid_warning = None;
        self.drag = None;

        if was_visible {
            cx.emit(CellEditorClosedEvent);
        }

        cx.notify();
    }

    fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let edited = self.input.read(cx).value().to_string();

        match self.resolve_edit(edited) {
            Ok(Some(value)) => {
                cx.emit(CellEditorSaveEvent {
                    row: self.row,
                    col: self.col,
                    value,
                });
                self.close(cx);
            }
            // Nothing changed: Save closes the dialog without an edit.
            Ok(None) => self.close(cx),
            Err(_) => cx.notify(),
        }
    }

    /// Replaces the text with `transform` of it, or shows why it cannot.
    fn transform(
        &mut self,
        transform: fn(&str, ValueFormat) -> Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.format.is_structured() {
            return;
        }

        let value = self.input.read(cx).value().to_string();
        match transform(&value, self.format) {
            Ok(next) => {
                self.input.update(cx, |state, cx| {
                    state.set_value(&next, window, cx);
                });
                self.validation_error = None;
            }
            Err(error) => self.validation_error = Some(error),
        }
        cx.notify();
    }

    fn compact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transform(compact_value, window, cx);
    }

    fn format_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transform(format_value, window, cx);
    }

    /// Puts the editor's text on the clipboard as it stands, unsaved edits
    /// included, or compacted. Text that does not compact is not copied and
    /// the parse error is shown instead.
    fn copy(&mut self, compact: bool, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();

        let text = if compact {
            if !self.format.is_structured() {
                return;
            }
            match compact_value(&value, self.format) {
                Ok(compact) => compact,
                Err(error) => {
                    self.validation_error = Some(error);
                    cx.notify();
                    return;
                }
            }
        } else {
            value
        };

        let bytes = text.len();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.emit(CellEditorCopiedEvent { compact, bytes });
    }

    fn toggle_maximized(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.maximized {
            self.maximized = false;
        } else {
            self.size = self.card_size(window.viewport_size());
            self.maximized = true;
        }
        self.drag = None;
        self.remember(cx);
        cx.notify();
    }

    fn begin_resize(
        &mut self,
        edge: ResizeEdge,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = self.card_size(window.viewport_size());
        self.size = start;
        self.maximized = false;
        self.drag = Some(ResizeDrag {
            edge,
            from: position,
            start,
        });
        cx.notify();
    }

    fn update_resize(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag else {
            return;
        };

        self.size = dragged_size(
            drag.start,
            drag.edge,
            drag.from,
            position,
            window.viewport_size(),
        );
        cx.notify();
    }

    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.remember(cx);
        }
        cx.notify();
    }

    fn remember(&self, cx: &mut Context<Self>) {
        remember_geometry(
            EditorGeometry {
                width: self.size.width.into(),
                height: self.size.height.into(),
                maximized: self.maximized,
            },
            cx,
        );
    }

    fn title(&self) -> String {
        let structured_text = !self.is_json && self.format.is_structured();

        match &self.column {
            Some((name, type_name)) => {
                // A text column read as JSON says both: "(TEXT · JSON)".
                let type_name = if structured_text {
                    format!("{type_name} · {}", self.format.label())
                } else {
                    type_name.clone()
                };
                dbflux_i18n::t!(
                    "modals.cell_editor.title_column",
                    column = name,
                    type_name = type_name
                )
            }
            None => match self.format {
                ValueFormat::Json => dbflux_i18n::t!("modals.cell_editor.title_json"),
                ValueFormat::Xml => dbflux_i18n::t!("modals.cell_editor.title_xml"),
                ValueFormat::Text => dbflux_i18n::t!("modals.cell_editor.title_text"),
            },
        }
    }
}

impl VimHost for CellEditorModal {
    fn vim(&self, input: EntityId) -> Option<&VimBinding> {
        self.vim.for_input(input)
    }

    fn vim_mut(&mut self, input: EntityId) -> Option<&mut VimBinding> {
        self.vim.for_input_mut(input)
    }

    /// `<leader> s` runs the dialog's own save, as its primary button does.
    fn vim_dialog_command(
        &mut self,
        _input: EntityId,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if command == Command::SaveQuery {
            self.save(window, cx);
        }
    }
}

impl EventEmitter<CellEditorSaveEvent> for CellEditorModal {}
impl EventEmitter<CellEditorClosedEvent> for CellEditorModal {}
impl EventEmitter<CellEditorCopiedEvent> for CellEditorModal {}

/// Vim's listeners on the editor's container. In Insert mode the modal's
/// Escape (`Cancel`) leaves Insert mode instead of closing, and in Normal
/// mode Enter (`Execute`) moves down instead of reaching the modal.
fn vim_wrapper(element: Div, input: EntityId, cx: &mut Context<CellEditorModal>) -> Div {
    let element = VimBinding::wire(element, input, cx);
    let element = VimBinding::capture_action::<crate::actions::Cancel, _>(element, input, cx);
    VimBinding::capture_action::<crate::actions::Execute, _>(element, input, cx)
}

impl Render for CellEditorModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }

        let structured = self.format.is_structured();
        let entity = cx.entity().downgrade();

        let close = {
            let entity = entity.clone();
            move |_window: &mut Window, cx: &mut App| {
                entity.update(cx, |this, cx| this.close(cx)).ok();
            }
        };

        let warning = self
            .invalid_warning
            .as_ref()
            .map(|(_, warning)| warning.clone());

        let copy_compact: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>> = structured
            .then(|| -> Box<dyn Fn(&ClickEvent, &mut Window, &mut App)> {
                Box::new(cx.listener(|this, _, _, cx| this.copy(true, cx)))
            });

        let mut editor = JsonEditorView::new(
            "cell-editor",
            &self.input,
            cx.listener(|this, _, window, cx| this.save(window, cx)),
            cx.listener(|this, _, _, cx| this.close(cx)),
        )
        .validation_error(self.validation_error.clone())
        .warning(warning)
        .format(self.format)
        .readonly(self.vim.locked(false))
        .below_editor(self.vim.render_indicator(cx))
        .min_editor_height(MIN_EDITOR_HEIGHT)
        .copy_buttons(
            cx.listener(|this, _, _, cx| this.copy(false, cx)),
            copy_compact,
        );

        if structured {
            editor = editor.show_format_buttons(
                cx.listener(|this, _, window, cx| this.format_text(window, cx)),
                cx.listener(|this, _, window, cx| this.compact(window, cx)),
            );
        }

        let maximize_label = if self.maximized {
            dbflux_i18n::t!("modals.cell_editor.restore")
        } else {
            dbflux_i18n::t!("modals.cell_editor.maximize")
        };
        let maximize = Button::new("cell-editor-maximize", "")
            .ghost()
            .inline()
            .icon(if self.maximized {
                AppIcon::Minimize2
            } else {
                AppIcon::Maximize2
            })
            .icon_size(ModalMetrics::CLOSE_ICON)
            .icon_only()
            .aria_label(maximize_label)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_maximized(window, cx)));

        let card = self.card_size(window.viewport_size());

        let on_start = {
            let entity = entity.clone();
            move |edge, position, window: &mut Window, cx: &mut App| {
                entity
                    .update(cx, |this, cx| this.begin_resize(edge, position, window, cx))
                    .ok();
            }
        };
        let on_move = {
            let entity = entity.clone();
            move |position, window: &mut Window, cx: &mut App| {
                entity
                    .update(cx, |this, cx| this.update_resize(position, window, cx))
                    .ok();
            }
        };
        let on_end = move |_window: &mut Window, cx: &mut App| {
            entity.update(cx, |this, cx| this.finish_resize(cx)).ok();
        };

        Modal::new(self.title())
            .id("cell-editor-modal")
            .focus_handle(&self.focus_handle)
            .on_close(close)
            .key_context(ContextId::CellEditorModal.as_gpui_context())
            .icon(AppIcon::Pencil)
            .width(card.width)
            .height(card.height)
            .header_actions(maximize)
            .resizable(self.drag.is_some(), on_start, on_move, on_end)
            .child(
                vim_wrapper(
                    self.vim
                        .leader_scope(div(), cx)
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .on_action(cx.listener(|this, _: &SaveEdit, window, cx| {
                            this.save(window, cx);
                        }))
                        .on_action(cx.listener(|this, _: &CopyEditorText, _, cx| {
                            this.copy(false, cx);
                        }))
                        .on_action(cx.listener(|this, _: &CopyEditorTextCompact, _, cx| {
                            this.copy(true, cx);
                        }))
                        .on_action(cx.listener(|this, _: &FormatEditorText, window, cx| {
                            this.format_text(window, cx);
                        }))
                        .on_action(cx.listener(|this, _: &CompactEditorText, window, cx| {
                            this.compact(window, cx);
                        }))
                        .on_action(cx.listener(|this, _: &ToggleDialogMaximized, window, cx| {
                            this.toggle_maximized(window, cx);
                        })),
                    self.vim.input_id(),
                    cx,
                )
                .child(editor.render(cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cell_editor_keys_resolve_in_both_locales() {
        let keys = [
            "modals.cell_editor.title_json",
            "modals.cell_editor.title_text",
            "modals.cell_editor.title_column",
            "modals.cell_editor.title_xml",
            "modals.cell_editor.maximize",
            "modals.cell_editor.restore",
            "modals.cell_editor.invalid_warning",
        ];

        for key in keys {
            let en = dbflux_i18n::t!(key, locale = "en");
            let es = dbflux_i18n::t!(key, locale = "es");
            assert!(!en.is_empty() && en != key, "en missing for {key}");
            assert!(!es.is_empty() && es != key, "es missing for {key}");
        }
    }

    #[test]
    fn cell_editor_title_json_diverges_between_locales() {
        let en = dbflux_i18n::t!("modals.cell_editor.title_json", locale = "en");
        let es = dbflux_i18n::t!("modals.cell_editor.title_json", locale = "es");
        assert_ne!(en, es);
    }
}

#[cfg(test)]
mod keyboard_tests {
    // Explicit imports rather than the parent glob: combining `use super::*`
    // with `#[gpui::test]` sends the gpui_macros expansion into unbounded
    // recursion.
    use super::{CellEditorModal, CellEditorSaveEvent};
    use crate::actions::RunCommand;
    use gpui::{
        AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
        Render, SharedString, Styled as _, TestAppContext, Window, div,
    };

    /// Stands in for the document behind the dialog: records every keymap
    /// command that bubbles out of it.
    struct Host {
        modal: Entity<CellEditorModal>,
        commands: Vec<SharedString>,
        saves: usize,
    }

    impl Render for Host {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .on_action(cx.listener(|this, action: &RunCommand, _, _| {
                    this.commands.push(action.command.clone());
                }))
                .child(self.modal.clone())
        }
    }

    fn host(window: &mut Window, cx: &mut Context<Host>) -> Host {
        let modal = cx.new(|cx| CellEditorModal::new(window, cx));
        cx.subscribe(&modal, |host, _, _: &CellEditorSaveEvent, _| {
            host.saves += 1
        })
        .detach();

        Host {
            modal,
            commands: Vec::new(),
            saves: 0,
        }
    }

    #[gpui::test]
    fn escape_closes_the_cell_editor(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            // The escape binding the app keymap gives the cell editor (see
            // the cell editor layer in `dbflux_ui_base::keymap`).
            cx.bind_keys([gpui::KeyBinding::new(
                "escape",
                crate::actions::Cancel,
                Some(dbflux_core::keymap_types::ContextId::CellEditorModal.as_gpui_context()),
            )]);
        });

        let (host, window) = cx.add_window_view(host);
        let modal = window.update(|_, cx| host.read(cx).modal.clone());

        window.update(|window, cx| {
            modal.update(cx, |modal, cx| {
                modal.open(0, 0, "{\"a\": 1}".to_string(), true, None, window, cx);
            });
        });
        window.run_until_parked();
        assert!(window.update(|_, cx| modal.read(cx).is_visible()));

        window.simulate_keystrokes("escape");

        assert!(
            !window.update(|_, cx| modal.read(cx).is_visible()),
            "Escape closes the editor through its own key context"
        );
    }

    /// Opening the editor puts the keyboard in its text buffer: text typed
    /// through the window reaches the editor without a click first, and
    /// Escape still closes the dialog from there.
    #[gpui::test]
    fn typing_right_after_opening_reaches_the_editor(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys([gpui::KeyBinding::new(
                "escape",
                crate::actions::Cancel,
                Some(dbflux_core::keymap_types::ContextId::CellEditorModal.as_gpui_context()),
            )]);
        });

        let (host, window) = cx.add_window_view(host);
        let modal = window.update(|_, cx| host.read(cx).modal.clone());

        window.update(|window, cx| {
            modal.update(cx, |modal, cx| {
                modal.open(0, 0, String::new(), false, None, window, cx);
            });
        });
        window.update(|window, _| window.refresh());
        window.run_until_parked();

        window.simulate_input("typed");
        window.run_until_parked();

        let value = window.update(|_, cx| modal.read(cx).input.read(cx).value().to_string());
        assert_eq!(value, "typed", "typing reaches the editor without a click");

        window.simulate_keystrokes("escape");

        assert!(
            !window.update(|_, cx| modal.read(cx).is_visible()),
            "Escape closes the editor while its buffer has focus"
        );
    }

    /// The save shortcut still reaches the dialog while its buffer has focus.
    #[gpui::test]
    fn the_save_shortcut_saves_from_the_focused_editor(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys([gpui::KeyBinding::new(
                "ctrl-s",
                crate::actions::SaveEdit,
                Some(dbflux_core::keymap_types::ContextId::CellEditorModal.as_gpui_context()),
            )]);
        });

        let (host, window) = cx.add_window_view(host);
        let modal = window.update(|_, cx| host.read(cx).modal.clone());

        let saved = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
        let _subscription = window.update(|_, cx| {
            let saved = saved.clone();
            cx.subscribe(&modal, move |_, event: &super::CellEditorSaveEvent, _| {
                *saved.borrow_mut() = Some(event.value.clone());
            })
        });

        window.update(|window, cx| {
            modal.update(cx, |modal, cx| {
                modal.open(0, 0, String::new(), false, None, window, cx);
            });
        });
        window.update(|window, _| window.refresh());
        window.run_until_parked();

        window.simulate_input("typed");
        window.simulate_keystrokes("ctrl-s");
        window.run_until_parked();

        assert_eq!(saved.borrow().as_deref(), Some("typed"));
        assert!(!window.update(|_, cx| modal.read(cx).is_visible()));
    }

    /// Opens the cell editor with the app's modal bindings for Escape and
    /// Enter and the leader sequences `space s` (Save) and `space r` (Run
    /// query), Vim mode set to `vim`, and the keyboard in the editor.
    fn open_with_vim<'a>(
        cx: &'a mut TestAppContext,
        vim: bool,
        value: &str,
    ) -> (
        Entity<Host>,
        Entity<CellEditorModal>,
        &'a mut gpui::VisualTestContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::actions::record_last_keystroke(cx);
            crate::vim::set_vim_enabled(cx, vim);
            let context = dbflux_core::keymap_types::ContextId::CellEditorModal.as_gpui_context();
            let leader = dbflux_core::keymap_types::ContextId::VimNormal.default_predicate();
            cx.bind_keys([
                gpui::KeyBinding::new("escape", crate::actions::Cancel, Some(context)),
                gpui::KeyBinding::new("enter", crate::actions::Execute, Some("Modal")),
                gpui::KeyBinding::new(
                    "space s",
                    crate::vim::LeaderCommand::new("save_query"),
                    Some(leader),
                ),
                gpui::KeyBinding::new(
                    "space r",
                    crate::vim::LeaderCommand::new("run_query"),
                    Some(leader),
                ),
            ]);
        });

        let (host, window) = cx.add_window_view(host);
        let modal = window.update(|_, cx| host.read(cx).modal.clone());
        let value = value.to_string();

        window.update(|window, cx| {
            modal.update(cx, |modal, cx| {
                modal.open(0, 0, value, false, None, window, cx);
            });
        });
        window.run_until_parked();

        (host, modal, window)
    }

    fn text_and_cursor(
        modal: &Entity<CellEditorModal>,
        window: &mut gpui::VisualTestContext,
    ) -> (String, usize) {
        window.update(|_, cx| {
            let state = modal.read(cx).input.read(cx);
            (state.value().to_string(), state.cursor())
        })
    }

    /// With Vim mode on, the first Escape leaves Insert mode and keeps the
    /// editor open, the second closes it; Enter in Normal mode moves down.
    #[gpui::test]
    fn vim_mode_edits_the_cell_and_escape_steps_out(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_with_vim(cx, true, "ab\ncd");

        window.simulate_keystrokes("enter");
        window.run_until_parked();
        assert_eq!(text_and_cursor(&modal, window), ("ab\ncd".into(), 3));

        window.simulate_keystrokes("i");
        window.simulate_input("X");
        window.simulate_keystrokes("escape");
        window.run_until_parked();
        assert_eq!(text_and_cursor(&modal, window).0, "ab\nXcd");
        assert!(
            window.update(|_, cx| modal.read(cx).is_visible()),
            "the first Escape only leaves Insert mode"
        );

        window.simulate_keystrokes("escape");
        window.run_until_parked();
        assert!(!window.update(|_, cx| modal.read(cx).is_visible()));
    }

    /// With Vim mode off, keys type and one Escape closes, as before.
    #[gpui::test]
    fn without_vim_mode_escape_closes_at_once(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_with_vim(cx, false, "ab");

        window.simulate_input("j");
        window.simulate_keystrokes("escape");
        window.run_until_parked();

        assert_eq!(text_and_cursor(&modal, window).0, "jab");
        assert!(!window.update(|_, cx| modal.read(cx).is_visible()));
    }

    /// `<leader> s` in Normal mode saves through the dialog's own save and
    /// closes it; nothing reaches the document behind the dialog.
    #[gpui::test]
    fn leader_s_saves_the_cell_through_the_dialog(cx: &mut TestAppContext) {
        let (host, modal, window) = open_with_vim(cx, true, "ab");
        // Normal mode locks the buffer against typing, not against the host.
        window.update(|window, cx| {
            modal.update(cx, |modal, cx| {
                modal
                    .input
                    .update(cx, |input, cx| input.set_value("abc", window, cx));
            });
        });

        window.simulate_keystrokes("space s");
        window.run_until_parked();

        assert_eq!(window.update(|_, cx| host.read(cx).saves), 1);
        assert!(!window.update(|_, cx| modal.read(cx).is_visible()));
        assert!(
            window.update(|_, cx| host.read(cx).commands.is_empty()),
            "Save never reaches the document behind the dialog"
        );
    }

    /// A leader command with no meaning in the dialog (run the query) does
    /// nothing: the dialog stays open with its text, no key reaches Vim, and
    /// nothing reaches the document behind it.
    #[gpui::test]
    fn leader_r_does_nothing_in_the_dialog(cx: &mut TestAppContext) {
        let (host, modal, window) = open_with_vim(cx, true, "ab");

        window.simulate_keystrokes("space r");
        window.run_until_parked();

        assert!(window.update(|_, cx| modal.read(cx).is_visible()));
        assert_eq!(text_and_cursor(&modal, window), ("ab".into(), 0));
        assert_eq!(window.update(|_, cx| host.read(cx).saves), 0);
        assert!(
            window.update(|_, cx| host.read(cx).commands.is_empty()),
            "Run query never reaches the document behind the dialog"
        );
    }

    /// Opens the dialog on `value` in a host window and replaces its text
    /// with `edited`, as typing would.
    fn open_and_edit<'a>(
        cx: &'a mut TestAppContext,
        value: &str,
        is_json: bool,
        edited: Option<&str>,
    ) -> (
        Entity<Host>,
        Entity<CellEditorModal>,
        &'a mut gpui::VisualTestContext,
    ) {
        cx.update(gpui_component::init);

        let (host, window) = cx.add_window_view(host);
        let modal = window.update(|_, cx| host.read(cx).modal.clone());
        let value = value.to_string();

        window.update(|window, cx| {
            modal.update(cx, |modal, cx| {
                modal.open(2, 1, value, is_json, None, window, cx);
            });
        });

        if let Some(edited) = edited {
            window.update(|window, cx| {
                modal.update(cx, |modal, cx| {
                    modal
                        .input
                        .update(cx, |input, cx| input.set_value(edited, window, cx));
                });
            });
        }
        window.run_until_parked();

        (host, modal, window)
    }

    /// An edited value is handed over and the dialog hides, without a save
    /// event: the host writes the value itself.
    #[gpui::test]
    fn an_edited_value_is_handed_over_for_a_close(cx: &mut TestAppContext) {
        let (host, modal, window) = open_and_edit(cx, "old", false, Some("new"));

        let taken =
            window.update(|_, cx| modal.update(cx, |modal, cx| modal.take_unsaved_edit(cx)));
        window.run_until_parked();

        let edit = taken
            .expect("a text value is always valid")
            .expect("an edited value is handed over");
        assert_eq!((edit.row, edit.col, edit.value.as_str()), (2, 1, "new"));
        assert!(!window.update(|_, cx| modal.read(cx).is_visible()));
        assert_eq!(
            window.update(|_, cx| host.read(cx).saves),
            0,
            "the hand-over emits no save event"
        );
    }

    /// A value that was only looked at is not an edit.
    #[gpui::test]
    fn an_unchanged_value_is_not_handed_over(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_and_edit(cx, "{\"a\":1}", true, None);

        let taken =
            window.update(|_, cx| modal.update(cx, |modal, cx| modal.take_unsaved_edit(cx)));

        assert_eq!(taken.map(|edit| edit.map(|edit| edit.value)), Ok(None));
        assert!(
            window.update(|_, cx| modal.read(cx).is_visible()),
            "the dialog stays as it is"
        );
    }

    /// A JSON value that does not parse stays in the dialog with its error.
    #[gpui::test]
    fn an_invalid_json_value_stays_in_the_dialog(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_and_edit(cx, "{\"a\":1}", true, Some("{\"a\":"));

        let taken =
            window.update(|_, cx| modal.update(cx, |modal, cx| modal.take_unsaved_edit(cx)));

        assert!(taken.is_err(), "an invalid value is refused");
        let (visible, has_error) = window.update(|_, cx| {
            let modal = modal.read(cx);
            (modal.is_visible(), modal.validation_error.is_some())
        });
        assert!(visible, "the dialog keeps the value");
        assert!(has_error, "the dialog shows why");
    }

    /// A closed dialog holds nothing.
    #[gpui::test]
    fn a_closed_dialog_hands_over_nothing(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (host, window) = cx.add_window_view(host);
        let modal = window.update(|_, cx| host.read(cx).modal.clone());

        let taken =
            window.update(|_, cx| modal.update(cx, |modal, cx| modal.take_unsaved_edit(cx)));

        assert_eq!(taken.map(|edit| edit.map(|edit| edit.value)), Ok(None));
    }
}

#[cfg(test)]
mod structured_text_tests {
    // Explicit imports rather than the parent glob: combining `use super::*`
    // with `#[gpui::test]` sends the gpui_macros expansion into unbounded
    // recursion.
    use super::{
        CellEditorCopiedEvent, CellEditorModal, CellEditorSaveEvent, DEFAULT_SIZE, EditorGeometry,
        MIN_SIZE, install_editor_geometry_store,
    };
    use crate::actions;
    use crate::components::value_format::{ValueFormat, format_value};
    use dbflux_core::keymap_types::ContextId;
    use gpui::{
        AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, ParentElement as _,
        Pixels, Point, Render, Size, Styled as _, TestAppContext, VisualTestContext, Window, div,
        point, px, size,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The API response of the issue, stored compact in a TEXT column.
    const RAW_RESULT: &str = r#"{"success":true,"msg":"ok","data":{"partner_user_id":"u-81","balances":[{"currency":"USDT","amount":"2725.8328350479","usd":2312.64496440},{"currency":"BTC","amount":"0.00010000","usd":1e2}]}}"#;

    const WINDOW: Size<Pixels> = Size {
        width: px(1600.0),
        height: px(1000.0),
    };

    struct Host {
        modal: Entity<CellEditorModal>,
        saved: Vec<String>,
        copied: Vec<CellEditorCopiedEvent>,
    }

    impl Render for Host {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.modal.clone())
        }
    }

    fn host(window: &mut Window, cx: &mut Context<Host>) -> Host {
        let modal = cx.new(|cx| CellEditorModal::new(window, cx));
        cx.subscribe(&modal, |host, _, event: &CellEditorSaveEvent, _| {
            host.saved.push(event.value.clone());
        })
        .detach();
        cx.subscribe(&modal, |host, _, event: &CellEditorCopiedEvent, _| {
            host.copied.push(event.clone());
        })
        .detach();

        Host {
            modal,
            saved: Vec::new(),
            copied: Vec::new(),
        }
    }

    /// The cell editor keys of the app keymap (the cell editor layer).
    fn bind_keys(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let context = Some(ContextId::CellEditorModal.as_gpui_context());
            cx.bind_keys([
                gpui::KeyBinding::new("escape", actions::Cancel, context),
                gpui::KeyBinding::new("ctrl-s", actions::SaveEdit, context),
                gpui::KeyBinding::new("ctrl-shift-k", actions::CopyEditorText, context),
                gpui::KeyBinding::new("ctrl-shift-h", actions::CopyEditorTextCompact, context),
                gpui::KeyBinding::new("ctrl-shift-u", actions::FormatEditorText, context),
                gpui::KeyBinding::new("ctrl-shift-j", actions::CompactEditorText, context),
                gpui::KeyBinding::new("ctrl-shift-enter", actions::ToggleDialogMaximized, context),
            ]);
        });
    }

    /// Opens a cell of a text column `raw_result (TEXT)` holding `value`.
    fn open_text_cell<'a>(
        cx: &'a mut TestAppContext,
        value: &str,
    ) -> (
        Entity<Host>,
        Entity<CellEditorModal>,
        &'a mut VisualTestContext,
    ) {
        bind_keys(cx);

        let (host, window) = cx.add_window_view(host);
        window.simulate_resize(WINDOW);
        let modal = window.update(|_, cx| host.read(cx).modal.clone());
        let value = value.to_string();

        window.update(|window, cx| {
            modal.update(cx, |modal, cx| {
                modal.open(
                    3,
                    1,
                    value,
                    false,
                    Some(("raw_result".to_string(), "TEXT".to_string())),
                    window,
                    cx,
                );
            });
        });
        window.run_until_parked();

        (host, modal, window)
    }

    fn set_text(modal: &Entity<CellEditorModal>, window: &mut VisualTestContext, text: &str) {
        window.update(|window, cx| {
            modal.update(cx, |modal, cx| {
                modal
                    .input
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            });
        });
        window.run_until_parked();
    }

    fn text(modal: &Entity<CellEditorModal>, window: &mut VisualTestContext) -> String {
        window.update(|_, cx| modal.read(cx).text(cx))
    }

    fn pretty() -> String {
        format_value(RAW_RESULT, ValueFormat::Json).expect("valid json")
    }

    fn clipboard(window: &mut VisualTestContext) -> Option<String> {
        window
            .read_from_clipboard()
            .and_then(|item| item.text().map(|text| text.to_string()))
    }

    #[gpui::test]
    fn a_compact_json_text_cell_opens_pretty_in_the_json_editor(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_text_cell(cx, RAW_RESULT);

        let (format, title, language) = window.update(|_, cx| {
            let modal = modal.read(cx);
            (
                modal.format(),
                modal.title(),
                modal.input.read(cx).language_name().to_string(),
            )
        });

        assert_eq!(format, ValueFormat::Json);
        assert_eq!(text(&modal, window), pretty());
        assert!(text(&modal, window).lines().count() > 10);
        assert_eq!(language, "json");
        assert!(title.contains("TEXT · JSON"), "{title}");
    }

    #[gpui::test]
    fn saving_an_untouched_value_writes_nothing(cx: &mut TestAppContext) {
        let (host, modal, window) = open_text_cell(cx, RAW_RESULT);

        window.simulate_keystrokes("ctrl-s");
        window.run_until_parked();

        assert!(window.update(|_, cx| host.read(cx).saved.is_empty()));
        assert!(!window.update(|_, cx| modal.read(cx).is_visible()));
    }

    #[gpui::test]
    fn an_edit_saves_compact_text_with_every_other_token_kept(cx: &mut TestAppContext) {
        let (host, modal, window) = open_text_cell(cx, RAW_RESULT);
        set_text(&modal, window, &pretty().replace("\"u-81\"", "\"u-82\""));

        window.simulate_keystrokes("ctrl-s");
        window.run_until_parked();

        let saved = window.update(|_, cx| host.read(cx).saved.clone());
        assert_eq!(saved, vec![RAW_RESULT.replace("\"u-81\"", "\"u-82\"")]);
    }

    #[gpui::test]
    fn invalid_json_is_saved_as_typed_after_a_warning(cx: &mut TestAppContext) {
        let (host, modal, window) = open_text_cell(cx, RAW_RESULT);
        let broken = pretty().replacen('}', "", 1);
        set_text(&modal, window, &broken);

        window.simulate_keystrokes("ctrl-s");
        window.run_until_parked();

        let (saved, visible, warned) = window.update(|_, cx| {
            let modal = modal.read(cx);
            (
                host.read(cx).saved.clone(),
                modal.is_visible(),
                modal.invalid_warning.is_some(),
            )
        });
        assert!(saved.is_empty(), "the first Save only warns");
        assert!(visible);
        assert!(warned, "the warning is shown before anything is saved");

        window.simulate_keystrokes("ctrl-s");
        window.run_until_parked();

        assert_eq!(
            window.update(|_, cx| host.read(cx).saved.clone()),
            vec![broken]
        );
        assert!(!window.update(|_, cx| modal.read(cx).is_visible()));
    }

    #[gpui::test]
    fn an_edit_after_the_warning_needs_a_new_warning(cx: &mut TestAppContext) {
        let (host, modal, window) = open_text_cell(cx, RAW_RESULT);
        set_text(&modal, window, "{\"a\":");
        window.simulate_keystrokes("ctrl-s");
        window.run_until_parked();

        set_text(&modal, window, "{\"a\":[");
        assert!(window.update(|_, cx| modal.read(cx).invalid_warning.is_none()));

        window.simulate_keystrokes("ctrl-s");
        window.run_until_parked();
        assert!(window.update(|_, cx| host.read(cx).saved.is_empty()));
    }

    #[gpui::test]
    fn plain_text_opens_and_saves_unchanged(cx: &mut TestAppContext) {
        let note = "{not json, just a note";
        let (host, modal, window) = open_text_cell(cx, note);

        assert_eq!(
            window.update(|_, cx| modal.read(cx).format()),
            ValueFormat::Text
        );
        assert_eq!(text(&modal, window), note);
        assert_eq!(
            window.update(|_, cx| modal.read(cx).title()),
            dbflux_i18n::t!(
                "modals.cell_editor.title_column",
                column = "raw_result",
                type_name = "TEXT"
            )
        );

        set_text(&modal, window, "edited  note ");
        window.simulate_keystrokes("ctrl-s");
        window.run_until_parked();

        assert_eq!(
            window.update(|_, cx| host.read(cx).saved.clone()),
            vec!["edited  note ".to_string()]
        );
    }

    #[gpui::test]
    fn bare_json_scalars_stay_plain_text(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_text_cell(cx, "2312.64496440");

        assert_eq!(
            window.update(|_, cx| modal.read(cx).format()),
            ValueFormat::Text
        );
        assert_eq!(text(&modal, window), "2312.64496440");
    }

    #[gpui::test]
    fn copy_puts_the_shown_text_on_the_clipboard(cx: &mut TestAppContext) {
        let (host, modal, window) = open_text_cell(cx, RAW_RESULT);
        let edited = pretty().replace("\"ok\"", "\"edited\"");
        set_text(&modal, window, &edited);

        window.simulate_keystrokes("ctrl-shift-k");
        window.run_until_parked();

        assert_eq!(clipboard(window), Some(edited.clone()));
        assert_eq!(
            window.update(|_, cx| host.read(cx).copied.clone()),
            vec![CellEditorCopiedEvent {
                compact: false,
                bytes: edited.len(),
            }]
        );
        assert!(
            window.update(|_, cx| modal.read(cx).is_visible()),
            "copying keeps the dialog open"
        );
    }

    #[gpui::test]
    fn copy_compact_copies_compact_json(cx: &mut TestAppContext) {
        let (host, _modal, window) = open_text_cell(cx, RAW_RESULT);

        window.simulate_keystrokes("ctrl-shift-h");
        window.run_until_parked();

        assert_eq!(clipboard(window).as_deref(), Some(RAW_RESULT));
        assert!(window.update(|_, cx| {
            host.read(cx)
                .copied
                .first()
                .is_some_and(|copy| copy.compact)
        }));
    }

    #[gpui::test]
    fn ctrl_c_with_a_selection_copies_only_the_selection(cx: &mut TestAppContext) {
        let (host, modal, window) = open_text_cell(cx, RAW_RESULT);
        window.update(|_, cx| {
            modal.update(cx, |modal, cx| {
                modal
                    .input
                    .update(cx, |input, cx| input.set_selected_range(4..13, cx));
            });
        });
        window.run_until_parked();

        window.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        window.run_until_parked();

        assert_eq!(clipboard(window).as_deref(), Some("\"success\""));
        assert!(window.update(|_, cx| host.read(cx).copied.is_empty()));
    }

    #[gpui::test]
    fn format_and_compact_rewrite_only_the_layout(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_text_cell(cx, RAW_RESULT);

        window.simulate_keystrokes("ctrl-shift-j");
        window.run_until_parked();
        assert_eq!(text(&modal, window), RAW_RESULT);

        window.simulate_keystrokes("ctrl-shift-u");
        window.run_until_parked();
        assert_eq!(text(&modal, window), pretty());
    }

    /// The card is centered in the window: the point just inside its
    /// bottom-right corner.
    fn corner_of(card: Size<Pixels>) -> Point<Pixels> {
        point(
            (WINDOW.width + card.width) / 2.0 - px(3.0),
            (WINDOW.height + card.height) / 2.0 - px(3.0),
        )
    }

    fn card(modal: &Entity<CellEditorModal>, window: &mut VisualTestContext) -> Size<Pixels> {
        window.update(|_, cx| modal.read(cx).card_size(WINDOW))
    }

    fn editor_width(modal: &Entity<CellEditorModal>, window: &mut VisualTestContext) -> Pixels {
        window.update(|_, cx| modal.read(cx).input.read(cx).input_bounds().size.width)
    }

    fn drag(window: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
        window.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        window.run_until_parked();
        window.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
        window.run_until_parked();
        window.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        window.run_until_parked();
    }

    #[gpui::test]
    fn dragging_the_corner_resizes_the_editor_down_to_a_minimum(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_text_cell(cx, RAW_RESULT);
        let start = card(&modal, window);
        assert_eq!(start, DEFAULT_SIZE);
        let editor_before = editor_width(&modal, window);

        let from = corner_of(start);
        drag(window, from, point(from.x + px(100.0), from.y + px(50.0)));

        assert_eq!(
            card(&modal, window),
            size(start.width + px(200.0), start.height + px(100.0)),
            "the centered card grows on both sides"
        );
        assert!(
            editor_width(&modal, window) > editor_before,
            "the editor grows with the card"
        );

        let from = corner_of(card(&modal, window));
        drag(window, from, point(px(0.0), px(0.0)));
        assert_eq!(card(&modal, window), MIN_SIZE);
    }

    #[gpui::test]
    fn maximize_toggles_and_restores_the_size(cx: &mut TestAppContext) {
        let (_host, modal, window) = open_text_cell(cx, RAW_RESULT);

        window.simulate_keystrokes("ctrl-shift-enter");
        window.run_until_parked();
        let maximized = card(&modal, window);
        assert!(window.update(|_, cx| modal.read(cx).is_maximized()));
        assert!(maximized.width > WINDOW.width * 0.9, "{maximized:?}");
        assert!(maximized.height > WINDOW.height * 0.9, "{maximized:?}");

        window.simulate_keystrokes("ctrl-shift-enter");
        window.run_until_parked();
        assert!(!window.update(|_, cx| modal.read(cx).is_maximized()));
        assert_eq!(card(&modal, window), DEFAULT_SIZE);
    }

    #[gpui::test]
    fn the_next_editor_opens_at_the_remembered_size(cx: &mut TestAppContext) {
        let stored = Rc::new(RefCell::new(Vec::<EditorGeometry>::new()));
        let store = stored.clone();
        cx.update(|cx| {
            install_editor_geometry_store(
                || None,
                move |geometry| store.borrow_mut().push(geometry),
                cx,
            )
        });

        let (_host, modal, window) = open_text_cell(cx, RAW_RESULT);
        let from = corner_of(card(&modal, window));
        drag(window, from, point(from.x + px(60.0), from.y + px(40.0)));
        let resized = card(&modal, window);
        window.simulate_keystrokes("escape");
        window.run_until_parked();

        assert_eq!(
            stored.borrow().last().copied(),
            Some(EditorGeometry {
                width: resized.width.into(),
                height: resized.height.into(),
                maximized: false,
            }),
            "the store gets the new size"
        );

        let other = window.update(|window, cx| cx.new(|cx| CellEditorModal::new(window, cx)));
        window.update(|window, cx| {
            other.update(cx, |modal, cx| {
                modal.open(0, 0, "x".to_string(), false, None, window, cx);
            });
        });

        assert_eq!(card(&other, window), resized);
    }

    #[gpui::test]
    fn a_stored_size_is_used_from_the_first_open(cx: &mut TestAppContext) {
        cx.update(|cx| {
            install_editor_geometry_store(
                || {
                    Some(EditorGeometry {
                        width: 900.0,
                        height: 700.0,
                        maximized: false,
                    })
                },
                |_| {},
                cx,
            )
        });

        let (_host, modal, window) = open_text_cell(cx, RAW_RESULT);
        assert_eq!(card(&modal, window), size(px(900.0), px(700.0)));
    }
}
