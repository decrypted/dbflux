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
