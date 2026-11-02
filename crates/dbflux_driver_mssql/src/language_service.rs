use dbflux_core::{
    DangerousQueryKind, EditorDiagnostic, ExecutionClassification, LanguageService,
    ValidationResult, classify_tsql_execution, detect_dangerous_sql,
};

/// Language service for T-SQL (Microsoft SQL Server).
///
/// Behaves like the generic `SqlLanguageService` for dangerous-query detection
/// (the destructive keywords DROP/TRUNCATE/DELETE are spelled identically), but
/// suppresses live parse diagnostics entirely. The bundled `tree-sitter-sequel`
/// grammar follows generic ANSI SQL and flags T-SQL-only constructs as errors —
/// `SELECT TOP n`, `OUTPUT INSERTED.*`, `MERGE`, `CROSS APPLY / OUTER APPLY`,
/// table hints like `WITH (NOLOCK)`, `OFFSET … ROWS FETCH NEXT … ROWS ONLY`,
/// etc. The server is the source of truth for syntax validity; surfacing parser
/// false-positives in the editor only adds noise.
///
/// It also raises the shared execution class: a T-SQL batch needs no `;` between
/// statements and treats an unknown first word as a procedure call, so what looks
/// like a read from its first keyword can hold a write
/// ([`classify_tsql_execution`]).
pub struct TSqlLanguageService;

impl LanguageService for TSqlLanguageService {
    fn validate(&self, _query: &str) -> ValidationResult {
        ValidationResult::Valid
    }

    fn detect_dangerous(&self, query: &str) -> Option<DangerousQueryKind> {
        detect_dangerous_sql(query)
    }

    fn editor_diagnostics(&self, _query: &str) -> Vec<EditorDiagnostic> {
        Vec::new()
    }

    fn classify_execution(&self, query: &str) -> Option<ExecutionClassification> {
        Some(classify_tsql_execution(query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbflux_core::{QueryLanguage, classify_query_for_governance};

    /// Read-only profiles and MCP ask this question, with this service.
    #[test]
    fn governance_refuses_what_sql_server_would_run_behind_a_read() {
        let service = TSqlLanguageService;

        for sql in [
            "EXPLAIN DELETE FROM t WHERE id = 1",
            "DESCRIBE x DELETE FROM t WHERE id = 1",
            "SHOW x DELETE FROM t WHERE id = 1",
            "SHOW x; DELETE FROM t WHERE id = 1",
            "/* /* */ */ EXPLAIN DELETE FROM t WHERE id = 1",
            "SELECT 1 EXEC('DELETE FROM t WHERE id = 1')",
        ] {
            assert!(
                !matches!(
                    classify_query_for_governance(&QueryLanguage::Sql, sql, Some(&service)),
                    ExecutionClassification::Metadata | ExecutionClassification::Read
                ),
                "{sql}"
            );
        }

        assert_eq!(
            classify_query_for_governance(
                &QueryLanguage::Sql,
                "SELECT TOP 1 * FROM t WITH (NOLOCK)",
                Some(&service),
            ),
            ExecutionClassification::Read
        );
    }

    #[test]
    fn validate_returns_valid_for_any_input() {
        let svc = TSqlLanguageService;

        assert!(matches!(
            svc.validate("SELECT TOP 10 * FROM sys.tables"),
            ValidationResult::Valid
        ));
        assert!(matches!(svc.validate(""), ValidationResult::Valid));
        assert!(matches!(
            svc.validate("SELECT FROM WHERE"),
            ValidationResult::Valid
        ));
    }

    #[test]
    fn editor_diagnostics_returns_empty() {
        let svc = TSqlLanguageService;

        assert!(
            svc.editor_diagnostics("SELECT TOP 1 id FROM dbo.users WITH (NOLOCK)")
                .is_empty()
        );
        assert!(svc.editor_diagnostics("EXEC sp_helpdb 'master'").is_empty());
        assert!(svc.editor_diagnostics("").is_empty());
    }

    #[test]
    fn detect_dangerous_drops_table() {
        let svc = TSqlLanguageService;

        assert_eq!(
            svc.detect_dangerous("DROP TABLE dbo.orders"),
            Some(DangerousQueryKind::Drop)
        );
        assert_eq!(svc.detect_dangerous("SELECT * FROM dbo.orders"), None);
    }
}
