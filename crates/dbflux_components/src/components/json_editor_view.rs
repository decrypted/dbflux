use crate::controls::Button;
use crate::icons::AppIcon;
use crate::primitives::{Chamfer, Icon};
use crate::tokens::{ChamferCut, ChromeColors, Fields, ModalMetrics};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::{Editor, EditorState};

use super::value_format::{ValueFormat, validate_value};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Pretty-print a JSON string with two-space indentation. Returns `None` if
/// the input is not valid JSON.
///
/// Only whitespace between tokens changes: keys keep their order and every
/// string and number stays byte for byte as written, so a decimal such as
/// `2312.64496440` or an escape such as `\u00e9` survives the round trip.
pub fn format_json(s: &str) -> Option<String> {
    relayout_json(s, Some("  "))
}

/// Compact a JSON string to a single line. Returns `None` if the input is not
/// valid JSON. Like [`format_json`], only the whitespace between tokens is
/// touched.
pub fn compact_json(s: &str) -> Option<String> {
    relayout_json(s, None)
}

/// Re-lays out valid JSON: with `indent`, one value per line in the layout
/// `serde_json` pretty-prints; without, no whitespace at all. The text is
/// checked by the parser first, so the scan below only has to tell strings,
/// punctuation and scalar tokens apart.
fn relayout_json(s: &str, indent: Option<&str>) -> Option<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(s).ok()?;

    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 4);
    let mut depth = 0usize;
    let mut index = 0usize;

    let newline = |out: &mut String, depth: usize| {
        if let Some(indent) = indent {
            out.push('\n');
            for _ in 0..depth {
                out.push_str(indent);
            }
        }
    };

    while let Some(&byte) = bytes.get(index) {
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => index += 1,
            b'"' => {
                let end = string_end(bytes, index);
                out.push_str(s.get(index..end)?);
                index = end;
            }
            b'{' | b'[' => {
                let close = if byte == b'{' { b'}' } else { b']' };
                let next = skip_whitespace(bytes, index + 1);
                out.push(char::from(byte));

                if bytes.get(next) == Some(&close) {
                    out.push(char::from(close));
                    index = next + 1;
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                    index += 1;
                }
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(char::from(byte));
                index += 1;
            }
            b',' => {
                out.push(',');
                newline(&mut out, depth);
                index += 1;
            }
            b':' => {
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                index += 1;
            }
            _ => {
                let start = index;
                while bytes.get(index).is_some_and(|byte| {
                    !matches!(
                        byte,
                        b' ' | b'\t'
                            | b'\n'
                            | b'\r'
                            | b','
                            | b':'
                            | b'{'
                            | b'}'
                            | b'['
                            | b']'
                            | b'"'
                    )
                }) {
                    index += 1;
                }
                out.push_str(s.get(start..index)?);
            }
        }
    }

    Some(out)
}

/// The index just past the closing quote of the string starting at `start`.
/// Quotes and backslashes are ASCII, so a byte scan never splits a character.
fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 1;

    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }

    bytes.len()
}

fn skip_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
    {
        index += 1;
    }

    index
}

/// Validate a JSON string. When `allow_empty` is true, an empty string is accepted.
pub fn validate_json(s: &str, allow_empty: bool) -> Result<(), String> {
    if s.is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(dbflux_i18n::t!("components.json_editor.empty_error"))
        };
    }

    serde_json::from_str::<serde_json::Value>(s)
        .map(|_| ())
        .map_err(|e| e.to_string().replace('\n', " "))
}

/// Label of the save shortcut bound to [`crate::actions::SaveEdit`] inside the
/// cell editor and the document preview.
pub const SAVE_SHORTCUT_LABEL: &str = if cfg!(target_os = "macos") {
    "Cmd S"
} else {
    "Ctrl S"
};

/// Status line under a JSON editor: green "valid JSON · N lines" while the
/// text parses, the parse error in red otherwise.
#[cfg(test)]
fn json_status(value: &str, error: Option<String>) -> Result<String, String> {
    structured_status(value, ValueFormat::Json, error)
}

/// The status line for text read as `format` (JSON or XML).
fn structured_status(
    value: &str,
    format: ValueFormat,
    error: Option<String>,
) -> Result<String, String> {
    if let Some(error) = error {
        return Err(error);
    }

    validate_value(value, format)?;

    let lines = value.lines().count().max(1);

    Ok(match (format, lines) {
        (ValueFormat::Xml, 1) => {
            dbflux_i18n::t!("components.json_editor.valid_xml.one", count = lines)
        }
        (ValueFormat::Xml, _) => {
            dbflux_i18n::t!("components.json_editor.valid_xml.many", count = lines)
        }
        (_, 1) => dbflux_i18n::t!("components.json_editor.valid.one", count = lines),
        _ => dbflux_i18n::t!("components.json_editor.valid.many", count = lines),
    })
}

/// Renders a JSON/text editor with its status line and footer (P1Flows,
/// "Edit properties"): the editor on the ground in a cut-8 frame, a status
/// line for JSON and XML, and a footer with Copy (when the owner offers it),
/// Format and Compact (structured text only), Cancel and Save.
pub struct JsonEditorView {
    id_prefix: &'static str,
    input: Entity<EditorState>,
    validation_error: Option<String>,
    /// Shown in the status line instead of the parse state, in the warning
    /// color: the text can still be saved, but the owner wants it read first.
    warning: Option<String>,
    /// What the status line checks the text as.
    format: ValueFormat,
    show_format_buttons: bool,
    on_copy: Option<ClickHandler>,
    on_copy_compact: Option<ClickHandler>,
    min_editor_height: Pixels,
    readonly: bool,
    below_editor: Option<AnyElement>,
    on_format: Option<ClickHandler>,
    on_compact: Option<ClickHandler>,
    on_save: ClickHandler,
    on_cancel: ClickHandler,
}

impl JsonEditorView {
    pub fn new(
        id_prefix: &'static str,
        input: &Entity<EditorState>,
        on_save: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id_prefix,
            input: input.clone(),
            validation_error: None,
            warning: None,
            format: ValueFormat::Json,
            show_format_buttons: false,
            on_copy: None,
            on_copy_compact: None,
            min_editor_height: px(300.0),
            readonly: false,
            below_editor: None,
            on_format: None,
            on_compact: None,
            on_save: Box::new(on_save),
            on_cancel: Box::new(on_cancel),
        }
    }

    pub fn validation_error(mut self, error: Option<String>) -> Self {
        self.validation_error = error;
        self
    }

    pub fn warning(mut self, warning: Option<String>) -> Self {
        self.warning = warning;
        self
    }

    /// The format the status line checks the text as (default: JSON).
    pub fn format(mut self, format: ValueFormat) -> Self {
        self.format = format;
        self
    }

    /// Adds Copy to the footer, and Copy compact when `on_copy_compact` is
    /// given, left of Format and Compact.
    pub fn copy_buttons(
        mut self,
        on_copy: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_copy_compact: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    ) -> Self {
        self.on_copy = Some(Box::new(on_copy));
        self.on_copy_compact = on_copy_compact;
        self
    }

    pub fn show_format_buttons(
        mut self,
        on_format: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_compact: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.show_format_buttons = true;
        self.on_format = Some(Box::new(on_format));
        self.on_compact = Some(Box::new(on_compact));
        self
    }

    pub fn min_editor_height(mut self, height: Pixels) -> Self {
        self.min_editor_height = height;
        self
    }

    /// Rejects user text changes. The editor element applies this to its
    /// state every frame, so an owner that locks the input (Vim's Normal
    /// mode) passes the lock here.
    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    /// A row drawn right under the editor frame, such as the Vim mode
    /// indicator.
    pub fn below_editor(mut self, element: Option<impl IntoElement>) -> Self {
        self.below_editor = element.map(IntoElement::into_any_element);
        self
    }

    pub fn render(self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let prefix = self.id_prefix;

        let format = self.format;
        let warning = self.warning;
        let status = self.show_format_buttons.then(|| {
            let value = self.input.read(cx).value().to_string();

            let (icon, color, text) = match (
                warning,
                structured_status(&value, format, self.validation_error.clone()),
            ) {
                (Some(warning), _) => (AppIcon::TriangleAlert, theme.warning, warning),
                (None, Ok(text)) => (AppIcon::CircleCheck, theme.success, text),
                (None, Err(error)) => (AppIcon::CircleAlert, theme.danger, error),
            };

            div()
                .flex()
                .items_center()
                .gap(ModalMetrics::FIELD_GAP)
                .text_size(ModalMetrics::FIELD_LABEL_FONT)
                .text_color(color)
                .child(Icon::new(icon).size(Fields::CHEVRON).color(color))
                .child(div().min_w_0().truncate().child(text))
        });

        let error_line = (!self.show_format_buttons)
            .then_some(self.validation_error)
            .flatten()
            .map(|error| {
                div()
                    .text_size(ModalMetrics::FIELD_LABEL_FONT)
                    .text_color(theme.danger)
                    .child(error)
            });

        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(ModalMetrics::BODY_GAP)
            .p(ModalMetrics::PADDING)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(self.min_editor_height)
                    .p(ModalMetrics::CODE_PADDING_X)
                    .overflow_hidden()
                    .child(
                        Chamfer::new(ChamferCut::INPUT)
                            .fill(theme.background)
                            .border(theme.border),
                    )
                    .child(
                        Editor::new(&self.input)
                            .readonly(self.readonly)
                            .w_full()
                            .h_full()
                            .font_family(crate::fonts::editor_family(cx))
                            .text_size(ModalMetrics::CODE_FONT)
                            .text_color(ChromeColors::strong(theme)),
                    ),
            )
            .children(self.below_editor)
            .children(status)
            .children(error_line);

        let has_left_group = self.on_copy.is_some();
        let footer = div()
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .justify_end()
            .gap(ModalMetrics::FOOTER_GAP)
            .px(ModalMetrics::PADDING)
            .py(ModalMetrics::FOOTER_PADDING_Y)
            .border_t_1()
            .border_color(theme.border)
            .when_some(self.on_copy, |footer, on_copy| {
                footer.child(
                    Button::new(
                        SharedString::from(format!("{prefix}-copy")),
                        dbflux_i18n::t!("components.json_editor.copy"),
                    )
                    .icon(AppIcon::Copy)
                    .on_click(on_copy),
                )
            })
            .when_some(self.on_copy_compact, |footer, on_copy_compact| {
                footer.child(
                    Button::new(
                        SharedString::from(format!("{prefix}-copy-compact")),
                        dbflux_i18n::t!("components.json_editor.copy_compact"),
                    )
                    .on_click(on_copy_compact),
                )
            })
            .when_some(self.on_format, |footer, on_format| {
                footer.child(
                    Button::new(
                        SharedString::from(format!("{prefix}-format")),
                        dbflux_i18n::t!("components.json_editor.format"),
                    )
                    .icon(AppIcon::Zap)
                    .on_click(on_format),
                )
            })
            .when_some(self.on_compact, |footer, on_compact| {
                footer.child(
                    Button::new(
                        SharedString::from(format!("{prefix}-compact")),
                        dbflux_i18n::t!("components.json_editor.compact"),
                    )
                    .on_click(on_compact),
                )
            })
            // Copy, Format and Compact sit on the left, Cancel and Save on
            // the right.
            .when(has_left_group, |footer| footer.child(div().flex_1()))
            .child(
                Button::new(
                    SharedString::from(format!("{prefix}-cancel")),
                    dbflux_i18n::t!("components.json_editor.cancel"),
                )
                .on_click(self.on_cancel),
            )
            .child(
                Button::new(
                    SharedString::from(format!("{prefix}-save")),
                    dbflux_i18n::t!("components.json_editor.save"),
                )
                .primary()
                .icon(AppIcon::Save)
                .kbd(SAVE_SHORTCUT_LABEL)
                .on_click(self.on_save),
            );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(body)
            .child(footer)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{json_status, validate_json};

    #[test]
    fn status_counts_the_lines_of_valid_json() {
        assert_eq!(
            json_status("{\n  \"a\": 1\n}", None),
            Ok(dbflux_i18n::t!(
                "components.json_editor.valid.many",
                count = 3
            ))
        );
        assert!(json_status("{", None).is_err());
        assert_eq!(
            json_status("{}", Some("bad".to_string())),
            Err("bad".to_string())
        );
    }

    #[test]
    fn validate_json_empty_error_matches_translated_catalog() {
        let error = validate_json("", false).unwrap_err();
        assert_eq!(
            error,
            dbflux_i18n::t!("components.json_editor.empty_error", locale = "en")
        );
    }

    #[test]
    fn validate_json_empty_error_diverges_between_locales() {
        let en = dbflux_i18n::t!("components.json_editor.empty_error", locale = "en");
        let es = dbflux_i18n::t!("components.json_editor.empty_error", locale = "es");
        assert_ne!(en, es);
    }

    #[test]
    fn json_editor_keys_resolve_in_both_locales() {
        let keys = [
            "components.json_editor.empty_error",
            "components.json_editor.format",
            "components.json_editor.compact",
            "components.json_editor.cancel",
            "components.json_editor.save",
            "components.json_editor.valid.one",
            "components.json_editor.valid.many",
        ];

        for key in keys {
            let en = dbflux_i18n::t!(key, locale = "en");
            let es = dbflux_i18n::t!(key, locale = "es");
            assert!(!en.is_empty() && en != key, "en missing for {key}");
            assert!(!es.is_empty() && es != key, "es missing for {key}");
        }
    }
}
