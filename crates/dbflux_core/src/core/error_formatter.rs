use crate::DbError;
use std::fmt;

/// Formatted error with structured information for display.
#[derive(Debug, Clone, Default)]
pub struct FormattedError {
    /// Primary error message.
    pub message: String,

    /// Additional detail about the error (e.g., PostgreSQL's DETAIL field).
    pub detail: Option<String>,

    /// Suggestion for how to fix the error (e.g., PostgreSQL's HINT field).
    pub hint: Option<String>,

    /// Error code from the database (e.g., SQLSTATE, MySQL error code).
    pub code: Option<String>,

    /// Location information if available.
    pub location: Option<ErrorLocation>,

    /// Whether the error is retriable (e.g., transient network issues).
    pub retriable: bool,
}

impl FormattedError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Default::default()
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_location(mut self, location: ErrorLocation) -> Self {
        self.location = Some(location);
        self
    }

    pub fn with_retriable(mut self, retriable: bool) -> Self {
        self.retriable = retriable;
        self
    }

    /// Convert to a single-line display string.
    pub fn to_display_string(&self) -> String {
        let mut parts = vec![self.message.clone()];

        if let Some(ref detail) = self.detail {
            parts.push(format!("Detail: {}", detail));
        }

        if let Some(ref hint) = self.hint {
            parts.push(format!("Hint: {}", hint));
        }

        if let Some(ref loc) = self.location {
            if let Some(ref table) = loc.table {
                parts.push(format!("Table: {}", table));
            }
            if let Some(ref column) = loc.column {
                parts.push(format!("Column: {}", column));
            }
            if let Some(ref constraint) = loc.constraint {
                parts.push(format!("Constraint: {}", constraint));
            }
        }

        if let Some(ref code) = self.code {
            parts.push(format!("Code: {}", code));
        }

        parts.join(". ")
    }

    /// Classify into the appropriate `DbError` variant for query errors.
    ///
    /// Uses SQLSTATE codes (when present) to route to semantic variants:
    /// - `42xxx` -> `SyntaxError`
    /// - `23xxx` -> `ConstraintViolation`
    /// - `28xxx` -> `AuthFailed`
    /// - `42501` / `42000` (with permission context) -> `PermissionDenied`
    /// - `42P01` / `1146` -> `ObjectNotFound`
    /// - Everything else -> `QueryFailed`
    pub fn into_query_error(self) -> DbError {
        if let Some(variant) = self.code.as_deref().and_then(classify_query_sqlstate) {
            return match variant {
                ErrorClass::Syntax => DbError::SyntaxError(self),
                ErrorClass::Constraint => DbError::ConstraintViolation(self),
                ErrorClass::Auth => DbError::AuthFailed(self),
                ErrorClass::Permission => DbError::PermissionDenied(self),
                ErrorClass::NotFound => DbError::ObjectNotFound(self),
            };
        }

        DbError::QueryFailed(self)
    }

    /// Classify into the appropriate `DbError` variant for connection errors.
    ///
    /// Uses SQLSTATE codes (when present) to route to semantic variants:
    /// - `28xxx` -> `AuthFailed`
    /// - Everything else -> `ConnectionFailed`
    pub fn into_connection_error(self) -> DbError {
        if self.code.as_deref().is_some_and(|c| c.starts_with("28")) {
            return DbError::AuthFailed(self);
        }

        DbError::ConnectionFailed(self)
    }
}

impl fmt::Display for FormattedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display_string())
    }
}

impl From<String> for FormattedError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for FormattedError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

enum ErrorClass {
    Syntax,
    Constraint,
    Auth,
    Permission,
    NotFound,
}

/// Classify a SQLSTATE or MySQL error code into a semantic error class.
fn classify_query_sqlstate(code: &str) -> Option<ErrorClass> {
    // Exact matches first (more specific)
    match code {
        // PostgreSQL: insufficient_privilege
        "42501" => return Some(ErrorClass::Permission),
        // PostgreSQL: undefined_table
        "42P01" => return Some(ErrorClass::NotFound),
        // PostgreSQL: undefined_column
        "42703" => return Some(ErrorClass::NotFound),
        // PostgreSQL: undefined_function
        "42883" => return Some(ErrorClass::NotFound),
        // MySQL: Table doesn't exist
        "1146" => return Some(ErrorClass::NotFound),
        // MySQL: Unknown column
        "1054" => return Some(ErrorClass::NotFound),
        // MySQL: Access denied
        "1044" | "1045" => return Some(ErrorClass::Auth),
        _ => {}
    }

    // Class-level matching (first 2 characters of SQLSTATE)
    if code.len() >= 2 {
        match &code[..2] {
            // Class 23: Integrity constraint violation
            "23" => return Some(ErrorClass::Constraint),
            // Class 28: Invalid authorization specification
            "28" => return Some(ErrorClass::Auth),
            // Class 42: Syntax error or access rule violation
            "42" => return Some(ErrorClass::Syntax),
            _ => {}
        }
    }

    None
}

/// Location information for database errors.
#[derive(Debug, Clone, Default)]
pub struct ErrorLocation {
    /// Schema where the error occurred.
    pub schema: Option<String>,

    /// Table where the error occurred.
    pub table: Option<String>,

    /// Column where the error occurred.
    pub column: Option<String>,

    /// Constraint that was violated.
    pub constraint: Option<String>,
}

impl ErrorLocation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_table(mut self, table: impl Into<String>) -> Self {
        self.table = Some(table.into());
        self
    }

    pub fn with_column(mut self, column: impl Into<String>) -> Self {
        self.column = Some(column.into());
        self
    }

    pub fn with_constraint(mut self, constraint: impl Into<String>) -> Self {
        self.constraint = Some(constraint.into());
        self
    }

    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.schema.is_none()
            && self.table.is_none()
            && self.column.is_none()
            && self.constraint.is_none()
    }
}

/// Trait for formatting database-specific errors into a structured format.
///
/// Each driver implements this to extract detailed error information
/// from their specific error types.
pub trait QueryErrorFormatter: Send + Sync {
    /// Format a query execution error.
    ///
    /// This is called when a SQL query or database operation fails.
    fn format_query_error(&self, error: &(dyn std::error::Error + 'static)) -> FormattedError;
}

/// Trait for formatting connection errors.
///
/// Separated from QueryErrorFormatter because connection errors often need
/// additional context (host, port) that query errors don't have.
pub trait ConnectionErrorFormatter: Send + Sync {
    /// Format a connection error with host/port context.
    fn format_connection_error(
        &self,
        error: &(dyn std::error::Error + 'static),
        host: &str,
        port: u16,
    ) -> FormattedError;

    /// Format a URI-based connection error.
    ///
    /// The URI should be sanitized (password removed) before display.
    fn format_uri_error(
        &self,
        error: &(dyn std::error::Error + 'static),
        sanitized_uri: &str,
    ) -> FormattedError;
}

/// Default implementation that just uses Display.
pub struct DefaultErrorFormatter;

impl QueryErrorFormatter for DefaultErrorFormatter {
    fn format_query_error(&self, error: &(dyn std::error::Error + 'static)) -> FormattedError {
        FormattedError::new(error.to_string())
    }
}

impl ConnectionErrorFormatter for DefaultErrorFormatter {
    fn format_connection_error(
        &self,
        error: &(dyn std::error::Error + 'static),
        host: &str,
        port: u16,
    ) -> FormattedError {
        FormattedError::new(format!("Failed to connect to {}:{}: {}", host, port, error))
    }

    fn format_uri_error(
        &self,
        error: &(dyn std::error::Error + 'static),
        sanitized_uri: &str,
    ) -> FormattedError {
        FormattedError::new(format!(
            "Failed to connect using URI {}: {}",
            sanitized_uri, error
        ))
    }
}

/// Sanitize a connection URI by removing credentials.
///
/// Returns a safe-to-display version of the URI with the userinfo password and
/// credential-bearing parameters replaced by `***`. See
/// [`redact_connection_credentials`] for the exact rules.
pub fn sanitize_uri(uri: &str) -> String {
    redact_connection_credentials(uri, "***").0
}

/// Redacts credentials from URLs ([`redact_uri_credentials`]) and from
/// `key=value` connection strings anywhere in `text`: an ADO string such as
/// `Server=h;User Id=u;Password=p` or a libpq `host=h password=p` string is
/// accepted wherever drivers take a URI.
///
/// The libpq pass (pairs end at whitespace) and the ADO pass (pairs end at
/// `;`) both run, one after the other, because either kind may contain the
/// other's separator inside a value. Each only replaces values of credential
/// keys, so the worst case is redacting too much.
///
/// Returns the redacted text and the number of values replaced.
pub fn redact_connection_credentials(text: &str, replacement: &str) -> (String, usize) {
    let (urls, url_count) = redact_uri_credentials(text, replacement);
    let (libpq, libpq_count) =
        redact_key_value_credentials(&urls, replacement, KeyValueSyntax::Libpq);
    // Without a `;` the whole text would be one ADO pair, and a sentence
    // that merely ends in `password=` would lose everything after it.
    if !libpq.contains(';') {
        return (libpq, url_count + libpq_count);
    }
    let (ado, ado_count) = redact_key_value_credentials(&libpq, replacement, KeyValueSyntax::Ado);
    (ado, url_count + libpq_count + ado_count)
}

fn is_sensitive_connection_key(key: &str) -> bool {
    SENSITIVE_URI_PARAMETERS
        .iter()
        .any(|name| key.eq_ignore_ascii_case(name))
        // `sslpassword`, `User Password`, ...
        || key.to_ascii_lowercase().ends_with("password")
}

/// A flag such as `save_password=true` in a log line is not a secret.
fn is_flag_value(value: &str) -> bool {
    let value = value.trim().trim_end_matches([',', '.', ')']);
    value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyValueSyntax {
    /// Pairs end at whitespace; whitespace around `=` is allowed.
    Libpq,
    /// Pairs end at `;`; keys may contain spaces.
    Ado,
}

impl KeyValueSyntax {
    fn is_pair_separator(self, c: char) -> bool {
        match self {
            Self::Libpq => c.is_whitespace(),
            Self::Ado => c == ';',
        }
    }
}

/// Replaces the values of credential keys in `key=value` pairs.
///
/// A quoted value (`"..."`, `'...'`, `{...}`) runs to its closing quote, so a
/// separator inside a password does not cut the redaction short; an
/// unterminated quote redacts to the end.
fn redact_key_value_credentials(
    text: &str,
    replacement: &str,
    syntax: KeyValueSyntax,
) -> (String, usize) {
    let is_pair_separator = |c: char| syntax.is_pair_separator(c);

    let mut output = String::with_capacity(text.len());
    let mut redaction_count = 0;
    let mut cursor = 0;

    while cursor < text.len() {
        let rest = &text[cursor..];
        let pair_start = rest
            .find(|c: char| !is_pair_separator(c))
            .unwrap_or(rest.len());
        output.push_str(&rest[..pair_start]);
        cursor += pair_start;

        let rest = &text[cursor..];
        if rest.is_empty() {
            break;
        }

        let key_end = rest
            .find(|c: char| c == '=' || is_pair_separator(c))
            .unwrap_or(rest.len());

        // libpq allows `password = secret`: the key ends at whitespace, but
        // the pair continues at the `=` after it.
        let equals = if rest[key_end..].starts_with('=') {
            Some(key_end)
        } else if syntax == KeyValueSyntax::Libpq && key_end > 0 {
            let after_key = &rest[key_end..];
            let gap = after_key.len() - after_key.trim_start().len();
            after_key[gap..].starts_with('=').then_some(key_end + gap)
        } else {
            None
        };

        let Some(equals) = equals else {
            output.push_str(&rest[..key_end]);
            cursor += key_end;
            continue;
        };

        let key = &rest[..key_end];
        let value_start = equals + 1;
        let value_end =
            value_start + connection_value_length(&rest[value_start..], is_pair_separator);
        let value = &rest[value_start..value_end];

        if is_sensitive_connection_key(key.trim())
            && !value.trim().is_empty()
            && !is_flag_value(value)
        {
            let leading = value.len() - value.trim_start().len();
            output.push_str(&rest[..value_start + leading]);
            output.push_str(replacement);
            redaction_count += 1;
        } else {
            output.push_str(&rest[..value_end]);
        }
        cursor += value_end;
    }

    (output, redaction_count)
}

/// Length in bytes of the value at the start of `value` (after its `=`).
fn connection_value_length(value: &str, is_pair_separator: impl Fn(char) -> bool) -> usize {
    let leading = value.len() - value.trim_start().len();
    let body = &value[leading..];

    let closing = match body.chars().next() {
        Some('"') => '"',
        Some('\'') => '\'',
        Some('{') => '}',
        _ => {
            return leading + body.find(&is_pair_separator).unwrap_or(body.len());
        }
    };

    let mut chars = body.char_indices().skip(1).peekable();
    while let Some((index, c)) = chars.next() {
        if c == '\\' {
            // libpq escapes with a backslash.
            chars.next();
        } else if c == closing {
            // ADO escapes a quote by doubling it.
            if chars.peek().is_some_and(|&(_, next)| next == closing) {
                chars.next();
            } else {
                return leading + index + c.len_utf8();
            }
        }
    }

    value.len()
}

/// Parameter names whose values are credentials when they appear in the query
/// string or in `;`-separated parameters of a URL. Compared case-insensitively.
const SENSITIVE_URI_PARAMETERS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "pass",
    "token",
    "access_token",
    "auth_token",
    "authtoken",
    "refresh_token",
    "id_token",
    "secret",
    "client_secret",
    "secret_key",
    "api_key",
    "apikey",
    "sessiontoken",
    "session_token",
    "x-amz-security-token",
    "x-amz-signature",
];

/// Redacts credentials from every URL embedded in `text`, whatever its scheme.
///
/// For each `scheme://` occurrence it replaces:
/// - the password of the userinfo (`scheme://user:password@host`), keeping the
///   user name and host so the message stays useful. The last `@` of the
///   authority ends the userinfo, so an unencoded `@` inside the password is
///   still covered, and bracketed IPv6 hosts are left intact;
/// - the value of credential-bearing parameters (`password=`, `pwd=`,
///   `token=`, `secret=`, ...) in the query string or in JDBC-style
///   `;key=value` segments.
///
/// A URL ends at whitespace, a quote or an angle bracket, so the function can
/// run over free text such as error messages and log lines. A userinfo without
/// a password (`scheme://user@host`) is left unchanged.
///
/// Returns the redacted text and the number of values replaced.
pub fn redact_uri_credentials(text: &str, replacement: &str) -> (String, usize) {
    let mut output = String::with_capacity(text.len());
    let mut redaction_count = 0;
    let mut cursor = 0;

    while let Some(offset) = text[cursor..].find("://") {
        let separator_start = cursor + offset;

        if !has_scheme_before(&text[..separator_start]) {
            output.push_str(&text[cursor..separator_start + 3]);
            cursor = separator_start + 3;
            continue;
        }

        let url_start = separator_start + 3;
        let url_end = text[url_start..]
            .find(is_url_terminator)
            .map_or(text.len(), |length| url_start + length);

        output.push_str(&text[cursor..url_start]);

        let (redacted_url, url_redactions) =
            redact_url_remainder(&text[url_start..url_end], replacement);
        output.push_str(&redacted_url);
        redaction_count += url_redactions;

        cursor = url_end;
    }

    output.push_str(&text[cursor..]);

    (output, redaction_count)
}

fn has_scheme_before(prefix: &str) -> bool {
    let scheme: String = prefix
        .chars()
        .rev()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
        .collect();

    scheme
        .chars()
        .last()
        .is_some_and(|first| first.is_ascii_alphabetic())
}

fn is_url_terminator(character: char) -> bool {
    character.is_whitespace() || matches!(character, '"' | '\'' | '`' | '<' | '>')
}

/// Redacts one URL given everything after its `scheme://`.
fn redact_url_remainder(remainder: &str, replacement: &str) -> (String, usize) {
    let authority_end = remainder
        .find(['/', '?', '#', ';'])
        .unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let tail = &remainder[authority_end..];

    let mut redaction_count = 0;
    let mut output = String::with_capacity(remainder.len());

    match authority.rsplit_once('@') {
        Some((userinfo, host)) => match userinfo.split_once(':') {
            Some((user, password)) if !password.is_empty() => {
                output.push_str(user);
                output.push(':');
                output.push_str(replacement);
                output.push('@');
                output.push_str(host);
                redaction_count += 1;
            }
            _ => output.push_str(authority),
        },
        None => output.push_str(authority),
    }

    let (redacted_tail, tail_redactions) = redact_sensitive_parameters(tail, replacement);
    output.push_str(&redacted_tail);
    redaction_count += tail_redactions;

    (output, redaction_count)
}

/// Replaces the values of sensitive `key=value` parameters that follow a `?`,
/// `&` or `;`, stopping each value at the next `&`, `;` or `#`.
fn redact_sensitive_parameters(tail: &str, replacement: &str) -> (String, usize) {
    let mut output = String::with_capacity(tail.len());
    let mut redaction_count = 0;
    let mut cursor = 0;

    while let Some(offset) = tail[cursor..].find(['?', '&', ';']) {
        let key_start = cursor + offset + 1;
        output.push_str(&tail[cursor..key_start]);
        cursor = key_start;

        let segment_end = tail[key_start..]
            .find(['&', ';', '#'])
            .map_or(tail.len(), |length| key_start + length);
        let segment = &tail[key_start..segment_end];

        let Some((key, value)) = segment.split_once('=') else {
            continue;
        };

        let is_sensitive = SENSITIVE_URI_PARAMETERS
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name));

        if is_sensitive && !value.is_empty() {
            output.push_str(key);
            output.push('=');
            output.push_str(replacement);
            redaction_count += 1;
            cursor = segment_end;
        }
    }

    output.push_str(&tail[cursor..]);

    (output, redaction_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_formatted_error_display() {
        let err = FormattedError::new("syntax error")
            .with_detail("near 'FROM'")
            .with_code("42601");

        assert_eq!(
            err.to_display_string(),
            "syntax error. Detail: near 'FROM'. Code: 42601"
        );
    }

    #[test]
    fn test_formatted_error_display_trait() {
        let err = FormattedError::new("test error");
        assert_eq!(format!("{}", err), "test error");
    }

    #[test]
    fn test_formatted_error_from_string() {
        let err: FormattedError = "hello".into();
        assert_eq!(err.message, "hello");

        let err: FormattedError = String::from("world").into();
        assert_eq!(err.message, "world");
    }

    #[test]
    fn test_formatted_error_with_location() {
        let err = FormattedError::new("duplicate key")
            .with_location(
                ErrorLocation::new()
                    .with_table("users")
                    .with_constraint("users_pkey"),
            )
            .with_code("23505");

        assert_eq!(
            err.to_display_string(),
            "duplicate key. Table: users. Constraint: users_pkey. Code: 23505"
        );
    }

    #[test]
    fn test_formatted_error_retriable() {
        let err = FormattedError::new("timeout").with_retriable(true);
        assert!(err.retriable);

        let err = FormattedError::new("syntax error");
        assert!(!err.retriable);
    }

    #[test]
    fn test_classify_constraint_violation() {
        let err = FormattedError::new("duplicate key").with_code("23505");
        match err.into_query_error() {
            DbError::ConstraintViolation(f) => assert_eq!(f.message, "duplicate key"),
            other => panic!("Expected ConstraintViolation, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_syntax_error() {
        let err = FormattedError::new("syntax error").with_code("42601");
        match err.into_query_error() {
            DbError::SyntaxError(f) => assert_eq!(f.message, "syntax error"),
            other => panic!("Expected SyntaxError, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_permission_denied() {
        let err = FormattedError::new("insufficient privilege").with_code("42501");
        match err.into_query_error() {
            DbError::PermissionDenied(f) => assert_eq!(f.message, "insufficient privilege"),
            other => panic!("Expected PermissionDenied, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_object_not_found() {
        let err = FormattedError::new("table not found").with_code("42P01");
        match err.into_query_error() {
            DbError::ObjectNotFound(f) => assert_eq!(f.message, "table not found"),
            other => panic!("Expected ObjectNotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_auth_failed() {
        let err = FormattedError::new("invalid password").with_code("28P01");
        match err.into_query_error() {
            DbError::AuthFailed(f) => assert_eq!(f.message, "invalid password"),
            other => panic!("Expected AuthFailed, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_auth_connection() {
        let err = FormattedError::new("invalid password").with_code("28P01");
        match err.into_connection_error() {
            DbError::AuthFailed(f) => assert_eq!(f.message, "invalid password"),
            other => panic!("Expected AuthFailed, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_no_code_defaults_to_query_failed() {
        let err = FormattedError::new("some error");
        match err.into_query_error() {
            DbError::QueryFailed(f) => assert_eq!(f.message, "some error"),
            other => panic!("Expected QueryFailed, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_mysql_not_found() {
        let err = FormattedError::new("table not found").with_code("1146");
        match err.into_query_error() {
            DbError::ObjectNotFound(f) => assert_eq!(f.message, "table not found"),
            other => panic!("Expected ObjectNotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_sanitize_uri_with_password() {
        let uri = "postgres://user:secret@localhost:5432/db";
        assert_eq!(sanitize_uri(uri), "postgres://user:***@localhost:5432/db");
    }

    #[test]
    fn test_sanitize_uri_without_password() {
        let uri = "postgres://localhost:5432/db";
        assert_eq!(sanitize_uri(uri), "postgres://localhost:5432/db");
    }

    fn redact(text: &str) -> String {
        redact_uri_credentials(text, "***").0
    }

    #[test]
    fn redacts_userinfo_password_for_every_scheme() {
        let cases = [
            ("postgres://u:pw@h:5432/db", "postgres://u:***@h:5432/db"),
            (
                "postgresql://u:pw@h:5432/db",
                "postgresql://u:***@h:5432/db",
            ),
            ("mysql://root:pw@h:3306/app", "mysql://root:***@h:3306/app"),
            ("mariadb://root:pw@h/app", "mariadb://root:***@h/app"),
            (
                "mongodb://u:pw@h1:27017,h2:27017/db",
                "mongodb://u:***@h1:27017,h2:27017/db",
            ),
            (
                "mongodb+srv://u:pw@cluster.example.net/db",
                "mongodb+srv://u:***@cluster.example.net/db",
            ),
            ("redis://:pw@h:6379/0", "redis://:***@h:6379/0"),
            ("rediss://default:pw@h:6380", "rediss://default:***@h:6380"),
            (
                "sqlserver://sa:pw@h:1433/master",
                "sqlserver://sa:***@h:1433/master",
            ),
            ("clickhouse://u:pw@h:8123", "clickhouse://u:***@h:8123"),
            ("http://u:pw@h:8086/api", "http://u:***@h:8086/api"),
            ("https://u:pw@h/path", "https://u:***@h/path"),
        ];

        for (input, expected) in cases {
            assert_eq!(redact(input), expected, "input: {input}");
        }
    }

    #[test]
    fn leaves_uris_without_password_unchanged() {
        for input in [
            "postgres://u@h:5432/db",
            "postgresql://h/db",
            "mongodb://h:27017/db?replicaSet=rs0",
            "redis://[::1]:6379",
            "https://example.com/a@b",
        ] {
            let (output, count) = redact_uri_credentials(input, "***");
            assert_eq!(output, input);
            assert_eq!(count, 0, "input: {input}");
        }
    }

    #[test]
    fn redacts_password_with_encoded_or_raw_at_sign() {
        assert_eq!(
            redact("postgresql://alice:p%40ss@localhost/app"),
            "postgresql://alice:***@localhost/app"
        );
        assert_eq!(
            redact("postgresql://alice:p@ss:w0rd@localhost/app"),
            "postgresql://alice:***@localhost/app"
        );
    }

    #[test]
    fn redacts_password_before_ipv6_host() {
        assert_eq!(
            redact("postgresql://u:pw@[2001:db8::1]:5432/db"),
            "postgresql://u:***@[2001:db8::1]:5432/db"
        );
        assert_eq!(redact("redis://:pw@[::1]:6379"), "redis://:***@[::1]:6379");
    }

    #[test]
    fn redacts_sensitive_query_and_jdbc_parameters() {
        assert_eq!(
            redact("postgresql://h/db?user=u&password=pw&sslmode=require"),
            "postgresql://h/db?user=u&password=***&sslmode=require"
        );
        assert_eq!(
            redact("https://h/api?Token=abc#frag"),
            "https://h/api?Token=***#frag"
        );
        assert_eq!(
            redact("mysql://h/db?PWD=x&secret=y"),
            "mysql://h/db?PWD=***&secret=***"
        );
        assert_eq!(
            redact("jdbc:sqlserver://h:1433;user=sa;password=pw;databaseName=app"),
            "jdbc:sqlserver://h:1433;user=sa;password=***;databaseName=app"
        );
    }

    #[test]
    fn sanitize_uri_redacts_ado_connection_strings() {
        assert_eq!(
            sanitize_uri("Server=tcp:h,1433;User Id=sa;Password=hunter2;Encrypt=true"),
            "Server=tcp:h,1433;User Id=sa;Password=***;Encrypt=true"
        );
        assert_eq!(
            sanitize_uri("server=h;pwd = hunter2 ;database=app"),
            "server=h;pwd = ***;database=app"
        );
        // Quoted values may hold the separator; doubled quotes are escapes.
        assert_eq!(
            sanitize_uri("Server=h;Password=\"hun;ter\"\"2\";Database=app"),
            "Server=h;Password=***;Database=app"
        );
        assert_eq!(
            sanitize_uri("Server=h;Password={hun;ter2};Database=app"),
            "Server=h;Password=***;Database=app"
        );
        assert_eq!(
            sanitize_uri("Server=h;Password='unterminated;Database=app"),
            "Server=h;Password=***"
        );
        assert_eq!(
            sanitize_uri("Server=h;User Id=sa;Database=app"),
            "Server=h;User Id=sa;Database=app"
        );
    }

    #[test]
    fn sanitize_uri_redacts_libpq_connection_strings() {
        assert_eq!(
            sanitize_uri("host=db user=app password=hunter2 dbname=app"),
            "host=db user=app password=*** dbname=app"
        );
        assert_eq!(
            sanitize_uri("host=db password='hun ter\\'2' sslpassword=k sslmode=require"),
            "host=db password=*** sslpassword=*** sslmode=require"
        );
        assert_eq!(sanitize_uri("host=db dbname=app"), "host=db dbname=app");
    }

    /// One `;` must not switch a libpq string to ADO rules for every pair.
    #[test]
    fn sanitize_uri_redacts_libpq_values_that_contain_a_semicolon() {
        assert_eq!(
            sanitize_uri("host=db password=hun;ter2 dbname=app"),
            "host=db password=*** dbname=app"
        );
        assert_eq!(
            sanitize_uri("host=db password='hun;ter2' dbname=app"),
            "host=db password=*** dbname=app"
        );
    }

    #[test]
    fn sanitize_uri_redacts_libpq_values_with_spaces_around_the_equals_sign() {
        assert_eq!(
            sanitize_uri("host=db password = hunter2 dbname=app"),
            "host=db password = *** dbname=app"
        );
        assert_eq!(
            sanitize_uri("host=db password =hunter2"),
            "host=db password =***"
        );
    }

    #[test]
    fn credential_flags_in_log_lines_are_kept() {
        let (redacted, count) = redact_connection_credentials(
            "Saving profile: save_password=true, has_password=false",
            "[REDACTED]",
        );
        assert_eq!(
            redacted,
            "Saving profile: save_password=true, has_password=false"
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn redacts_urls_inside_free_text() {
        let (output, count) = redact_uri_credentials(
            "Failed to connect using URI postgresql://u:pw@h:5432: refused; retry mysql://r:x@h2/db",
            "[REDACTED]",
        );

        assert_eq!(
            output,
            "Failed to connect using URI postgresql://u:[REDACTED]@h:5432: refused; retry mysql://r:[REDACTED]@h2/db"
        );
        assert_eq!(count, 2);
    }

    #[test]
    fn stops_at_quotes_and_ignores_schemeless_separators() {
        assert_eq!(
            redact(r#"{"uri":"redis://u:pw@h:6379","note":"a@b"}"#),
            r#"{"uri":"redis://u:***@h:6379","note":"a@b"}"#
        );
        assert_eq!(redact("://u:pw@h"), "://u:pw@h");
    }

    #[test]
    fn test_error_location_is_empty() {
        assert!(ErrorLocation::new().is_empty());
        assert!(!ErrorLocation::new().with_table("users").is_empty());
    }
}
