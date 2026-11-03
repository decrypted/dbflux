use dbflux_core::ExecutionClassification::{self, Admin, Destructive, Metadata, Read, Write};
use dbflux_core::{DangerousQueryKind, LanguageService};

use super::{Dialect, detect_dialect};
use crate::language_service::InfluxLanguageService;

fn rate(query: &str) -> ExecutionClassification {
    InfluxLanguageService
        .classify_execution(query)
        .expect("this driver rates everything")
}

fn is_refused(class: ExecutionClassification) -> bool {
    !matches!(class, Metadata | Read)
}

/// `SELECT ... INTO` wrote on InfluxDB 1.8 in each of these, or would have if its
/// shape compiled. `/'/` is a regex holding a quote, and a scan that only knows
/// strings reads everything up to the next quote as one literal.
#[test]
fn influxql_into_is_a_write_however_it_is_spelled() {
    for sql in [
        "SELECT * INTO hit FROM cpu GROUP BY *",
        "select * into hit from cpu group by *",
        "SeLeCt\t*\nInTo\nhit\nFROM\tcpu GROUP BY *",
        "SELECT*INTO hit FROM cpu GROUP BY *",
        "SELECT/*c*/*/*c*/INTO/*c*/hit/*c*/FROM cpu",
        "-- c\nSELECT * INTO hit FROM cpu GROUP BY *",
        "/* c */ SELECT * INTO hit FROM cpu GROUP BY *",
        "SELECT value INTO \"hit\" FROM cpu",
        "SELECT value INTO d.autogen.hit FROM cpu",
        "SELECT value INTO d.\"autogen\".\"hit\" FROM cpu",
        "SELECT value INTO hit FROM (SELECT value FROM cpu)",
        "SELECT value INTO hit FROM cpu WHERE host = 'a\\'b' OR host = 'a'",
        "SELECT value FROM cpu WHERE host = 'x\\'' ; SELECT value INTO hit FROM cpu",
        "SELECT value FROM /'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value FROM cpu, /'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value FROM d.autogen./'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT /'/ FROM cpu ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value, /'/ FROM cpu ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT mean(/'/) FROM cpu ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value FROM cpu WHERE host =~ /'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value FROM cpu WHERE host !~ /'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value FROM cpu GROUP BY /'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SHOW SERIES FROM /'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SHOW TAG VALUES WITH KEY =~ /'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value FROM cpu --c\r; SELECT value INTO hit FROM cpu",
        // A `|>` in a string or comment does not make it Flux.
        "SELECT value INTO hit FROM cpu WHERE host = '|>'",
        "-- |>\nSELECT value INTO hit FROM cpu",
        "SELECT value INTO hit FROM(SELECT value FROM cpu) WHERE host = '|>'",
        // Did not write when measured, and has no business being called a read.
        "EXPLAIN ANALYZE SELECT value INTO hit FROM cpu",
        "SELECT value*1INTO hit FROM cpu",
    ] {
        assert_eq!(detect_dialect(sql), Dialect::InfluxQl, "{sql:?}");
        assert!(is_refused(rate(sql)), "{sql:?}");
    }
}

#[test]
fn influxql_statements_rate_by_what_they_do() {
    for (sql, expected) in [
        ("DROP MEASUREMENT cpu", Destructive),
        ("drop\tmeasurement cpu", Destructive),
        ("DROP\nMEASUREMENT cpu", Destructive),
        ("DROP/**/MEASUREMENT cpu", Destructive),
        ("-- c\nDROP MEASUREMENT cpu", Destructive),
        ("/* c */ DROP MEASUREMENT cpu", Destructive),
        ("DROP DATABASE d", Destructive),
        ("DROP SERIES FROM cpu", Destructive),
        ("DROP SHARD 1", Destructive),
        ("DROP RETENTION POLICY rp ON d", Destructive),
        ("DROP CONTINUOUS QUERY cq ON d", Destructive),
        ("DROP SUBSCRIPTION s ON d.autogen", Destructive),
        ("DROP USER u", Destructive),
        ("DELETE FROM cpu", Destructive),
        ("DELETE WHERE time < now()", Destructive),
        (
            "ALTER RETENTION POLICY autogen ON d DURATION 1h",
            Destructive,
        ),
        ("CREATE DATABASE d", Admin),
        (
            "CREATE RETENTION POLICY rp ON d DURATION 1h REPLICATION 1",
            Admin,
        ),
        (
            "CREATE CONTINUOUS QUERY cq ON d BEGIN SELECT mean(value) INTO hit FROM cpu GROUP BY time(1h) END",
            Admin,
        ),
        ("CREATE USER u WITH PASSWORD 'x'", Admin),
        (
            "CREATE SUBSCRIPTION s ON d.autogen DESTINATIONS ALL 'http://x'",
            Admin,
        ),
        ("SET PASSWORD FOR u = 'x'", Admin),
        ("GRANT ALL TO u", Admin),
        ("REVOKE ALL FROM u", Admin),
        ("KILL QUERY 1", Admin),
        ("INSERT cpu value=2", Write),
        // Several statements take the strongest.
        ("SELECT value FROM cpu; DROP MEASUREMENT cpu", Destructive),
        ("SHOW MEASUREMENTS; DROP MEASUREMENT cpu", Destructive),
        ("SHOW MEASUREMENTS;KILL QUERY 1", Admin),
        (
            "SELECT value FROM /'/ ; DROP MEASUREMENT cpu ; SELECT value FROM /'/",
            Destructive,
        ),
    ] {
        assert_eq!(rate(sql), expected, "{sql:?}");
    }
}

#[test]
fn influxql_reads_stay_reads() {
    for sql in [
        "SELECT * FROM cpu",
        "select value from cpu where time > now() - 1h",
        "SELECT \"into\" FROM cpu",
        "SELECT value FROM \"into\"",
        "SELECT value AS \"into\" FROM cpu",
        "SELECT value FROM cpu WHERE host = 'INTO hit'",
        "SELECT value FROM cpu WHERE host = 'DROP MEASUREMENT cpu'",
        "SELECT value FROM cpu WHERE host = 'it\\'s; DROP DATABASE d'",
        "SELECT value FROM cpu -- INTO hit",
        "SELECT value FROM cpu -- ; DROP DATABASE d",
        "/* DROP DATABASE d */ SELECT value FROM cpu",
        "SELECT value / 2 FROM /cpu/",
        "SELECT value FROM /cpu|mem/",
        "SELECT value FROM /into/",
        "SELECT value FROM /a\\/b/",
        "SELECT value FROM cpu WHERE host =~ /into|drop/",
        "SELECT value FROM cpu GROUP BY /ho/",
        "SELECT mean(value) FROM cpu GROUP BY time(1h) fill(0)",
        "SELECT value FROM (SELECT value FROM cpu)",
        "SELECT to_x FROM cpu",
        "SELECT value FROM cpu; SELECT value FROM mem",
        "SELECT value FROM cpu WHERE host = '|>'",
        "SELECT value FROM cpu WHERE host = 'to(bucket: \"x\")'",
    ] {
        assert_eq!(rate(sql), Read, "{sql:?}");
    }

    for sql in [
        "SHOW MEASUREMENTS",
        "show databases",
        "SHOW SERIES",
        "SHOW QUERIES",
        "SHOW TAG VALUES WITH KEY = \"host\"",
        "SHOW MEASUREMENTS; SHOW SERIES",
        "EXPLAIN SELECT value FROM cpu",
        "EXPLAIN ANALYZE SELECT value FROM cpu",
        "",
        "   ",
        "-- only a comment",
        "/* only a comment */",
    ] {
        assert_eq!(rate(sql), Metadata, "{sql:?}");
    }
}

#[test]
fn influxql_rates_up_what_it_cannot_place() {
    for sql in [
        "SELECT to FROM cpu",
        "SHOW MEASUREMENTS to",
        "SELECT value FROM cpu WHERE host = 'open",
        "SELECT value FROM \"open",
        "SELECT value FROM /open",
        "SELECT value FROM cpu /* open",
        "something else",
        "(SELECT value FROM cpu)",
        "select = 1",
        "--1\nselect = to",
    ] {
        assert!(is_refused(rate(sql)), "{sql:?}");
    }
}

#[test]
fn influxql_confirmation_finds_the_statement_however_it_is_spelled() {
    use DangerousQueryKind::{Alter, DeleteNoWhere, Drop, Script};

    for (sql, expected) in [
        ("DROP MEASUREMENT cpu", Some(Drop)),
        ("drop\tmeasurement cpu", Some(Drop)),
        ("DROP\nMEASUREMENT cpu", Some(Drop)),
        ("DROP/**/MEASUREMENT cpu", Some(Drop)),
        ("-- c\nDROP MEASUREMENT cpu", Some(Drop)),
        ("/* c */ DROP MEASUREMENT cpu", Some(Drop)),
        ("DELETE FROM cpu", Some(DeleteNoWhere)),
        ("-- c\nDELETE FROM cpu", Some(DeleteNoWhere)),
        (
            "ALTER RETENTION POLICY autogen ON d DURATION 1h",
            Some(Alter),
        ),
        ("SELECT value FROM cpu; DROP MEASUREMENT cpu", Some(Script)),
        ("SHOW MEASUREMENTS; DELETE FROM cpu", Some(Script)),
        (
            "SELECT value FROM /'/ ; DROP MEASUREMENT cpu ; SELECT value FROM /'/",
            Some(Script),
        ),
        ("DELETE FROM cpu WHERE time < now()", None),
        (
            "SELECT value FROM cpu WHERE host = 'DROP MEASUREMENT cpu'",
            None,
        ),
        (
            "SELECT value FROM cpu; DELETE FROM cpu WHERE time < now()",
            None,
        ),
        ("-- DROP MEASUREMENT cpu\nSELECT value FROM cpu", None),
        ("SHOW MEASUREMENTS", None),
    ] {
        assert_eq!(
            InfluxLanguageService.detect_dangerous(sql),
            expected,
            "{sql:?}"
        );
    }
}

/// Each of these ran on InfluxDB 2.7 and wrote points or sent the request, unless
/// its note says it only reached the point of connecting.
#[test]
fn flux_that_acts_is_a_write_however_it_is_reached() {
    for flux in [
        r#"from(bucket:"b") |> range(start:0) |> to(bucket:"hit")"#,
        r#"from(bucket:"b")|>range(start:0)|>to(bucket:"hit")"#,
        r#"from(bucket:"b") |> range(start:0) |> to (bucket:"hit")"#,
        "from(bucket:\"b\") |> range(start:0) |> to(\nbucket:\"hit\")",
        "from(bucket:\"b\") |> range(start:0) // it's\n |> to(bucket:\"hit\")",
        // A quote in a regex must not read as opening a string.
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host =~ /"/ or true) |> to(bucket:"hit") |> filter(fn:(r)=> r.host =~ /"/)"#,
        r#"to(tables: from(bucket:"b") |> range(start:0), bucket:"hit")"#,
        "t = from(bucket:\"b\") |> range(start:0)\nto(tables: t, bucket:\"hit\")",
        "f = to\nfrom(bucket:\"b\") |> range(start:0) |> f(bucket:\"hit\")",
        "select = to\nselect(tables: from(bucket:\"b\") |> range(start:0), bucket:\"hit\")",
        "--1\nselect = to\nselect(tables: from(bucket:\"b\") |> range(start:0), bucket:\"hit\")",
        // Packages, under their own names and others.
        "import \"experimental\"\nfrom(bucket:\"b\") |> range(start:0) |> experimental.to(bucket:\"hit\")",
        "import e \"experimental\"\nfrom(bucket:\"b\") |> range(start:0) |> e.to(bucket:\"hit\")",
        "import \"influxdata/influxdb\"\nfrom(bucket:\"b\") |> range(start:0) |> influxdb.to(bucket:\"hit\")",
        "import \"influxdata/influxdb\"\nfrom(bucket:\"b\") |> range(start:0) |> influxdb.wideTo(bucket:\"hit\")",
        "import \"influxdata/influxdb\"\nf = influxdb.to\nfrom(bucket:\"b\") |> range(start:0) |> f(bucket:\"hit\")",
        "import \"influxdata/influxdb\"\nm = influxdb\nfrom(bucket:\"b\") |> range(start:0) |> m.to(bucket:\"hit\")",
        "import \"influxdata/influxdb\"\nfrom(bucket:\"b\") |> range(start:0) |> influxdb[\"to\"](bucket:\"hit\")",
        "import \"http\"\nhttp.post(url:\"u\", data: bytes(v:\"x\"))",
        "import \"http\"\nx = http.post(url:\"u\", data: bytes(v:\"x\"))\nfrom(bucket:\"b\") |> range(start:0)",
        "import \"http\"\nf = http.post\nfrom(bucket:\"b\") |> range(start:0) |> map(fn:(r)=>({r with s: f(url:\"u\", data: bytes(v:\"x\"))}))",
        "import h \"http\"\nfrom(bucket:\"b\") |> range(start:0) |> map(fn:(r)=>({r with s: h.post(url:\"u\", data: bytes(v:\"x\"))}))",
        "import \"http\"\ne = http.endpoint(url:\"u\")\nfrom(bucket:\"b\") |> range(start:0) |> e(mapFn:(r)=>({headers:{}, data: bytes(v:\"x\")}))()",
        // A string's `${...}` is code.
        "import \"http\"\nimport \"array\"\narray.from(rows:[{a:\"${string(v: http.post(url:\"u\", data: bytes(v:\"x\")))}\"}])",
        "import \"http/requests\"\nimport \"array\"\nr = requests.post(url:\"u\")\narray.from(rows:[{a:r.statusCode}])",
        "import \"http/requests\"\nimport \"array\"\nr = requests.get(url:\"u\")\narray.from(rows:[{a:r.statusCode}])",
        "import \"http/requests\"\nimport \"array\"\nr = requests.do(method:\"DELETE\", url:\"u\")\narray.from(rows:[{a:r.statusCode}])",
        "import \"experimental/http\"\nimport \"array\"\nr = http.get(url:\"u\")\narray.from(rows:[{a:r.statusCode}])",
        "import \"experimental/csv\"\ncsv.from(url:\"u\")",
        "import \"experimental/prometheus\"\nprometheus.scrape(url:\"u\")",
        "import \"slack\"\nimport \"array\"\ns = slack.message(url:\"u\", token:\"t\", channel:\"c\", text:\"x\", color:\"good\")\narray.from(rows:[{a:s}])",
        "import \"pushbullet\"\nimport \"array\"\ns = pushbullet.pushNote(url:\"u\", token:\"t\", title:\"t\", text:\"x\")\narray.from(rows:[{a:s}])",
        "import \"influxdata/influxdb/monitor\"\nfrom(bucket:\"b\") |> range(start:0) |> monitor.check(crit:(r)=>true, messageFn:(r)=>\"m\", data:{})",
        // These got as far as connecting to the unreachable address they were given.
        "import \"sql\"\nfrom(bucket:\"b\") |> range(start:0) |> sql.to(driverName:\"postgres\", dataSourceName:\"d\", table:\"t\")",
        "import \"sql\"\nsql.from(driverName:\"postgres\", dataSourceName:\"d\", query:\"DELETE FROM t\")",
        "import \"kafka\"\nfrom(bucket:\"b\") |> range(start:0) |> kafka.to(brokers:[\"b\"], topic:\"t\")",
        "import \"experimental/mqtt\"\nfrom(bucket:\"b\") |> range(start:0) |> mqtt.to(broker:\"b\", topic:\"t\")",
        "import \"experimental/mqtt\"\nimport \"array\"\ns = mqtt.publish(broker:\"b\", topic:\"t\", message:\"x\")\narray.from(rows:[{a:s}])",
        // From reading the package, as these need a service to run against.
        "import \"pagerduty\"\nfrom(bucket:\"b\") |> range(start:0) |> map(fn:(r)=>({r with s: pagerduty.sendEvent()}))",
        "import \"experimental/influxdb\"\nimport \"array\"\nr = influxdb.api(method:\"post\", path:\"/x\")\narray.from(rows:[{a:r.statusCode}])",
        "import \"contrib/chobbs/discord\"\nfrom(bucket:\"b\") |> range(start:0) |> map(fn:(r)=>({r with s: discord.send()}))",
        "import \"contrib/sranka/teams\"\nfrom(bucket:\"b\") |> teams.endpoint(url:\"u\")",
        "import \"contrib/qxip/clickhouse\"\nclickhouse.query(query:\"DROP TABLE t\")",
        "import \"contrib/bonitoo-io/tickscript\"\nfrom(bucket:\"b\") |> tickscript.alert()",
        "import \"influxdata/influxdb/monitor\"\nfrom(bucket:\"b\") |> monitor.notify(endpoint: e, data: {})",
        // Not a package of today's Flux, so nothing says what it does.
        "import \"somewhere/else\"\nfrom(bucket:\"b\") |> range(start:0)",
    ] {
        assert_eq!(detect_dialect(flux), Dialect::Flux, "{flux:?}");
        assert_eq!(rate(flux), Write, "{flux:?}");
    }
}

/// A write with no pipeline and no `import` still reads as InfluxQL, whose first
/// word then says it is not a read.
#[test]
fn flux_with_no_pipeline_is_not_a_read_either() {
    for flux in [
        r#"to(bucket:"hit")"#,
        r#"to(tables: t, bucket:"hit")"#,
        "// it's\nbuckets() |> to(bucket:\"hit\")",
    ] {
        assert!(is_refused(rate(flux)), "{flux:?}");
    }
}

#[test]
fn flux_delete_is_destructive() {
    for flux in [
        r#"from(bucket:"b") |> delete(start: 0)"#,
        "import \"influxdata/influxdb/v1\"\ninfluxdb.delete(bucket: \"b\", predicate: (r) => true)",
    ] {
        assert_eq!(rate(flux), Destructive, "{flux:?}");
        assert_eq!(
            InfluxLanguageService.detect_dangerous(flux),
            Some(DangerousQueryKind::DeleteNoWhere),
            "{flux:?}"
        );
    }
}

/// Each of these ran on InfluxDB 2.7 and wrote nothing.
#[test]
fn flux_reads_stay_reads() {
    for flux in [
        r#"from(bucket:"b") |> range(start: -1h)"#,
        r#"from (bucket:"b") |> range(start: -1h)"#,
        // A column, a key or a string named `to`.
        r#"from(bucket:"b") |> range(start:0) |> map(fn:(r)=>({r with x: r.to}))"#,
        r#"from(bucket:"b") |> range(start:0) |> map(fn:(r)=>({r with to: "x"}))"#,
        r#"from(bucket:"b") |> range(start:0) |> map(fn:(r)=>({to: r._value}))"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r["to"] == "x")"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r._field == "to(")"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r._field == "to(bucket:\"hit\")")"#,
        r#"from(bucket:"b") |> range(start:0) |> rename(columns:{to:"x"})"#,
        r#"from(bucket:"b") |> range(start:0) |> keep(columns:["to","_value"])"#,
        r#"from(bucket:"b") |> range(start:0) // |> to(bucket:"hit")"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host =~ /to\(/ or true)"#,
        "// to(bucket: \"hit\")\nfrom(bucket:\"b\") |> range(start:0)",
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> "${r.host}" == "a")"#,
        r#"from(bucket:"b") |> range(start:0) |> map(fn:(r)=>({r with y: r._value / 2.0}))"#,
        // A word in a string is not the call, and `tomorrow` is not `to`.
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.a == "http.post(")"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.tomorrow == 1)"#,
        r#"from(bucket:"b") |> range(start:0) |> toFloat()"#,
        // Read-only members of the packages that also hold a write.
        "import \"influxdata/influxdb\"\ninfluxdb.from(bucket:\"b\") |> range(start:0)",
        "import \"influxdata/influxdb/monitor\"\nmonitor.from(start:0)",
        "import \"influxdata/influxdb/monitor\"\nfrom(bucket:\"b\") |> range(start:0) |> monitor.deadman(t:now())",
        "import \"influxdata/influxdb/tasks\"\nimport \"array\"\narray.from(rows:[{a:tasks.lastSuccess(orTime:now())}])",
        "import \"experimental\"\nfrom(bucket:\"b\") |> range(start:0) |> experimental.alignTime()",
        "import \"http\"\nfrom(bucket:\"b\") |> range(start:0) |> map(fn:(r)=>({r with s: http.pathEscape(inputString:r.host)}))",
        "import \"csv\"\ncsv.from(csv:\"a\")",
        "import \"array\"\narray.from(rows:[{a:1}])",
        "import \"strings\"\nfrom(bucket:\"b\") |> range(start:0) |> map(fn:(r)=>({r with s: strings.toUpper(v:r.host)}))",
    ] {
        assert_eq!(rate(flux), Read, "{flux:?}");
    }
}

#[test]
fn flux_rates_up_what_it_cannot_place() {
    for flux in [
        r#"from(bucket:"open) |> range(start:0)"#,
        r#"from(bucket:"b") |> filter(fn:(r)=> r.a =~ /open)"#,
        r#"from(bucket:"b") |> filter(fn:(r)=> "${r.a" == "x")"#,
        // No source this scan knows of.
        r#"buckets() |> limit(n:1)"#,
        "x = 1",
    ] {
        assert_eq!(rate(flux), Write, "{flux:?}");
    }
}

#[test]
fn the_dialect_comes_from_what_each_language_would_run() {
    for query in [
        r#"from(bucket:"b")"#,
        r#"from(bucket:"b") |> range(start:0)"#,
        "import \"array\"\narray.from(rows:[{a:1}])",
        "import a \"array\"\na.from(rows:[{a:1}])",
        "x = 1",
        "// c\nfrom(bucket:\"b\")",
        "option task = {name: \"t\"}\nfrom(bucket:\"b\")",
        "package main\nimport \"array\"",
    ] {
        assert_eq!(detect_dialect(query), Dialect::Flux, "{query:?}");
    }

    for query in [
        "SELECT * FROM cpu",
        "SELECT a FROM b WHERE c = '|>'",
        "-- |>\nSELECT a FROM b",
        "SHOW DATABASES",
        "DROP DATABASE d",
    ] {
        assert_eq!(detect_dialect(query), Dialect::InfluxQl, "{query:?}");
    }
}

/// `--` is a comment in InfluxQL and two minus signs in Flux. Each of these ran on
/// InfluxDB 2.7 and wrote, while the InfluxQL reading saw a comment.
#[test]
fn what_a_dash_comment_hides_counts_if_it_could_run_as_flux() {
    for flux in [
        r#"--1 from(bucket:"b") |> range(start:0) |> to(bucket:"hit")"#,
        "--1 show = 1 from(bucket:\"b\") |> range(start:0) |> to(bucket:\"hit\")\nshow",
        "--1 x = 1\nfrom(bucket:\"b\") |> range(start:0) |> to(bucket:\"hit\")",
        r#"--1 to(tables: t, bucket:"hit")"#,
        r#"--1 {f: to}.f(tables: range(tables: from(bucket:"b"), start: 0), bucket:"hit")"#,
        // A lone `\r` ends the comment in InfluxQL only, so Flux's `//` skips the
        // statement InfluxQL reads next, and the line after it runs.
        "--1 //\rSELECT value FROM cpu\nto(tables: range(tables: from(bucket:\"b\"), start: 0), bucket:\"hit\")",
        "--1 //\rSHOW SERIES\nto(tables: range(tables: from(bucket:\"b\"), start: 0), bucket:\"hit\")",
        "--//\rSELECT value FROM cpu\n1\nto(tables: range(tables: from(bucket:\"b\"), start: 0), bucket:\"hit\")",
        "--1 //\rSELECT value FROM cpu\nfrom(bucket:\"b\", host:\"http://x\", token:\"t\")",
        // InfluxQL has no read with an unquoted `to`, whatever the comment says.
        "--1 //\nSELECT value FROM cpu\nto(tables: t, bucket:\"hit\")",
        "SELECT value FROM cpu\nto(tables: t, bucket:\"hit\")",
        // Nothing here ends the line in either language, so all of it is one comment
        // to InfluxQL too, and the next line opens with `to`.
        "--1 //\u{2028}SELECT value FROM cpu\nto(tables: t, bucket:\"hit\")",
        "--1 //\u{85}SELECT value FROM cpu\nto(tables: t, bucket:\"hit\")",
        "--1 //\u{c}SELECT value FROM cpu\nto(tables: t, bucket:\"hit\")",
        "--1 //\u{b}SELECT value FROM cpu\nto(tables: t, bucket:\"hit\")",
        "--1 \u{c}to(tables: t, bucket:\"hit\")",
        "--1 \u{b}to(tables: t, bucket:\"hit\")",
        "--1 \rto(tables: t, bucket:\"hit\")",
        // Flux reads the comment's `"` as opening a string that takes the next line,
        // so what InfluxQL reads as a comment after `SHOW SERIES` is code.
        "--1 \"\nSHOW SERIES -- \" to(tables: range(tables: from(bucket:\"b\"), start: 0), bucket:\"hit\") \"x\"",
        // Did not compile when measured, as an import must come first.
        "--1\nimport \"http\"\nx = http.post(url:\"u\", data: bytes(v:\"x\"))",
    ] {
        assert_eq!(rate(flux), Write, "{flux:?}");
    }

    // It cannot be told which language runs a `|>` behind a `--`.
    assert_eq!(rate("-- |>\nSELECT value FROM cpu"), Write);

    assert_eq!(
        InfluxLanguageService.detect_dangerous("--1 from(bucket:\"b\") |> delete(start:0)"),
        Some(DangerousQueryKind::DeleteNoWhere)
    );

    for comment in [
        "-- rollup to hourly\nSELECT value FROM cpu",
        "SELECT value FROM cpu -- send to ops",
        "SELECT value FROM cpu -- INTO hit",
        "SELECT value FROM cpu -- ; DROP DATABASE d",
        // A predicate lexes as Flux as well, and must not make the comment a program.
        "SELECT mean(value) FROM cpu WHERE time >= now() - 1h -- compare to yesterday",
        "-- rollup to hourly\nSELECT mean(value) FROM cpu WHERE host = 'a' GROUP BY time(1h)",
        "SELECT * FROM cpu WHERE host = 'a' -- owner ops@example.com",
        "SELECT * FROM cpu WHERE host != 'a' AND host =~ /b/ -- a 5\" disk",
        "-- a = b\nSELECT value FROM cpu WHERE host = 'a'",
        // Line ends that hide nothing, also in a file from an old editor.
        "-- old mac line\rSELECT value FROM cpu WHERE host = 'a'",
        "-- windows line\r\nSELECT value FROM cpu WHERE host = 'a'",
        "-- rollup to hourly\rSELECT value FROM cpu WHERE host != 'a'",
        "SELECT value FROM cpu -- a\u{2028}b\u{85}c\u{c}d",
    ] {
        assert_eq!(rate(comment), Read, "{comment:?}");
    }
    assert_eq!(rate("-- a note to self"), Metadata);
}

/// Each of these wrote on InfluxDB 1.8. A wrong reading of where a regex opens or
/// ends leaves the middle statement inside what looks like a string.
#[test]
fn influxql_regexes_end_where_the_server_ends_them() {
    for sql in [
        "SELECT value FROM /\\\\/ ' / ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value FROM /a\\/' / ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SHOW MEASUREMENTS WITH MEASUREMENT = /'/ ; SELECT value INTO hit FROM cpu ; SHOW MEASUREMENTS WITH MEASUREMENT = /'/",
        "SHOW SERIES CARDINALITY FROM /'/ ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        "SELECT value FROM cpu; /** ' **/ SELECT value INTO hit FROM cpu",
        "SELECT value FROM cpu WHERE host = 'a' /* ' */ ; SELECT value INTO hit FROM cpu",
        "SELECT value FROM cpu WHERE host = \"a\\'b\" ; SELECT value INTO hit FROM cpu ; SELECT value FROM /'/",
        ";;; SELECT value INTO hit FROM cpu",
        "SELECT value INTO hit\rFROM cpu",
        "SELECT value*1.5INTO hit FROM cpu",
    ] {
        assert_eq!(rate(sql), Write, "{sql:?}");
    }
}

/// Each of these wrote points on InfluxDB 2.7.
#[test]
fn flux_strings_comments_and_regexes_end_where_the_server_ends_them() {
    for flux in [
        // `$$` takes the `{` out of the interpolation it looks like.
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host != "$${") |> to(bucket:"hit") |> filter(fn:(r)=> r.host != "}")"#,
        // Only `\n` ends a comment, so the `"` is still inside it.
        "from(bucket:\"b\") |> range(start:0) // a\r\"\n |> to(bucket:\"hit\") // \"",
        // Only `\/` is an escape, so `\\/` escapes the `/` and the `"` is in the regex.
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host =~ /\\/"/ or true) |> to(bucket:"hit") |> filter(fn:(r)=> r.host =~ /"/ or true)"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host =~ /\/"/ or true) |> to(bucket:"hit") |> filter(fn:(r)=> r.host =~ /"/ or true)"#,
        // A regex opening an interpolation, and a call behind one.
        "import \"http\"\nimport \"array\"\ns = \"${if \"a\" =~ /\"/ then string(v: http.post(url:\"u\", data: bytes(v:\"x\"))) else \"b\"}\"\narray.from(rows:[{a:s}])",
        // A number ends where its digits do.
        r#"x = 1to(tables: from(bucket:"b") |> range(start:0), bucket:"hit")"#,
        r#"y = 1.to(tables: from(bucket:"b") |> range(start:0), bucket:"hit")"#,
    ] {
        assert_eq!(rate(flux), Write, "{flux:?}");
    }
}

/// These send a request or open a connection on InfluxDB 2.7, whatever they read.
#[test]
fn flux_that_reaches_another_host_rates_up() {
    for flux in [
        "import \"socket\"\nsocket.from(url:\"tcp://x:1\", decoder:\"csv\")",
        "import \"experimental/usage\"\nusage.limits(host:\"u\", orgID:\"o\", token:\"t\")",
        "import \"experimental/usage\"\nusage.from(start:0, host:\"u\", orgID:\"o\", token:\"t\")",
        "import \"experimental/bigtable\"\nbigtable.from(token:\"t\", project:\"p\", instance:\"i\", table:\"t\")",
        r#"from(bucket:"b", host:"http://x", token:"t") |> range(start:0)"#,
        "import \"influxdata/influxdb\"\ninfluxdb.from(bucket:\"b\", host:\"http://x\", token:\"t\")",
        "import \"influxdata/influxdb/v1\"\nv1.databases(host:\"http://x\", token:\"t\")",
        r#"from(bucket:"b", "host":"http://x", token:"t") |> range(start:0)"#,
        r#"from("bucket":"b", "ho\x73t":"http://x", "token":"t") |> range(start:0)"#,
        "bucket = \"b\"\nhost = \"http://x\"\ntoken = \"t\"\nfrom(bucket, host, token) |> range(start:0)",
        r#"--1 from(bucket:"b", host:"http://x", token:"t")"#,
        "h = \"http://x\"\nfrom(bucket:\"b\", host:h, token:\"t\") |> range(start:0)",
        "f = from\nf(bucket:\"b\", host:\"http://x\", token:\"t\") |> range(start:0)",
        "g = {f: from}\ng.f(bucket:\"b\", host:\"http://x\", token:\"t\") |> range(start:0)",
        "(from)(bucket:\"b\", host:\"http://x\", token:\"t\") |> range(start:0)",
    ] {
        assert_eq!(rate(flux), Write, "{flux:?}");
    }
}

#[test]
fn flux_this_scan_does_not_follow_rates_up() {
    for flux in [
        "import \"universe\"\nfrom(bucket:\"b\") |> universe.to(bucket:\"hit\")",
        "import u \"universe\"\nfrom(bucket:\"b\") |> u.to(bucket:\"hit\")",
        "@edition(\"2022.1\")\nimport \"http\"\nx = http.post(url:\"u\", data: bytes(v:\"x\"))\nfrom(bucket:\"b\")",
        "@edition(\"2022.1\")\nfrom(bucket:\"b\") |> range(start:0)",
        "package main\nimport \"http\"\nx = http.post(url:\"u\")\nfrom(bucket:\"b\")",
        "builtin foo : (x: int) => int\nfrom(bucket:\"b\") |> range(start:0)",
    ] {
        assert_eq!(rate(flux), Write, "{flux:?}");
    }
}

/// What looks like what rates up above, and read on InfluxDB 2.7.
#[test]
fn flux_that_only_looks_like_those_stays_a_read() {
    for flux in [
        r#"from(bucket:"b") |> range(start:0) |> map(fn:(r)=>({r with host: "x"}))"#,
        r#"from(bucket:"b") |> range(start:0) |> map(fn:(r)=>({host: r.a, url: r.b}))"#,
        r#"from(bucket:"b") |> range(start:0) |> rename(columns:{host:"x", url:"y"})"#,
        r#"from(bucket:"b") |> range(start:0) |> group(columns:["host"])"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host == "a" and exists r.url)"#,
        // A variable of that name is a value being passed, not an argument's name.
        "host = \"server01\"\nfrom(bucket:\"b\") |> range(start:-1h) |> filter(fn: (r) => r.host == host)",
        "import \"strings\"\nhost = \"a\"\nfrom(bucket:\"b\") |> range(start:0) |> filter(fn: (r) => strings.containsStr(v: r.host, substr: host))",
        "url = \"x\"\nfrom(bucket:\"b\") |> range(start:0) |> filter(fn: (r) => r.url == url)",
        "only = (tables=<-, host) => tables |> filter(fn: (r) => r.host == host)\nfrom(bucket:\"b\") |> range(start:0)",
        "only = (host, url) => host + url\nfrom(bucket:\"b\") |> range(start:0)",
        "host = \"a\"\nfrom(bucket:\"b\") |> range(start:0) |> filter(fn: (r) => if (host) == \"a\" then true else (r.host) == (host))",
        "host = \"a\"\nf = () => { return (host) }\nfrom(bucket:\"b\") |> range(start:0)",
        r#"from(bucket:"b") |> range(start:0) |> map(fn:(r)=>({"a b": r.host}))"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host == "$")"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host == "a$b")"#,
        r#"from(bucket:"b") |> range(start:0) |> filter(fn:(r)=> r.host == "a\${to(x)}")"#,
        "from(bucket:\"b\") |> range(start:0) |> filter(fn:(r)=> r.host =~ /a\nb/ or true) |> limit(n:1)",
        "import \"universe\"\nfrom(bucket:\"b\") |> universe.limit(n:1)",
        "package main\nimport \"array\"\narray.from(rows:[{a:1}])",
    ] {
        assert_eq!(rate(flux), Read, "{flux:?}");
    }
}
