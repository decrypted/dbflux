//! SQL expressions a grid cell can be set to instead of a literal value
//! (`NOW()`, `"qty" + 1`), and the per-dialect suggestions offered for them.
//!
//! An expression reaches the database verbatim, as the right-hand side of one
//! `SET` assignment or one `VALUES` slot of the statement the grid's save
//! path generates. [`validate_cell_expression`] keeps it inside that slot.

/// Why a text cannot be sent as a cell expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellExpressionError {
    /// Nothing but whitespace.
    Empty,
    /// A quoted string or identifier is not closed.
    UnterminatedQuote,
    /// Parentheses do not pair up.
    UnbalancedParentheses,
    /// A `;` outside quotes would end the statement and start another one.
    StatementSeparator,
    /// A comment outside quotes would swallow the rest of the statement,
    /// its `WHERE` clause included.
    Comment,
    /// A `,` outside parentheses would assign a second column.
    TopLevelComma,
}

impl std::fmt::Display for CellExpressionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Empty => "the expression is empty",
            Self::UnterminatedQuote => "a quote in the expression is not closed",
            Self::UnbalancedParentheses => "the parentheses in the expression do not match",
            Self::StatementSeparator => "the expression contains a `;` outside quotes",
            Self::Comment => "the expression contains a comment",
            Self::TopLevelComma => "the expression contains a `,` outside parentheses",
        };
        f.write_str(message)
    }
}

impl std::error::Error for CellExpressionError {}

/// Checks that `expression` is one self-contained SQL value expression.
///
/// This is a lexical check, not a parser: the database still decides whether
/// the expression is valid. What it rules out is text that would change the
/// statement around the slot the expression is written into: a comment
/// would hide the `WHERE` clause and turn a one-row `UPDATE` into an update
/// of every row, a `;` would start a second statement on drivers that run
/// batches, and a top-level `,` would assign another column. Quotes are
/// skipped as `'...'`, `"..."`, `` `...` `` and `[...]`, with doubled quotes
/// as escapes; a backslash-escaped quote reads as the end of the string and
/// fails the check rather than slip through.
pub fn validate_cell_expression(expression: &str) -> Result<(), CellExpressionError> {
    if expression.trim().is_empty() {
        return Err(CellExpressionError::Empty);
    }

    let chars: Vec<char> = expression.chars().collect();
    let mut depth: usize = 0;
    let mut index = 0;

    while let Some(&ch) = chars.get(index) {
        let next = chars.get(index + 1).copied();

        match ch {
            '\'' | '"' | '`' | '[' => {
                let close = if ch == '[' { ']' } else { ch };
                index = skip_quoted(&chars, index + 1, close)?;
                continue;
            }
            '-' if next == Some('-') => return Err(CellExpressionError::Comment),
            '/' if next == Some('*') => return Err(CellExpressionError::Comment),
            '#' => return Err(CellExpressionError::Comment),
            ';' => return Err(CellExpressionError::StatementSeparator),
            '(' => depth += 1,
            ')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or(CellExpressionError::UnbalancedParentheses)?;
            }
            ',' if depth == 0 => return Err(CellExpressionError::TopLevelComma),
            _ => {}
        }

        index += 1;
    }

    if depth != 0 {
        return Err(CellExpressionError::UnbalancedParentheses);
    }

    Ok(())
}

/// The index just past the `close` that ends a quoted run starting at
/// `start`, where a doubled `close` stands for itself.
fn skip_quoted(chars: &[char], start: usize, close: char) -> Result<usize, CellExpressionError> {
    let mut index = start;

    while let Some(&ch) = chars.get(index) {
        if ch == close {
            if chars.get(index + 1) == Some(&close) {
                index += 2;
                continue;
            }
            return Ok(index + 1);
        }
        index += 1;
    }

    Err(CellExpressionError::UnterminatedQuote)
}

/// What a column's type makes useful to suggest for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellExpressionColumn {
    /// Date and time of day (`timestamp`, `datetime`, `datetime2`).
    Timestamp,
    Date,
    Time,
    /// A native uuid type (`uuid`, `uniqueidentifier`).
    Uuid,
    /// A uuid spelled out in 36 characters (`char(36)`).
    UuidText,
    /// A uuid packed into 16 bytes (`binary(16)`).
    UuidBinary,
    Numeric,
    Text,
    Other,
}

impl CellExpressionColumn {
    /// Classifies a driver-reported column type such as `timestamp without
    /// time zone`, `varchar(255)` or `int unsigned`.
    pub fn classify(type_name: &str) -> Self {
        let lower = type_name.trim().to_ascii_lowercase();
        let base = lower
            .split(|c: char| c == '(' || c.is_whitespace())
            .next()
            .unwrap_or("");
        let length = parenthesized_length(&lower);

        match base {
            "timestamp" | "timestamptz" | "datetime" | "datetime2" | "smalldatetime"
            | "datetimeoffset" => Self::Timestamp,
            "date" => Self::Date,
            "time" | "timetz" => Self::Time,
            "uuid" | "uniqueidentifier" => Self::Uuid,
            "binary" if length == Some(16) => Self::UuidBinary,
            "char" | "character" | "varchar" | "nchar" | "nvarchar" if length == Some(36) => {
                Self::UuidText
            }
            "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint" | "int2"
            | "int4" | "int8" | "serial" | "smallserial" | "bigserial" | "decimal" | "dec"
            | "numeric" | "fixed" | "float" | "float4" | "float8" | "double" | "real" | "money"
            | "smallmoney" => Self::Numeric,
            "char" | "character" | "varchar" | "nchar" | "nvarchar" | "text" | "tinytext"
            | "mediumtext" | "longtext" | "ntext" | "citext" | "clob" | "string" => Self::Text,
            _ => Self::Other,
        }
    }
}

/// The single length of a type such as `char(36)`, when it has exactly one.
fn parenthesized_length(type_name: &str) -> Option<u32> {
    let open = type_name.find('(')?;
    let close = type_name[open..].find(')')? + open;
    type_name.get(open + 1..close)?.trim().parse().ok()
}

/// The expressions one dialect offers per kind of column.
///
/// Entries may contain `{col}`, which stands for the quoted name of the
/// column being edited. Numeric columns always get `{col} + 1` and
/// `{col} - 1`, and every column gets `NULL`, plus `DEFAULT` where the
/// dialect accepts it in `UPDATE ... SET` and `INSERT ... VALUES`.
#[derive(Debug, Clone, Copy)]
pub struct CellExpressionVocabulary {
    pub timestamp: &'static [&'static str],
    pub date: &'static [&'static str],
    pub time: &'static [&'static str],
    pub uuid: &'static [&'static str],
    pub uuid_text: &'static [&'static str],
    pub uuid_binary: &'static [&'static str],
    pub text: &'static [&'static str],
    pub supports_default: bool,
}

/// The suggestions of `vocabulary` for a column named `quoted_column` (as the
/// dialect quotes it) of type `type_name`, most specific first.
pub fn cell_expression_suggestions(
    vocabulary: &CellExpressionVocabulary,
    quoted_column: &str,
    type_name: &str,
) -> Vec<String> {
    const NUMERIC: &[&str] = &["{col} + 1", "{col} - 1"];

    let class = CellExpressionColumn::classify(type_name);
    let specific: &[&str] = match class {
        CellExpressionColumn::Timestamp => vocabulary.timestamp,
        CellExpressionColumn::Date => vocabulary.date,
        CellExpressionColumn::Time => vocabulary.time,
        CellExpressionColumn::Uuid => vocabulary.uuid,
        CellExpressionColumn::UuidText => vocabulary.uuid_text,
        CellExpressionColumn::UuidBinary => vocabulary.uuid_binary,
        CellExpressionColumn::Numeric => NUMERIC,
        CellExpressionColumn::Text | CellExpressionColumn::Other => &[],
    };
    // A uuid kept as text is still text.
    let text: &[&str] = match class {
        CellExpressionColumn::Text | CellExpressionColumn::UuidText => vocabulary.text,
        _ => &[],
    };

    let mut suggestions: Vec<String> = specific
        .iter()
        .chain(text)
        .map(|template| template.replace("{col}", quoted_column))
        .collect();

    suggestions.push("NULL".to_string());
    if vocabulary.supports_default {
        suggestions.push("DEFAULT".to_string());
    }

    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_calls_and_arithmetic_are_valid() {
        for expression in [
            "NOW()",
            "CURRENT_TIMESTAMP",
            "\"qty\" + 1",
            "DATE_ADD(NOW(), INTERVAL 1 DAY)",
            "CONCAT(`name`, ';', '--')",
            "'it''s'",
            "[weird]] name] + 1",
            "DEFAULT",
        ] {
            assert_eq!(validate_cell_expression(expression), Ok(()), "{expression}");
        }
    }

    #[test]
    fn text_that_would_escape_its_slot_is_rejected() {
        let cases = [
            ("  ", CellExpressionError::Empty),
            ("1 -- WHERE", CellExpressionError::Comment),
            ("1 /* x */", CellExpressionError::Comment),
            ("1 # x", CellExpressionError::Comment),
            ("1; DELETE FROM t", CellExpressionError::StatementSeparator),
            ("1, other = 2", CellExpressionError::TopLevelComma),
            ("NOW(", CellExpressionError::UnbalancedParentheses),
            ("NOW())", CellExpressionError::UnbalancedParentheses),
            ("'open", CellExpressionError::UnterminatedQuote),
            ("'a\\'; DROP", CellExpressionError::StatementSeparator),
        ];

        for (expression, expected) in cases {
            assert_eq!(
                validate_cell_expression(expression),
                Err(expected),
                "{expression}"
            );
        }
    }

    #[test]
    fn column_types_are_classified_by_their_base_type() {
        use CellExpressionColumn::*;

        for (type_name, expected) in [
            ("timestamp without time zone", Timestamp),
            ("DATETIME", Timestamp),
            ("datetime2(7)", Timestamp),
            ("date", Date),
            ("time(3)", Time),
            ("uuid", Uuid),
            ("uniqueidentifier", Uuid),
            ("char(36)", UuidText),
            ("varchar(255)", Text),
            ("binary(16)", UuidBinary),
            ("binary(8)", Other),
            ("int unsigned", Numeric),
            ("double precision", Numeric),
            ("numeric(10,2)", Numeric),
            ("interval", Other),
            ("point", Other),
            ("character varying", Text),
            ("text", Text),
            ("jsonb", Other),
        ] {
            assert_eq!(
                CellExpressionColumn::classify(type_name),
                expected,
                "{type_name}"
            );
        }
    }

    const VOCABULARY: CellExpressionVocabulary = CellExpressionVocabulary {
        timestamp: &["NOW()"],
        date: &["CURRENT_DATE"],
        time: &["CURRENT_TIME"],
        uuid: &["UUID()"],
        uuid_text: &["UUID()"],
        uuid_binary: &["UUID_BIN()"],
        text: &["UPPER({col})"],
        supports_default: true,
    };

    #[test]
    fn suggestions_name_the_quoted_column_and_end_with_null_and_default() {
        assert_eq!(
            cell_expression_suggestions(&VOCABULARY, "\"qty\"", "integer"),
            ["\"qty\" + 1", "\"qty\" - 1", "NULL", "DEFAULT"]
        );
        assert_eq!(
            cell_expression_suggestions(&VOCABULARY, "\"id\"", "char(36)"),
            ["UUID()", "UPPER(\"id\")", "NULL", "DEFAULT"]
        );
        assert_eq!(
            cell_expression_suggestions(&VOCABULARY, "\"at\"", "timestamp"),
            ["NOW()", "NULL", "DEFAULT"]
        );
    }

    #[test]
    fn default_is_left_out_where_the_dialect_rejects_it() {
        let vocabulary = CellExpressionVocabulary {
            supports_default: false,
            ..VOCABULARY
        };

        assert_eq!(
            cell_expression_suggestions(&vocabulary, "\"blob\"", "jsonb"),
            ["NULL"]
        );
    }
}
