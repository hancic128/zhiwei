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

| Tool | Parameters | Underlying Endpoint |
| --- | --- | --- |
| `list_nodes` | — | `GET /v1/nodes` |
| `get_node` | `node_id` | `GET /v1/nodes` (filtered by id) |
| `get_telemetry` | `node_id`, `limit?` (default 100) | `GET /v1/nodes/:id/telemetry` |
| `list_alerts` | — | `GET /v1/alerts` |
| `list_certs` | — | `GET /v1/cert-sources` |
| `list_containers` | `node_id` | `GET /v1/nodes/:id/containers` |
| `list_processes` | `node_id`, `sort?`, `limit?` | `GET /v1/nodes/:id/processes` |

Destructive operations (`reboot` / `shutdown` / `kill_process` / `container_action` /
`renew_cert`) are **not** exposed to MCP. MCP is read-only; management
operations go through the console or `POST /v1/exec`.

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
