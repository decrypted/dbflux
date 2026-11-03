use dbflux_core::{
    DangerousQueryKind, EditorDiagnostic, ExecutionClassification, LanguageService,
    ValidationResult, classify_tsql_execution, detect_dangerous_sql, tsql_drop_in_batch,
    tsql_write_behind_read,
};

/// Language service for T-SQL (Microsoft SQL Server).
///
/// Starts from the generic `SqlLanguageService` dangerous-query detection (the
/// destructive keywords DROP/TRUNCATE/DELETE are spelled identically), but
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
/// ([`classify_tsql_execution`]). A writable profile asks before running one
/// ([`tsql_write_behind_read`]), and before a `DROP` or `TRUNCATE` behind anything
/// else ([`tsql_drop_in_batch`]). A batch can call for several of these and the
/// shared check's kinds at once, so all of them are reported: routine scripts get
/// one of them silenced, which must not silence the others.
pub struct TSqlLanguageService;

impl LanguageService for TSqlLanguageService {
    fn validate(&self, _query: &str) -> ValidationResult {
        ValidationResult::Valid
    }

    fn detect_dangerous(&self, query: &str) -> Option<DangerousQueryKind> {
        self.detect_dangerous_all(query).into_iter().next()
    }

    fn detect_dangerous_all(&self, query: &str) -> Vec<DangerousQueryKind> {
        let mut kinds = Vec::new();

        for kind in [
            tsql_write_behind_read(query),
            tsql_drop_in_batch(query),
            detect_dangerous_sql(query),
        ]
        .into_iter()
        .flatten()
        {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }

        kinds
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
            "SELECT 1. EXEC('DELETE FROM t WHERE id = 1')",
            "SELECT 1DELETE FROM t WHERE id = 1",
            "SELECT 1 FROM # EXEC('DELETE FROM t WHERE id = 1')",
            "SELECT 1--c\rEXEC('DELETE FROM t WHERE id = 1')",
        ] {
            assert!(
                !matches!(
                    classify_query_for_governance(&QueryLanguage::Sql, sql, Some(&service)),
                    ExecutionClassification::Metadata | ExecutionClassification::Read
                ),
                "{sql}"
            );
        }

        for sql in [
            "SELECT TOP 1 * FROM t WITH (NOLOCK)",
            "SELECT * FROM sys.fn_get_audit_file('x*.sqlaudit', DEFAULT, DEFAULT)",
            "SELECT * FROM dbo.Conversation",
            "SELECT c.Dialog FROM dbo.Chat c",
            "SELECT * FROM dbo.Call",
            "SELECT id AS Gehäuse FROM t",
        ] {
            assert_eq!(
                classify_query_for_governance(&QueryLanguage::Sql, sql, Some(&service)),
                ExecutionClassification::Read,
                "{sql}"
            );
        }
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

    /// Run on a writable profile: the editor asks when this answers.
    #[test]
    fn a_write_behind_a_read_needs_a_confirmation_and_a_plain_one_keeps_its_rules() {
        let service = TSqlLanguageService;

        for (sql, word) in [
            ("SELECT 1 DELETE FROM t WHERE id = 1", "DELETE"),
            ("SELECT 1 EXEC('DELETE FROM t WHERE id = 1')", "EXEC"),
            ("SELECT 1. EXEC('DELETE FROM t WHERE id = 1')", "EXEC"),
            ("SELECT 1DELETE FROM t WHERE id = 1", "DELETE"),
            ("SELECT 1 UPDATE t SET a = 1 WHERE id = 1", "UPDATE"),
            ("SELECT 1 INSERT t VALUES (1)", "INSERT"),
            // These get no pass from another kind's settings: "Don't ask again" on
            // a script or `Require WHERE` off would otherwise silence them.
            ("SELECT 1; DROP TABLE t", "DROP"),
            ("SELECT 1 DROP TABLE u; DELETE FROM t", "DROP"),
            ("WITH c AS (SELECT 1 AS a) DELETE FROM t", "DELETE"),
        ] {
            assert_eq!(
                service.detect_dangerous(sql),
                Some(DangerousQueryKind::WriteBehindRead { word: Some(word) }),
                "{sql}"
            );
        }

        for (sql, expected) in [
            ("DELETE FROM t WHERE id = 1", None),
            ("UPDATE t SET a = 1 WHERE id = 1", None),
            ("SELECT TOP 1 * FROM t WITH (NOLOCK)", None),
            ("SELECT [delete], 'EXEC(x)' FROM t", None),
            ("DELETE FROM t", Some(DangerousQueryKind::DeleteNoWhere)),
            ("DROP TABLE t", Some(DangerousQueryKind::Drop)),
            (
                "DROP TABLE t; DELETE FROM u",
                Some(DangerousQueryKind::Drop),
            ),
            ("TRUNCATE TABLE t", Some(DangerousQueryKind::Truncate)),
            (
                "ALTER TABLE t DROP COLUMN a",
                Some(DangerousQueryKind::Alter),
            ),
            (
                "DELETE FROM t WHERE id = 1 DROP TABLE u",
                Some(DangerousQueryKind::Drop),
            ),
            ("DELETE FROM t DROP TABLE u", Some(DangerousQueryKind::Drop)),
            (
                "INSERT t VALUES (3) DROP TABLE u",
                Some(DangerousQueryKind::Drop),
            ),
            (
                "UPDATE t SET a = 1 WHERE id = 1 TRUNCATE TABLE u",
                Some(DangerousQueryKind::Truncate),
            ),
            (
                "DECLARE @a INT DROP TABLE u",
                Some(DangerousQueryKind::Drop),
            ),
            (
                "SET NOCOUNT ON DROP TABLE u",
                Some(DangerousQueryKind::Drop),
            ),
            ("IF 1 = 1 DROP TABLE u", Some(DangerousQueryKind::Drop)),
            ("EXEC sp_who DROP TABLE u", Some(DangerousQueryKind::Drop)),
            ("sp_who DROP TABLE u", Some(DangerousQueryKind::Drop)),
            // A second write of a lesser kind behind the first gets no prompt.
            (
                "DELETE FROM t WHERE id = 1 DELETE FROM u WHERE id = 2",
                None,
            ),
            ("INSERT t VALUES (3) DELETE FROM u WHERE id = 2", None),
            (
                "DELETE FROM t WHERE [drop] = 1 AND a = 'DROP TABLE u' -- DROP TABLE u",
                None,
            ),
            ("INSERT t VALUES ('TRUNCATE TABLE u') /* DROP */", None),
        ] {
            assert_eq!(service.detect_dangerous(sql), expected, "{sql}");
        }
    }

    #[test]
    fn every_kind_a_batch_calls_for_is_reported() {
        use DangerousQueryKind::{DeleteNoWhere, Drop, Script, Truncate};

        let service = TSqlLanguageService;
        let hidden = |word| DangerousQueryKind::WriteBehindRead { word: Some(word) };

        for (sql, expected) in [
            (
                "WITH d AS (SELECT id FROM orders GROUP BY id HAVING COUNT(*) > 1) DELETE FROM orders",
                vec![hidden("DELETE"), DeleteNoWhere],
            ),
            (
                "SELECT COUNT(*) FROM t; DROP TABLE t",
                vec![hidden("DROP"), Drop, Script],
            ),
            (
                "DROP TABLE IF EXISTS #stage; DELETE FROM orders",
                vec![Drop, Script],
            ),
            ("DELETE FROM t DROP TABLE u", vec![Drop, DeleteNoWhere]),
            (
                "DELETE FROM t TRUNCATE TABLE u",
                vec![Truncate, DeleteNoWhere],
            ),
            ("SELECT a INTO #w FROM orders", vec![hidden("INTO")]),
            ("DROP TABLE t", vec![Drop]),
            ("DELETE FROM t WHERE id = 1", vec![]),
            ("SELECT 1", vec![]),
        ] {
            assert_eq!(service.detect_dangerous_all(sql), expected, "{sql}");
            assert_eq!(
                service.detect_dangerous(sql),
                expected.first().copied(),
                "{sql}"
            );
        }
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
