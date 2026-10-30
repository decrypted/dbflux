use super::{
    AutoQuoteEdit, AutoQuoteState, Comparison, Keystroke, completed_comparison, keystroke,
    quotes_values,
};
use dbflux_core::ColumnKind;

fn comparison(text: &str, typed: char) -> Option<Comparison> {
    completed_comparison(text, text.len(), typed)
}

fn column(text: &str, typed: char) -> Option<String> {
    comparison(text, typed).map(|comparison| comparison.column)
}

#[test]
fn each_operator_fires_on_the_character_that_completes_it() {
    assert_eq!(column("method =", '='), Some("method".into()));
    assert_eq!(column("method !=", '='), Some("method".into()));
    assert_eq!(column("method<>", '>'), Some("method".into()));
    assert_eq!(column("created_at >=", '='), Some("created_at".into()));
    assert_eq!(column("created_at <=", '='), Some("created_at".into()));
    assert_eq!(column("created_at > ", ' '), Some("created_at".into()));
    assert_eq!(column("name like ", ' '), Some("name".into()));
    assert_eq!(column("name NOT LIKE ", ' '), Some("name".into()));
    assert_eq!(column("name ILIKE ", ' '), Some("name".into()));
    assert_eq!(
        column("status = 1 AND method =", '='),
        Some("method".into())
    );
    assert_eq!(column("\"Method Name\" =", '='), Some("Method Name".into()));
    assert_eq!(
        column("created_by.email =", '='),
        Some("created_by.email".into())
    );

    assert_eq!(comparison("method =", '=').map(|c| c.insert), Some(" ''"));
    assert_eq!(comparison("name LIKE ", ' ').map(|c| c.insert), Some("''"));
}

#[test]
fn a_prefix_of_a_longer_operator_or_no_column_does_not_fire() {
    // `<` and `>` may still become `<=`, `>=` or `<>`.
    assert_eq!(column("created_at >", '>'), None);
    assert_eq!(column("created_at <", '<'), None);
    // `<>` and `>=` already fired on their last character.
    assert_eq!(column("method <> ", ' '), None);
    assert_eq!(column("created_at >= ", ' '), None);
    assert_eq!(column("name LIKE  ", ' '), None);
    assert_eq!(column("unlike ", ' '), None);
    assert_eq!(column("lower(name) =", '='), None);
    assert_eq!(column("1 =", '='), None);
    assert_eq!(column("=", '='), None);
    assert_eq!(column("a ==", '='), None);
}

#[test]
fn inside_a_literal_or_an_in_list_nothing_fires() {
    assert_eq!(column("note = 'a =", '='), None);
    assert_eq!(column("note = 'it''s =", '='), None);
    assert_eq!(column("note = 'x' AND name =", '='), Some("name".into()));
    assert_eq!(column("id IN (SELECT id FROM t WHERE name =", '='), None);
    assert_eq!(column("id IN(a =", '='), None);
    assert_eq!(column("(status = 1) AND name =", '='), Some("name".into()));
}

#[test]
fn a_value_after_the_caret_keeps_its_place() {
    let text = "method = 'x'";
    assert_eq!(completed_comparison(text, "method =".len(), '='), None);
    let text = "method = AND id = 1";
    assert!(completed_comparison(text, "method =".len(), '=').is_some());
}

#[test]
fn strings_and_dates_take_quotes_numbers_and_booleans_do_not() {
    assert_eq!(quotes_values(ColumnKind::Text, "varchar"), Some(true));
    assert_eq!(
        quotes_values(ColumnKind::Unknown, "character varying(20)"),
        Some(true)
    );
    assert_eq!(
        quotes_values(ColumnKind::Timestamp, "timestamptz"),
        Some(true)
    );
    assert_eq!(quotes_values(ColumnKind::Unknown, "date"), Some(true));
    assert_eq!(quotes_values(ColumnKind::Unknown, "interval"), Some(true));
    assert_eq!(quotes_values(ColumnKind::Unknown, "uuid"), Some(true));
    assert_eq!(quotes_values(ColumnKind::Integer, "int4"), Some(false));
    assert_eq!(quotes_values(ColumnKind::Unknown, "INTEGER"), Some(false));
    assert_eq!(
        quotes_values(ColumnKind::Unknown, "numeric(10,2)"),
        Some(false)
    );
    assert_eq!(quotes_values(ColumnKind::Unknown, "boolean"), Some(false));
    assert_eq!(quotes_values(ColumnKind::Unknown, "year"), Some(false));
    assert_eq!(quotes_values(ColumnKind::Text, "jsonb"), None);
    assert_eq!(quotes_values(ColumnKind::Unknown, "geometry"), None);
}

#[test]
fn one_character_typed_or_deleted_is_a_keystroke() {
    assert_eq!(
        keystroke("ab", "abc", 3),
        Some(Keystroke::Typed { at: 2, ch: 'c' })
    );
    assert_eq!(
        keystroke("ac", "abc", 2),
        Some(Keystroke::Typed { at: 1, ch: 'b' })
    );
    assert_eq!(
        keystroke("abc", "ac", 1),
        Some(Keystroke::Deleted { at: 1, ch: 'b' })
    );
    assert_eq!(keystroke("a", "abc", 3), None, "a paste is no keystroke");
    assert_eq!(keystroke("abc", "xbc", 1), None);
}

/// Feeds `state` one typed character at the end, applying the edit it asks
/// for the way the field does; returns the text and caret.
fn type_char(state: &mut AutoQuoteState, text: &mut String, caret: &mut usize, ch: char) {
    text.insert(*caret, ch);
    *caret += ch.len_utf8();
    apply(state, text, caret, |_| true);
}

fn apply(
    state: &mut AutoQuoteState,
    text: &mut String,
    caret: &mut usize,
    quoted: impl Fn(&str) -> bool,
) {
    match state.on_change(text, *caret, quoted) {
        Some(AutoQuoteEdit::InsertPair(insert)) => {
            text.insert_str(*caret, insert);
            let at = *caret;
            *caret = at + insert.len() - 1;
            state.inserted(at, insert, text.clone());
        }
        Some(AutoQuoteEdit::Replace {
            range,
            text: with,
            caret: to,
        }) => {
            text.replace_range(range, &with);
            *caret = to;
            state.replaced(text.clone());
        }
        None => {}
    }
}

fn typed(chars: &str) -> (String, usize, AutoQuoteState) {
    let mut state = AutoQuoteState::default();
    let (mut text, mut caret) = (String::new(), 0);
    for ch in chars.chars() {
        type_char(&mut state, &mut text, &mut caret, ch);
    }
    (text, caret, state)
}

#[test]
fn typing_a_comparison_leaves_the_caret_between_quotes() {
    let (text, caret, _) = typed("method =");
    assert_eq!(text, "method = ''");
    assert_eq!(caret, "method = '".len());
}

#[test]
fn a_quote_typed_before_the_closing_one_steps_over_it() {
    let (text, caret, _) = typed("method =getbets'");
    assert_eq!(text, "method = 'getbets'");
    assert_eq!(caret, text.len(), "the caret is after the closing quote");

    let (text, _, _) = typed("method =getbets' AND x");
    assert_eq!(text, "method = 'getbets' AND x");
}

#[test]
fn the_habitual_space_and_quote_after_the_operator_are_absorbed() {
    let (text, caret, _) = typed("method = 'getbets'");
    assert_eq!(text, "method = 'getbets'");
    assert_eq!(caret, text.len());
}

#[test]
fn backspace_right_after_the_insertion_removes_the_pair() {
    let (mut text, mut caret, mut state) = typed("method =");
    text.remove(caret - 1);
    caret -= 1;
    apply(&mut state, &mut text, &mut caret, |_| true);

    assert_eq!(text, "method =");
    assert_eq!(caret, text.len());
}

#[test]
fn a_column_without_quotes_or_a_paste_gets_nothing() {
    let mut state = AutoQuoteState::default();
    let (mut text, mut caret) = ("id =".to_string(), 4);
    state.text = "id ".to_string();
    apply(&mut state, &mut text, &mut caret, |_| false);
    assert_eq!(text, "id =");

    let mut state = AutoQuoteState {
        text: "method ".to_string(),
        pasting: true,
        ..AutoQuoteState::default()
    };
    let (mut text, mut caret) = ("method =".to_string(), 8);
    apply(&mut state, &mut text, &mut caret, |_| true);
    assert_eq!(text, "method =", "a pasted `=` is not typing");
}
