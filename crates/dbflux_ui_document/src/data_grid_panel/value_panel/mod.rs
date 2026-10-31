//! Value panel content for the workspace-level inspector rail.
//!
//! # `ValuePanelContent`
//!
//! Shows one cell's full value in a code editor, read as JSON, XML, or plain
//! text. All chrome (title bar, close button, resize grip) is owned by
//! `WorkspaceInspector`, the same as the row inspector.
//!
//! # Editing
//!
//! The panel is a real editor, not a viewer: Save writes through
//! `DataGridPanel::handle_cell_editor_save`, which is the same edit-buffer path
//! the inline grid editor and the cell-editor modal use. A saved value is
//! therefore a pending change like any other — the row still needs Save Row to
//! reach the database, and Revert All undoes it.
//!
//! What is written is exactly what stands in the editor. Pretty-printing is an
//! explicit button rather than something Save does silently, so formatting a
//! value to read it never rewrites what is stored.
//!
//! # Following the cursor
//!
//! `DataGridPanel` reloads the panel as the selection moves, but only while the
//! editor is unmodified — see `is_modified`. Otherwise moving the cursor would
//! silently discard whatever the user had typed.
//!
//! # Images
//!
//! A binary cell, or a text cell holding base64, whose bytes start like an
//! image opens on the picture (see `image_view`), with a switch to the hex
//! dump or the text. Binary cells are read-only here: their hex dump is a
//! view of the bytes, not something that can be written back.

mod host;
mod image_view;
pub(crate) mod picture;

pub(crate) use image_view::ValueView;
pub(crate) use picture::{CellBytes, ImageKind};

use dbflux_components::components::value_format::{
    self, ValueFormat, compact_value, format_value, validate_value,
};
use dbflux_components::controls::{
    Button, ButtonVariant, GpuiInput as Input, InputEvent, InputState,
};
use dbflux_components::icons::AppIcon;
use dbflux_components::primitives::{Icon, Text};
use dbflux_components::tokens::{FontSizes, Heights, Radii, Spacing};
use dbflux_components::vim::{VimBinding, VimHost};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::EditorState;
use gpui_component::{ActiveTheme, Sizable};

/// Everything the panel needs to show one cell.
#[derive(Clone, Debug)]
pub(crate) struct ValuePanelTarget {
    /// Visual row index, matching `CellCoord::row`.
    pub row: usize,
    pub col: usize,
    pub column_name: String,
    /// The cell's text exactly as the edit buffer holds it.
    pub value: String,
    /// The stored bytes of a binary cell; `None` for every other value,
    /// and for a binary cell with a pending edit.
    pub bytes: Option<CellBytes>,
    /// False for read-only results and read-only columns; hides the footer.
    pub editable: bool,
}

/// A button of the panel, run from the keyboard through the grid's Toolbar
/// submenu (see `ValuePanelContent::buttons`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ValuePanelButton {
    /// A format of the header's selector.
    Format(ValueFormat),
    /// The header's word-wrap toggle.
    Wrap,
    /// The footer's Format: pretty-prints the value.
    PrettyPrint,
    Compact,
    Revert,
    Save,
    /// The Image / Hex / Text switch of a value that holds an image.
    View(ValueView),
    SaveImage,
    CopyImage,
}

/// What the panel asks of the grid, which owns the database writes, the
/// save prompt and the clipboard toasts.
#[derive(Clone, Debug)]
pub(crate) enum ValuePanelEvent {
    /// Routed into the shared cell-save path.
    Save {
        row: usize,
        col: usize,
        value: String,
    },
    SaveImage {
        kind: ImageKind,
        bytes: std::sync::Arc<[u8]>,
        column: String,
    },
    CopyImage {
        kind: ImageKind,
        bytes: std::sync::Arc<[u8]>,
    },
}

pub(crate) struct ValuePanelContent {
    target: ValuePanelTarget,
    format: ValueFormat,
    word_wrap: bool,
    /// The text last written into the editor. `is_modified` compares against
    /// this rather than against the cell, so pretty-printing alone does not
    /// count as an edit the user must resolve.
    loaded_text: String,
    input: Entity<EditorState>,
    /// Vim mode for the editor; rebuilt with it.
    vim: VimBinding,
    /// Whether the editor holds the keyboard. The results keymap binds bare
    /// letters to grid commands, so `DataGridPanel::active_context` has to
    /// hand the keyboard to the text layer while the user is typing here.
    editor_focused: bool,
    _input_subscription: Subscription,
    error: Option<String>,
    /// Holds the keyboard while the user reads the panel without editing;
    /// Enter moves it on into the editor.
    focus_handle: FocusHandle,
    /// The image the value holds, if it starts like one.
    image: Option<image_view::ImageView>,
    /// Whether the picture is shown instead of the editor.
    show_image: bool,
    _release: Subscription,
}

impl EventEmitter<ValuePanelEvent> for ValuePanelContent {}

impl VimHost for ValuePanelContent {
    fn vim(&self, input: EntityId) -> Option<&VimBinding> {
        self.vim.for_input(input)
    }

    fn vim_mut(&mut self, input: EntityId) -> Option<&mut VimBinding> {
        self.vim.for_input_mut(input)
    }
}

impl ValuePanelContent {
    pub(crate) fn new(
        target: ValuePanelTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let format = value_format::detect_format(&target.value);
        let word_wrap = true;
        let text = stored_text(&target, format);
        let (input, subscription) = build_input(text.clone(), format, word_wrap, window, cx);
        let vim = VimBinding::new(input.clone(), window, cx);

        let mut panel = Self {
            target,
            format,
            word_wrap,
            loaded_text: text,
            input,
            vim,
            editor_focused: false,
            _input_subscription: subscription,
            error: None,
            focus_handle: cx.focus_handle(),
            image: None,
            show_image: false,
            _release: cx.on_release(|panel, cx| panel.release_image(None, cx)),
        };

        let input = panel.vim.input_id();
        VimBinding::follow_setting(&mut panel, input, cx);
        panel.load_image(Some(window), cx);
        panel
    }

    /// Replaces the editor, with a fresh Vim binding for it.
    fn install_input(
        &mut self,
        input: Entity<EditorState>,
        subscription: Subscription,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.vim = VimBinding::new(input.clone(), window, cx);
        self.input = input;
        self._input_subscription = subscription;
        let input = self.vim.input_id();
        VimBinding::follow_setting(self, input, cx);
    }

    /// The panel's own focus handle, which keyboard focus takes when it moves
    /// into the panel from the grid.
    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Hand the keyboard to the editor, to change the value.
    ///
    /// Marks the editor as focused right away rather than on its `Focus`
    /// event, which arrives only with the next frame: until then the grid
    /// would keep reporting the panel's keys and claim the letters typed.
    pub(crate) fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| state.focus(window, cx));
        self.editor_focused = true;
        cx.notify();
    }

    /// Take the keyboard back from the editor to the panel, keeping the
    /// text as typed. Mirrors `focus_editor`.
    pub(crate) fn leave_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        self.editor_focused = false;
        cx.notify();
    }

    /// Scroll the value by a line, a page, or to either end. The editor
    /// clamps the offset to its content when it lays out.
    pub(crate) fn scroll(
        &self,
        step: crate::data_grid_panel::side_island::IslandScroll,
        cx: &mut Context<Self>,
    ) {
        use crate::data_grid_panel::side_island::IslandScroll;

        self.input.update(cx, |state, cx| {
            let Some(line_height) = state.line_height() else {
                return;
            };

            let viewport = state.input_bounds().size.height;
            let page = (viewport - line_height).max(line_height);
            let offset = state.scroll_offset();

            let target = match step {
                IslandScroll::LineUp => offset.y + line_height,
                IslandScroll::LineDown => offset.y - line_height,
                IslandScroll::PageUp => offset.y + page,
                IslandScroll::PageDown => offset.y - page,
                IslandScroll::Top => Pixels::ZERO,
                IslandScroll::Bottom => Pixels::MIN,
            };

            state.set_scroll_offset(point(offset.x, target.min(Pixels::ZERO)), cx);
        });
    }

    /// Whether the panel's editor currently owns the keyboard.
    pub(crate) fn editor_has_focus(&self) -> bool {
        self.editor_focused
    }

    #[cfg(test)]
    pub(crate) fn editor_text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Point the panel at a different cell, re-detecting the format.
    pub(crate) fn open(
        &mut self,
        target: ValuePanelTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.format = value_format::detect_format(&target.value);
        self.target = target;
        self.error = None;
        self.reload(window, cx);
        self.load_image(Some(window), cx);
    }

    /// The cell the panel is currently showing.
    pub(crate) fn target_cell(&self) -> (usize, usize) {
        (self.target.row, self.target.col)
    }

    /// Whether the editor holds something other than what was loaded into it.
    ///
    /// Drives whether the panel may follow the cursor: an unsaved edit pins it
    /// to its cell until the user saves or reverts.
    pub(crate) fn is_modified(&self, cx: &App) -> bool {
        self.input.read(cx).value().as_ref() != self.loaded_text
    }

    /// Rebuild the editor from the target value in the current format.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = stored_text(&self.target, self.format);
        let (input, subscription) =
            build_input(text.clone(), self.format, self.word_wrap, window, cx);
        self.install_input(input, subscription, window, cx);
        self.editor_focused = false;
        self.loaded_text = text;
        cx.notify();
    }

    fn set_format(&mut self, format: ValueFormat, window: &mut Window, cx: &mut Context<Self>) {
        if self.format == format {
            return;
        }

        // Carry the user's edits across the switch rather than dropping back to
        // the stored value; only the presentation is changing.
        let current = self.input.read(cx).value().to_string();
        let was_modified = current != self.loaded_text;

        self.format = format;
        self.error = None;

        let text = if was_modified {
            current
        } else {
            stored_text(&self.target, format)
        };

        let (input, subscription) = build_input(text.clone(), format, self.word_wrap, window, cx);
        self.install_input(input, subscription, window, cx);
        self.editor_focused = false;
        // A modified buffer stays modified across the switch, so `loaded_text`
        // must keep pointing at the stored value's rendering.
        self.loaded_text = if was_modified {
            stored_text(&self.target, format)
        } else {
            text
        };
        cx.notify();
    }

    fn toggle_word_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.word_wrap = !self.word_wrap;
        let wrap = self.word_wrap;
        self.input.update(cx, |state, cx| {
            state.set_soft_wrap(wrap, window, cx);
        });
        cx.notify();
    }

    fn apply_transform(
        &mut self,
        transform: fn(&str, ValueFormat) -> Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.input.read(cx).value().to_string();
        match transform(&current, self.format) {
            Ok(next) => {
                self.error = None;
                self.input.update(cx, |state, cx| {
                    state.set_value(next, window, cx);
                });
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let text = self.loaded_text.clone();
        self.input.update(cx, |state, cx| {
            state.set_value(text, window, cx);
        });
        cx.notify();
    }

    /// The buttons the panel shows enabled right now, the format and view
    /// shown excepted: the other view of a value holding an image, then on
    /// the picture Save image and Copy image, otherwise the other formats
    /// (not for a hex dump) and the wrap toggle, then for an editable value
    /// Format and Compact (structured formats) and, once the value changed,
    /// Revert and Save.
    pub(crate) fn buttons(&self, cx: &App) -> Vec<ValuePanelButton> {
        let mut buttons: Vec<ValuePanelButton> = self
            .other_views()
            .into_iter()
            .map(ValuePanelButton::View)
            .collect();

        if self.shows_image() {
            if self.decoded_image().is_some() {
                buttons.push(ValuePanelButton::SaveImage);
                buttons.push(ValuePanelButton::CopyImage);
            }
            return buttons;
        }

        if self.target.bytes.is_none() {
            buttons.extend(
                ValueFormat::ALL
                    .into_iter()
                    .filter(|format| *format != self.format)
                    .map(ValuePanelButton::Format),
            );
        }
        buttons.push(ValuePanelButton::Wrap);

        if !self.target.editable {
            return buttons;
        }

        if self.format.is_structured() {
            buttons.push(ValuePanelButton::PrettyPrint);
            buttons.push(ValuePanelButton::Compact);
        }

        if self.is_modified(cx) {
            buttons.push(ValuePanelButton::Revert);
            buttons.push(ValuePanelButton::Save);
        }

        buttons
    }

    /// Runs what `button` runs when clicked.
    pub(crate) fn press(
        &mut self,
        button: ValuePanelButton,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match button {
            ValuePanelButton::Format(format) => self.set_format(format, window, cx),
            ValuePanelButton::Wrap => self.toggle_word_wrap(window, cx),
            ValuePanelButton::PrettyPrint => self.apply_transform(format_value, window, cx),
            ValuePanelButton::Compact => self.apply_transform(compact_value, window, cx),
            ValuePanelButton::Revert => self.revert(window, cx),
            ValuePanelButton::Save => self.save(cx),
            ValuePanelButton::View(view) => self.set_view(view, cx),
            ValuePanelButton::SaveImage => self.request_save_image(cx),
            ValuePanelButton::CopyImage => self.request_copy_image(cx),
        }
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();

        if let Err(error) = validate_value(&value, self.format) {
            self.error = Some(error);
            cx.notify();
            return;
        }

        self.error = None;
        // The saved text becomes the new baseline: the panel is no longer
        // modified and may follow the cursor again.
        self.loaded_text = value.clone();
        self.target.value = value.clone();

        cx.emit(ValuePanelEvent::Save {
            row: self.target.row,
            col: self.target.col,
            value,
        });
        cx.notify();
    }
}

/// Pretty-print on open when the value parses, otherwise show it untouched so
/// a malformed value is still readable and fixable.
fn initial_text(value: &str, format: ValueFormat) -> String {
    format_value(value, format).unwrap_or_else(|_| value.to_string())
}

/// What the editor opens with: the hex dump of a binary cell, otherwise
/// the text as `initial_text` presents it.
fn stored_text(target: &ValuePanelTarget, format: ValueFormat) -> String {
    match &target.bytes {
        Some(cell) => cell.hex_dump(),
        None => initial_text(&target.value, format),
    }
}

fn build_input(
    text: String,
    format: ValueFormat,
    word_wrap: bool,
    window: &mut Window,
    cx: &mut Context<ValuePanelContent>,
) -> (Entity<EditorState>, Subscription) {
    // The editor language is fixed at construction in `gpui-component`, so a
    // format switch replaces the whole input rather than mutating it.
    let input = cx.new(|cx| {
        EditorState::new(window, cx)
            .language(format.editor_language())
            .line_number(true)
            .soft_wrap(word_wrap)
    });

    input.update(cx, |state, cx| {
        state.set_value(text, window, cx);
    });

    let subscription = cx.subscribe(&input, |this, _input, event: &InputEvent, cx| match event {
        InputEvent::Focus => {
            this.editor_focused = true;
            cx.notify();
        }
        InputEvent::Blur => {
            this.editor_focused = false;
            cx.notify();
        }
        // A keystroke can change what Save and Revert are allowed to do.
        InputEvent::Change => cx.notify(),
        _ => {}
    });

    (input, subscription)
}

impl Render for ValuePanelContent {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Cloned rather than borrowed: `cx.listener` below needs `cx` mutably.
        let theme = cx.theme().clone();
        let editable = self.target.editable;
        let header = self.render_header(cx).into_any_element();
        let notice = self.render_notice(cx).map(IntoElement::into_any_element);
        // The picture replaces the editor and its footer. The editor entity
        // is kept, so switching back finds the text as it was.
        let (body, footer) = if self.shows_image() {
            (
                self.render_image_body(cx),
                Some(self.render_image_footer(cx).into_any_element()),
            )
        } else {
            (self.render_editor(cx), None)
        };
        let on_editor = footer.is_none();

        div()
            .id("value-panel-content")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .children(notice)
            .child(body)
            .when(on_editor, |d| d.children(self.vim.render_indicator(cx)))
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .px(Spacing::SM)
                        .py(Spacing::XS)
                        .flex()
                        .items_center()
                        .gap(Spacing::XS)
                        .bg(theme.danger.opacity(0.1))
                        .border_t_1()
                        .border_color(theme.danger.opacity(0.3))
                        .child(
                            Icon::new(AppIcon::CircleAlert)
                                .size(Heights::ICON_SM)
                                .danger(),
                        )
                        .child(Text::caption(error).font_size(FontSizes::XS).danger()),
                )
            })
            .children(footer)
            .when(on_editor && editable, |d| d.child(self.render_footer(cx)))
            .when(on_editor && !editable, |d| {
                d.child(
                    div()
                        .px(Spacing::SM)
                        .py(Spacing::XS)
                        .border_t_1()
                        .border_color(theme.border)
                        .child(
                            Text::caption(dbflux_i18n::t!("components.value_panel.read_only"))
                                .font_size(FontSizes::XS)
                                .color(theme.muted_foreground),
                        ),
                )
            })
    }
}

impl ValuePanelContent {
    fn render_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        VimBinding::capture_run_command(
            VimBinding::wire(self.vim.leader_scope(div(), cx), self.vim.input_id(), cx),
            self.vim.input_id(),
            cx,
        )
        .id("value-panel-editor")
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .child(self.vim.editor(self.target.bytes.is_some()).h_full())
        .into_any_element()
    }

    /// Column name, the Image / Hex switch of an image, and on the editor
    /// the format selector (not for a hex dump) and the word-wrap toggle.
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let current = self.format;
        let word_wrap = self.word_wrap;
        let on_editor = !self.shows_image();
        let formats = if on_editor && self.target.bytes.is_none() {
            ValueFormat::ALL.to_vec()
        } else {
            Vec::new()
        };
        let view_chips = self.render_view_chips(cx);

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(Spacing::XS)
            .px(Spacing::SM)
            .py(Spacing::XS)
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.secondary.opacity(0.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(Text::body_sm(self.target.column_name.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(view_chips)
                    .children(formats.into_iter().map(|format| {
                        let is_active = format == current;
                        div()
                            .id(SharedString::from(format!(
                                "value-panel-format-{}",
                                format.editor_language()
                            )))
                            .px(Spacing::XS)
                            .rounded(Radii::SM)
                            .cursor_pointer()
                            .text_size(FontSizes::XS)
                            .when(is_active, |d| d.bg(theme.accent.opacity(0.15)))
                            .when(!is_active, |d| d.hover(|d| d.bg(theme.secondary)))
                            .child(Text::body(format.label()).font_size(FontSizes::XS).color(
                                if is_active {
                                    theme.foreground
                                } else {
                                    theme.muted_foreground
                                },
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_format(format, window, cx);
                            }))
                    }))
                    .when(on_editor, |d| {
                        d.child(
                            div()
                                .id("value-panel-wrap")
                                .px(Spacing::XS)
                                .rounded(Radii::SM)
                                .cursor_pointer()
                                .text_size(FontSizes::XS)
                                .when(word_wrap, |d| d.bg(theme.accent.opacity(0.15)))
                                .when(!word_wrap, |d| d.hover(|d| d.bg(theme.secondary)))
                                .child(
                                    Text::body(dbflux_i18n::t!("components.value_panel.wrap"))
                                        .font_size(FontSizes::XS)
                                        .color(if word_wrap {
                                            theme.foreground
                                        } else {
                                            theme.muted_foreground
                                        }),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_word_wrap(window, cx);
                                })),
                        )
                    }),
            )
    }

    /// Format / Compact on the left, Revert / Save on the right — the same
    /// arrangement as the cell-editor modal.
    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let structured = self.format.is_structured();
        // Save and Revert only mean something while the editor differs from
        // what is stored, and `save` resets that baseline — so both go quiet
        // again the moment the value is committed.
        let is_modified = self.is_modified(cx);

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(Spacing::XS)
            .px(Spacing::SM)
            .py(Spacing::XS)
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(Spacing::XS)
                    .when(structured, |d| {
                        d.child(
                            Button::new(
                                "value-panel-format",
                                dbflux_i18n::t!("components.json_editor.format"),
                            )
                            .variant(ButtonVariant::Ghost)
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.apply_transform(format_value, window, cx);
                                },
                            )),
                        )
                        .child(
                            Button::new(
                                "value-panel-compact",
                                dbflux_i18n::t!("components.json_editor.compact"),
                            )
                            .variant(ButtonVariant::Ghost)
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.apply_transform(compact_value, window, cx);
                                },
                            )),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(Spacing::XS)
                    .child(
                        Button::new(
                            "value-panel-revert",
                            dbflux_i18n::t!("components.value_panel.revert"),
                        )
                        .variant(ButtonVariant::Ghost)
                        .disabled(!is_modified)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.revert(window, cx);
                        })),
                    )
                    .child(
                        Button::new(
                            "value-panel-save",
                            dbflux_i18n::t!("components.json_editor.save"),
                        )
                        .variant(if is_modified {
                            ButtonVariant::Primary
                        } else {
                            ButtonVariant::Ghost
                        })
                        .disabled(!is_modified)
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.save(cx);
                        })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests;
