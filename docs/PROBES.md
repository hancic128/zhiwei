# Service Health Probes

> How to configure service health checks in ZhiWei.

ZhiWei supports three probe types: **HTTP**, **TCP**, and **TLS**. Probes are organized under **Services**, which aggregate the health of multiple endpoints.

## Concepts

| Concept | Description |
|---|---|
| **Service** | A logical grouping of probes (e.g., "API Server") |
| **Probe** | A single health check against one endpoint |
| **Node** | Which host runs the probe (probes run on enrolled nodes) |

Services roll up probe states:

```
all ok → ok
any degraded (no down) → degraded
any down → down
no probes / all unknown → unknown
```

---

## Services

Services organize probes and provide a group-level health view.

### Create a Service

```
POST /v1/services
Authorization: Bearer <admin token>
```

```json
{
  "name": "API Server",
  "description": "Main REST API",
  "group_name": "production",
  "tier": 1
}
```

| Field | Description |
|---|---|
| `name` | Unique service name |
| `description` | Optional description |
| `group_name` | Grouping label (e.g., "production", "staging") |
| `tier` | Service tier (1–3, informational) |

### Update a Service

```
PATCH /v1/services/:id
Authorization: Bearer <admin token>
```

All fields optional:
```json
{ "name": "New Name", "group_name": "staging", "tier": 2, "enabled": false }
```

### Delete a Service

```
DELETE /v1/services/:id
Authorization: Bearer <admin token>
```

Deleting a service deletes all its probes.

---

## Probes

### Probe Parameters

| Parameter | Type | Default | Range |
|---|---|---|---|
| `interval_seconds` | int | 60 | 10–86400 |
| `timeout_ms` | int | 5000 | 100–60000 |
| `failure_threshold` | int | 3 | 1–100 |

**Failure threshold**: consecutive failures before marking `down`. A probe must fail `failure_threshold` times in a row to transition `ok` → `degraded` → `down`.

### State Machine

```
ok ──(fail)──────────────────────────────→ degraded ──(N fails)──→ down
ok ──(ok)──────────────────────────────────────────────────────────────────→ ok
degraded ──(ok)──→ ok
down ──(ok)──→ ok
```

---

## HTTP Probe

Checks an HTTP endpoint.

### Target

```json
{
  "url": "https://example.com/healthz",
  "method": "GET",
  "headers": { "Authorization": "Bearer token" },
  "body": ""
}
```

| Field | Required | Description |
|---|---|---|
| `url` | yes | Full URL, must start with `http://` or `https://` |
| `method` | no | `GET` (default), `POST`, `HEAD` |
| `headers` | no | Custom request headers |
| `body` | no | Request body for POST |

### Expectations

```json
{
  "status": [200, 201],
  "body_contains": ["ok"],
  "tls_verify": true,
  "max_latency_ms": 5000
}
```

| Field | Default | Description |
|---|---|---|
| `status` | `[200, 299]` | Acceptable status codes |
| `body_contains` | any | Strings that must appear in response body |
| `tls_verify` | `true` | Verify TLS certificate |
| `max_latency_ms` | none | Latency threshold; exceeded = `degraded` |

**Result logic**:
- Connect timeout → `down`
- Wrong status code → `down`
- Body missing expected string → `degraded`
- Latency exceeded → `degraded`

### Example

```json
{
  "service_id": "uuid",
  "name": "Health Check",
  "kind": "http",
  "target_json": "{\"url\":\"https://example.com/healthz\"}",
  "expect_json": "{\"status\":[200],\"body_contains\":[\"ok\"]}",
  "interval_seconds": 30,
  "timeout_ms": 5000,
  "failure_threshold": 3
}
```

---

## TCP Probe

Checks TCP port connectivity.

### Target

```json
{
  "host": "db.example.com",
  "port": 5432
}
```

| Field | Required | Description |
|---|---|---|
| `host` | yes | Hostname or IP |
| `port` | yes | Port number (1–65535) |

### Expectations

```json
{
  "banner_contains": "PostgreSQL",
  "max_latency_ms": 5000
}
```

| Field | Default | Description |
|---|---|---|
| `banner_contains` | any | String that must appear in server banner |
| `max_latency_ms` | none | Latency threshold; exceeded = `degraded` |

**Result logic**:
- Connection refused / timeout → `down`
- Banner mismatch → `degraded`
- Latency exceeded → `degraded`
- Connection success + banner matches → `ok`

### Example

```json
{
  "service_id": "uuid",
  "name": "Postgres Check",
  "kind": "tcp",
  "target_json": "{\"host\":\"db.example.com\",\"port\":5432}",
  "expect_json": "{\"banner_contains\":\"PostgreSQL\",\"max_latency_ms\":3000}",
  "interval_seconds": 60,
  "timeout_ms": 3000,
  "failure_threshold": 3
}
```

---

## TLS Probe

Checks TLS certificate validity.

### Target

```json
{
  "host": "secure.example.com",
  "port": 443,
  "sni": "custom.sni.example.com"
}
```

| Field | Required | Description |
|---|---|---|
| `host` | yes | Hostname for connection |
| `port` | no | Port (default 443) |
| `sni` | no | Custom SNI hostname |

### Expectations

```json
{
  "verify": true,
  "min_days_valid": 30,
  "max_latency_ms": 5000
}
```

| Field | Default | Description |
|---|---|---|
| `verify` | `true` | Verify certificate chain |
| `min_days_valid` | none | Warn if cert expires within N days |
| `max_latency_ms` | none | Latency threshold; exceeded = `degraded` |

**Result logic**:
- Connection failure → `down`
- TLS handshake failure → `down`
- Certificate expired → `down`
- Expires within `min_days_valid` → `degraded`
- Latency exceeded → `degraded`
- All checks pass → `ok`

### Example

```json
{
  "service_id": "uuid",
  "name": "TLS Check",
  "kind": "tls",
  "target_json": "{\"host\":\"secure.example.com\",\"port\":443}",
  "expect_json": "{\"verify\":true,\"min_days_valid\":30}",
  "interval_seconds": 3600,
  "timeout_ms": 10000,
  "failure_threshold": 1
}
```

---

## Node Assignment

By default, probes run on **all enrolled nodes**. To restrict to specific nodes:

```json
{
  "node_ids": ["node-uuid-1", "node-uuid-2"]
}
```

Empty array = run on all nodes. Use this to probe services that are only accessible from certain nodes (e.g., internal databases).

---

## Probe CRUD API

### Services

| Method | Path | Description |
|---|---|---|
| GET | `/v1/services` | List services (with probes + health state) |
| POST | `/v1/services` | Create service |
| PATCH | `/v1/services/:id` | Update service |
| DELETE | `/v1/services/:id` | Delete service (cascades probes) |
| GET | `/v1/services/timeline` | Health timeline |

### Probes

| Method | Path | Description |
|---|---|---|
| GET | `/v1/probes` | List all probes |
| POST | `/v1/probes` | Create probe |
| POST | `/v1/probes/test` | Test probe config (no persistence) |
| PATCH | `/v1/probes/:id` | Update probe |
| DELETE | `/v1/probes/:id` | Delete probe |
| GET | `/v1/probes/:id/results` | Historical probe results |

---

## Test a Probe

Before creating, test the configuration:

```
POST /v1/probes/test
Authorization: Bearer <admin token>
```

```json
{
  "kind": "http",
  "target_json": "{\"url\":\"https://example.com/healthz\"}",
  "expect_json": "{\"status\":[200]}",
  "timeout_ms": 5000
}
```

Response:

```json
{
  "state": "ok",
  "latency_ms": 125.5,
  "status_code": 200,
  "reason": "ok"
}
```

---

## Complete Example

1. **Create a service**:
```sh
curl -X POST https://<monitor>/v1/services \
  -H "Authorization: Bearer <admin-token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Web App","group_name":"production","tier":1}'
```

2. **Add HTTP probe**:
```sh
curl -X POST https://<monitor>/v1/probes \
  -H "Authorization: Bearer <admin-token>" \
  -H "Content-Type: application/json" \
  -d '{
    "service_id": "<service-uuid>",
    "name": "Homepage",
    "kind": "http",
    "target_json": "{\"url\":\"https://example.com/\"}",
    "expect_json": "{\"status\":[200],\"body_contains\":[\"Welcome\"]}",
    "interval_seconds": 60,
    "timeout_ms": 5000,
    "failure_threshold": 3
  }'
```

3. **Add TLS probe for API**:
```sh
curl -X POST https://<monitor>/v1/probes \
  -H "Authorization: Bearer <admin-token>" \
  -H "Content-Type: application/json" \
  -d '{
    "service_id": "<service-uuid>",
    "name": "API TLS",
    "kind": "tls",
    "target_json": "{\"host\":\"api.example.com\",\"port\":443}",
    "expect_json": "{\"verify\":true,\"min_days_valid\":14}",
    "interval_seconds": 3600,
    "failure_threshold": 1
  }'
```

4. **View service health**:
```sh
curl https://<monitor>/v1/services \
  -H "Authorization: Bearer <admin-token>"
```

Probes run on enrolled nodes every `interval_seconds`. Results trigger `service_offline` / `service_online` built-in alerts (see [Alerting](./ALERTS.md)).
