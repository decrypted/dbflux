use dbflux_policy::ExecutionClassification::{self, Admin, Destructive, Read, Write};

use super::{classify_tsql_execution, tsql_batch_has_implicit_call};

const D: &str = "DELETE FROM t WHERE id = 1";
const X: &str = "EXEC('DELETE FROM t WHERE id = 1')";

fn is_refused(class: ExecutionClassification) -> bool {
    !matches!(
        class,
        ExecutionClassification::Metadata | ExecutionClassification::Read
    )
}

/// Every text here ran its `DELETE` on a real SQL Server (see
/// `mssql_rating_refuses_what_the_server_runs`). `{X}` is the `EXEC` form, whose
/// `DELETE` sits in a string, so only the `EXEC` can give it away.
#[test]
fn what_the_server_runs_is_refused_however_it_is_spelled() {
    for shape in [
        "SELECT 1. {X}",
        "SELECT 1.e5 {X}",
        "SELECT 1.e5{X}",
        "SELECT 1e5{X}",
        "SELECT .5 {X}",
        "SELECT $1. {X}",
        "SELECT $1.{X}",
        "SELECT $1{X}",
        "SELECT $ {X}",
        "SELECT $.5{X}",
        "SELECT 0x1 {X}",
        "SELECT 0x {X}",
        "SELECT 1E {X}",
        "SELECT 1E{X}",
        "SELECT 1{D}",
        "SELECT 1.{D}",
        "SELECT $1.{D}",
        "SELECT $1{D}",
        "SELECT 0x1 {D}",
        "SELECT 1e5{D}",
        "SELECT .5{D}",
        "SELECT 'a'{X}",
        "SELECT 1 [a]{X}",
        "SELECT 1 \"a\"{X}",
        "SELECT N'a'{X}",
        "SELECT 'a'{D}",
        "SELECT 1 [a]{D}",
        "SELECT 1/*c*/{X}",
        "SELECT 1--c\n{X}",
        "SELECT 1--c\r{X}",
        "SELECT 1./*c*/{X}",
        "SELECT 1. /*c*/ {X}",
        "SELECT 1. --c\n{X}",
        "SELECT 1 /* /* */ */ {X}",
        "SELECT (1){X}",
        "(SELECT 1){X}",
        "SELECT 1 FROM # {X}",
        "SELECT 1. EXECUTE('{D}')",
        "SELECT 1. EXEC sp_executesql N'{D}'",
        "SELECT 1\u{00A0}{X}",
        "SELECT 1\u{200B}{X}",
        "SELECT 1\u{1}{X}",
        "SELECT 1.\u{00A0}{X}",
        "SELECT 1. USE master",
        "SELECT 1 USE master",
    ] {
        let sql = shape.replace("{X}", X).replace("{D}", D);
        assert!(is_refused(classify_tsql_execution(&sql)), "{sql:?}");
    }
}

#[test]
fn other_writes_rate_by_what_they_do() {
    for (sql, expected) in [
        ("SELECT 1. INSERT INTO t VALUES (9)", Write),
        ("SELECT 1.INSERT t VALUES (9)", Write),
        ("SELECT 1. UPDATE t SET id = 9", Write),
        ("SELECT 1. MERGE t AS a USING u ON 1 = 1", Write),
        ("SELECT 1. TRUNCATE TABLE t", Destructive),
        ("SELECT 1.DROP TABLE t", Destructive),
        ("SELECT 1 DROP TABLE t", Destructive),
        ("SHOW x; DROP TABLE t", Destructive),
        ("SELECT 1 KILL 55", Admin),
        ("SELECT 1. KILL 55", Admin),
        ("SELECT 1 SHUTDOWN", Admin),
        ("SELECT 1. EXEC xp_cmdshell 'dir'", Admin),
        ("SELECT 1 EXEC master.dbo.xp_cmdshell 'dir'", Admin),
        ("SELECT 1 CHECKPOINT", Admin),
        ("SELECT 1 SETUSER 'dbo'", Admin),
        ("SELECT 1 SET IDENTITY_INSERT t ON", Write),
        ("SELECT 1 DISABLE TRIGGER ALL ON t", Write),
        ("SELECT 1 IF 1 = 1 PRINT 'x'", Write),
        ("SELECT 1 WAITFOR DELAY '00:01'", Write),
        ("SELECT 1 COMMIT", Write),
        ("SELECT 1 {call dbo.p}", Write),
        ("SELECT 1; EXPLAIN DELETE FROM t", Write),
        ("EXPLAIN SELECT 1", Write),
        ("/* /* */ */ EXPLAIN SELECT 1", Write),
        ("--c\rSHOW x", Write),
        ("(DESCRIBE x)", Write),
        (
            "SELECT * FROM OPENROWSET(BULK 'x', SINGLE_BLOB) AS b",
            Admin,
        ),
        ("SELECT * FROM OPENQUERY(l, 'SELECT 1')", Write),
    ] {
        assert_eq!(classify_tsql_execution(sql), expected, "{sql:?}");
    }
}

/// A word the scanner cannot place rates up: a keyword behind a character it may
/// not split at as the server does, and text whose end it never found.
#[test]
fn what_cannot_be_told_rates_up() {
    for sql in [
        "SELECT 1 x\u{0345}KILL 55",
        "SELECT 1 \u{0661}KILL 55",
        "SELECT 'unterminated",
        "SELECT [unterminated",
        "SELECT 1 /* unterminated",
        "SELECT 1 --c\u{2028}KILL 55",
        "SELECT 1 --c\u{85}KILL 55",
        "SELECT probe.exec FROM probe",
        "SELECT 1 . KILL 55",
        "SELECT 1EXEC('x')",
    ] {
        assert!(is_refused(classify_tsql_execution(sql)), "{sql:?}");
    }
}

#[test]
fn reads_stay_reads() {
    for sql in [
        "SELECT 1",
        "SELECT 1;",
        "SELECT 1. AS a, .5 AS b, 1.e5 AS c, 1E AS d, $1.5 AS e, 0xFF AS f",
        "SELECT TOP 10 * FROM dbo.orders WITH (NOLOCK) ORDER BY id DESC",
        "SELECT [delete], [use], 'DROP TABLE t' AS [kill] FROM t",
        "SELECT \"delete\", N'KILL 55', 'it''s EXEC(''x'')' FROM t",
        "SELECT x.[exec], dbo.backup_log.id FROM x JOIN dbo.backup_log ON 1 = 1",
        "SELECT @begin, @@ROWCOUNT, #kill.id, ##use.id FROM #kill, ##use",
        "/* DELETE FROM t */ SELECT 1 -- DROP TABLE t",
        "/* /* KILL 55 */ EXEC('x') */ SELECT 1",
        "SELECT REPLACE(name, 'a', 'b'), LEFT(a, 1), RIGHT(a, 1) FROM t",
        "SELECT COALESCE(a, 1), NULLIF(a, 1), CONVERT(int, a), TRY_CONVERT(int, a) FROM t",
        "SELECT USER, CURRENT_USER, SESSION_USER, SYSTEM_USER, CURRENT_TIMESTAMP",
        "WITH c AS (SELECT 1 AS a) SELECT a FROM c",
        "SELECT a FROM t UNION ALL SELECT a FROM u EXCEPT SELECT a FROM v INTERSECT SELECT 1",
        "SELECT a FROM t WHERE EXISTS (SELECT 1) AND a IN (1, 2) OR a BETWEEN 1 AND 2",
        "SELECT a FROM t WHERE a LIKE 'x%' ESCAPE '!' AND b IS NOT NULL AND c > ALL (SELECT 1)",
        "SELECT CASE WHEN a = 1 THEN 2 ELSE 3 END FROM t CROSS JOIN u LEFT OUTER JOIN v ON 1 = 1",
        "SELECT a FROM t CROSS APPLY (VALUES (1)) AS v(b) FULL JOIN u ON 1 = 1",
        "SELECT a, COUNT(*) FROM t GROUP BY a HAVING COUNT(*) > 1 ORDER BY a ASC",
        "SELECT a, SUM(b) OVER (PARTITION BY a ORDER BY c ROWS BETWEEN CURRENT ROW AND 1 FOLLOWING) FROM t",
        "SELECT a FROM t ORDER BY a OFFSET 0 ROWS FETCH NEXT 5 ROWS ONLY",
        "SELECT TOP 10 PERCENT a FROM t TABLESAMPLE (10 PERCENT)",
        "SELECT * FROM t FOR SYSTEM_TIME AS OF '2026-01-01'",
        "SELECT * FROM t FOR SYSTEM_TIME FROM '2026-01-01' TO '2026-02-01'",
        "SELECT a FROM t FOR XML PATH('x')",
        "SELECT a FROM t FOR JSON AUTO",
        "SELECT * FROM OPENJSON(@j) WITH (a INT '$.a')",
        "SELECT p.* FROM t PIVOT (SUM(a) FOR b IN ([x], [y])) AS p",
        "SELECT p.* FROM t UNPIVOT (a FOR b IN ([x], [y])) AS p",
        "SELECT CAST(a AS NATIONAL CHARACTER VARYING(10)), CAST(b AS DOUBLE PRECISION) FROM t",
        "SELECT STRING_AGG(a, ',') WITHIN GROUP (ORDER BY a) FROM t",
        "SELECT a COLLATE Latin1_General_CI_AS, $IDENTITY, $ROWGUID FROM t",
        "SELECT * FROM t WITH (INDEX(ix), HOLDLOCK) WHERE CONTAINS(a, 'x')",
        "SELECT * FROM t OPTION (RECOMPILE)",
        "SELECT * FROM t OPTION (USE HINT('ENABLE_PARALLEL_PLAN_PREFERENCE'))",
        "SELECT * FROM t OPTION (RECOMPILE, USE HINT('X'))",
        "SELECT * FROM t OPTION (RECOMPILE,/*c*/use--c\nhint('X'))",
        "SELECT * FROM t OPTION (USE PLAN N'<x/>')",
        "SELECT 1 EXPLAIN, 2 SHOW",
    ] {
        assert_eq!(classify_tsql_execution(sql), Read, "{sql:?}");
    }
}

/// Only an option list makes `USE HINT` a hint. Anywhere else it is a `USE` of a
/// database of that name.
#[test]
fn use_counts_as_a_hint_only_inside_an_option_list() {
    for sql in [
        "SELECT 1 USE HINT",
        "SELECT 1 USE PLAN",
        "SELECT 1. USE HINT",
        "USE master",
        "SELECT (1) USE HINT",
        "SELECT 1 OPTION (RECOMPILE) USE master",
        "SELECT 1 OPTION (USE HINT('X')) USE master",
    ] {
        assert!(is_refused(classify_tsql_execution(sql)), "{sql:?}");
    }
}

#[test]
fn the_implicit_call_is_found_in_any_statement_behind_any_comment() {
    for sql in [
        "EXPLAIN SELECT 1",
        "explain select 1",
        "DESCRIBE t",
        "desc t",
        "SHOW TABLES",
        "/* /* */ */ EXPLAIN DELETE FROM t",
        "-- why\nSHOW x DELETE FROM t",
        "--c\rSHOW x DELETE FROM t",
        "SELECT 1; DESCRIBE x DELETE FROM t",
        "(EXPLAIN DELETE FROM t)",
    ] {
        assert!(tsql_batch_has_implicit_call(sql), "{sql:?}");
    }

    for sql in [
        "SELECT 1",
        "EXEC [SHOW]",
        "EXEC dbo.explain_me",
        "explain_me",
        "[EXPLAIN] 1",
        "SELECT a FROM t ORDER BY a DESC",
        "SELECT 'EXPLAIN'; SELECT 2",
        "",
    ] {
        assert!(!tsql_batch_has_implicit_call(sql), "{sql:?}");
    }
}
