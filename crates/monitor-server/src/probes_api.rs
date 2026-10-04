//! HTTP API for service health (probes).
//!
//! Two types of callers:
//!   - Nodes: `GET /v1/probe-config` (pull config), `POST /v1/probe-results` (submit results),
//!     use Ed25519 request signing, can only operate their own data;
//!   - Console: `/v1/probes`, use admin token.
//!
//! Original `/v1/overview` replaced by `/v1/todo` (`todo_api`) -- see
//! `docs/superpowers/specs/2026-09-19-product-structure-design.md`.

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

// ---------- Node side ----------

/// `GET /v1/probe-config?node_id=<id>` -- node pulls probes it should execute
pub async fn probe_config_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let pq = uri
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str);
    let (node_id, _pub) = match verify_node(&state, &headers, method.as_str(), pq, &[]).await {
        Ok(id) => id,
        Err((code, msg)) => return err(code, msg),
    };
    if q.get("node_id").map(String::as_str) != Some(node_id.as_str()) {
        return err(
            StatusCode::FORBIDDEN,
            "Can only fetch this node's probe config",
        );
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
    /// ok | degraded | down (node determines by expect; unknown values treated as down)
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

/// `POST /v1/probe-results` -- node submits a batch of probe results
pub async fn probe_results_ingest_handler(
    State(state): State<AppState>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let pq = uri
        .path_and_query()
        .map_or("/", hyper::http::uri::PathAndQuery::as_str);
    let (node_id, _pub) = match verify_node(&state, &headers, method.as_str(), pq, &body).await {
        Ok(id) => id,
        Err((code, msg)) => return err(code, msg),
    };

    let parsed = match parse_probe_results(&body) {
        Ok(v) => v,
        Err((code, msg)) => return err(code, msg),
    };

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
        if ingest_one_result(&state, &node_id, &hostname, item, now).await {
            stored += 1;
        }
    }

    debug!(node_id = %node_id.as_str(), stored, "probe results stored");
    if stored == 0 && !parsed.results.is_empty() {
        return err(StatusCode::BAD_REQUEST, "No results were accepted");
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Decode + size-check the batch body. On failure returns `(status, message)` for the caller
/// to hand to [`err`].
fn parse_probe_results(body: &Bytes) -> Result<ProbeResultsBody, (StatusCode, String)> {
    let parsed: ProbeResultsBody = serde_json::from_slice(body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid body: {e}")))?;
    if parsed.results.len() > 500 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Batch size cannot exceed 500".to_string(),
        ));
    }
    Ok(parsed)
}

/// Persist one submitted probe result. Returns whether a row was stored (false for
/// non-existent probes, ownership mismatches, or write failures).
async fn ingest_one_result(
    state: &AppState,
    node_id: &zhiwei_common::NodeId,
    hostname: &str,
    item: &ProbeResultItem,
    now: i64,
) -> bool {
    let result_state = normalize_result_state(&item.state);

    let probe = match state.storage.probes().find_probe(&item.probe_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            debug!(probe_id = %item.probe_id, "Received result for non-existent probe, ignored");
            return false;
        }
        Err(e) => {
            warn!(error = %e, "Failed to query probes");
            return false;
        }
    };

    // Ownership check: when probe is bound to a different node, don't accept this node's results
    if !probe.node_ids.is_empty() && !probe.node_ids.iter().any(|n| n == node_id.as_str()) {
        return false;
    }

    let ts = if item.ts_unix_nano > 0 {
        item.ts_unix_nano
    } else {
        now
    };
    store_result(state, &probe, node_id, hostname, item, ts, result_state).await
}

fn normalize_result_state(state: &str) -> &'static str {
    match state {
        "ok" => "ok",
        "degraded" => "degraded",
        _ => "down",
    }
}

async fn store_result(
    state: &AppState,
    probe: &zhiwei_storage::probes_repo::Probe,
    node_id: &zhiwei_common::NodeId,
    hostname: &str,
    item: &ProbeResultItem,
    ts: i64,
    result_state: &'static str,
) -> bool {
    match state
        .storage
        .probes()
        .record_result(
            probe,
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
            crate::alerts::on_probe_transition(
                state,
                probe,
                &transition,
                node_id.as_str(),
                hostname,
            )
            .await;
            true
        }
        Err(e) => {
            warn!(error = %e, probe = %probe.name, "Failed to write probe results");
            false
        }
    }
}

// ---------- Console side ----------

/// Convert "ok count / total per bucket" to one curve per object (`t` milliseconds, `v` percentage).
/// Buckets with no results at all are skipped, making the curve break rather than drop to 0.
fn ratio_series(rows: Vec<(String, i64, i64, i64)>) -> HashMap<String, Vec<serde_json::Value>> {
    let mut series: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for (key, bucket, ok, total) in rows {
        if total <= 0 {
            continue;
        }
        let ratio = f64::from(i32::try_from(ok).unwrap_or(0)) * 100.0_f64
            / f64::from(i32::try_from(total).unwrap_or(1));
        series.entry(key).or_default().push(serde_json::json!({
            "t": bucket / 1_000_000,
            "v": ratio,
        }));
    }
    series
}

/// `GET /v1/services/timeline?from=<ms>&to=<ms>&buckets=N&level=service|probe`
///
/// Health timeline: "what proportion of probes in this time bucket were ok", one line per object.
/// Uses proportion instead of single probe results -- raw results are affected by sampling density,
/// curves become jittery; proportions are comparable across different probe frequencies on the same scale.
///
/// `level=probe` (default): one line per probe.
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
        return err(StatusCode::BAD_REQUEST, "'from' must be earlier than 'to'");
    }
    let buckets = q
        .get("buckets")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(60)
        .clamp(10, 240);
    let bucket_ns = (to_ms - from_ms) * 1_000_000 / buckets;

    probe_timeline(&state, from_ms, to_ms, bucket_ns).await
}

/// One line per probe.
async fn probe_timeline(state: &AppState, from_ms: i64, to_ms: i64, bucket_ns: i64) -> Response {
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
    let probes = state
        .storage
        .probes()
        .list_probes()
        .await
        .unwrap_or_default();

    let mut series = ratio_series(rows);
    let out: Vec<serde_json::Value> = probes
        .iter()
        .filter_map(|p| {
            let points = series.remove(&p.id)?;
            Some(serde_json::json!({
                "id": p.id,
                "name": p.name,
                "group": serde_json::Value::Null,
                "points": points,
            }))
        })
        .collect();

    Json(serde_json::json!({
        "from_ms": from_ms,
        "to_ms": to_ms,
        "bucket_ms": bucket_ns / 1_000_000,
        "level": "probe",
        "series": out,
    }))
    .into_response()
}

/// Flat probe list (with state): for "probes" standalone view and node detail pages
pub async fn list_probes_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    match state.storage.probes().list_probes().await {
        Ok(list) => Json(list).into_response(),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list probes: {e}"),
        ),
    }
}

/// Deduplicate and remove empty from node id list (order is UI checkbox order, kept stable)
fn normalize_node_ids(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in ids {
        let id = raw.trim();
        if !id.is_empty() && !out.iter().any(|v| v == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// Validate and normalize probe kind / target / expect
fn normalize_probe_parts(
    kind: &str,
    target_json: &str,
    expect_json: &str,
) -> Result<(String, String), String> {
    if !["http", "tcp", "tls"].contains(&kind) {
        return Err("kind must be http/tcp/tls".into());
    }
    let target: serde_json::Value =
        serde_json::from_str(target_json).map_err(|e| format!("target is not valid JSON: {e}"))?;
    let expect: serde_json::Value = if expect_json.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(expect_json).map_err(|e| format!("expect is not valid JSON: {e}"))?
    };

    // Required fields: probes without target address will always fail, block at creation time
    match kind {
        "http" => {
            let url = target.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err("HTTP probe: target.url must start with http:// or https://".into());
            }
        }
        "tcp" | "tls" => {
            let host = target.get("host").and_then(|v| v.as_str()).unwrap_or("");
            if host.trim().is_empty() {
                return Err(format!("{kind} probe: target.host cannot be empty"));
            }
            let port = target
                .get("port")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if !(1..=65535).contains(&port) {
                return Err(format!("{kind} probe: target.port must be between 1-65535"));
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
    #[derive(serde::Deserialize)]
    struct Body {
        name: String,
        #[serde(default)]
        description: String,
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
        /// Bound execution nodes; omitted or empty array = any node
        #[serde(default)]
        node_ids: Vec<String>,
        #[serde(default = "default_enabled")]
        enabled: bool,
    }
    const fn default_interval() -> i64 {
        60
    }
    const fn default_timeout() -> i64 {
        5000
    }
    const fn default_threshold() -> i64 {
        3
    }
    const fn default_enabled() -> bool {
        true
    }
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    if b.name.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "Probe name cannot be empty");
    }
    let (target_json, expect_json) =
        match normalize_probe_parts(&b.kind, &b.target_json, &b.expect_json) {
            Ok(v) => v,
            Err(e) => return err(StatusCode::BAD_REQUEST, e),
        };

    let input = zhiwei_storage::probes_repo::ProbeInput {
        name: b.name.trim().to_string(),
        description: b.description.trim().to_string(),
        kind: b.kind,
        target_json,
        expect_json,
        interval_seconds: b.interval_seconds.clamp(10, 86_400),
        timeout_ms: b.timeout_ms.clamp(100, 60_000),
        failure_threshold: b.failure_threshold.clamp(1, 100),
        node_ids: normalize_node_ids(&b.node_ids),
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

/// `POST /v1/probes/test` -- one-time test before saving, runs but doesn't store.
///
/// Initiated from console (monitor), quickly confirms target is reachable and expect config is correct;
/// real probes are still run periodically by executing nodes. Conclusion's reason/args handed to
/// frontend for messaging.
pub async fn test_probe_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
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
    const fn default_timeout() -> i64 {
        5000
    }
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }

    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };
    // Uses same validation as create probe: what tests pass must match what gets stored
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
    #[derive(serde::Deserialize)]
    struct Body {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
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
        /// Giving array replaces all bound nodes as a whole (`[]` = revert to "any node")
        #[serde(default)]
        node_ids: Option<Vec<String>>,
        #[serde(default)]
        enabled: Option<bool>,
    }
    if !read_auth_ok(&state, &headers).await {
        return err(
            StatusCode::UNAUTHORIZED,
            "authentication required (Bearer admin token)",
        );
    }
    let b: Body = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("invalid body: {e}")),
    };

    // Changing target requires giving kind at the same time, otherwise can't validate required fields
    let target_json = match (&b.kind, &b.target_json) {
        (Some(kind), Some(target)) => {
            let expect = b.expect_json.clone().unwrap_or_else(|| "{}".into());
            match normalize_probe_parts(kind, target, &expect) {
                Ok((t, _)) => Some(t),
                Err(e) => return err(StatusCode::BAD_REQUEST, e),
            }
        }
        (None, Some(_)) => {
            return err(
                StatusCode::BAD_REQUEST,
                "Changing target requires providing kind at the same time",
            )
        }
        _ => None,
    };

    let patch = zhiwei_storage::probes_repo::ProbePatch {
        name: b.name,
        description: b.description,
        kind: b.kind,
        target_json,
        expect_json: b.expect_json,
        interval_seconds: b.interval_seconds.map(|v| v.clamp(10, 86_400)),
        timeout_ms: b.timeout_ms.map(|v| v.clamp(100, 60_000)),
        failure_threshold: b.failure_threshold.map(|v| v.clamp(1, 100)),
        node_ids: b.node_ids.as_deref().map(normalize_node_ids),
        enabled: b.enabled,
    };
    let now = zhiwei_common::Timestamp::now().unix_nano();
    match state.storage.probes().update_probe(&id, &patch, now).await {
        Ok(()) => {
            // Disabling probe: close its unresolved alerts -- no longer dispatched, the previous
            // "probe down" shouldn't continue occupying the todo list (stats also exclude disabled in backend)
            if b.enabled == Some(false) {
                if let Err(e) = state
                    .storage
                    .alerts()
                    .resolve_open_probe_alerts(&id, now)
                    .await
                {
                    warn!(error = %e, "Failed to close probe alerts");
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
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
        warn!(error = %e, "Failed to close probe alerts");
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

// ---------- Overview aggregation ----------

/// Container "failed" detection (for overview cards):
/// dead / restarting always count as failed; exited looks at exit code (anything other than `Exited (0)`);
/// running but health check reports unhealthy also counts.
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
