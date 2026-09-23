//! 服务健康度（服务 / 探针）的 HTTP 接口。
//!
//! 两类调用方：
//!   - 节点：`GET /v1/probe-config`（拉配置）、`POST /v1/probe-results`（交结果），
//!     走 Ed25519 请求签名，只能操作自己；
//!   - 控制台：`/v1/services`、`/v1/probes`，走 admin token。
//!
//! 原 `/v1/overview` 已被 `/v1/todo`（`todo_api`）取代——见
//! `docs/superpowers/specs/2026-09-19-product-structure-design.md`。

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use tracing::{debug, warn};

use crate::routes::{err, read_auth_ok, verify_node};
use crate::state::AppState;

// ---------- 节点侧 ----------

/// `GET /v1/probe-config?node_id=<id>` —— 节点拉取本机要执行的探针
pub async fn probe_config_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let (node_id, _pub) = match verify_node(&state, &headers, method.as_str(), pq, &[]).await {
        Ok(id) => id,
        Err((code, msg)) => return err(code, msg),
    };
    if q.get("node_id").map(String::as_str) != Some(node_id.as_str()) {
        return err(StatusCode::FORBIDDEN, "只能拉取本节点的探针配置");
    }

    let probes = match state
        .storage
        .probes()
        .probes_for_node(node_id.as_str())
        .await
    {
        Ok(p) => p,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("list probes: {e}"),
            )
        }
    };

    let list: Vec<serde_json::Value> = probes
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "service": p.service_name,
                "name": p.name,
                "kind": p.kind,
                "target": serde_json::from_str::<serde_json::Value>(&p.target_json)
                    .unwrap_or(serde_json::Value::Null),
                "expect": serde_json::from_str::<serde_json::Value>(&p.expect_json)
                    .unwrap_or(serde_json::Value::Null),
                "interval_seconds": p.interval_seconds,
                "timeout_ms": p.timeout_ms,
            })
        })
        .collect();

    Json(serde_json::json!({ "node_id": node_id.as_str(), "probes": list })).into_response()
}

#[derive(serde::Deserialize)]
struct ProbeResultItem {
    probe_id: String,
    #[serde(default)]
    ts_unix_nano: i64,
    /// ok | degraded | down（节点按 expect 判定；未知取值按 down 处理）
    state: String,
    #[serde(default)]
    latency_ms: Option<f64>,
    #[serde(default)]
    status_code: Option<i64>,
    #[serde(default)]
    error: String,
}

#[derive(serde::Deserialize)]
struct ProbeResultsBody {
    results: Vec<ProbeResultItem>,
}

/// `POST /v1/probe-results` —— 节点上报一批探针结果
pub async fn probe_results_ingest_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let (node_id, _pub) = match verify_node(&state, &headers, method.as_str(), pq, &body).await {
        Ok(id) => id,
        Err((code, msg)) => return err(code, msg),
    };

    let parsed: ProbeResultsBody = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if parsed.results.len() > 500 {
        return err(StatusCode::BAD_REQUEST, "单批结果不能超过 500 条");
    }

    let hostname = state
        .storage
        .nodes()
        .find_by_id(&node_id)
        .await
        .ok()
        .flatten()
        .map(|n| n.hostname)
        .unwrap_or_default();

    let now = zhiwei_common::Timestamp::now().unix_nano();
    let mut stored = 0usize;

    for item in &parsed.results {
        let result_state = match item.state.as_str() {
            "ok" => "ok",
            "degraded" => "degraded",
            _ => "down",
        };

        let probe = match state.storage.probes().find_probe(&item.probe_id).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                debug!(probe_id = %item.probe_id, "上报了不存在的探针，已忽略");
                continue;
            }
            Err(e) => {
                warn!(error = %e, "查询探针失败");
                continue;
            }
        };

        // 归属校验：探针绑定了别的节点时，不接受这台节点的结果
        if let Some(bound) = &probe.node_id {
            if bound != node_id.as_str() {
                continue;
            }
        }

        let ts = if item.ts_unix_nano > 0 {
            item.ts_unix_nano
        } else {
            now
        };

        match state
            .storage
            .probes()
            .record_result(
                &probe,
                node_id.as_str(),
                ts,
                result_state,
                item.latency_ms,
                item.status_code,
                &item.error,
            )
            .await
        {
            Ok(transition) => {
                stored += 1;
                crate::alerts::on_probe_transition(
                    &state,
                    &probe,
                    &transition,
                    node_id.as_str(),
                    &hostname,
                )
                .await;
            }
            Err(e) => warn!(error = %e, probe = %probe.name, "写入探针结果失败"),
        }
    }

    debug!(node_id = %node_id.as_str(), stored, "probe results stored");
    if stored == 0 && !parsed.results.is_empty() {
        return err(StatusCode::BAD_REQUEST, "没有任何结果被接受");
    }
    StatusCode::NO_CONTENT.into_response()
}

// ---------- 控制台侧 ----------

/// `GET /v1/services/timeline?from=<ms>&to=<ms>&buckets=N`
///
/// 服务健康时间线：每个时间桶里「有多少比例的探测是 ok 的」，一条线一个服务。
/// 用比例而不是单次探测结果——单次结果受采样密度影响，曲线会毛刺化；
/// 比例能在同一个尺度上比较不同频率的探针。
pub async fn services_timeline_handler(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let now_ms = zhiwei_common::Timestamp::now().unix_nano() / 1_000_000;
    let to_ms = q
        .get("to")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(now_ms);
    let from_ms = q
        .get("from")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(to_ms - 24 * 3600 * 1000);
    if from_ms >= to_ms {
        return err(StatusCode::BAD_REQUEST, "from 必须早于 to");
    }
    let buckets = q
        .get("buckets")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(60)
        .clamp(10, 240);
    let bucket_ns = (to_ms - from_ms) * 1_000_000 / buckets;

    let rows = match state
        .storage
        .probes()
        .health_buckets(from_ms * 1_000_000, to_ms * 1_000_000, bucket_ns)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("query probe results: {e}"),
            )
        }
    };
    let services = state
        .storage
        .probes()
        .list_services()
        .await
        .unwrap_or_default();

    // service_id -> 点集（按桶顺序）
    let mut series: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for (service_id, bucket, ok, total) in rows {
        if total <= 0 {
            continue;
        }
        let ratio = ok as f64 * 100.0 / total as f64;
        series
            .entry(service_id)
            .or_default()
            .push(serde_json::json!({
                "t": bucket / 1_000_000,
                "v": ratio,
            }));
    }

    let out: Vec<serde_json::Value> = services
        .iter()
        .filter_map(|s| {
            let points = series.remove(&s.id)?;
            Some(serde_json::json!({
                "service_id": s.id,
                "name": s.name,
                "points": points,
            }))
        })
        .collect();

    Json(serde_json::json!({
        "from_ms": from_ms,
        "to_ms": to_ms,
        "bucket_ms": bucket_ns / 1_000_000,
        "services": out,
    }))
    .into_response()
}

pub async fn list_services_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    match state.storage.probes().services_with_probes().await {
        Ok(list) => Json(list).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list services: {e}"),
        ),
    }
}

pub async fn create_service_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        group_name: String,
        #[serde(default = "default_tier")]
        tier: i64,
    }
    fn default_tier() -> i64 {
        2
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if b.name.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "服务名不能为空");
    }

    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state
        .storage
        .probes()
        .create_service(
            b.name.trim(),
            &b.description,
            &b.group_name,
            b.tier.clamp(1, 3),
            now,
        )
        .await
    {
        Ok(svc) => (StatusCode::CREATED, Json(svc)).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create service: {e}"),
        ),
    }
}

pub async fn patch_service_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        group_name: Option<String>,
        #[serde(default)]
        tier: Option<i64>,
        #[serde(default)]
        enabled: Option<bool>,
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    let patch = zhiwei_storage::probes_repo::ServicePatch {
        name: b.name,
        description: b.description,
        group_name: b.group_name,
        tier: b.tier,
        enabled: b.enabled,
    };
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state
        .storage
        .probes()
        .update_service(&id, &patch, now)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("patch service: {e}"),
        ),
    }
}

pub async fn delete_service_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    // 先关掉该服务下探针的未解决告警，避免留下永远无法恢复的孤儿告警
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state.storage.probes().list_probes().await {
        Ok(probes) => {
            for p in probes.iter().filter(|p| p.service_id == id) {
                if let Err(e) = state
                    .storage
                    .alerts()
                    .resolve_open_probe_alerts(&p.id, now)
                    .await
                {
                    warn!(error = %e, "关闭探针告警失败");
                }
            }
        }
        Err(e) => warn!(error = %e, "读取探针失败"),
    }

    match state.storage.probes().delete_service(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("delete service: {e}"),
        ),
    }
}

/// 扁平探针列表（含状态）：给「探针」独立视图与节点详情用
pub async fn list_probes_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    match state.storage.probes().services_with_probes().await {
        Ok(services) => {
            let flat: Vec<_> = services.into_iter().flat_map(|s| s.probes).collect();
            Json(flat).into_response()
        }
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list probes: {e}"),
        ),
    }
}

/// 校验并规范化探针的 kind / target / expect
fn normalize_probe_parts(
    kind: &str,
    target_json: &str,
    expect_json: &str,
) -> Result<(String, String), String> {
    if !["http", "tcp", "tls"].contains(&kind) {
        return Err("kind 必须是 http/tcp/tls".into());
    }
    let target: serde_json::Value =
        serde_json::from_str(target_json).map_err(|e| format!("target 不是合法 JSON: {e}"))?;
    let expect: serde_json::Value = if expect_json.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(expect_json).map_err(|e| format!("expect 不是合法 JSON: {e}"))?
    };

    // 必填字段：没有目标地址的探针永远只会失败，创建时就拦下来
    match kind {
        "http" => {
            let url = target.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err("http 探针的 target.url 必须以 http:// 或 https:// 开头".into());
            }
        }
        "tcp" | "tls" => {
            let host = target.get("host").and_then(|v| v.as_str()).unwrap_or("");
            if host.trim().is_empty() {
                return Err(format!("{kind} 探针的 target.host 不能为空"));
            }
            let port = target.get("port").and_then(|v| v.as_i64()).unwrap_or(0);
            if !(1..=65535).contains(&port) {
                return Err(format!("{kind} 探针的 target.port 必须在 1-65535"));
            }
        }
        _ => {}
    }

    Ok((target.to_string(), expect.to_string()))
}

pub async fn create_probe_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        service_id: String,
        name: String,
        kind: String,
        #[serde(default)]
        target_json: String,
        #[serde(default)]
        expect_json: String,
        #[serde(default = "default_interval")]
        interval_seconds: i64,
        #[serde(default = "default_timeout")]
        timeout_ms: i64,
        #[serde(default = "default_threshold")]
        failure_threshold: i64,
        #[serde(default)]
        node_id: Option<String>,
        #[serde(default = "default_enabled")]
        enabled: bool,
    }
    fn default_interval() -> i64 {
        60
    }
    fn default_timeout() -> i64 {
        5000
    }
    fn default_threshold() -> i64 {
        3
    }
    fn default_enabled() -> bool {
        true
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if b.name.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "探针名不能为空");
    }
    if state
        .storage
        .probes()
        .find_service(&b.service_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return err(StatusCode::BAD_REQUEST, "service_id 不存在");
    }
    let (target_json, expect_json) =
        match normalize_probe_parts(&b.kind, &b.target_json, &b.expect_json) {
            Ok(v) => v,
            Err(e) => return err(StatusCode::BAD_REQUEST, e),
        };

    let input = zhiwei_storage::probes_repo::ProbeInput {
        service_id: b.service_id,
        name: b.name.trim().to_string(),
        kind: b.kind,
        target_json,
        expect_json,
        interval_seconds: b.interval_seconds.clamp(10, 86_400),
        timeout_ms: b.timeout_ms.clamp(100, 60_000),
        failure_threshold: b.failure_threshold.clamp(1, 100),
        node_id: b.node_id.filter(|s| !s.trim().is_empty()),
        location: "node".into(),
        enabled: b.enabled,
    };
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state.storage.probes().create_probe(&input, now).await {
        Ok(p) => (StatusCode::CREATED, Json(p)).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create probe: {e}"),
        ),
    }
}

/// `POST /v1/probes/test` —— 保存前的一次性测试，只跑不落库。
///
/// 从控制台（monitor）发起，用来快速确认目标可达、期望配置写得对；
/// 真正的探针仍然由执行节点周期性运行。结论里的 reason/args 交给前端做文案。
pub async fn test_probe_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        kind: String,
        #[serde(default)]
        target_json: String,
        #[serde(default)]
        expect_json: String,
        #[serde(default = "default_timeout")]
        timeout_ms: i64,
    }
    fn default_timeout() -> i64 {
        5000
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    // 与创建探针用同一套校验：测出来的东西必须和存下去的一致
    let (target_json, expect_json) =
        match normalize_probe_parts(&b.kind, &b.target_json, &b.expect_json) {
            Ok(v) => v,
            Err(e) => return err(StatusCode::BAD_REQUEST, e),
        };
    let target: serde_json::Value = serde_json::from_str(&target_json).unwrap_or_default();
    let expect: serde_json::Value = serde_json::from_str(&expect_json).unwrap_or_default();

    let out = crate::probe_test::run(&b.kind, &target, &expect, b.timeout_ms).await;
    Json(serde_json::json!({
        "state": out.state,
        "latency_ms": out.latency_ms,
        "status_code": out.status_code,
        "reason": out.reason,
        "args": out.args,
    }))
    .into_response()
}

pub async fn patch_probe_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    #[derive(serde::Deserialize)]
    struct Body {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        target_json: Option<String>,
        #[serde(default)]
        expect_json: Option<String>,
        #[serde(default)]
        interval_seconds: Option<i64>,
        #[serde(default)]
        timeout_ms: Option<i64>,
        #[serde(default)]
        failure_threshold: Option<i64>,
        #[serde(default)]
        node_id: Option<String>,
        #[serde(default)]
        enabled: Option<bool>,
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };

    // 改 target 必须连同 kind 一起给，否则无法校验必填字段
    let target_json = match (&b.kind, &b.target_json) {
        (Some(kind), Some(target)) => {
            let expect = b.expect_json.clone().unwrap_or_else(|| "{}".into());
            match normalize_probe_parts(kind, target, &expect) {
                Ok((t, _)) => Some(t),
                Err(e) => return err(StatusCode::BAD_REQUEST, e),
            }
        }
        (None, Some(_)) => return err(StatusCode::BAD_REQUEST, "改 target 需要同时给 kind"),
        _ => None,
    };

    let patch = zhiwei_storage::probes_repo::ProbePatch {
        name: b.name,
        kind: b.kind,
        target_json,
        expect_json: b.expect_json,
        interval_seconds: b.interval_seconds.map(|v| v.clamp(10, 86_400)),
        timeout_ms: b.timeout_ms.map(|v| v.clamp(100, 60_000)),
        failure_threshold: b.failure_threshold.map(|v| v.clamp(1, 100)),
        node_id: b
            .node_id
            .map(|s| if s.trim().is_empty() { None } else { Some(s) }),
        enabled: b.enabled,
    };
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state.storage.probes().update_probe(&id, &patch, now).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("patch probe: {e}"),
        ),
    }
}

pub async fn delete_probe_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let now = zhiwei_common::Timestamp::now().unix_nano();
    if let Err(e) = state
        .storage
        .alerts()
        .resolve_open_probe_alerts(&id, now)
        .await
    {
        warn!(error = %e, "关闭探针告警失败");
    }
    match state.storage.probes().delete_probe(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("delete probe: {e}"),
        ),
    }
}

pub async fn probe_results_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    match state.storage.probes().recent_results(&id, limit).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list probe results: {e}"),
        ),
    }
}

// ---------- 概览聚合 ----------

/// 容器「异常」判定（概览卡片用）：
/// dead / restarting 一律算异常；exited 看退出码（`Exited (0)` 之外都算）；
/// running 但健康检查报 unhealthy 也算。
pub fn container_is_failed(state: &str, status: &str) -> bool {
    let state = state.to_ascii_lowercase();
    let status = status.to_ascii_lowercase();
    match state.as_str() {
        "dead" | "restarting" => true,
        "exited" => !status.starts_with("exited (0)"),
        "running" => status.contains("unhealthy"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_failed_follows_exit_code_and_health() {
        assert!(!container_is_failed("exited", "Exited (0) 4 months ago"));
        assert!(container_is_failed("exited", "Exited (1) 8 months ago"));
        assert!(container_is_failed("exited", "Exited (137) 5 months ago"));
        assert!(container_is_failed("dead", "Dead"));
        assert!(container_is_failed(
            "restarting",
            "Restarting (1) 2 seconds ago"
        ));
        assert!(!container_is_failed("running", "Up 2 hours (healthy)"));
        assert!(container_is_failed("running", "Up 2 hours (unhealthy)"));
        assert!(!container_is_failed("created", "Created"));
        assert!(!container_is_failed("paused", "Up 3 days (Paused)"));
    }

    #[test]
    fn probe_parts_validation_requires_a_target() {
        assert!(normalize_probe_parts("http", r#"{"url":"https://a.example/hz"}"#, "{}").is_ok());
        assert!(normalize_probe_parts("http", r#"{"url":"a.example"}"#, "{}").is_err());
        assert!(normalize_probe_parts("tcp", r#"{"host":"db","port":5432}"#, "{}").is_ok());
        assert!(normalize_probe_parts("tcp", r#"{"host":"db","port":0}"#, "{}").is_err());
        assert!(normalize_probe_parts("tls", r#"{"host":"a.example","port":443}"#, "{}").is_ok());
        assert!(normalize_probe_parts("grpc", "{}", "{}").is_err());
        assert!(normalize_probe_parts("http", "not-json", "{}").is_err());
    }
}
