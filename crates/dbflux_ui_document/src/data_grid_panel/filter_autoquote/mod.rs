//! Quotes for the value after a comparison in the WHERE field (opt-in).
//!
//! Typing `method =` on a text or date/time column leaves `method = '|'`,
//! the caret between the quotes. The field sees only text changes, so the
//! typed character is recovered by comparing the text before and after one
//! change: a single inserted character is typing, anything larger (a paste,
//! a completion, an undo) never triggers.
//!
//! The comparisons that trigger, and the character that completes them:
//!
//! | Operator | Fires on |
//! |----------|----------|
//! | `=`, `!=`, `<=`, `>=` | the `=` |
//! | `<>` | the `>` |
//! | `<`, `>` | the space typed after them (`<` alone may still become `<=`) |
//! | `LIKE`, `NOT LIKE`, `ILIKE`, `NOT ILIKE` | the space typed after the keyword |
//!
//! `IS NULL`, `IN (...)` and `BETWEEN` never get quotes.
//!
//! The field's editor pairs quotes itself: a `'` typed in front of a closing
//! quote steps over it without changing the text. That is what closes the
//! literal after its value, and it is also what a `'` typed out of habit
//! right after the operator does to the still empty pair, so the next
//! character typed after an empty pair is moved inside it.

#[cfg(test)]
mod tests;

use super::DataGridPanel;
use crate::completion_support::normalize_identifier;
use dbflux_core::{ColumnKind, QueryLanguage};
use gpui::{Context, Window};
use std::ops::Range;

/// Whether a column's values are written as SQL string literals.
pub(super) fn quotes_values(kind: ColumnKind, type_name: &str) -> Option<bool> {
    let type_name = type_name.to_ascii_lowercase();
    let has = |part: &str| type_name.contains(part);

    // Checked before the kind: MySQL's YEAR is a number, and a driver may
    // report json as text although `col = '...'` would compare it as text.
    if has("json") || has("bson") || has("bytea") || has("blob") || has("binary") {
        return None;
    }
    if has("bool") || has("bit") || type_name == "year" {
        return Some(false);
    }
    // Before the numeric names, which `int` of `interval` would match.
    if has("interval") {
        return Some(true);
    }

    match kind {
        ColumnKind::Text | ColumnKind::Timestamp => return Some(true),
        ColumnKind::Integer | ColumnKind::Float => return Some(false),
        _ => {}
    }

    let numeric = [
        "int", "serial", "float", "double", "real", "numeric", "decimal", "number", "money",
    ];
    if numeric.iter().any(|part| has(part)) {
        return Some(false);
    }

    let quoted = [
        "char", "text", "string", "clob", "uuid", "enum", "date", "time", "interval", "inet",
        "cidr", "macaddr", "xml", "name",
    ];
    quoted.iter().any(|part| has(part)).then_some(true)
}

/// A comparison just completed by the typed character, and the column it
/// compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Comparison {
    /// The column as written, without identifier quotes, dotted path kept.
    pub column: String,
    /// What to insert at the caret: the quotes, after a space unless the
    /// typed character was one.
    pub insert: &'static str,
}

/// The comparison completed when `typed` was inserted just before `cursor`,
/// or `None` when this character completes none, or completes one inside a
/// string literal or an `IN (...)` list.
pub(super) fn completed_comparison(text: &str, cursor: usize, typed: char) -> Option<Comparison> {
    let before = text.get(..cursor)?;
    let without_typed = before.strip_suffix(typed)?;

    let (operator_start, insert) = match typed {
        '=' => {
            let start = match without_typed.chars().last() {
                Some('!' | '<' | '>') => without_typed.len() - 1,
                Some('=') => return None,
                _ => without_typed.len(),
            };
            (start, " ''")
        }
        '>' if without_typed.ends_with('<') => (without_typed.len() - 1, " ''"),
        ' ' => (space_completed_operator(without_typed)?, "''"),
        _ => return None,
    };

    let head = before.get(..operator_start)?;
    if inside_literal_or_in_list(head) || !ends_condition(text.get(cursor..)?) {
        return None;
    }

    let column = column_before(head.trim_end())?;
    Some(Comparison { column, insert })
}

/// Whether the text after the caret leaves room for a value: nothing, a
/// closing parenthesis, or the next `AND` / `OR`. A comparison typed in front
/// of an existing value must not get a second one.
fn ends_condition(after: &str) -> bool {
    let after = after.trim_start();
    let word: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();

    after.is_empty()
        || after.starts_with(')')
        || word.eq_ignore_ascii_case("and")
        || word.eq_ignore_ascii_case("or")
}

/// Start of the operator a space completes: a lone `<` / `>`, or `LIKE` /
/// `ILIKE` with an optional `NOT` before it.
fn space_completed_operator(text: &str) -> Option<usize> {
    if text.ends_with(char::is_whitespace) {
        return None;
    }

    if let Some(head) = text.strip_suffix(['<', '>']) {
        return (!head.ends_with(['<', '>', '=', '!'])).then_some(head.len());
    }

    let word_start = text
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(0, |index| index + 1);
    let word = text.get(word_start..)?;
    if !word.eq_ignore_ascii_case("like") && !word.eq_ignore_ascii_case("ilike") {
        return None;
    }

    let head = text.get(..word_start)?.trim_end();
    let not_start = head.len().saturating_sub(3);
    match head.get(not_start..) {
        Some(not)
            if not.eq_ignore_ascii_case("not")
                && head
                    .get(..not_start)
                    .is_some_and(|rest| rest.is_empty() || rest.ends_with(char::is_whitespace)) =>
        {
            Some(not_start)
        }
        _ => Some(word_start),
    }
}

/// The identifier that ends `text`: bare (`method`, `user.email`) or quoted
/// with `"`, `` ` `` or `[]`. Quotes are dropped.
fn column_before(text: &str) -> Option<String> {
    let closing = text.chars().last()?;
    let opening = match closing {
        '"' => '"',
        '`' => '`',
        ']' => '[',
        _ => {
            let start = text
                .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$' || c == '.'))
                .map_or(0, |index| index + 1);
            let column = text.get(start..)?.trim_matches('.');
            let starts_like_identifier = column
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_');
            return (starts_like_identifier && !is_keyword(column)).then(|| column.to_string());
        }
    };

    let inner = text.get(..text.len() - closing.len_utf8())?;
    let start = inner.rfind(opening)?;
    let name = inner.get(start + opening.len_utf8()..)?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Words that end a condition without naming a column (`... AND =`).
fn is_keyword(word: &str) -> bool {
    ["and", "or", "not", "where", "null", "true", "false"]
        .iter()
        .any(|keyword| word.eq_ignore_ascii_case(keyword))
}

/// Whether the end of `text` sits inside a quoted literal or identifier,
/// or inside the parentheses of an `IN`.
fn inside_literal_or_in_list(text: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut parens: Vec<bool> = Vec::new();
    let mut previous_word = String::new();
    let mut word = String::new();

    for c in text.chars() {
        if let Some(open) = quote {
            // A doubled quote is an escaped one: it closes and reopens.
            if c == open {
                quote = None;
            }
            continue;
        }

        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            previous_word = std::mem::take(&mut word);
        }

        match c {
            '\'' | '"' | '`' => quote = Some(c),
            '(' => parens.push(previous_word.eq_ignore_ascii_case("in")),
            ')' => {
                parens.pop();
            }
            _ => {}
        }
        if !c.is_whitespace() {
            previous_word.clear();
        }
    }

    quote.is_some() || parens.iter().any(|in_list| *in_list)
}

/// One change of the field's text, as a single character typed or deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Keystroke {
    Typed { at: usize, ch: char },
    Deleted { at: usize, ch: char },
}

/// The single character `new` gained or lost against `old` at `cursor`.
pub(super) fn keystroke(old: &str, new: &str, cursor: usize) -> Option<Keystroke> {
    if new.len() > old.len() {
        let ch = new.get(..cursor)?.chars().last()?;
        let at = cursor - ch.len_utf8();
        let rebuilt = format!("{}{}", new.get(..at)?, new.get(cursor..)?);
        (rebuilt == old).then_some(Keystroke::Typed { at, ch })
    } else if new.len() < old.len() {
        let ch = old.get(cursor..)?.chars().next()?;
        let expected = format!(
            "{}{}",
            old.get(..cursor)?,
            old.get(cursor + ch.len_utf8()..)?
        );
        (new == expected).then_some(Keystroke::Deleted { at: cursor, ch })
    } else {
        None
    }
}

/// The quotes this feature inserted and the caret has not left yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AutoQuotedPair {
    /// Byte offset of the opening quote.
    pub open: usize,
    /// Byte offset of the closing quote.
    pub close: usize,
    /// Whether a space was inserted before the opening quote.
    pub spaced: bool,
}

/// What the field should do about one keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AutoQuoteEdit {
    /// Insert `text` at the caret and leave the caret before its last
    /// character, between the quotes.
    InsertPair(&'static str),
    /// Replace `range` with `text` and put the caret at `caret`.
    Replace {
        range: Range<usize>,
        text: String,
        caret: usize,
    },
}

/// Per-field state: the text as last seen, the live pair, and whether the
/// next change is a paste.
#[derive(Debug, Default)]
pub(super) struct AutoQuoteState {
    pub text: String,
    pub pair: Option<AutoQuotedPair>,
    pub pasting: bool,
}

impl AutoQuoteState {
    /// Decides the edit for the change from `self.text` to `text`, with the
    /// caret now at `cursor`, and updates the pair. `column_quoted` answers
    /// whether a column's values take quotes.
    pub(super) fn on_change(
        &mut self,
        text: &str,
        cursor: usize,
        column_quoted: impl Fn(&str) -> bool,
    ) -> Option<AutoQuoteEdit> {
        let old = std::mem::replace(&mut self.text, text.to_string());
        // The change event of an edit this state made itself, or one that
        // only moved the caret.
        if old == text {
            return None;
        }
        if std::mem::take(&mut self.pasting) {
            self.pair = None;
            return None;
        }

        match keystroke(&old, text, cursor) {
            Some(Keystroke::Typed { at, ch }) => self.on_typed(text, at, ch, cursor, column_quoted),
            Some(Keystroke::Deleted { at, ch }) => self.on_deleted(at, ch),
            None => {
                self.pair = None;
                None
            }
        }
    }

    fn on_typed(
        &mut self,
        text: &str,
        at: usize,
        ch: char,
        cursor: usize,
        column_quoted: impl Fn(&str) -> bool,
    ) -> Option<AutoQuoteEdit> {
        if let Some(pair) = self.pair.as_mut() {
            let len = ch.len_utf8();
            let empty = pair.close == pair.open + 1;

            // The space typed out of habit after the operator.
            if at == pair.close && ch == ' ' && empty {
                return Some(AutoQuoteEdit::Replace {
                    range: at..at + len,
                    text: String::new(),
                    caret: at,
                });
            }

            // A quote typed in front of the closing one steps over it. The
            // editor's own quote pairing usually does this before the text
            // changes; this covers an editor that inserted the quote.
            if at == pair.close && ch == '\'' {
                if !empty {
                    self.pair = None;
                }
                return Some(AutoQuoteEdit::Replace {
                    range: at..at + len,
                    text: String::new(),
                    caret: at + 1,
                });
            }

            // A quote typed out of habit after the operator stepped over the
            // still empty pair; the value typed next belongs inside it.
            if empty && at == pair.close + 1 && !matches!(ch, ' ' | ')' | '\'') {
                pair.close += len;
                return Some(AutoQuoteEdit::Replace {
                    range: at - 1..at + len,
                    text: format!("{ch}'"),
                    caret: at - 1 + len,
                });
            }

            if at > pair.open && at <= pair.close {
                pair.close += len;
                return None;
            }
            self.pair = None;
        }

        let comparison = completed_comparison(text, cursor, ch)?;
        column_quoted(&comparison.column).then_some(AutoQuoteEdit::InsertPair(comparison.insert))
    }

    fn on_deleted(&mut self, at: usize, ch: char) -> Option<AutoQuoteEdit> {
        let pair = self.pair.take()?;

        // Backspace over the opening quote of a still empty pair takes the
        // closing quote, now at `at`, and the space inserted with them.
        if ch == '\'' && at == pair.open && pair.close == pair.open + 1 {
            let start = if pair.spaced {
                at.saturating_sub(1)
            } else {
                at
            };
            return Some(AutoQuoteEdit::Replace {
                range: start..at + 1,
                text: String::new(),
                caret: start,
            });
        }

        if at > pair.open && at < pair.close {
            self.pair = Some(AutoQuotedPair {
                close: pair.close - ch.len_utf8(),
                ..pair
            });
        }
        None
    }

    /// Records the pair just inserted at `cursor`, and the text that holds it.
    pub(super) fn inserted(&mut self, cursor: usize, insert: &str, text: String) {
        let close = cursor + insert.len() - 1;
        self.pair = Some(AutoQuotedPair {
            open: close - 1,
            close,
            spaced: insert.starts_with(' '),
        });
        self.text = text;
    }

    /// Records the text after a replacement this feature made.
    pub(super) fn replaced(&mut self, text: String) {
        self.text = text;
    }
}

impl DataGridPanel {
    /// Whether the WHERE field of this grid inserts quotes: the setting is
    /// on and the source is a table of a SQL connection.
    fn autoquote_applies(&self, cx: &gpui::App) -> bool {
        let super::DataSource::Table { profile_id, .. } = &self.source else {
            return false;
        };

        let state = self.app_state.read(cx);
        state.general_settings().grid_filter_autoquote
            && state
                .connections()
                .get(profile_id)
                .is_some_and(|connected| {
                    connected.connection.metadata().query_language == QueryLanguage::Sql
                })
    }

    /// Whether `column` of this table holds values written as strings, from
    /// the metadata the WHERE completion uses, or the shown result's
    /// columns. A one-hop dotted path looks in the referenced table.
    fn column_takes_quotes(&self, column: &str) -> bool {
        let wanted = normalize_identifier(column);

        if let Some(cache) = &self.filter_bar.filter_completion_cache {
            let cache = cache.borrow();
            let found = match wanted.split_once('.') {
                None => cache
                    .source_columns
                    .iter()
                    .find(|info| normalize_identifier(&info.name) == wanted),
                Some((link, target)) => cache.fk_links.get(link).and_then(|fk| {
                    let key = (
                        fk.referenced_schema.as_deref().map(normalize_identifier),
                        normalize_identifier(&fk.referenced_table),
                    );
                    cache.joined_columns.get(&key).and_then(|columns| {
                        columns
                            .iter()
                            .find(|info| normalize_identifier(&info.name) == target)
                    })
                }),
            };
            if let Some(info) = found {
                return quotes_values(ColumnKind::Unknown, &info.type_name).unwrap_or(false);
            }
        }

        self.result
            .columns
            .iter()
            .find(|meta| normalize_identifier(&meta.name) == wanted)
            .and_then(|meta| quotes_values(meta.kind, &meta.type_name))
            .unwrap_or(false)
    }

    /// Runs after every change of the WHERE field.
    pub(super) fn filter_autoquote_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.filter_bar.filter_input.read(cx);
        let text = input.value().to_string();
        let cursor = input.cursor();

        if !self.autoquote_applies(cx) {
            self.filter_bar.autoquote.text = text;
            self.filter_bar.autoquote.pair = None;
            return;
        }

        let mut state = std::mem::take(&mut self.filter_bar.autoquote);
        let edit = state.on_change(&text, cursor, |column| self.column_takes_quotes(column));
        self.filter_bar.autoquote = state;

        let Some(edit) = edit else {
            return;
        };

        match edit {
            AutoQuoteEdit::InsertPair(insert) => {
                let after = self.filter_bar.filter_input.update(cx, |input, cx| {
                    input.insert(insert, window, cx);
                    let caret = cursor + insert.len() - 1;
                    input.set_selected_range(caret..caret, cx);
                    input.value().to_string()
                });
                self.filter_bar.autoquote.inserted(cursor, insert, after);
            }
            AutoQuoteEdit::Replace { range, text, caret } => {
                let after = self.filter_bar.filter_input.update(cx, |input, cx| {
                    input.set_selected_range(range.clone(), cx);
                    input.replace(text, window, cx);
                    input.set_selected_range(caret..caret, cx);
                    input.value().to_string()
                });
                self.filter_bar.autoquote.replaced(after);
            }
        }
    }
}
