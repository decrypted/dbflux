//! Live integration tests for the InfluxDB driver.
//!
//! All tests are `#[ignore]` — they require a running Docker daemon and are
//! invoked explicitly:
//!
//!   cargo test -p dbflux_driver_influxdb --test live_integration -- --ignored
//!
//! Tests spin up real InfluxDB containers via testcontainers and verify the full
//! round-trip from connection to query result.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::result_large_err
)]

use dbflux_core::{
    CollectionBrowseRequest, CollectionRef, ColumnKind, Connection, ConnectionProfile, DbConfig,
    DbDriver, DbError, ExecutionContext, ExecutionSourceContext, InfluxVersion, Pagination,
    QueryLanguage, QueryRequest, QueryResult, SchemaLoadingStrategy, Value,
};
use dbflux_driver_influxdb::InfluxDriver;
use dbflux_test_support::containers;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_v2_profile(endpoint: &str, bucket: Option<&str>, org: &str) -> ConnectionProfile {
    ConnectionProfile::new_with_driver(
        "influxdb-v2-test",
        dbflux_core::DbKind::InfluxDB,
        "builtin:influxdb",
        DbConfig::InfluxDB {
            version: InfluxVersion::V2,
            url: endpoint.to_string(),
            org: Some(org.to_string()),
            default_bucket: bucket.map(str::to_string),
            retention_policy: None,
            user: None,
            request_timeout_seconds: None,
        },
    )
}

fn make_v1_profile(endpoint: &str, database: &str) -> ConnectionProfile {
    ConnectionProfile::new_with_driver(
        "influxdb-v1-test",
        dbflux_core::DbKind::InfluxDB,
        "builtin:influxdb",
        DbConfig::InfluxDB {
            version: InfluxVersion::V1,
            url: endpoint.to_string(),
            org: None,
            default_bucket: Some(database.to_string()),
            retention_policy: None,
            user: None,
            request_timeout_seconds: None,
        },
    )
}

fn connect_v2(
    endpoint: &str,
    bucket: &str,
    org: &str,
    token: &str,
) -> Result<Box<dyn Connection>, DbError> {
    let driver = InfluxDriver::new();
    let profile = make_v2_profile(endpoint, Some(bucket), org);
    let secret = dbflux_core::secrecy::SecretString::new(token.to_string().into());
    driver.connect_with_secrets(&profile, Some(&secret), None)
}

fn connect_v2_without_default_bucket(
    endpoint: &str,
    org: &str,
    token: &str,
) -> Result<Box<dyn Connection>, DbError> {
    let driver = InfluxDriver::new();
    let profile = make_v2_profile(endpoint, None, org);
    let secret = dbflux_core::secrecy::SecretString::new(token.to_string().into());
    driver.connect_with_secrets(&profile, Some(&secret), None)
}

fn connect_v1(endpoint: &str, database: &str) -> Result<Box<dyn Connection>, DbError> {
    let driver = InfluxDriver::new();
    let profile = make_v1_profile(endpoint, database);
    driver.connect_with_secrets(&profile, None, None)
}

/// Write line-protocol data to InfluxDB v2 via the `/api/v2/write` endpoint.
fn write_v2_line_protocol(
    endpoint: &str,
    token: &str,
    org: &str,
    bucket: &str,
    lines: &str,
) -> Result<(), DbError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| DbError::connection_failed(e.to_string()))?;

    let url = format!(
        "{}/api/v2/write?org={}&bucket={}&precision=ms",
        endpoint.trim_end_matches('/'),
        urlencoding::encode(org),
        urlencoding::encode(bucket),
    );

    let resp = client
        .post(&url)
        .header("Authorization", format!("Token {token}"))
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(lines.to_string())
        .send()
        .map_err(|e| DbError::connection_failed(e.to_string()))?;

    if resp.status().is_success() || resp.status().as_u16() == 204 {
        Ok(())
    } else {
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Err(DbError::query_failed(format!(
            "write failed: HTTP {status}: {body}"
        )))
    }
}

/// Write line-protocol data to InfluxDB v1 via the `/write` endpoint.
fn write_v1_line_protocol(endpoint: &str, database: &str, lines: &str) -> Result<(), DbError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| DbError::connection_failed(e.to_string()))?;

    let url = format!(
        "{}/write?db={}&precision=ms",
        endpoint.trim_end_matches('/'),
        urlencoding::encode(database),
    );

    let resp = client
        .post(&url)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(lines.to_string())
        .send()
        .map_err(|e| DbError::connection_failed(e.to_string()))?;

    if resp.status().is_success() || resp.status().as_u16() == 204 {
        Ok(())
    } else {
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Err(DbError::query_failed(format!(
            "write failed: HTTP {status}: {body}"
        )))
    }
}

/// `/query` answers 200 even when the statement fails, with the error inside
/// the JSON body, so the status alone does not prove the database exists. A
/// write sent before the server lists it fails with "database not found".
fn setup_v1_database(endpoint: &str, database: &str) -> Result<(), DbError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| DbError::connection_failed(error.to_string()))?;

    v1_statement(&client, endpoint, &format!("CREATE DATABASE {database}"))?;

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let listed = v1_statement(&client, endpoint, "SHOW DATABASES")?;
        if v1_lists_database(&listed, database) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(DbError::query_failed(format!(
                "database {database} was created but is not listed: {listed}"
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn v1_statement(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    statement: &str,
) -> Result<serde_json::Value, DbError> {
    let url = format!(
        "{}/query?q={}",
        endpoint.trim_end_matches('/'),
        urlencoding::encode(statement),
    );

    let resp = client
        .post(&url)
        .send()
        .map_err(|error| DbError::connection_failed(error.to_string()))?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .map_err(|error| DbError::connection_failed(error.to_string()))?;
    if status >= 300 {
        return Err(DbError::query_failed(format!(
            "{statement} failed: HTTP {status}: {body}"
        )));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
        DbError::query_failed(format!("{statement}: invalid JSON {body}: {error}"))
    })?;
    let statement_error = parsed["results"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|result| result["error"].as_str());
    if let Some(error) = statement_error {
        return Err(DbError::query_failed(format!(
            "{statement} failed: {error}"
        )));
    }

    Ok(parsed)
}

fn v1_lists_database(show_databases: &serde_json::Value, database: &str) -> bool {
    show_databases["results"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|result| result["series"].as_array().into_iter().flatten())
        .flat_map(|series| series["values"].as_array().into_iter().flatten())
        .any(|row| row[0].as_str() == Some(database))
}

/// Build a query request with an explicit collection window (triggers time injection).
fn query_with_window(sql: &str) -> QueryRequest {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let start_ms = now_ms - 60 * 60 * 1000; // 1 hour ago
    let end_ms = now_ms + 60 * 1000; // 1 minute in the future

    let ctx = ExecutionContext {
        source: Some(ExecutionSourceContext::CollectionWindow {
            targets: vec![],
            start_ms,
            end_ms,
            query_mode: Some("influxql".to_string()),
        }),
        ..Default::default()
    };

    QueryRequest::new(sql).with_execution_context(Some(ctx))
}

/// Build a Flux query request with window injection.
fn flux_query_with_window(flux: &str) -> QueryRequest {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let start_ms = now_ms - 60 * 60 * 1000;
    let end_ms = now_ms + 60 * 1000;

    let ctx = ExecutionContext {
        source: Some(ExecutionSourceContext::CollectionWindow {
            targets: vec![],
            start_ms,
            end_ms,
            query_mode: Some("flux".to_string()),
        }),
        ..Default::default()
    };

    QueryRequest::new(flux).with_execution_context(Some(ctx))
}

/// Generate N line-protocol points for measurement `metric`.
///
/// Uses millisecond timestamps; the write helper passes `precision=ms` to the
/// InfluxDB v2 write API so no nanosecond conversion is needed.
fn generate_metric_points(count: usize) -> String {
    let base_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    (0..count)
        .map(|i| {
            let ts_ms = base_ms - (i as i64) * 1000;
            format!("metric,host=server{} value={}i {}", i % 5, i, ts_ms)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// D.4.2 — v2 InfluxQL happy path with time injection
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v2_influxql_happy_path_with_time_injection() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let lines = generate_metric_points(100);
        write_v2_line_protocol(&cfg.endpoint, &cfg.token, &cfg.org, &cfg.bucket, &lines)?;

        // Allow InfluxDB a moment to index the written data.
        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;

        let dbs = conn.list_databases()?;
        assert!(
            !dbs.is_empty(),
            "list_databases must return at least the test bucket"
        );

        let req = query_with_window("SELECT * FROM metric LIMIT 10");
        let result = conn.execute(&req)?;

        assert_eq!(
            result.rows.len(),
            10,
            "must return exactly 10 rows with LIMIT 10"
        );
        assert!(
            result.resolved_window.is_some(),
            "time injection must produce a resolved_window"
        );

        let extra = result
            .metadata_extra
            .as_ref()
            .expect("metadata_extra must be populated");
        assert_eq!(
            extra.get("language").and_then(|v| v.as_str()),
            Some("influxql")
        );
        assert_eq!(extra.get("version").and_then(|v| v.as_str()), Some("v2"));
        assert_eq!(
            extra.get("injected_window").and_then(|v| v.as_bool()),
            Some(true)
        );

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// D.4.3 — v2 Flux happy path with time injection
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v2_flux_happy_path_with_time_injection() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let lines = generate_metric_points(20);
        write_v2_line_protocol(&cfg.endpoint, &cfg.token, &cfg.org, &cfg.bucket, &lines)?;

        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;

        let flux = format!(
            r#"from(bucket: "{}")
  |> filter(fn: (r) => r._measurement == "metric")
  |> limit(n: 5)"#,
            cfg.bucket
        );

        let req = flux_query_with_window(&flux);
        let result = conn.execute(&req)?;

        assert!(!result.rows.is_empty(), "Flux query must return rows");
        assert!(
            result.resolved_window.is_some(),
            "time injection must produce a resolved_window for Flux"
        );

        let extra = result
            .metadata_extra
            .as_ref()
            .expect("metadata_extra must be set");
        assert_eq!(extra.get("language").and_then(|v| v.as_str()), Some("flux"));
        assert_eq!(extra.get("version").and_then(|v| v.as_str()), Some("v2"));

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// D.4.4 — v2 explicit time range → no injection
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v2_explicit_time_range_no_injection() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let lines = generate_metric_points(10);
        write_v2_line_protocol(&cfg.endpoint, &cfg.token, &cfg.org, &cfg.bucket, &lines)?;

        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;

        // Explicit time predicate in WHERE — injection must be skipped.
        let req = query_with_window("SELECT * FROM metric WHERE time > now() - 1h");
        let result = conn.execute(&req)?;

        assert!(
            result.resolved_window.is_none(),
            "explicit WHERE time must suppress injection: resolved_window must be None"
        );

        let extra = result
            .metadata_extra
            .as_ref()
            .expect("metadata_extra must be set");
        assert_eq!(
            extra.get("injected_window").and_then(|v| v.as_bool()),
            Some(false),
            "injected_window must be false when the query already has a time predicate"
        );

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// D.4.5 — v2 bad query: malformed InfluxQL → clean error message
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v2_bad_query_surfaces_clean_error() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;

        let req = QueryRequest::new("THIS IS NOT VALID INFLUXQL !!!!");
        let err = conn.execute(&req).expect_err("malformed query must fail");

        let msg = err.to_string().to_ascii_lowercase();
        assert!(
            msg.contains("bad request") || msg.contains("error") || msg.contains("parse"),
            "error message must be actionable (not raw debug): {msg}"
        );

        // Must NOT be raw Rust debug output.
        assert!(
            !msg.contains("error { ") && !msg.contains("kind:"),
            "error must not leak raw Rust debug output: {msg}"
        );

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// D.4.6 — v2 auth failure → DbError::AuthFailed with clean message
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v2_wrong_token_returns_auth_failed() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        // Connect with a deliberately wrong token.
        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, "WRONG-TOKEN-XYZ")?;

        let req = QueryRequest::new("SELECT * FROM metric");
        let err = conn.execute(&req).expect_err("wrong token must fail");

        assert!(
            matches!(err, DbError::AuthFailed(_)),
            "wrong token must produce DbError::AuthFailed, got: {err:?}"
        );

        let msg = err.to_string().to_ascii_lowercase();
        assert!(
            msg.contains("unauthorized") || msg.contains("auth") || msg.contains("token"),
            "auth error message must be descriptive: {msg}"
        );

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// D.4.7 — v1 happy path
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v1_influxql_happy_path() -> Result<(), DbError> {
    containers::with_influxdb_v1(|cfg| {
        let database = "testdb";
        setup_v1_database(&cfg.endpoint, database)?;

        let lines = generate_metric_points(20);
        write_v1_line_protocol(&cfg.endpoint, database, &lines)?;

        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v1(&cfg.endpoint, database)?;

        let dbs = conn.list_databases()?;
        assert!(
            dbs.iter().any(|db| db.name == database),
            "list_databases must include '{database}', got: {dbs:?}"
        );

        let req = query_with_window("SELECT * FROM metric LIMIT 5");
        let result = conn.execute(&req)?;

        assert_eq!(result.rows.len(), 5, "LIMIT 5 must return 5 rows");
        assert!(
            result.resolved_window.is_some(),
            "time injection must produce a resolved_window"
        );

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// D.4.8 — v1 Flux rejected synchronously (no HTTP call needed)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v1_flux_query_rejected_without_http() -> Result<(), DbError> {
    containers::with_influxdb_v1(|cfg| {
        let database = "testdb";
        setup_v1_database(&cfg.endpoint, database)?;

        let conn = connect_v1(&cfg.endpoint, database)?;

        // Flux on v1 must fail immediately (before any HTTP call) because
        // resolve_language() rejects it. We record the time to verify it's fast.
        let before = std::time::Instant::now();

        let ctx = ExecutionContext {
            source: Some(ExecutionSourceContext::CollectionWindow {
                targets: vec![],
                start_ms: 0,
                end_ms: 1,
                query_mode: Some("flux".to_string()),
            }),
            ..Default::default()
        };
        let req = QueryRequest::new("from(bucket: \"x\") |> range(start: -1h)")
            .with_execution_context(Some(ctx));
        let err = conn.execute(&req).expect_err("Flux on v1 must fail");

        let elapsed = before.elapsed();

        assert!(
            matches!(err, DbError::QueryFailed(_)),
            "Flux on v1 must produce DbError::QueryFailed, got: {err:?}"
        );
        let msg = err.to_string().to_ascii_lowercase();
        assert!(
            msg.contains("flux") && msg.contains("not supported"),
            "error must mention Flux not supported: {msg}"
        );

        // Verify the rejection was synchronous (no HTTP round-trip needed).
        assert!(
            elapsed < Duration::from_secs(2),
            "Flux rejection must be synchronous (took {}ms)",
            elapsed.as_millis()
        );

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// D.4.9 — Audit emission integration: metadata_extra fields on v2 InfluxQL query
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v2_influxql_metadata_extra_contains_audit_fields() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let lines = generate_metric_points(5);
        write_v2_line_protocol(&cfg.endpoint, &cfg.token, &cfg.org, &cfg.bucket, &lines)?;

        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;

        let req = query_with_window("SELECT * FROM metric LIMIT 3");
        let result = conn.execute(&req)?;

        let extra = result
            .metadata_extra
            .as_ref()
            .expect("metadata_extra must be populated for InfluxDB queries");

        // Required fields per REQ-9 / D.3.2 spec.
        assert_eq!(
            extra.get("language").and_then(|v| v.as_str()),
            Some("influxql"),
            "language field must be 'influxql'"
        );
        assert_eq!(
            extra.get("version").and_then(|v| v.as_str()),
            Some("v2"),
            "version field must be 'v2'"
        );
        assert!(
            extra.contains_key("bucket_or_database"),
            "bucket_or_database field must be present"
        );
        assert!(
            extra.contains_key("injected_window"),
            "injected_window field must be present"
        );
        assert_eq!(
            extra.get("injected_window").and_then(|v| v.as_bool()),
            Some(true),
            "injected_window must be true when the window was injected"
        );
        assert!(
            extra.contains_key("resolved_window_start_ms"),
            "resolved_window_start_ms must be present when window was injected"
        );
        assert!(
            extra.contains_key("resolved_window_end_ms"),
            "resolved_window_end_ms must be present when window was injected"
        );

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Instance catalog (issue #511)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Docker daemon"]
fn v2_instance_metrics_fetch_through_execute() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;

        // Collected rather than asserted per metric: a declared name missing
        // from the scrape is the failure this test exists to catch, and one
        // run should name every one of them instead of stopping at the first.
        let mut unresolved: Vec<String> = Vec::new();

        for (prom_name, metric_id, _, _, _) in
            dbflux_driver_influxdb::instance_catalog::METRIC_FIELDS
        {
            let ctx = ExecutionContext {
                source: Some(ExecutionSourceContext::InstanceMetricQuery {
                    metric_id: metric_id.to_string(),
                    start_ms: 0,
                    end_ms: 1,
                }),
                ..Default::default()
            };
            let req = QueryRequest::new("").with_execution_context(Some(ctx));

            let result = match conn.execute(&req) {
                Ok(result) => result,
                Err(error) => {
                    unresolved.push(format!("{metric_id} ({prom_name}): {error}"));
                    continue;
                }
            };

            assert_eq!(
                result.columns.len(),
                2,
                "metric '{metric_id}' must return exactly 2 columns"
            );
            assert_eq!(
                result.columns[0].kind,
                dbflux_core::ColumnKind::Timestamp,
                "metric '{metric_id}' column 0 must be Timestamp"
            );
            assert_eq!(
                result.columns[1].kind,
                dbflux_core::ColumnKind::Float,
                "metric '{metric_id}' column 1 must be Float"
            );
            assert_eq!(
                result.rows.len(),
                1,
                "metric '{metric_id}' must return exactly one row"
            );
        }

        assert!(
            unresolved.is_empty(),
            "declared metrics absent from this server's /metrics scrape:\n  {}",
            unresolved.join("\n  ")
        );

        Ok(())
    })
}

#[test]
#[ignore = "requires Docker daemon"]
fn v2_instance_inspectors_fetch_through_execute() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;

        for metric_id in ["influx.health", "influx.metrics"] {
            let ctx = ExecutionContext {
                source: Some(ExecutionSourceContext::InstanceInspectorQuery {
                    metric_id: metric_id.to_string(),
                }),
                ..Default::default()
            };
            let req = QueryRequest::new("").with_execution_context(Some(ctx));

            let result = conn
                .execute(&req)
                .unwrap_or_else(|e| panic!("inspector '{metric_id}' failed: {e}"));

            assert!(
                !result.columns.is_empty(),
                "inspector '{metric_id}' must return at least one column"
            );
            assert!(
                !result.rows.is_empty(),
                "inspector '{metric_id}' must return at least one row"
            );
        }

        Ok(())
    })
}

#[test]
#[ignore = "requires Docker daemon"]
fn v1_instance_catalog_is_none_and_degrades_cleanly() -> Result<(), DbError> {
    containers::with_influxdb_v1(|cfg| {
        setup_v1_database(&cfg.endpoint, "dbflux_test_db")?;

        let conn = connect_v1(&cfg.endpoint, "dbflux_test_db")?;

        assert!(
            conn.instance_catalog().is_none(),
            "InfluxDB v1 must not advertise an instance catalog"
        );

        let ctx = ExecutionContext {
            source: Some(ExecutionSourceContext::InstanceInspectorQuery {
                metric_id: "influx.health".to_string(),
            }),
            ..Default::default()
        };
        let req = QueryRequest::new("").with_execution_context(Some(ctx));

        // v1 serves `/metrics` too, so the version gate in execute() is what
        // stops an instance query routed here from being answered by a catalog
        // this connection reports it does not have.
        let error = conn
            .execute(&req)
            .expect_err("an instance query on v1 must be refused, not answered");
        assert!(
            matches!(error, DbError::NotSupported(_)),
            "expected NotSupported, got {error:?}"
        );

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Measurement browse: the query the UI shows is the query that runs, and the
// result carries the column kinds the chart view needs.
// ---------------------------------------------------------------------------

fn browse_first_page(database: &str) -> CollectionBrowseRequest {
    CollectionBrowseRequest::new(CollectionRef::new(database, "metric")).with_pagination(
        Pagination::Offset {
            limit: 10,
            offset: 0,
        },
    )
}

/// Asserts a browse result has a timestamp column and a numeric column, which
/// is what chart detection needs to open a time-series source as a chart.
fn assert_chartable(result: &QueryResult) {
    assert!(
        result
            .columns
            .iter()
            .any(|column| column.kind == ColumnKind::Timestamp),
        "browse result must carry a timestamp column: {:?}",
        result.columns
    );
    assert!(
        result
            .columns
            .iter()
            .any(|column| matches!(column.kind, ColumnKind::Integer | ColumnKind::Float)),
        "browse result must carry a numeric column: {:?}",
        result.columns
    );
}

#[test]
#[ignore = "requires Docker daemon"]
fn v2_measurement_browse_runs_the_query_it_describes() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let lines = generate_metric_points(20);
        write_v2_line_protocol(&cfg.endpoint, &cfg.token, &cfg.org, &cfg.bucket, &lines)?;

        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;
        let request = browse_first_page(&cfg.bucket);

        let described = conn
            .query_generator()
            .and_then(|generator| generator.collection_browse_query(&request))
            .expect("InfluxDB must describe its browse query");
        assert_eq!(described.language, QueryLanguage::InfluxQuery);
        assert_eq!(
            described.text,
            "SELECT * FROM \"metric\" ORDER BY time DESC LIMIT 10 OFFSET 0"
        );

        let result = conn.browse_collection(&request)?;

        assert_eq!(
            result.rows.len(),
            10,
            "the browse must honour the page size"
        );
        assert_chartable(&result);

        Ok(())
    })
}

#[test]
#[ignore = "requires Docker daemon"]
fn v1_measurement_browse_runs_the_query_it_describes() -> Result<(), DbError> {
    containers::with_influxdb_v1(|cfg| {
        let database = "testdb";
        setup_v1_database(&cfg.endpoint, database)?;
        write_v1_line_protocol(&cfg.endpoint, database, &generate_metric_points(20))?;

        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v1(&cfg.endpoint, database)?;
        let request = browse_first_page(database);

        let described = conn
            .query_generator()
            .and_then(|generator| generator.collection_browse_query(&request))
            .expect("InfluxDB must describe its browse query");
        assert_eq!(described.language, QueryLanguage::InfluxQuery);
        assert_eq!(
            described.text,
            "SELECT * FROM \"metric\" ORDER BY time DESC LIMIT 10 OFFSET 0"
        );

        let result = conn.browse_collection(&request)?;

        assert_eq!(
            result.rows.len(),
            10,
            "the browse must honour the page size"
        );
        assert_chartable(&result);

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Per-bucket measurement listing: any bucket expands, not only the profile
// default, and browsed timestamps arrive as datetimes rather than epoch values.
// ---------------------------------------------------------------------------

fn measurement_names(conn: &dyn Connection, database: &str) -> Result<Vec<String>, DbError> {
    Ok(conn
        .schema_for_database(database)?
        .tables
        .into_iter()
        .map(|table| table.name)
        .collect())
}

#[test]
#[ignore = "requires Docker daemon"]
fn v2_bucket_lists_measurements_without_a_default_bucket() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        let lines = generate_metric_points(5);
        write_v2_line_protocol(&cfg.endpoint, &cfg.token, &cfg.org, &cfg.bucket, &lines)?;

        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v2_without_default_bucket(&cfg.endpoint, &cfg.org, &cfg.token)?;

        assert_eq!(
            conn.schema_loading_strategy(),
            SchemaLoadingStrategy::LazyPerDatabase
        );
        assert_eq!(measurement_names(conn.as_ref(), &cfg.bucket)?, ["metric"]);

        let result = conn.browse_collection(&browse_first_page(&cfg.bucket))?;
        let time_index = result
            .columns
            .iter()
            .position(|column| column.name == "time")
            .expect("browse result carries a time column");
        assert!(
            result
                .rows
                .iter()
                .all(|row| matches!(row[time_index], Value::DateTime(_))),
            "time values must be datetimes, not epoch integers: {:?}",
            result.rows.first()
        );

        Ok(())
    })
}

#[test]
#[ignore = "requires Docker daemon"]
fn v1_database_other_than_the_default_lists_its_measurements() -> Result<(), DbError> {
    containers::with_influxdb_v1(|cfg| {
        setup_v1_database(&cfg.endpoint, "primary")?;
        setup_v1_database(&cfg.endpoint, "secondary")?;
        write_v1_line_protocol(&cfg.endpoint, "secondary", &generate_metric_points(5))?;

        std::thread::sleep(Duration::from_millis(500));

        let conn = connect_v1(&cfg.endpoint, "primary")?;

        assert_eq!(measurement_names(conn.as_ref(), "secondary")?, ["metric"]);
        assert!(measurement_names(conn.as_ref(), "primary")?.is_empty());

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// The rating against what the server does
// ---------------------------------------------------------------------------

fn is_refused(connection: &dyn Connection, query: &str) -> bool {
    !matches!(
        dbflux_core::classify_query_for_governance(
            &QueryLanguage::InfluxQuery,
            query,
            Some(connection.language_service()),
        ),
        dbflux_core::ExecutionClassification::Metadata | dbflux_core::ExecutionClassification::Read
    )
}

/// These wrote on a real server while the rating saw a read, so a chart, a
/// read-only profile and MCP `read_query` let them through. Each must both write
/// and be refused, and what looks like them but reads must stay allowed.
#[test]
#[ignore = "requires Docker daemon"]
fn v1_rating_refuses_the_influxql_the_server_writes_with() -> Result<(), DbError> {
    containers::with_influxdb_v1(|cfg| {
        let database = "testdb";
        setup_v1_database(&cfg.endpoint, database)?;
        write_v1_line_protocol(&cfg.endpoint, database, "cpu,host=a value=1 1700000000000")?;
        std::thread::sleep(Duration::from_millis(500));

        let connection = connect_v1(&cfg.endpoint, database)?;
        let has_measurement = |name: &str| -> Result<bool, DbError> {
            let shown = connection.execute(&QueryRequest::new("SHOW MEASUREMENTS"))?;
            Ok(shown
                .rows
                .iter()
                .flatten()
                .any(|value| matches!(value, Value::Text(text) if text == name)))
        };

        for (name, template) in [
            ("hit_plain", "SELECT value INTO {} FROM cpu"),
            ("hit_lower", "select value into {} from cpu"),
            ("hit_lines", "SeLeCt\tvalue\nInTo\n{}\nFROM\tcpu"),
            ("hit_comment", "-- c\nSELECT value INTO {} FROM cpu"),
            ("hit_block", "/* c */ SELECT value INTO {} FROM cpu"),
            ("hit_quoted", "SELECT value INTO \"{}\" FROM cpu"),
            (
                "hit_escape",
                "SELECT value FROM cpu WHERE host = 'x\\'' ; SELECT value INTO {} FROM cpu",
            ),
            (
                "hit_regex",
                "SELECT value FROM /'/ ; SELECT value INTO {} FROM cpu ; SELECT value FROM /'/",
            ),
            (
                "hit_match",
                "SELECT value FROM cpu WHERE host =~ /'/ ; SELECT value INTO {} FROM cpu ; SELECT value FROM /'/",
            ),
            (
                "hit_backslashes",
                "SELECT value FROM /\\\\/ ' / ; SELECT value INTO {} FROM cpu ; SELECT value FROM /'/",
            ),
            (
                "hit_with_measurement",
                "SHOW MEASUREMENTS WITH MEASUREMENT = /'/ ; SELECT value INTO {} FROM cpu ; SHOW MEASUREMENTS WITH MEASUREMENT = /'/",
            ),
            (
                "hit_show",
                "SHOW SERIES FROM /'/ ; SELECT value INTO {} FROM cpu ; SELECT value FROM /'/",
            ),
        ] {
            let query = template.replace("{}", name);

            assert!(
                is_refused(&*connection, &query),
                "rated as a read: {query:?}"
            );
            assert!(!has_measurement(name)?, "{name} exists before {query:?}");
            connection.execute(&QueryRequest::new(query.clone()))?;
            assert!(has_measurement(name)?, "the server did not run {query:?}");
        }

        write_v1_line_protocol(&cfg.endpoint, database, "gone,host=a value=1 1700000000000")?;
        std::thread::sleep(Duration::from_millis(500));

        let behind_a_read = "SELECT value FROM /'/ ; DROP MEASUREMENT gone ; SELECT value FROM /'/";
        assert!(is_refused(&*connection, behind_a_read));
        assert!(has_measurement("gone")?);
        connection.execute(&QueryRequest::new(behind_a_read))?;
        assert!(!has_measurement("gone")?, "the server did not drop it");

        for query in [
            "SELECT value FROM cpu",
            "SELECT \"into\" FROM cpu",
            "SELECT value FROM cpu WHERE host = 'INTO hit_string'",
            "SELECT value FROM cpu -- INTO hit_line",
            "SELECT value FROM /into/",
            "SELECT value / 2 FROM /cpu/",
            "SHOW MEASUREMENTS",
        ] {
            assert!(!is_refused(&*connection, query), "refused {query:?}");
            connection.execute(&QueryRequest::new(query))?;
        }
        assert!(!has_measurement("hit_string")? && !has_measurement("hit_line")?);

        Ok(())
    })
}

#[test]
#[ignore = "requires Docker daemon"]
fn v2_rating_refuses_the_flux_the_server_writes_with() -> Result<(), DbError> {
    containers::with_influxdb_v2(|cfg| {
        write_v2_line_protocol(
            &cfg.endpoint,
            &cfg.token,
            &cfg.org,
            &cfg.bucket,
            "cpu,host=a value=1,to=2 1700000000000",
        )?;
        std::thread::sleep(Duration::from_millis(500));

        let connection = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;
        let bucket = &cfg.bucket;
        let source = format!(
            r#"from(bucket: "{bucket}") |> range(start: 0) |> filter(fn: (r) => r._measurement == "cpu")"#
        );
        let points_in = |name: &str| -> Result<usize, DbError> {
            let query = format!(
                r#"from(bucket: "{bucket}") |> range(start: 0) |> filter(fn: (r) => r._measurement == "{name}")"#
            );
            Ok(connection
                .execute(&flux_query_with_window(&query))?
                .rows
                .len())
        };

        for (name, template) in [
            ("to_pipe", "{SOURCE} {SET} |> to(bucket: \"{BUCKET}\")"),
            ("to_tight", "{SOURCE} {SET}|>to(bucket:\"{BUCKET}\")"),
            ("to_space", "{SOURCE} {SET} |> to (bucket: \"{BUCKET}\")"),
            (
                "to_comment",
                "{SOURCE} {SET} // it's\n |> to(bucket: \"{BUCKET}\")",
            ),
            (
                "to_regex",
                "{SOURCE} |> filter(fn: (r) => r.host =~ /\"/ or true) {SET} |> to(bucket: \"{BUCKET}\") |> filter(fn: (r) => r.host =~ /\"/)",
            ),
            ("to_dash", "--1 {SOURCE} {SET} |> to(bucket: \"{BUCKET}\")"),
            (
                "to_record",
                "--1 {f: to}.f(tables: {SOURCE} {SET}, bucket: \"{BUCKET}\")",
            ),
            (
                "to_dash_string",
                "--1 \"\nSHOW SERIES -- \" to(tables: {SOURCE} {SET}, bucket: \"{BUCKET}\") \"x\"",
            ),
            (
                "to_dash_show",
                "--1 show = 1 {SOURCE} {SET} |> to(bucket: \"{BUCKET}\")\nshow",
            ),
            (
                "to_dollar",
                "{SOURCE} |> filter(fn: (r) => r.host != \"$${\") {SET} |> to(bucket: \"{BUCKET}\") |> filter(fn: (r) => r.host != \"}\")",
            ),
            (
                "to_return",
                "{SOURCE} {SET} // a\r\"\n |> to(bucket: \"{BUCKET}\") // \"",
            ),
            (
                "to_backslashes",
                "{SOURCE} |> filter(fn: (r) => r.host =~ /\\\\/\"/ or true) {SET} |> to(bucket: \"{BUCKET}\") |> filter(fn: (r) => r.host =~ /\"/ or true)",
            ),
            (
                "to_tables",
                "to(tables: {SOURCE} {SET}, bucket: \"{BUCKET}\")",
            ),
            (
                "to_name",
                "f = to\n{SOURCE} {SET} |> f(bucket: \"{BUCKET}\")",
            ),
            (
                "to_select",
                "select = to\nselect(tables: {SOURCE} {SET}, bucket: \"{BUCKET}\")",
            ),
            (
                "to_package",
                "import \"influxdata/influxdb\"\n{SOURCE} {SET} |> influxdb.to(bucket: \"{BUCKET}\")",
            ),
            (
                "to_member",
                "import \"influxdata/influxdb\"\nm = influxdb\n{SOURCE} {SET} |> m.to(bucket: \"{BUCKET}\")",
            ),
            (
                "to_index",
                "import \"influxdata/influxdb\"\n{SOURCE} {SET} |> influxdb[\"to\"](bucket: \"{BUCKET}\")",
            ),
            (
                "to_wide",
                "import \"influxdata/influxdb\"\n{SOURCE} {SET} |> pivot(rowKey: [\"_time\"], columnKey: [\"_field\"], valueColumn: \"_value\") |> influxdb.wideTo(bucket: \"{BUCKET}\")",
            ),
            (
                "to_experimental",
                "import \"experimental\"\n{SOURCE} {SET} |> pivot(rowKey: [\"_time\"], columnKey: [\"_field\"], valueColumn: \"_value\") |> experimental.to(bucket: \"{BUCKET}\")",
            ),
            (
                "to_alias",
                "import e \"experimental\"\n{SOURCE} {SET} |> pivot(rowKey: [\"_time\"], columnKey: [\"_field\"], valueColumn: \"_value\") |> e.to(bucket: \"{BUCKET}\")",
            ),
        ] {
            let set = format!(r#"|> set(key: "_measurement", value: "{name}")"#);
            let flux = template
                .replace("{SOURCE}", &source)
                .replace("{SET}", &set)
                .replace("{BUCKET}", bucket);

            assert!(is_refused(&*connection, &flux), "rated as a read: {flux:?}");
            assert_eq!(points_in(name)?, 0, "{name} exists before {flux:?}");
            connection.execute(&flux_query_with_window(&flux))?;
            std::thread::sleep(Duration::from_millis(200));
            assert!(points_in(name)? > 0, "the server did not run {flux:?}");
        }

        for flux in [
            format!("{source} |> limit(n: 1)"),
            format!(r#"{source} |> map(fn: (r) => ({{r with to: "x"}}))"#),
            format!(r#"{source} |> filter(fn: (r) => r._field == "to(")"#),
            format!(r#"{source} |> filter(fn: (r) => r["to"] == "x" or true)"#),
            format!(r#"{source} |> filter(fn: (r) => r.host =~ /to\(/ or true)"#),
            format!(r#"{source} |> keep(columns: ["to", "_value"])"#),
            format!("{source} // |> to(bucket: \"{bucket}\")"),
        ] {
            assert!(!is_refused(&*connection, &flux), "refused {flux:?}");
            connection.execute(&flux_query_with_window(&flux))?;
        }

        Ok(())
    })
}

/// A write hidden behind every mix of the characters that open or close something
/// in the language. What rates as a read goes to a real server, which must not
/// have written once all of them ran. A text the server refuses runs nothing, so
/// only the lexer's wrong readings of what the server accepts can fail this.
#[test]
#[ignore = "requires Docker daemon"]
fn v1_nothing_rated_a_read_writes() -> Result<(), DbError> {
    const HEADS: &[&str] = &[
        "SELECT value FROM ",
        "SELECT ",
        "SELECT value, ",
        "SELECT mean(",
        "SELECT value FROM cpu, ",
        "SELECT value FROM d.autogen.",
        "SELECT value FROM cpu WHERE host =~ ",
        "SELECT value FROM cpu WHERE host = ",
        "SELECT value FROM cpu GROUP BY ",
        "SHOW SERIES FROM ",
        "SHOW MEASUREMENTS WITH MEASUREMENT = ",
    ];
    const PIECES: &[&str] = &[
        "", "/", "'", "\"", "\\", "\\/", "\\\\", "\\'", "--", "/*", "*/", "\n", "\r", ";", "cpu",
        "$", "(", ")", "/'/", "\u{2028}", "\u{85}", "\u{c}", "\u{b}",
    ];
    const TAILS: &[&str] = &[
        "SELECT value FROM /'/",
        "SELECT value FROM /\"/",
        "SELECT value FROM cpu WHERE host = '",
        "SELECT value FROM cpu -- '",
        "SELECT value FROM cpu /* ' */",
        "' /",
    ];

    containers::with_influxdb_v1(|cfg| {
        let database = "testdb";
        setup_v1_database(&cfg.endpoint, database)?;
        write_v1_line_protocol(&cfg.endpoint, database, "cpu,host=a value=1 1700000000000")?;
        std::thread::sleep(Duration::from_millis(500));

        let connection = connect_v1(&cfg.endpoint, database)?;
        let mut sent: Vec<String> = Vec::new();

        for head in HEADS {
            for first in PIECES {
                for second in PIECES {
                    for third in PIECES {
                        for tail in TAILS {
                            let query = format!(
                                "{head}{first}{second}{third} ; SELECT value INTO h{} FROM cpu ; {tail}",
                                sent.len()
                            );

                            if is_refused(&*connection, &query) {
                                continue;
                            }

                            // A refusal is what most of these get.
                            let _ = connection.execute(&QueryRequest::new(query.clone()));
                            sent.push(query);
                        }
                    }
                }
            }
        }

        let shown = connection.execute(&QueryRequest::new("SHOW MEASUREMENTS"))?;
        let written: Vec<&String> = shown
            .rows
            .iter()
            .flatten()
            .filter_map(|value| match value {
                Value::Text(name) => name.strip_prefix('h')?.parse::<usize>().ok(),
                _ => None,
            })
            .filter_map(|index| sent.get(index))
            .collect();

        eprintln!("sent {} texts rated as reads", sent.len());
        assert!(
            written.is_empty(),
            "rated as reads, and wrote: {written:#?}"
        );
        Ok(())
    })
}

/// The same for Flux, with `--` and `//` around the hidden write.
#[test]
#[ignore = "requires Docker daemon"]
fn v2_nothing_rated_a_read_writes() -> Result<(), DbError> {
    const OPENERS: &[&str] = &[
        "",
        " // ",
        " --1 ",
        " |> filter(fn: (r) => r.host != \"",
        " |> filter(fn: (r) => r.host =~ /",
        " |> filter(fn: (r) => \"${r.host}\" != \"",
        " |> filter(fn: (r) => \"${\"",
    ];
    const INSIDES: &[&str] = &[
        "", "\"", "/", "\\", "\\\\", "\\/", "\\\\/", "\\\"", "$", "$$", "${", "$${", "$\\", "\r",
        "\r\"", "\n", "\n\"", "'", "}", "--", "//",
    ];
    const CLOSERS: &[&str] = &[
        "",
        "\")",
        "/ or true)",
        "\" or true)",
        ")",
        "}\")",
        "}\" != \"\")",
    ];

    const AFTER_OPENERS: &[&str] = &[
        "",
        " // ",
        " |> filter(fn: (r) => r.host != \"",
        " |> filter(fn: (r) => r.host =~ /",
    ];
    const AFTER_INSIDES: &[&str] = &["", "\"", "/", "\\/", "$", "${", "\r\"", "\n", "}"];
    const AFTER_CLOSERS: &[&str] = &["", "\")", "/ or true)", ")"];
    const LINE_ENDS: &[&str] = &[
        "\r", "\n", "\r\n", "\u{2028}", "\u{85}", "\u{c}", "\u{b}", " ",
    ];

    containers::with_influxdb_v2(|cfg| {
        write_v2_line_protocol(
            &cfg.endpoint,
            &cfg.token,
            &cfg.org,
            &cfg.bucket,
            "cpu,host=a value=1 1700000000000",
        )?;
        std::thread::sleep(Duration::from_millis(500));

        let connection = connect_v2(&cfg.endpoint, &cfg.bucket, &cfg.org, &cfg.token)?;
        let bucket = &cfg.bucket;
        let source = format!(r#"from(bucket: "{bucket}") |> range(start: 0)"#);
        let mut sent: Vec<String> = Vec::new();

        for before in ["", "--1 ", "x = 1 ", "// \n", "\"a\" "] {
            for opener in OPENERS {
                for inside in INSIDES {
                    for closer in CLOSERS {
                        for after_opener in AFTER_OPENERS {
                            for after_inside in AFTER_INSIDES {
                                for after_closer in AFTER_CLOSERS {
                                    let flux = format!(
                                        "{before}{source}{opener}{inside}{closer} \
                                         |> set(key: \"_measurement\", value: \"h{}\") \
                                         |> to(bucket: \"{bucket}\")\
                                         {after_opener}{after_inside}{after_closer}",
                                        sent.len()
                                    );

                                    if is_refused(&*connection, &flux) {
                                        continue;
                                    }

                                    // A refusal is what most of these get.
                                    let _ = connection.execute(&flux_query_with_window(&flux));
                                    sent.push(flux);
                                }
                            }
                        }
                    }
                }
            }
        }

        // InfluxQL's reading of a line that Flux reads differently. These hold no
        // `range`, so the driver bounds them by the window, which this point is in.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        write_v2_line_protocol(
            &cfg.endpoint,
            &cfg.token,
            &cfg.org,
            &cfg.bucket,
            &format!("cpu,host=a value=1 {now_ms}"),
        )?;

        for comment in ["--1 //", "--1 ", "--// ", "-- ", "--1 \"", "--1 (/", "--1"] {
            for first_end in LINE_ENDS {
                for statement in [
                    "SELECT value FROM cpu",
                    "SHOW SERIES",
                    "EXPLAIN SELECT value FROM cpu",
                    "",
                ] {
                    for second_end in LINE_ENDS {
                        for filler in ["", "1\n", "\" ", "/) "] {
                            let flux = format!(
                                "{comment}{first_end}{statement}{second_end}{filler}\
                                 to(tables: set(tables: from(bucket: \"{bucket}\"), \
                                 key: \"_measurement\", value: \"h{}\"), bucket: \"{bucket}\")",
                                sent.len()
                            );

                            if is_refused(&*connection, &flux) {
                                continue;
                            }

                            let _ = connection.execute(&flux_query_with_window(&flux));
                            sent.push(flux);
                        }
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_millis(500));
        let all = format!(
            r#"import "influxdata/influxdb/schema"
schema.measurements(bucket: "{bucket}")"#
        );
        let shown = connection.execute(&flux_query_with_window(&all))?;
        let written: Vec<&String> = shown
            .rows
            .iter()
            .flatten()
            .filter_map(|value| match value {
                Value::Text(name) => name.strip_prefix('h')?.parse::<usize>().ok(),
                _ => None,
            })
            .filter_map(|index| sent.get(index))
            .collect();

        eprintln!("sent {} texts rated as reads", sent.len());
        assert!(
            written.is_empty(),
            "rated as reads, and wrote: {written:#?}"
        );
        assert!(!shown.rows.is_empty(), "the listing must have worked");
        Ok(())
    })
}
