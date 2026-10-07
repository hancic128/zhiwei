# API Reference

ZhiWei exposes two APIs with different authentication:

| Audience | Path Prefix | Credential | Used by |
| --- | --- | --- | --- |
| Console / External | `/v1/*` | `Authorization: Bearer <admin token or AI token>` | Browser, scripts, AI clients |
| Nodes | `/v1/enroll`, `/v1/telemetry`, `/v1/inventory` | bootstrap token (enroll) / Ed25519 request signing (rest) | node-agent |

`admin.token` is a **single-value** credential (`<data-dir>/admin.token` or
`ZHIWEI_ADMIN_TOKEN`). AI tokens are **multi-value and revocable** read-only
credentials for MCP and external AI clients.

---

## Read Endpoints

All require `Authorization: Bearer <admin token or AI token>`.

| Method | Path | Description |
| --- | --- | --- |
| GET | `/v1` | Overview: `{ service, version, authenticated, endpoints, nodes, telemetry_batches }` |
| GET | `/v1/nodes` | Node list (includes `host_info`, latest telemetry frame, `alias` and `tags`) |
| GET | `/v1/nodes/:id/telemetry?limit=N` | Recent telemetry frames for a node |
| GET | `/v1/nodes/:id/series?range=...` | Time series (downsampled) |
| GET | `/v1/nodes/:id/containers` | Latest container snapshot for a node |
| GET | `/v1/nodes/:id/processes` | Latest process snapshot for a node |
| GET | `/v1/containers` | Cluster-wide container view |
| GET | `/v1/certificates` | Certificate inventory |
| GET | `/v1/cert-sources` | Certificate scan sources (nodes + paths) |
| GET | `/v1/alerts` | Alert instances (active + historical) |
| GET | `/v1/rules` | Alert rules |
| GET | `/v1/channels` | Notification channels |
| GET | `/v1/services`, `/v1/probes` | Service health probes |
| GET | `/v1/todo` | Todo aggregation (sorted by urgency) |
| GET | `/v1/help` | Help page markdown: `{ locale, body }` |
| GET | `/healthz` | Health check (**no auth required**) |

## Write Endpoints (admin token only)

AI tokens **cannot** call these — requests return 401.

| Method | Path | Description |
| --- | --- | --- |
| POST | `/v1/admin/token` | Change console credential, body `{ current, new }` |
| POST | `/v1/ai-tokens` | Create AI token |
| DELETE | `/v1/ai-tokens/:id` | Revoke AI token |
| POST | `/v1/enroll-tokens` | Create one-time enrollment token |
| DELETE | `/v1/enroll-tokens/:id` | Revoke enrollment token |
| PATCH | `/v1/nodes/:id` | Update node alias / tags, body `{ alias?, tags? }` |
| DELETE | `/v1/nodes/:id` | Permanently delete node and its data |
| POST/PATCH/DELETE | `/v1/rules`, `/v1/channels`, `/v1/services`, `/v1/probes`, `/v1/cert-sources` | CRUD for various configs |
| POST | `/v1/exec` | Dispatch command to node |

---

## AI Token

AI tokens are **read-only** credentials for MCP / external AI clients.
They are completely isolated from the admin token: they cannot modify
configuration or create other tokens.

### Create

```sh
curl -X POST https://<monitor>/v1/ai-tokens \
  -H "Authorization: Bearer <admin token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "claude-desktop-home"}'
```

Response `token` is like `ait_<base64url>`, shown **only once**:

```json
{
  "id": "ait-675cba",
  "name": "claude-desktop-home",
  "token": "ait_U-LXMwc2Hhf6-zSyXEQ5mXvRLCORlAoqfmNN1wuy0o8",
  "created_at_unix_nano": 1790038765833242000,
  "warning": "Plaintext token shown once only — save it now"
}
```

### List / Revoke

```sh
curl https://<monitor>/v1/ai-tokens -H "Authorization: Bearer <admin token>"
curl -X DELETE https://<monitor>/v1/ai-tokens/ait-675cba \
  -H "Authorization: Bearer <admin token>"
```

Revocation takes effect on the **next request**. `last_used_at_unix_nano` for auditing.

---

## Enrollment Token

A one-time bootstrap token generated at runtime, used with `install-node.sh`.

| Method | Path | Body / Description |
| --- | --- | --- |
| GET | `/v1/enroll-tokens` | List non-expired token metadata (no plaintext) |
| POST | `/v1/enroll-tokens` | `{ ttl_secs?, label? }` → returns `enroll_command` |
| DELETE | `/v1/enroll-tokens/:id` | Revoke |

```sh
curl -X POST https://<monitor>/v1/enroll-tokens \
  -H "Authorization: Bearer <admin token>" \
  -H "Content-Type: application/json" \
  -d '{"ttl_secs": 86400, "label": "prod-web-01"}'
```

Monitor URL in `enroll_command` is inferred from `X-Forwarded-Proto` + `Host`.

---

## MCP Server (for AI clients)

MCP uses **SSE transport**:

```
POST https://<monitor>/mcp/sse
Authorization: Bearer <AI token>
Content-Type: application/json
```

Response is `text/event-stream`, each event a JSON-RPC 2.0 message:

```
event: message
data: {"jsonrpc":"2.0","id":1,"result":{...}}
```

Protocol version `2024-11-05`. Supports `initialize` / `notifications/initialized` /
`ping` / `tools/list` / `tools/call`. Does **not** support `resources/*` / `prompts/*`.

> Using `admin token` with MCP returns 403 — must use AI token.
> This is intentional: MCP is a persistent integration and should use
> independently revocable credentials.

### Tool List

Tools mirror the console's REST endpoints. Read-only tools fetch data; management
tools (create / update / delete / exec) are also exposed and run under the same AI
token. Arbitrary shell is never possible — `exec_command` only accepts the whitelisted
actions in `proto/control.proto` (which the node re-validates before running).

**Read-only**

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `list_nodes` | — | `GET /v1/nodes` |
| `get_node` | `node_id` | `GET /v1/nodes` (filtered by id) |
| `get_telemetry` | `node_id`, `limit?` (default 100) | `GET /v1/nodes/:id/telemetry` |
| `list_alerts` | `since?`, `until?`, `status?`, `sources?`, `limit?`, `offset?` | `GET /v1/alerts` |
| `list_certs` | — | `GET /v1/cert-sources` |
| `list_containers` | `node_id` | `GET /v1/nodes/:id/containers` |
| `list_processes` | `node_id` | `GET /v1/nodes/:id/processes` |
| `get_services_timeline` | `from?`, `to?`, `buckets?`, `level?` | `GET /v1/services/timeline` |
| `list_command_history` | `node_id?`, `limit?` | `GET /v1/commands/history` |
| `get_command` | `command_id` | `GET /v1/commands/:id` |
| `get_todo` | `since?`, `until?`, `status?`, `sources?`, `page?`, `page_size?`, `offset?` | `GET /v1/todo` |
| `get_retention` | — | `GET /v1/retention` |

**Nodes**

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `update_node` | `node_id`, `alias?`, `tags?` | `PATCH /v1/nodes/:id` |
| `delete_node` | `node_id`, `force?` | `DELETE /v1/nodes/:id` |

**Alert rules** (`op` = `gt`/`gte`/`lt`/`lte`/`eq`; `severity` = `warning`/`critical`)

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `list_rules` | — | `GET /v1/rules` |
| `create_rule` | `name`, `metric`, `op`, `threshold`, `duration_seconds?`, `severity?` | `POST /v1/rules` |
| `update_rule` | `rule_id`, `name?`, `metric?`, `op?`, `threshold?`, `duration_seconds?`, `severity?`, `enabled?` | `PATCH /v1/rules/:id` |
| `delete_rule` | `rule_id` | `DELETE /v1/rules/:id` |
| `list_builtin_rules` | — | `GET /v1/builtin-alerts` |
| `update_builtin_rule` | `rule_id`, `enabled?`, `threshold?`, `duration_seconds?` | `PATCH /v1/builtin-alerts/:id` |

**Alert actions**

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `silence_alert` | `alert_id`, `minutes` | `POST /v1/alerts/:id/silence` |
| `resolve_alert` | `alert_id` | `POST /v1/alerts/:id/resolve` |

**Certificate sources** (empty `node_id` = all nodes)

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `create_cert_source` | `path`, `node_id?`, `notify_enabled?`, `notify_days_before?` | `POST /v1/cert-sources` |
| `test_cert_source` | `node_id`, `path` | `POST /v1/cert-sources/test` |
| `update_cert_source` | `source_id`, `node_id?`, `path?`, `enabled?`, `notify_enabled?`, `notify_days_before?` | `PATCH /v1/cert-sources/:id` |
| `delete_cert_source` | `source_id` | `DELETE /v1/cert-sources/:id` |

**Notification channels** (`kind` = `feishu`/`slack`/`bluebird`/`webhook`;
`min_severity` = `info`/`warning`/`critical`)

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `list_channels` | — | `GET /v1/channels` |
| `create_channel` | `name`, `kind?`, `url?`, `secret?`, `app_id?`, `receive_id?`, `receive_id_type?`, `min_severity?` | `POST /v1/channels` |
| `test_channel` | `kind`, `url?`, `secret?`, `app_id?`, `receive_id?`, `receive_id_type?` | `POST /v1/channels/test` |
| `update_channel` | `channel_id`, `name?`, `url?`, `secret?`, `app_id?`, `receive_id?`, `receive_id_type?`, `min_severity?`, `enabled?` | `PATCH /v1/channels/:id` |
| `delete_channel` | `channel_id` | `DELETE /v1/channels/:id` |

**Probes** (`kind` = `http`/`tcp`/`tls`; `target_json` is a JSON string, e.g.
`{"url":"https://example.com"}` or `{"host":"1.2.3.4","port":443}`)

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `list_probes` | — | `GET /v1/probes` |
| `create_probe` | `name`, `kind`, `target_json`, `description?`, `expect_json?`, `interval_seconds?`, `timeout_ms?`, `failure_threshold?`, `node_ids?`, `enabled?` | `POST /v1/probes` |
| `test_probe` | `kind`, `target_json`, `expect_json?`, `timeout_ms?` | `POST /v1/probes/test` |
| `update_probe` | `probe_id`, `name?`, `description?`, `kind?`, `target_json?`, `expect_json?`, `interval_seconds?`, `timeout_ms?`, `failure_threshold?`, `node_ids?`, `enabled?` | `PATCH /v1/probes/:id` |
| `delete_probe` | `probe_id` | `DELETE /v1/probes/:id` |
| `get_probe_results` | `probe_id`, `limit?` | `GET /v1/probes/:id/results` |

**Control commands** (whitelisted actions only; runs on a later node poll)

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `exec_command` | `node_id`, `action`, `params?` | `POST /v1/exec` |

`action` is one of `noop`, `fetch_logs`, `kill_process`, `restart_host`,
`shutdown_host`, `container_start`, `container_stop`, `container_restart`,
`container_remove`, `refresh_inventory`, `scan_certs`, `upgrade_agent`,
`rollback_agent`. Returns a `command_id` to poll via `get_command`.

### Client Configuration Example

Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "zhiwei": {
      "url": "https://<monitor>/mcp/sse",
      "headers": {
        "Authorization": "Bearer ait_xxxxxxxx"
      }
    }
  }
}
```

### Error Conventions

| Condition | Behavior |
| --- | --- |
| Missing / wrong token | HTTP 401, JSON `{ error }` |
| Using admin token | HTTP 403, hint to use AI token |
| JSON parse failure | JSON-RPC `-32700` |
| Unknown method | JSON-RPC `-32601` |
| Missing parameters | JSON-RPC `-32602` |
| Tool internal error | `result.isError = true` + error text |

---

## Node-Side Endpoints

| Method | Path | Credential |
| --- | --- | --- |
| POST | `/v1/enroll` | `Authorization: Bearer <bootstrap token>` + protobuf |
| POST | `/v1/telemetry` | Ed25519 request signing |
| POST | `/v1/inventory` | Ed25519 request signing |

Signature validation: `crates/common/src/auth.rs` (time window ±300s + nonce anti-replay).
