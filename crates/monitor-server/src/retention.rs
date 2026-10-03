//! Data retention and downsampling.
//!
//! Design: `docs/superpowers/specs/2026-09-19-product-structure-design.md` §8
//!
//! Two things:
//!   1. Compress sealed hours into aggregate rows (raw 10s data rolls off
//!      later; long-term trends live in these aggregates);
//!   2. Rolling delete of expired raw data and aggregate rows.
//!
//! Why this itself is "cognitive-light": a monitoring tool that fills its
//! users' disks is the most ironic kind of failure. So retention is
//! automatic, and **retention task failures must show up in the todo list**.

/// Raw data retention in days. At central scale (10 nodes) ≈ 224 MB; at
/// upper scale can lower to 7 days.
pub const RAW_RETENTION_DAYS: i64 = 14;

/// Hourly aggregate retention in days (≈ 2 years).
pub const HOURLY_RETENTION_DAYS: i64 = 730;

/// Max hours processed in one retention run — prevents catching up too much
/// after a long downtime.
const MAX_HOURS_PER_RUN: i64 = 48;

/// Lookback window when no historical aggregation exists yet (hours).
const INITIAL_LOOKBACK_HOURS: i64 = 24;

pub const NANOS_PER_HOUR: i64 = 3_600_000_000_000;

/// Floor down to the start of an hour.
pub const fn hour_start(ts_ns: i64) -> i64 {
    ts_ns.div_euclid(NANOS_PER_HOUR) * NANOS_PER_HOUR
}

/// Aggregate result for one (node, hour, metric).
#[derive(Debug, Clone, PartialEq)]
pub struct MetricAgg {
    pub metric: String,
    pub avg: f64,
    pub min: f64,
    pub max: f64,
    pub first: f64,
    pub last: f64,
    pub samples: i64,
}

/// Compress "time-ordered (metric name, value) pairs" into one aggregate row
/// per metric.
///
/// Keeping first / last is so counter-type metrics (network bytes) can still
/// recover rate: `rate = (last - first) / 3600`; storing only avg/min/max
/// makes counters unrecoverable.
pub fn aggregate(samples: &[(String, f64)]) -> Vec<MetricAgg> {
    let mut out: Vec<MetricAgg> = Vec::new();
    let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut sums: Vec<f64> = Vec::new();

    for (metric, value) in samples {
        if let Some(&i) = index.get(metric.as_str()) {
            let a: &mut MetricAgg = &mut out[i];
            a.min = a.min.min(*value);
            a.max = a.max.max(*value);
            a.last = *value;
            a.samples += 1;
            sums[i] += *value;
        } else {
            index.insert(metric.as_str(), out.len());
            sums.push(*value);
            out.push(MetricAgg {
                metric: metric.clone(),
                avg: 0.0, // computed once at the end
                min: *value,
                max: *value,
                first: *value,
                last: *value,
                samples: 1,
            });
        }
    }

    for (i, a) in out.iter_mut().enumerate() {
        a.avg = sums[i] / f64::from(i32::try_from(a.samples).unwrap_or(1));
    }
    out
}

/// All aggregatable (metric name, value) pairs in one batch.
///
/// In addition to `metrics[]`, **network cumulative values must be added here**:
/// they're not in `metrics[]` (one per interface), they need to be summed
/// across interfaces — same logic as `routes::extract_metric`. Symptom of
/// skipping this step: recent network curves look normal, but long windows
/// (using hourly aggregation) are empty.
pub fn metrics_of(batch: &TelemetryBatch) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = batch
        .metrics
        .iter()
        .map(|m| (m.name.clone(), m.value))
        .collect();
    for name in ["host.net.rx_bytes", "host.net.tx_bytes", "host.mem.usage"] {
        if let Some(v) = crate::routes::extract_metric(batch, name) {
            out.push((name.to_string(), v));
        }
    }
    out
}

// ---------- Storage and scheduling-related implementation ----------

use std::collections::BTreeMap;
use std::time::Duration;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use prost::Message as _;
use tracing::{info, warn};
use zhiwei_proto::telemetry::TelemetryBatch;

use crate::routes::{err, read_auth_ok};
use crate::state::AppState;

const NANOS_PER_DAY: i64 = 86_400_000_000_000;

/// `source_ref` for platform-internal anomaly alerts (one per issue type,
/// not stacked per occurrence).
const RETENTION_SOURCE_REF: &str = "retention";

/// `GET /v1/retention` — retention policy. For the settings page:
/// the numbers are only defined in retention.rs, frontend doesn't hardcode,
/// avoiding drift between the two sides.
pub async fn retention_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    Json(serde_json::json!({
        "raw_days": RAW_RETENTION_DAYS,
        "hourly_days": HOURLY_RETENTION_DAYS,
    }))
    .into_response()
}

/// Wait a bit after startup before running the first round, to avoid
/// contention with startup-time collection / migrations.
const BOOT_DELAY: Duration = Duration::from_secs(30);

/// Interval between rounds. Retention isn't real-time, 10 minutes is plenty.
const INTERVAL: Duration = Duration::from_secs(600);

#[derive(Debug, Default)]
pub struct Stats {
    /// How many (node, hour) buckets were processed
    pub buckets: i64,
    /// How many aggregate rows were written / upserted
    pub rows: i64,
    pub raw_deleted: u64,
    pub hourly_deleted: u64,
}

/// Run one retention round: compress sealed hours into aggregates, then roll
/// off expired data.
///
/// **Does not delete data that hasn't been aggregated yet**: the deletion
/// line is `min(raw retention line, how far aggregation got)`, so after a long
/// downtime (watermark far behind) aggregation catches up first, then deletes
/// resume — rather than throwing away raw data before it was aggregated.
pub async fn run_once(state: &AppState, now_ns: i64) -> anyhow::Result<Stats> {
    let telemetry = state.storage.telemetry();
    let now_hour = hour_start(now_ns);
    let watermark = telemetry.max_hourly_ts().await?;
    let aggregated_through = watermark
        .map_or(now_hour - INITIAL_LOOKBACK_HOURS * NANOS_PER_HOUR, |w| {
            w + NANOS_PER_HOUR
        });

    // Only process already-sealed hours (excluding the current hour),
    // max 48 at a time
    let start = aggregated_through.max(now_hour - MAX_HOURS_PER_RUN * NANOS_PER_HOUR);
    let end = now_hour;

    let mut totals = Stats::default();
    if start < end {
        let rows = telemetry.batches_in_window(start, end).await?;
        let mut buckets: BTreeMap<(String, i64), Vec<(String, f64)>> = BTreeMap::new();
        for (node_id, ts, payload) in rows {
            let Ok(batch) = TelemetryBatch::decode(&payload[..]) else {
                continue; // undecodable legacy data shouldn't tank the whole retention round
            };
            let bucket = buckets.entry((node_id, hour_start(ts))).or_default();
            bucket.extend(metrics_of(&batch));
        }
        for ((node_id, hour), samples) in buckets {
            for a in aggregate(&samples) {
                telemetry
                    .upsert_hourly(
                        &node_id, hour, &a.metric, a.avg, a.min, a.max, a.first, a.last, a.samples,
                    )
                    .await?;
                totals.rows += 1;
            }
            totals.buckets += 1;
        }
    }

    let raw_cutoff = now_ns - RAW_RETENTION_DAYS * NANOS_PER_DAY;
    let hourly_cutoff = now_ns - HOURLY_RETENTION_DAYS * NANOS_PER_DAY;
    // Deletion line doesn't cross "how far aggregation got" — otherwise raw
    // data that hasn't been aggregated yet would be discarded.
    totals.raw_deleted = telemetry
        .delete_raw_before(raw_cutoff.min(aggregated_through))
        .await?;
    totals.hourly_deleted = telemetry.delete_hourly_before(hourly_cutoff).await?;
    Ok(totals)
}

/// Background loop. On failure, open a platform alert (which shows up in the
/// todo list and goes through existing webhook notifications); auto-closes
/// after recovery — retention itself also needs to be observed.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(BOOT_DELAY).await;
        loop {
            let now_ns = zhiwei_common::Timestamp::now().unix_nano();
            match run_once(&state, now_ns).await {
                Ok(totals) => {
                    if totals.rows > 0 || totals.raw_deleted > 0 || totals.hourly_deleted > 0 {
                        info!(
                            buckets = totals.buckets,
                            rows = totals.rows,
                            raw_deleted = totals.raw_deleted,
                            hourly_deleted = totals.hourly_deleted,
                            "Retention done"
                        );
                    }
                    let _ = state
                        .storage
                        .alerts()
                        .resolve_platform_alerts(RETENTION_SOURCE_REF, now_ns)
                        .await;
                }
                Err(e) => {
                    warn!(error = %e, "Retention failed");
                    let message = format!(
                        "Data retention task failed: {e}. Raw data is not deleted yet (not lost), \
                         but disk usage will continue to grow — this alert closes automatically once fixed."
                    );
                    let _ = state
                        .storage
                        .alerts()
                        .open_platform_alert(
                            RETENTION_SOURCE_REF,
                            "Retention task failed",
                            "critical",
                            &message,
                            now_ns,
                        )
                        .await;
                }
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    // Tests assert exact computed averages (e.g. `(10+20+30)/3 == 20.0`,
    // counter deltas `4_600 - 1_000 == 3_600`). Floating-point equality is
    // intentional — values are integer-valued by construction.
    #![allow(clippy::float_cmp)]

    use super::*;

    #[test]
    fn hour_start_floors_to_the_hour() {
        // 2026-09-19T12:34:56Z
        let t = 1_789_826_096_000_000_000i64;
        assert_eq!(
            hour_start(t),
            1_789_826_096_000_000_000i64 / NANOS_PER_HOUR * NANOS_PER_HOUR
        );
        // Exactly on the hour: no change
        assert_eq!(hour_start(2 * NANOS_PER_HOUR), 2 * NANOS_PER_HOUR);
        // 1 ns before the hour: falls into the previous hour
        assert_eq!(hour_start(2 * NANOS_PER_HOUR - 1), NANOS_PER_HOUR);
    }

    #[test]
    fn aggregate_of_nothing_is_nothing() {
        assert!(aggregate(&[]).is_empty());
    }

    #[test]
    fn aggregate_groups_by_metric_keeping_time_order() {
        let s = vec![
            ("host.cpu.usage".to_string(), 10.0),
            ("host.mem.usage".to_string(), 40.0),
            ("host.cpu.usage".to_string(), 20.0),
            ("host.mem.usage".to_string(), 50.0),
            ("host.cpu.usage".to_string(), 30.0),
        ];
        let out = aggregate(&s);
        assert_eq!(out.len(), 2);

        let cpu = out.iter().find(|a| a.metric == "host.cpu.usage").unwrap();
        assert_eq!(cpu.first, 10.0);
        assert_eq!(cpu.last, 30.0);
        assert_eq!(cpu.min, 10.0);
        assert_eq!(cpu.max, 30.0);
        assert_eq!(cpu.avg, 20.0);
        assert_eq!(cpu.samples, 3);

        let mem = out.iter().find(|a| a.metric == "host.mem.usage").unwrap();
        assert_eq!(mem.first, 40.0);
        assert_eq!(mem.last, 50.0);
        assert_eq!(mem.avg, 45.0);
        assert_eq!(mem.samples, 2);
    }

    #[test]
    fn aggregate_keeps_counter_first_and_last_for_rate() {
        // Cumulative network bytes: monotonically increasing, first/last
        // determine this hour's rate
        let s = vec![
            ("host.net.rx_bytes".to_string(), 1_000.0),
            ("host.net.rx_bytes".to_string(), 4_600.0),
        ];
        let out = aggregate(&s);
        assert_eq!(out[0].first, 1_000.0);
        assert_eq!(out[0].last, 4_600.0);
        // Doesn't mean "3600 bytes transferred in this hour total", but
        // enables rate recovery:
        assert_eq!(out[0].last - out[0].first, 3_600.0);
    }

    #[test]
    fn aggregate_of_a_single_sample_has_all_equal_bounds() {
        let s = vec![("host.disk.usage".to_string(), 92.5)];
        let out = aggregate(&s);
        assert_eq!(out[0].avg, 92.5);
        assert_eq!(out[0].min, 92.5);
        assert_eq!(out[0].max, 92.5);
        assert_eq!(out[0].first, 92.5);
        assert_eq!(out[0].last, 92.5);
        assert_eq!(out[0].samples, 1);
    }

    #[test]
    fn metrics_of_sums_network_counters_across_interfaces() {
        // Network cumulative values aren't in metrics[] (one per interface),
        // but long windows still need to draw them; symptom of skipping this
        // step is "recent looks normal, long window empty curve".
        use zhiwei_proto::telemetry::{Metric, NetworkInterface, TelemetryBatch};
        let batch = TelemetryBatch {
            node_id: "n1".into(),
            metrics: vec![Metric {
                name: "host.cpu.usage".into(),
                value: 12.0,
                labels: std::collections::HashMap::default(),
            }],
            network: vec![
                NetworkInterface {
                    name: "eth0".into(),
                    rx_bytes: 100,
                    tx_bytes: 10,
                    ..Default::default()
                },
                NetworkInterface {
                    name: "eth1".into(),
                    rx_bytes: 200,
                    tx_bytes: 20,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let m = metrics_of(&batch);
        assert!(m.contains(&("host.cpu.usage".to_string(), 12.0)));
        assert!(m.contains(&("host.net.rx_bytes".to_string(), 300.0)));
        assert!(m.contains(&("host.net.tx_bytes".to_string(), 30.0)));
    }
}
