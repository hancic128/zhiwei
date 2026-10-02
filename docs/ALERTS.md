# Alerting Guide

> How to configure alerts and notifications in ZhiWei.

ZhiWei's alerting system has two layers:

| Layer | Purpose | Examples |
|---|---|---|
| **Alert Rules** | Threshold-based metric alerts | CPU > 90% for 5min |
| **Built-in Alerts** | Event-based alerts | Node offline, probe down, cert expiring |

Both feed into **Notification Channels** (webhooks, Slack, Feishu, etc.).

---

## Alert Rules

Alert rules evaluate telemetry thresholds per node.

### Structure

| Field | Type | Description |
|---|---|---|
| `name` | string | Rule display name |
| `metric` | string | Metric key (see below) |
| `op` | enum | `gt` / `gte` / `lt` / `lte` / `eq` |
| `threshold` | float | Threshold value |
| `duration_seconds` | int | How long condition must persist before firing (0 = immediate) |
| `severity` | enum | `warning` or `critical` |
| `enabled` | bool | Whether rule is active |

### Available Metrics

| Metric Key | Description | Unit |
|---|---|---|
| `host.cpu.usage` | CPU utilization | 0–100 (%) |
| `host.mem.usage` | Memory utilization | 0–100 (%) |
| `host.disk.usage` | Root disk utilization | 0–100 (%) |
| `host.load.1` | 1-minute load average | raw |
| `host.load.5` | 5-minute load average | raw |
| `host.io.read_bps` | Disk read throughput | bytes/s |
| `host.io.write_bps` | Disk write throughput | bytes/s |
| `host.net.rx_bps` | Network receive rate | bytes/s |
| `host.net.tx_bps` | Network transmit rate | bytes/s |
| `container.cpu.usage` | Container CPU (per container) | 0–100 (%) |
| `container.mem.usage` | Container memory (per container) | 0–100 (%) |

> **Note**: Container metrics use the container name as the metric suffix, e.g. `container.cpu.usage:nginx`.

### Create a Rule

```
POST /v1/rules
Authorization: Bearer <admin token>
```

```json
{
  "name": "High CPU Alert",
  "metric": "host.cpu.usage",
  "op": "gt",
  "threshold": 90.0,
  "duration_seconds": 300,
  "severity": "critical"
}
```

Response: `201 Created` with `{ "id": 1 }`.

### Example Rules

**CPU spike (immediate critical)**:
```json
{
  "name": "CPU Spike",
  "metric": "host.cpu.usage",
  "op": "gt",
  "threshold": 95.0,
  "duration_seconds": 0,
  "severity": "critical"
}
```

**Memory sustained high (5min)**:
```json
{
  "name": "Memory Pressure",
  "metric": "host.mem.usage",
  "op": "gt",
  "threshold": 85.0,
  "duration_seconds": 300,
  "severity": "warning"
}
```

**Disk full warning**:
```json
{
  "name": "Disk Full Warning",
  "metric": "host.disk.usage",
  "op": "gt",
  "threshold": 80.0,
  "duration_seconds": 0,
  "severity": "warning"
}
```

---

## Built-in Alerts

Built-in alerts are event-driven and cannot be configured with thresholds — they fire on state transitions.

### Available Built-in Alerts

| ID | Description | Default |
|---|---|---|
| `node_offline` | Node stops reporting for 60s | enabled |
| `node_online` | Node comes back online | enabled |
| `service_offline` | Probe transitions to `down` | enabled |
| `service_online` | Probe recovers from `down` | enabled |
| `container_stopped` | Container stops | enabled |
| `container_started` | Container starts | enabled |
| `cert_expiring` | Certificate expires within notify_days | enabled |
| `cert_expired` | Certificate has expired | enabled |

### Enable / Disable Built-in Alerts

```
PATCH /v1/builtin-alerts/:id
Authorization: Bearer <admin token>
```

```json
{ "enabled": false }
```

---

## Notification Channels

### Channel Types

| Type | Config Required |
|---|---|
| `webhook` | URL, optional Bearer token |
| `slack` | Incoming webhook URL |
| `feishu` | App ID, App Secret, receive_id |
| `bluebird` | URL + Bearer token (generic format) |

### Create a Channel

```
POST /v1/channels
Authorization: Bearer <admin token>
```

**Generic Webhook**:
```json
{
  "name": "My Webhook",
  "kind": "webhook",
  "url": "https://example.com/webhook",
  "secret": "optional-bearer-token",
  "min_severity": "warning"
}
```

**Slack**:
```json
{
  "name": "Slack Alerts",
  "kind": "slack",
  "url": "https://hooks.slack.com/services/xxx/yyy/zzz",
  "min_severity": "warning"
}
```

**Feishu**:
```json
{
  "name": "Feishu Alerts",
  "kind": "feishu",
  "app_id": "cli_xxxxxxxxxxxxxx",
  "secret": "xxxxxxxxxxxxxxxx",
  "receive_id": "oc_xxxxxxxxxxxxxx",
  "receive_id_type": "chat_id",
  "min_severity": "warning"
}
```

### Test a Channel

```
POST /v1/channels/test
Authorization: Bearer <admin token>
```

Sends a test notification. Response:

```json
{
  "ok": true,
  "detail": ""
}
```

---

## Webhook Payload Format

### Generic Webhook (webhook / bluebird)

**Firing alert**:
```json
{
  "title": "🔴 Critical · High CPU Usage (server-01)",
  "text": "🔴 Critical · High CPU Usage (server-01)\
CPU Usage >90% (current 95.0%, duration 300s)",
  "body": "CPU Usage >90% (current 95.0%, duration 300s)",
  "level": "critical",
  "color": "#cf222e",
  "emoji": "🔴",
  "fields": [
    {"label": "Node", "value": "server-01"},
    {"label": "Metric", "value": "CPU Usage"},
    {"label": "Value", "value": "95.0%"},
    {"label": "Threshold", "value": "> 90%"}
  ],
  "rule": "High CPU Usage",
  "severity": "critical",
  "hostname": "server-01",
  "metric": "host.cpu.usage",
  "value": "CPU Usage >90% (current 95.0%, duration 300s)",
  "at_unix_nano": 1727846400000000000
}
```

**Resolved alert**:
```json
{
  "title": "✅ Resolved · High CPU Usage (server-01)",
  "text": "✅ Resolved · High CPU Usage (server-01)\
CPU Usage >90% (current 85.0%)",
  "body": "CPU Usage >90% (current 85.0%)",
  "level": "resolved",
  "color": "#2da44e",
  "emoji": "✅",
  ...
}
```

### Severity Styles

| Severity | color | Emoji |
|---|---|---|
| `critical` | `#cf222e` | 🔴 |
| `warning` | `#d93f0b` | 🟠 |
| `resolved` | `#2da44e` | ✅ |

### HTTP Request

```
POST <url>
Content-Type: application/json
Authorization: Bearer <token>   # if secret configured
```

Request timeout: 10 seconds (5s connect + 5s send).

---

## Alert Lifecycle

### State Machine

```
Normal → (condition met) → Breaching (duration countdown)
Breaching → (duration reached) → Firing (notification sent)
Breaching → (condition cleared) → Normal
Firing → (condition cleared) → Resolved (recovery notification sent)
```

### Deduplication

Alerts are deduplicated by `(rule_id, node_id)` for metric rules, or by `source_ref` for built-in alerts. A new alert is not created if one is already open for the same key.

### Silence

Silence an active alert for a period:

```
POST /v1/alerts/:id/silence
Authorization: Bearer <admin token>
```

```json
{ "minutes": 60 }
```

Maximum: 10080 minutes (7 days). Silenced alerts do not send notifications but remain visible in the UI.

---

## API Reference

### Rules

| Method | Path | Description |
|---|---|---|
| GET | `/v1/rules` | List all rules |
| POST | `/v1/rules` | Create rule |
| PATCH | `/v1/rules/:id` | Update rule |
| DELETE | `/v1/rules/:id` | Delete rule |

### Built-in Alerts

| Method | Path | Description |
|---|---|---|
| GET | `/v1/builtin-alerts` | List built-in alert states |
| PATCH | `/v1/builtin-alerts/:id` | Enable / disable |

### Channels

| Method | Path | Description |
|---|---|---|
| GET | `/v1/channels` | List channels |
| POST | `/v1/channels` | Create channel |
| POST | `/v1/channels/test` | Test channel |
| PATCH | `/v1/channels/:id` | Update channel |
| DELETE | `/v1/channels/:id` | Delete channel |

### Alerts

| Method | Path | Description |
|---|---|---|
| GET | `/v1/alerts` | List alerts (open + resolved) |
| POST | `/v1/alerts/:id/silence` | Silence alert |
| POST | `/v1/alerts/:id/resolve` | Manually resolve alert |

---

## Complete Example

1. **Create a webhook channel**:
```sh
curl -X POST https://<monitor>/v1/channels \
  -H "Authorization: Bearer <admin-token>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Slack Alerts",
    "kind": "slack",
    "url": "https://hooks.slack.com/services/xxx/yyy/zzz",
    "min_severity": "warning"
  }'
```

2. **Create a CPU alert rule**:
```sh
curl -X POST https://<monitor>/v1/rules \
  -H "Authorization: Bearer <admin-token>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "High CPU",
    "metric": "host.cpu.usage",
    "op": "gt",
    "threshold": 90.0,
    "duration_seconds": 300,
    "severity": "critical"
  }'
```

3. **Verify alerts fire** (triggers when condition is met):
```
GET /v1/alerts
```
