//! `SavedChart` — persisted record for a named chart with its query and spec.
//!
//! `SavedChart` is the domain type for a named chart. Persistence is handled by
//! `dbflux_ui_base::SavedChartManager` backed by SQLite.
//!
//! # Crate placement
//!
//! `SavedChart` lives in `dbflux_components` rather than `dbflux_core` because
//! it embeds `ChartSpec` and `BindingSpec`, which are owned by this crate.
//!
//! # Schema note
//!
//! `SavedChartSource` was introduced as a breaking change from the old
//! `query: String` field. The `chart-everywhere` feature was unreleased at the
//! time, so no migration is needed. Old JSON without a `source` field
//! deserialises to `SavedChartSource::Query { query: "" }` via the
//! `#[serde(default)]` path.

use crate::chart::{BindingSpec, ChartSpec};
use chrono::{DateTime, Utc};
use dbflux_core::{
    CollectionRef, ExecutionContext, ExecutionSourceContext, Identifiable, ResolvedWindow,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// SavedChartSource
// ---------------------------------------------------------------------------

/// One CloudWatch metric series persisted on a multi-series chart.
///
/// Each series carries its own namespace, metric name, dimensions, sampling
/// period, statistic, region override, and optional display label. The chart
/// engine receives one Y column per series in the result set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricSeries {
    pub namespace: String,
    pub metric_name: String,
    /// Ordered (name, value) dimension pairs. Empty for scalar metrics.
    pub dimensions: Vec<(String, String)>,
    /// Sampling period in seconds.
    pub period_seconds: u32,
    /// CloudWatch statistic (e.g. "Average", "Sum").
    pub statistic: String,
    /// AWS region override; `None` means use the connection's default region.
    pub region: Option<String>,
    /// Optional display label for the legend; falls back to `metric_name` when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The data source for a saved chart.
///
/// `Query` wraps a SQL/Flux/etc. query string and is executed inside
/// `ChartDocument`. `Collection` represents a collection-browse source
/// (Mongo collection, InfluxDB measurement) — opening it re-opens the
/// underlying `DataDocument` in chart mode. `Metric` persists a list of
/// CloudWatch metric series so one chart can plot N metrics on shared axes;
/// importing a CloudWatch widget with multiple `metrics` entries yields one
/// `SavedChart` with one `MetricSeries` per entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum SavedChartSource {
    /// A query-string source executed inside ChartDocument.
    Query { query: String },
    /// A collection-browse source (no query string; the driver builds the request).
    Collection {
        collection_ref: CollectionRef,
        /// The time window that was active when the chart was saved, if any.
        time_window: Option<ResolvedWindow>,
    },
    /// A CloudWatch metric source. Carries one or more series; the chart opens
    /// via `MetricSource` (which holds the same list) and the driver issues a
    /// single GetMetricData request batching every series.
    Metric { series: Vec<MetricSeries> },
    /// A per-driver instance metric source. Identified by a driver-defined
    /// `metric_id` string (e.g. `"pg.cache_hit_ratio"`). The chart opens a
    /// time-bounded query via `InstanceMetricQuery`.
    InstanceMetric { metric_id: String },
}

impl Default for SavedChartSource {
    fn default() -> Self {
        SavedChartSource::Query {
            query: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Quick-select time-range presets stored alongside a chart.
///
/// Mirrors the variants in `dbflux_components::common::time_range::TimeRange` but
/// lives here so `SavedChart` can be (de)serialized without a GPUI dependency.
/// Phase D will bridge between the two types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TimeRangePreset {
    Last15min,
    LastHour,
    Last6Hours,
    #[default]
    Last24Hours,
    Last7Days,
}

/// Refresh behaviour for a saved chart when it is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SavedChartRefreshPolicy {
    /// No automatic refresh; user must trigger re-execution manually.
    #[default]
    Off,
    /// Re-execute the query every `every_secs` seconds.
    Interval { every_secs: u32 },
    /// Re-execute once automatically when the chart is opened.
    OnOpen,
}

// ---------------------------------------------------------------------------
// SavedChartContext
// ---------------------------------------------------------------------------

/// Where a saved query chart runs, kept from its execution context at Save.
///
/// Names only, which are no more sensitive than the query saved beside them.
/// The rest of `ExecutionContext` stays out on purpose: the connection is the
/// chart's `profile_id`, and the time bounds come from the chart's own range.
///
/// Empty on a chart saved before this existed: it sends no database and runs on
/// the connection's own.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedChartContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// What the query reads from within the connection, such as log groups.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    /// The driver's query mode, which names the language the query is written
    /// in and so the language the read check judges it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_mode: Option<String>,
}

impl SavedChartContext {
    /// Keeps what can be stored of `context`. `has_databases` is false for a
    /// driver with one fixed database, where there is no other one to read and
    /// a chart has nothing to remember.
    pub fn capture(context: &ExecutionContext, has_databases: bool) -> Self {
        let (targets, query_mode) = match &context.source {
            Some(ExecutionSourceContext::CollectionWindow {
                targets,
                query_mode,
                ..
            }) => (targets.clone(), query_mode.clone()),
            _ => (Vec::new(), None),
        };

        Self {
            database: context.database.clone().filter(|_| has_databases),
            schema: context.schema.clone(),
            container: context.container.clone(),
            targets,
            query_mode,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The execution context a reopened chart runs in. A run replaces the
    /// bounds with the chart's time range, so `start_ms` and `end_ms` only
    /// matter to a run before the chart has one.
    pub fn execution_context(&self, start_ms: i64, end_ms: i64) -> ExecutionContext {
        let has_source = !self.targets.is_empty() || self.query_mode.is_some();

        ExecutionContext {
            connection_id: None,
            database: self.database.clone(),
            schema: self.schema.clone(),
            container: self.container.clone(),
            source: has_source.then(|| ExecutionSourceContext::CollectionWindow {
                targets: self.targets.clone(),
                start_ms,
                end_ms,
                query_mode: self.query_mode.clone(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// SavedChart
// ---------------------------------------------------------------------------

/// A persisted chart record.
///
/// Only the query string (or collection reference) is persisted — raw result
/// data is never stored. `chart_spec` and `bindings` carry the full rendering
/// configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedChart {
    /// Stable identity for upsert and deduplication.
    pub id: Uuid,
    /// User-supplied display name.
    pub name: String,
    /// The connection profile this chart was created under.
    pub profile_id: Uuid,
    /// Data source for this chart.
    ///
    /// Old JSON without this field (or with only a `query` top-level key) will
    /// fail to parse; since the chart-everywhere feature was unreleased when
    /// this field was introduced, no migration is needed.
    #[serde(default)]
    pub source: SavedChartSource,
    /// Where a query source runs. Absent from anything written before it
    /// existed, which then reads as an empty context.
    #[serde(default, skip_serializing_if = "SavedChartContext::is_empty")]
    pub context: SavedChartContext,
    /// Serialized chart spec. Uses `#[serde(default)]` fields so old JSON
    /// without newer fields is still loadable.
    pub chart_spec: ChartSpec,
    /// Column bindings for the AxisBar.
    pub bindings: BindingSpec,
    /// Optional time-range preset applied when the chart is opened.
    #[serde(default)]
    pub time_range_preset: Option<TimeRangePreset>,
    /// Refresh policy applied while the chart is open.
    #[serde(default)]
    pub refresh_policy: SavedChartRefreshPolicy,
    /// Creation timestamp (UTC).
    pub created_at: DateTime<Utc>,
    /// Last-modified timestamp (UTC); updated on every upsert.
    pub updated_at: DateTime<Utc>,
}

impl SavedChart {
    /// Create a new `SavedChart` from a query string source.
    pub fn new_query(
        name: String,
        profile_id: Uuid,
        query: String,
        chart_spec: ChartSpec,
        bindings: BindingSpec,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            profile_id,
            source: SavedChartSource::Query { query },
            context: SavedChartContext::default(),
            chart_spec,
            bindings,
            time_range_preset: None,
            refresh_policy: SavedChartRefreshPolicy::Off,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a new `SavedChart` from a collection-browse source.
    pub fn new_collection(
        name: String,
        profile_id: Uuid,
        collection_ref: CollectionRef,
        time_window: Option<ResolvedWindow>,
        chart_spec: ChartSpec,
        bindings: BindingSpec,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            profile_id,
            source: SavedChartSource::Collection {
                collection_ref,
                time_window,
            },
            context: SavedChartContext::default(),
            chart_spec,
            bindings,
            time_range_preset: None,
            refresh_policy: SavedChartRefreshPolicy::Off,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a new `SavedChart` from a CloudWatch metric source.
    ///
    /// `series` must be non-empty; callers building from a single metric pass
    /// a one-element vec. Each entry is plotted as a distinct line/series on
    /// the chart.
    pub fn new_metric(
        name: String,
        profile_id: Uuid,
        series: Vec<MetricSeries>,
        chart_spec: ChartSpec,
        bindings: BindingSpec,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            profile_id,
            source: SavedChartSource::Metric { series },
            context: SavedChartContext::default(),
            chart_spec,
            bindings,
            time_range_preset: None,
            refresh_policy: SavedChartRefreshPolicy::Off,
            created_at: now,
            updated_at: now,
        }
    }

    /// Convenience: returns the query string if this chart has a `Query` source.
    pub fn query(&self) -> Option<&str> {
        match &self.source {
            SavedChartSource::Query { query } => Some(query.as_str()),
            SavedChartSource::Collection { .. }
            | SavedChartSource::Metric { .. }
            | SavedChartSource::InstanceMetric { .. } => None,
        }
    }

    /// Returns `true` if this chart has a `Collection` source.
    pub fn is_collection_source(&self) -> bool {
        matches!(self.source, SavedChartSource::Collection { .. })
    }

    /// Returns `true` if this chart has a `Metric` source.
    pub fn is_metric_source(&self) -> bool {
        matches!(self.source, SavedChartSource::Metric { .. })
    }
}

impl Identifiable for SavedChart {
    fn id(&self) -> Uuid {
        self.id
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::{AggKind, AxisKind, AxisSpec, ChartKind, SeriesSpec};
    use dbflux_core::CollectionRef;

    fn sample_spec() -> ChartSpec {
        ChartSpec {
            kind: ChartKind::Line,
            x_axis: AxisSpec {
                column_index: 0,
                label: "time".to_string(),
                kind: AxisKind::Time,
                unit: None,
            },
            series: vec![SeriesSpec {
                column_index: 1,
                label: "value".to_string(),
                color_slot: 0,
                group_value: None,
            }],
            legend_visible: false,
            decimation_threshold: 10_000,
            binding: BindingSpec::default(),
            track_source_indices: false,
            y_scale: crate::chart::YScale::Linear,
        }
    }

    fn console_context() -> ExecutionContext {
        ExecutionContext {
            connection_id: Some(Uuid::new_v4()),
            database: Some("analytics".to_string()),
            schema: Some("reporting".to_string()),
            container: Some("orders".to_string()),
            source: Some(ExecutionSourceContext::CollectionWindow {
                targets: vec!["/aws/app".to_string()],
                start_ms: 11,
                end_ms: 22,
                query_mode: Some("sql".to_string()),
            }),
        }
    }

    /// Names travel; the connection id and the time bounds do not, since they
    /// only mean something on this machine at this moment.
    #[test]
    fn capture_keeps_names_and_leaves_out_the_connection_and_the_bounds() {
        let stored = SavedChartContext::capture(&console_context(), true);

        assert_eq!(
            stored,
            SavedChartContext {
                database: Some("analytics".to_string()),
                schema: Some("reporting".to_string()),
                container: Some("orders".to_string()),
                targets: vec!["/aws/app".to_string()],
                query_mode: Some("sql".to_string()),
            }
        );

        let restored = stored.execution_context(100, 200);
        assert_eq!(restored.connection_id, None);
        assert_eq!(restored.database.as_deref(), Some("analytics"));
        assert_eq!(restored.schema.as_deref(), Some("reporting"));
        assert_eq!(restored.container.as_deref(), Some("orders"));
        assert!(matches!(
            restored.source,
            Some(ExecutionSourceContext::CollectionWindow {
                ref targets,
                start_ms: 100,
                end_ms: 200,
                ref query_mode,
            }) if targets == &["/aws/app"] && query_mode.as_deref() == Some("sql")
        ));
    }

    #[test]
    fn capture_keeps_no_database_for_a_driver_without_any() {
        let stored = SavedChartContext::capture(&console_context(), false);

        assert_eq!(stored.database, None);
        assert_eq!(stored.schema.as_deref(), Some("reporting"));
    }

    /// No source of its own: a chart then keeps the window source its data
    /// source builds, without targets, as before.
    #[test]
    fn an_empty_context_restores_to_an_empty_execution_context() {
        let restored = SavedChartContext::default().execution_context(100, 200);

        assert_eq!(restored.database, None);
        assert!(restored.source.is_none());
    }

    /// What portable JSON of a chart holds, and what one written before the
    /// field existed reads as.
    #[test]
    fn the_context_round_trips_and_an_old_record_reads_as_empty() {
        let mut chart = sample_chart("c", Uuid::new_v4());
        chart.context = SavedChartContext::capture(&console_context(), true);

        let json = serde_json::to_value(&chart).expect("serialize");
        let parsed: SavedChart = serde_json::from_value(json.clone()).expect("deserialize");
        assert_eq!(parsed.context, chart.context);

        let mut old = json;
        old.as_object_mut().expect("an object").remove("context");
        let parsed: SavedChart = serde_json::from_value(old).expect("old record");
        assert!(parsed.context.is_empty());

        let empty = serde_json::to_value(sample_chart("c", Uuid::new_v4())).expect("serialize");
        assert!(
            empty.get("context").is_none(),
            "a chart without a context writes what an old build reads"
        );
    }

    fn sample_chart(name: &str, profile_id: Uuid) -> SavedChart {
        SavedChart::new_query(
            name.to_string(),
            profile_id,
            "SELECT * FROM test".to_string(),
            sample_spec(),
            BindingSpec {
                x: 0,
                y: vec![1],
                group_by: None,
                filter: None,
                aggregation: AggKind::None,
            },
        )
    }

    fn sample_collection_chart(name: &str, profile_id: Uuid) -> SavedChart {
        SavedChart::new_collection(
            name.to_string(),
            profile_id,
            CollectionRef::new("mydb", "measurements"),
            None,
            sample_spec(),
            BindingSpec {
                x: 0,
                y: vec![1],
                group_by: None,
                filter: None,
                aggregation: AggKind::None,
            },
        )
    }

    fn sample_metric_chart(name: &str, profile_id: Uuid) -> SavedChart {
        SavedChart::new_metric(
            name.to_string(),
            profile_id,
            vec![MetricSeries {
                namespace: "AWS/EC2".to_string(),
                metric_name: "CPUUtilization".to_string(),
                dimensions: vec![
                    ("InstanceId".to_string(), "i-12345".to_string()),
                    ("Region".to_string(), "us-east-1".to_string()),
                ],
                period_seconds: 300,
                statistic: "Average".to_string(),
                region: Some("us-east-1".to_string()),
                label: None,
            }],
            sample_spec(),
            BindingSpec {
                x: 0,
                y: vec![1],
                group_by: None,
                filter: None,
                aggregation: AggKind::None,
            },
        )
    }

    /// Metric source: `query()` returns `None` and `is_metric_source()` returns `true`.
    #[test]
    fn metric_source_query_returns_none() {
        let profile_id = Uuid::new_v4();
        let chart = sample_metric_chart("test", profile_id);
        assert_eq!(chart.query(), None);
        assert!(!chart.is_collection_source());
        assert!(chart.is_metric_source());
    }

    /// Metric source: every series field survives a round-trip through `SavedChartSource`.
    #[test]
    fn metric_source_fields_accessible() {
        let profile_id = Uuid::new_v4();
        let chart = sample_metric_chart("my-chart", profile_id);
        if let SavedChartSource::Metric { series } = &chart.source {
            assert_eq!(series.len(), 1);
            let s = &series[0];
            assert_eq!(s.namespace, "AWS/EC2");
            assert_eq!(s.metric_name, "CPUUtilization");
            assert_eq!(s.dimensions.len(), 2);
            assert_eq!(
                s.dimensions[0],
                ("InstanceId".to_string(), "i-12345".to_string())
            );
            assert_eq!(
                s.dimensions[1],
                ("Region".to_string(), "us-east-1".to_string())
            );
            assert_eq!(s.period_seconds, 300);
            assert_eq!(s.statistic, "Average");
            assert_eq!(s.region.as_deref(), Some("us-east-1"));
            assert!(s.label.is_none());
        } else {
            panic!("expected Metric variant");
        }
    }

    /// Multi-series metric chart: each series is independently accessible.
    #[test]
    fn metric_source_supports_multiple_series() {
        let profile_id = Uuid::new_v4();
        let chart = SavedChart::new_metric(
            "multi".to_string(),
            profile_id,
            vec![
                MetricSeries {
                    namespace: "AWS/RDS".to_string(),
                    metric_name: "CPUUtilization".to_string(),
                    dimensions: vec![(
                        "DBInstanceIdentifier".to_string(),
                        "primary-db".to_string(),
                    )],
                    period_seconds: 60,
                    statistic: "Average".to_string(),
                    region: Some("us-east-1".to_string()),
                    label: None,
                },
                MetricSeries {
                    namespace: "AWS/RDS".to_string(),
                    metric_name: "CPUUtilization".to_string(),
                    dimensions: vec![(
                        "DBInstanceIdentifier".to_string(),
                        "replica-db".to_string(),
                    )],
                    period_seconds: 60,
                    statistic: "Average".to_string(),
                    region: Some("us-east-1".to_string()),
                    label: Some("Replica".to_string()),
                },
            ],
            sample_spec(),
            BindingSpec::default(),
        );

        let SavedChartSource::Metric { series } = &chart.source else {
            panic!("expected Metric variant");
        };
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].dimensions[0].1, "primary-db");
        assert_eq!(series[1].dimensions[0].1, "replica-db");
        assert_eq!(series[1].label.as_deref(), Some("Replica"));
    }

    /// T-CE-I07: Query source query() helper returns Some.
    #[test]
    fn query_helper_returns_some_for_query_source() {
        let profile_id = Uuid::new_v4();
        let chart = sample_chart("test", profile_id);
        assert_eq!(chart.query(), Some("SELECT * FROM test"));
        assert!(!chart.is_collection_source());
    }

    /// T-CE-I07: Collection source query() helper returns None.
    #[test]
    fn query_helper_returns_none_for_collection_source() {
        let profile_id = Uuid::new_v4();
        let chart = sample_collection_chart("test", profile_id);
        assert_eq!(chart.query(), None);
        assert!(chart.is_collection_source());
    }
}
