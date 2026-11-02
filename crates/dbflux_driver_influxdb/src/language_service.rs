//! Language service for InfluxDB's two query dialects (InfluxQL and Flux).
//!
//! `Connection::language_service()` carries no per-query context, unlike
//! `InfluxConnection::resolve_language` (`connection.rs`), which knows the
//! active dialect from the `QueryRequest`'s query mode. This service instead
//! sniffs the dialect straight from the query text: Flux scripts are
//! pipeline-shaped (`from(...) |> ...`), open with an `import` or an assignment,
//! everything else is treated as InfluxQL, matching the driver's own default for
//! both InfluxDB versions.

use dbflux_core::{
    DangerousQueryKind, Diagnostic, ExecutionClassification, LanguageService, ValidationResult,
};

use crate::influx_safety::{
    Dialect, classify_flux, classify_influxql, detect_dangerous_flux, detect_dangerous_influxql,
    detect_dialect,
};

pub struct InfluxLanguageService;

impl LanguageService for InfluxLanguageService {
    fn validate(&self, query: &str) -> ValidationResult {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return ValidationResult::Valid;
        }

        match unbalanced_delimiters(trimmed) {
            Some(message) => ValidationResult::SyntaxError(Diagnostic::error(message)),
            None => ValidationResult::Valid,
        }
    }

    fn detect_dangerous(&self, query: &str) -> Option<DangerousQueryKind> {
        match detect_dialect(query) {
            Dialect::InfluxQl => detect_dangerous_influxql(query),
            Dialect::Flux => detect_dangerous_flux(query),
        }
    }

    fn classify_execution(&self, query: &str) -> Option<ExecutionClassification> {
        Some(match detect_dialect(query) {
            Dialect::InfluxQl => classify_influxql(query),
            Dialect::Flux => classify_flux(query),
        })
    }
}

/// Check that parentheses/brackets/braces are balanced and every quoted
/// region is closed. This is a syntactic sanity check, not a parser: it
/// rejects obviously malformed input (an unterminated string, a stray closing
/// bracket) and accepts everything else, since neither InfluxQL nor Flux has
/// a bundled grammar in this codebase.
fn unbalanced_delimiters(query: &str) -> Option<String> {
    let mut stack: Vec<char> = Vec::new();
    let mut quote: Option<char> = None;

    for c in query.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }

        match c {
            '\'' | '"' => quote = Some(c),
            '(' | '[' | '{' => stack.push(c),
            ')' if stack.pop() != Some('(') => return Some("Unmatched ')'".to_string()),
            ']' if stack.pop() != Some('[') => return Some("Unmatched ']'".to_string()),
            '}' if stack.pop() != Some('{') => return Some("Unmatched '}'".to_string()),
            _ => {}
        }
    }

    if quote.is_some() {
        return Some("Unterminated string literal".to_string());
    }

    stack
        .pop()
        .map(|unclosed| format!("Unmatched '{unclosed}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== InfluxQL dangerous patterns ====================

    #[test]
    fn influxql_drop_database_is_dangerous() {
        assert_eq!(
            InfluxLanguageService.detect_dangerous("DROP DATABASE mydb"),
            Some(DangerousQueryKind::Drop)
        );
    }

    #[test]
    fn influxql_drop_measurement_is_dangerous() {
        assert_eq!(
            InfluxLanguageService.detect_dangerous("DROP MEASUREMENT cpu"),
            Some(DangerousQueryKind::Drop)
        );
    }

    #[test]
    fn influxql_drop_series_is_dangerous() {
        assert_eq!(
            InfluxLanguageService.detect_dangerous("DROP SERIES FROM cpu WHERE region = 'us-west'"),
            Some(DangerousQueryKind::Drop)
        );
    }

    #[test]
    fn influxql_drop_retention_policy_is_dangerous() {
        assert_eq!(
            InfluxLanguageService.detect_dangerous("DROP RETENTION POLICY \"weekly\" ON mydb"),
            Some(DangerousQueryKind::Drop)
        );
    }

    #[test]
    fn influxql_drop_shard_is_dangerous() {
        assert_eq!(
            InfluxLanguageService.detect_dangerous("DROP SHARD 1"),
            Some(DangerousQueryKind::Drop)
        );
    }

    #[test]
    fn influxql_delete_without_where_is_dangerous() {
        assert_eq!(
            InfluxLanguageService.detect_dangerous("DELETE FROM cpu"),
            Some(DangerousQueryKind::DeleteNoWhere)
        );
    }

    #[test]
    fn influxql_delete_with_where_is_safe() {
        assert_eq!(
            InfluxLanguageService
                .detect_dangerous("DELETE FROM cpu WHERE time < '2024-01-01T00:00:00Z'"),
            None
        );
    }

    #[test]
    fn influxql_select_is_safe() {
        assert_eq!(
            InfluxLanguageService.detect_dangerous("SELECT * FROM cpu WHERE time > now() - 1h"),
            None
        );
    }

    #[test]
    fn influxql_drop_word_inside_string_is_safe() {
        // "drop" appearing inside a string literal must not trigger the gate.
        let query = "SELECT * FROM cpu WHERE status = 'drop'";
        assert_eq!(InfluxLanguageService.detect_dangerous(query), None);
    }

    // ==================== Flux dangerous patterns ====================

    #[test]
    fn flux_bare_delete_call_is_dangerous() {
        let query =
            r#"delete(bucket: "b", start: 2024-01-01T00:00:00Z, stop: 2024-01-02T00:00:00Z)"#;
        assert_eq!(
            InfluxLanguageService.detect_dangerous(query),
            Some(DangerousQueryKind::DeleteNoWhere)
        );
    }

    #[test]
    fn flux_namespaced_delete_call_is_dangerous() {
        let query = r#"
import "influxdata/influxdb/v1"
influxdb.delete(bucket: "b", predicate: (r) => true, start: 2024-01-01T00:00:00Z, stop: 2024-01-02T00:00:00Z)
"#;
        assert_eq!(
            InfluxLanguageService.detect_dangerous(query),
            Some(DangerousQueryKind::DeleteNoWhere)
        );
    }

    #[test]
    fn flux_query_with_drop_word_in_string_is_safe() {
        let query =
            r#"from(bucket:"b") |> range(start: -1h) |> filter(fn: (r) => r.tag == "drop")"#;
        assert_eq!(InfluxLanguageService.detect_dangerous(query), None);
    }

    #[test]
    fn flux_identifier_containing_delete_is_not_a_call() {
        let query = r#"from(bucket:"b") |> filter(fn: (r) => r._field == "mydelete")"#;
        assert_eq!(InfluxLanguageService.detect_dangerous(query), None);
    }

    // ==================== classify_execution ====================

    #[test]
    fn influxql_select_classifies_as_read() {
        assert_eq!(
            InfluxLanguageService.classify_execution("SELECT * FROM cpu"),
            Some(ExecutionClassification::Read)
        );
    }

    #[test]
    fn influxql_show_classifies_as_metadata() {
        assert_eq!(
            InfluxLanguageService.classify_execution("SHOW MEASUREMENTS"),
            Some(ExecutionClassification::Metadata)
        );
    }

    #[test]
    fn influxql_delete_classifies_as_destructive() {
        assert_eq!(
            InfluxLanguageService.classify_execution("DELETE FROM cpu"),
            Some(ExecutionClassification::Destructive)
        );
    }

    #[test]
    fn influxql_insert_classifies_as_write() {
        assert_eq!(
            InfluxLanguageService.classify_execution("INSERT cpu,host=a value=1"),
            Some(ExecutionClassification::Write)
        );
    }

    #[test]
    fn flux_from_pipeline_classifies_as_read() {
        let query = r#"from(bucket:"b") |> range(start: -1h)"#;
        assert_eq!(
            InfluxLanguageService.classify_execution(query),
            Some(ExecutionClassification::Read)
        );
    }

    #[test]
    fn flux_delete_call_classifies_as_destructive() {
        let query =
            r#"delete(bucket: "b", start: 2024-01-01T00:00:00Z, stop: 2024-01-02T00:00:00Z)"#;
        assert_eq!(
            InfluxLanguageService.classify_execution(query),
            Some(ExecutionClassification::Destructive)
        );
    }

    // ==================== validate ====================

    #[test]
    fn empty_query_is_valid() {
        assert!(matches!(
            InfluxLanguageService.validate(""),
            ValidationResult::Valid
        ));
    }

    #[test]
    fn valid_influxql_has_no_syntax_error() {
        assert!(matches!(
            InfluxLanguageService.validate("SELECT * FROM cpu WHERE time > now() - 1h"),
            ValidationResult::Valid
        ));
    }

    #[test]
    fn valid_flux_has_no_syntax_error() {
        let query =
            r#"from(bucket:"b") |> range(start: -1h) |> filter(fn: (r) => r._field == "usage")"#;
        assert!(matches!(
            InfluxLanguageService.validate(query),
            ValidationResult::Valid
        ));
    }

    #[test]
    fn unbalanced_parens_is_syntax_error() {
        assert!(matches!(
            InfluxLanguageService.validate("SELECT * FROM cpu WHERE time IN (1, 2"),
            ValidationResult::SyntaxError(_)
        ));
    }

    #[test]
    fn unterminated_string_is_syntax_error() {
        assert!(matches!(
            InfluxLanguageService.validate("SELECT * FROM cpu WHERE status = 'open"),
            ValidationResult::SyntaxError(_)
        ));
    }

    #[test]
    fn stray_closing_paren_is_syntax_error() {
        assert!(matches!(
            InfluxLanguageService.validate("SELECT * FROM cpu)"),
            ValidationResult::SyntaxError(_)
        ));
    }
}
