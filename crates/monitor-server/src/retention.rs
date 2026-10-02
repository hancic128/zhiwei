//! 数据留存与降采样。
//!
//! 设计：`docs/superpowers/specs/2026-09-19-product-structure-design.md` §8
//!
//! 两件事：
//!   1. 把已封口的小时压成聚合行（原始 10 秒数据之后会滚掉，长趋势由它承载）；
//!   2. 滚动删除过期的原始数据与聚合行。
//!
//! 为什么这件事本身属于「认知轻」：一个会把自己用户的磁盘填满的监控工具，
//! 是最讽刺的失败方式。所以留存是自动的，而且**留存任务失败必须出现在待办里**。

/// 原始数据保留天数。中心规模（10 节点）下约 224 MB；上限规模可下调到 7 天。
pub const RAW_RETENTION_DAYS: i64 = 14;

/// 小时聚合保留天数（约 2 年）。
pub const HOURLY_RETENTION_DAYS: i64 = 730;

/// 一次留存运行最多处理多少个小时——防止长期停机后一次性补太多。
const MAX_HOURS_PER_RUN: i64 = 48;

/// 没有历史聚合时的回溯窗口（小时）。
const INITIAL_LOOKBACK_HOURS: i64 = 24;

pub const NANOS_PER_HOUR: i64 = 3_600_000_000_000;

/// 向下取整到一个小时的起点。
pub fn hour_start(ts_ns: i64) -> i64 {
    ts_ns.div_euclid(NANOS_PER_HOUR) * NANOS_PER_HOUR
}

/// 一个 (节点, 小时, 指标) 的聚合结果。
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

/// 把「按时间顺序排列的 (指标名, 值)」压成每个指标一条聚合。
///
/// 保留 first / last 是为了计数器类指标（网络字节数）还能还原速率：
/// `rate = (last - first) / 3600`；只存 avg/min/max 的话计数器就没法还原了。
pub fn aggregate(samples: &[(String, f64)]) -> Vec<MetricAgg> {
    let mut out: Vec<MetricAgg> = Vec::new();
    let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut sums: Vec<f64> = Vec::new();

    for (metric, value) in samples {
        match index.get(metric.as_str()) {
            Some(&i) => {
                let a: &mut MetricAgg = &mut out[i];
                a.min = a.min.min(*value);
                a.max = a.max.max(*value);
                a.last = *value;
                a.samples += 1;
                sums[i] += *value;
            }
            None => {
                index.insert(metric.as_str(), out.len());
                sums.push(*value);
                out.push(MetricAgg {
                    metric: metric.clone(),
                    avg: 0.0, // 最后统一算
                    min: *value,
                    max: *value,
                    first: *value,
                    last: *value,
                    samples: 1,
                });
            }
        }
    }

    for (i, a) in out.iter_mut().enumerate() {
        a.avg = sums[i] / a.samples as f64;
    }
    out
}

/// 一批遥测里所有可聚合的 (指标名, 值)。
///
/// 除 `metrics[]` 之外，**网络累计量也要在这里补上**：它不在 `metrics[]` 里
/// （每网卡一份），要跨网卡求和——与 `routes::extract_metric` 同一口径。
/// 漏了这一步的症状是：最近的网络曲线正常，但长窗口（走小时聚合）是空的。
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

// ---------- 与存储、调度有关的实现 ----------

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

/// 平台自身异常告警的 `source_ref`（一类问题一条，不按次堆积）。
const RETENTION_SOURCE_REF: &str = "retention";

/// `GET /v1/retention` —— 留存策略。设置页展示用：
/// 数字只有 retention.rs 一处定义，前端不硬编码，避免两边漂移。
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

/// 启动后先等一会儿再跑第一轮，避免和启动时的采集/迁移抢资源。
const BOOT_DELAY: Duration = Duration::from_secs(30);

/// 两轮之间的间隔。留存不是实时任务，10 分钟足够。
const INTERVAL: Duration = Duration::from_secs(600);

#[derive(Debug, Default)]
pub struct Stats {
    /// 处理了多少个 (节点, 小时)
    pub buckets: i64,
    /// 写入 / 覆盖了多少行聚合
    pub rows: i64,
    pub raw_deleted: u64,
    pub hourly_deleted: u64,
}

/// 跑一轮留存：先把已封口的小时压成聚合，再滚掉过期数据。
///
/// **不会删掉还没聚合的数据**：删除线取 `min(原始保留线, 已聚合到哪儿)`，
/// 所以长期停机之后（水位线落后很多）会先补聚合、再恢复删除，
/// 而不是把没来得及聚合的原始数据直接丢掉。
pub async fn run_once(state: &AppState, now_ns: i64) -> anyhow::Result<Stats> {
    let telemetry = state.storage.telemetry();
    let now_hour = hour_start(now_ns);
    let watermark = telemetry.max_hourly_ts().await?;
    let aggregated_through = watermark
        .map(|w| w + NANOS_PER_HOUR)
        .unwrap_or(now_hour - INITIAL_LOOKBACK_HOURS * NANOS_PER_HOUR);

    // 只处理已经封口的小时（不含当前这一小时），一次最多 48 个
    let start = aggregated_through.max(now_hour - MAX_HOURS_PER_RUN * NANOS_PER_HOUR);
    let end = now_hour;

    let mut stats = Stats::default();
    if start < end {
        let rows = telemetry.batches_in_window(start, end).await?;
        let mut buckets: BTreeMap<(String, i64), Vec<(String, f64)>> = BTreeMap::new();
        for (node_id, ts, payload) in rows {
            let Ok(batch) = TelemetryBatch::decode(&payload[..]) else {
                continue; // 解不开的旧数据不该拖垮整轮留存
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
                stats.rows += 1;
            }
            stats.buckets += 1;
        }
    }

    let raw_cutoff = now_ns - RAW_RETENTION_DAYS * NANOS_PER_DAY;
    let hourly_cutoff = now_ns - HOURLY_RETENTION_DAYS * NANOS_PER_DAY;
    // 删除线不越过「已聚合到哪儿」，否则会丢掉还没聚合的原始数据
    stats.raw_deleted = telemetry
        .delete_raw_before(raw_cutoff.min(aggregated_through))
        .await?;
    stats.hourly_deleted = telemetry.delete_hourly_before(hourly_cutoff).await?;
    Ok(stats)
}

/// 后台循环。失败时开一条平台告警（会出现在待办里，并走既有 webhook 通知），
/// 恢复后自动关闭——留存这件事本身也要被观测。
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(BOOT_DELAY).await;
        loop {
            let now_ns = zhiwei_common::Timestamp::now().unix_nano();
            match run_once(&state, now_ns).await {
                Ok(stats) => {
                    if stats.rows > 0 || stats.raw_deleted > 0 || stats.hourly_deleted > 0 {
                        info!(
                            buckets = stats.buckets,
                            rows = stats.rows,
                            raw_deleted = stats.raw_deleted,
                            hourly_deleted = stats.hourly_deleted,
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
    use super::*;

    #[test]
    fn hour_start_floors_to_the_hour() {
        // 2026-09-19T12:34:56Z
        let t = 1_789_826_096_000_000_000i64;
        assert_eq!(
            hour_start(t),
            1_789_826_096_000_000_000i64 / NANOS_PER_HOUR * NANOS_PER_HOUR
        );
        // 正好在整点：不动
        assert_eq!(hour_start(2 * NANOS_PER_HOUR), 2 * NANOS_PER_HOUR);
        // 整点前 1 纳秒：落到上一个小时
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
        // 网络累计字节数：单调递增，first/last 决定这一小时的速率
        let s = vec![
            ("host.net.rx_bytes".to_string(), 1_000.0),
            ("host.net.rx_bytes".to_string(), 4_600.0),
        ];
        let out = aggregate(&s);
        assert_eq!(out[0].first, 1_000.0);
        assert_eq!(out[0].last, 4_600.0);
        // 不代表「这一小时总共传了 3600 字节」，而是可还原速率：
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
        // 网络累计量不在 metrics[] 里（每网卡一份），但长窗口也要画得出来；
        // 漏掉这一步的症状是「最近正常、长窗口空曲线」。
        use zhiwei_proto::telemetry::{Metric, NetworkInterface, TelemetryBatch};
        let batch = TelemetryBatch {
            node_id: "n1".into(),
            metrics: vec![Metric {
                name: "host.cpu.usage".into(),
                value: 12.0,
                labels: Default::default(),
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
