# Service Health Monitoring

> v1/v2 "service health" only supports agent-local HTTP/HTTPS probes plus
> certificate checks. ZhiWei extends this into a full observability capability:
> multiple probe types, SLA tracking, public status pages, incident management.

> **Note on scope** — this document describes both the **current** capabilities
> and the **design direction**. As of `0.0.1`, only **HTTP / HTTPS / TCP / TLS
> probes** are implemented (sections 2.1–2.4 below). The remaining sections
> (status pages, incidents, RBAC, multi-tenant) are **planned but not yet
> shipped**; see [docs/roadmap.md](./roadmap.md) for the order they will land.
> Items in the "ZhiWei Extension" column that aren't yet implemented are
> marked `[planned]` in the table below.

## 1. Design Goals

| Dimension | Old Approach | ZhiWei Extension | Status |
| --- | --- | --- | --- |
| Probe types | HTTP / HTTPS | HTTP / HTTPS / TCP / gRPC / DNS / ICMP / TLS / command / multi-step | HTTP / HTTPS / TCP / TLS shipped; rest planned |
| Probe executor | agent (single point) | agent / monitor / external (geo-distributed) | agent-only in 0.0.1 |
| Scheduling | Single interval | interval + retries + backoff + maintenance windows | interval + failure_threshold only |
| Thresholds | Single (ok / fail) | warning + critical (latency, error rate, packet loss) | severity on alerts only; probes are `ok`/`degraded`/`down` |
| SLA | None | uptime % + MTTR + MTBF + error budget | `[planned]` |
| Status page | None | public/private status pages + subscriptions | `[planned]` |
| Incidents | None | incident timeline + blast radius + postmortems | `[planned]` |
| Multi-tenant | None | per-team probes + RBAC | `[planned]` — ZhiWei is single-operator |

## 2. Probe Types

### 2.1 HTTP / HTTPS

```yaml
type: http
method: GET                  # GET / POST / PUT / HEAD
url: https://api.example.com/health
headers:
  Authorization: "Bearer xxx"
  X-Region: us-west
body: |                      # optional, used for POST
  {"ping": "ok"}
expect:
  status: [200, 204]         # expected status codes (any in the list passes)
  body_match:                # response body must contain (any passes)
    - '"status":"ok"'
    - '"alive":true'
  body_regex: '"version":\s*"[0-9]+\.[0-9]+"'  # full regex
  headers_match:
    Content-Type: application/json
  max_latency_ms: 500        # warning threshold
  min_latency_ms: 0
tls:
  verify: true               # false = skip certificate validation (self-signed)
  sni: api.example.com
  min_tls_version: "1.2"
auth:
  basic: { user: x, pass: y }
  bearer: xxx
  oauth2: { token_url, client_id, client_secret, scope }
```

### 2.2 TCP

```yaml
type: tcp
host: db.example.com
port: 5432
expect:
  banner_match: "^.*PostgreSQL.*$"   # optional banner regex
  max_latency_ms: 200
```

### 2.3 gRPC

```yaml
type: grpc
address: grpc.example.com:443
service: grpc.health.v1.Health        # standard health check
method: Check
metadata:
  Authorization: "Bearer xxx"
expect:
  status: SERVING                     # standard grpc-health status
  max_latency_ms: 300
tls:
  verify: true
```

### 2.4 DNS

```yaml
type: dns
server: 1.1.1.1                       # optional; defaults to system resolver
domain: example.com
query_type: A                         # A / AAAA / CNAME / MX / TXT / NS / SOA
expect:
  answers_include: ["1.2.3.4"]        # records that must be present
  answers_exclude: ["0.0.0.0"]        # records that must NOT be present
  max_latency_ms: 100
```

### 2.5 ICMP (ping)

```yaml
type: icmp
host: 1.2.3.4
count: 3
expect:
  packet_loss_pct: 0                  # warning: 1-5; critical: >5
  max_avg_latency_ms: 50
```

### 2.6 TLS Handshake (standalone)

```yaml
type: tls
host: api.example.com
port: 443
expect:
  min_tls_version: "1.2"
  cert_cn: api.example.com
  cert_expires_in_days: 30            # warning threshold (independent of cert tracking)
  max_latency_ms: 500
```

### 2.7 Custom Command

```yaml
type: command
# Only runs on the specified node (agent whitelist required)
node: bj-1
command: ["redis-cli", "-h", "localhost", "ping"]
expect:
  exit_code: 0
  stdout_match: "PONG"
  max_latency_ms: 100
```

> Custom commands are gated by both the agent whitelist and server-side RBAC.
> See SECURITY.md for details.

### 2.8 Probe Composition (multi-step)

```yaml
type: sequence
steps:
  - type: dns
    domain: api.example.com
    query_type: A
    # extract an answer into a variable
    extract: ip_address
  - type: http
    url: "http://{{ .ip_address }}:8080/health"   # use the previous variable
    expect: { status: [200] }
  - type: tls
    host: "{{ .ip_address }}"
    port: 8443

type: parallel
steps:
  - type: http
    url: https://api.example.com/db
    expect: { status: [200] }
  - type: http
    url: https://api.example.com/cache
    expect: { status: [200] }
aggregate:
  # all-ok is ok; any warning → degraded; any critical → critical
  strategy: worst_of
```

## 3. Probe Scheduling

```yaml
schedule:
  interval: 30s              # check frequency
  jitter: 5s                 # random jitter to avoid thundering herds
  timeout: 5s                # per-attempt timeout
  retries: 2                 # retries on failure (exponential backoff)
  retry_backoff: exponential # fixed / linear / exponential
  # maintenance windows: pause + suppress notifications
  maintenance_windows:
    - start: "2026-01-15T02:00:00Z"
      end:   "2026-01-15T04:00:00Z"
      reason: "Database migration"
```

## 4. Execution Location

```yaml
location:
  # pick one or more (multiple = multi-point probing)
  agent: "lab-1"             # run on the named agent (good for intranet)
  monitor: true              # run in the monitor process (good for public internet)
  external_probes:
    - region: us-east-1      # multiple geo regions
      url: https://probe-us-east.zhiwei.io
    - region: eu-west-1
      url: https://probe-eu-west.zhiwei.io
```

Aggregation across multi-point probes:
- `best_of`: any ok is ok (public reachability)
- `worst_of`: all must be ok (strong consistency)
- `majority`: more than half must be ok
- `quorum`: custom threshold

## 5. SLA and SLO

### 5.1 SLO Configuration

```yaml
slo:
  availability:
    target: 99.9%            # monthly target
    window: 30d
    error_budget: 43m        # allowed downtime in 30 days

  latency:
    p95_target: 200ms
    window: 7d
    burn_rate_alert: 14.4x   # multi-window burn rate
```

### 5.2 Live Calculation

```
uptime_pct = (checks_ok + checks_warning) / total_checks
error_budget_remaining = error_budget_total - downtime_paid
burn_rate = (1 - current_availability) / (1 - target_availability)
```

### 5.3 Burn-Rate Alerts

Referencing the Google SRE workbook:
- 2% of the budget burned in 1h → page
- 5% of the budget burned in 6h → page
- 10% of the budget burned in 3d → ticket

## 6. State Machine

```
            ┌─────────┐
            │  OK     │◄────────────────────┐
            └────┬────┘                     │
                 │ check fails N times       │
                 ▼                           │
            ┌─────────┐                     │
            │ DEGRADED│ warning threshold    │
            └────┬────┘                     │
                 │ check fails more          │
                 ▼                           │
            ┌─────────┐                     │
            │ DOWN    │ critical threshold   │
            └────┬────┘                     │
                 │ check recovers            │
                 │ + enter cooldown         │
                 ▼                           │
            ┌─────────┐                     │
            │ RECOVER │                     │
            └────┬────┘                     │
                 │ cooldown elapsed + ok     │
                 └──────────────────────────┘
```

Timeline fields:
- `last_state_change_at`
- `consecutive_failures`
- `total_downtime_in_window`
- `incidents` (merged contiguous failure events)

## 7. Status Page

### 7.1 Internal Status Page

```
/health/services
  - group by team / region
  - current state + recent incidents + SLA dashboard
  - RBAC controls visibility
```

### 7.2 Public Status Page (optional)

```
/status/<tenant>/
  - minimal UI (statuspage.io style)
  - current state (up / degraded / down / maintenance)
  - 90-day uptime bars
  - incident timeline
  - subscribe: email / RSS / webhook / Slack
```

### 7.3 Embedding

```html
<iframe src="https://status.example.com/embed/services"
        width="100%" height="200"></iframe>
```

## 8. Incident Management

### 8.1 Automatic Incident Opening

```
When a service transitions OK → DEGRADED/DOWN:
  - auto-create an incident (open)
  - notify on-call (per the service's on-call schedule)
  - link the most recent N failed probe results
When the service returns to OK and the cooldown elapses:
  - mark the incident resolved
  - compute downtime + blast radius
  - generate a postmortem draft (manual edits allowed)
```

### 8.2 Postmortem Template

```markdown
# Incident: <service> DOWN

## Summary
- Start: 2026-01-15 02:14 UTC
- End: 2026-01-15 02:47 UTC
- Duration: 33 minutes
- Impact: 99.9% SLO consumed 4% of the error budget

## Timeline
- 02:14:00 Probe failed 3 times in a row
- 02:14:30 Alert fired; on-call paged
- 02:18:00 On-call acknowledged: DB primary CPU at 100%
- 02:32:00 Long-running transaction killed
- 02:47:00 Probes recovered

## Root Cause
The DB primary was saturated by an unoptimized bulk query.

## Action Items
- [ ] Add an index for the query (owner: alice, due: 2026-01-22)
- [ ] Add a CPU threshold alert on the DB (owner: bob, due: 2026-01-18)
- [ ] Write a runbook (owner: alice, due: 2026-01-25)
```

## 9. Notification Routing

```yaml
# shares notify config with alert rules
notify:
  on_state_change:
    - from: ok
      to: down
      severity: critical
      receivers: [pagerduty]
    - from: down
      to: ok
      severity: info
      receivers: [slack-incidents]

  template: |
    {{ .service.name }} state changed
    {{ .from }} → {{ .to }}
    Duration: {{ .duration }}
    Last check: HTTP {{ .last_check.status }} · {{ .last_check.latency_ms }}ms
```

## 10. RBAC

> Decision on 2026-09-19: **users and roles are not in scope** for v0.0.x; this
> section is documentation only.

```
roles:
  viewer:     list / status page / read SLA dashboards
  editor:     create/edit/enable/disable probes / configure SLOs
  oncall:     editor + claim incidents / trigger maintenance windows
  admin:      all of the above + delete services / manage RBAC
```

## 11. Data Model

```sql
-- probe definitions
CREATE TABLE probes (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    service_id      TEXT NOT NULL,
    type            TEXT NOT NULL,            -- http/tcp/grpc/dns/icmp/tls/command/sequence/parallel
    config          TEXT NOT NULL,            -- JSON
    schedule        TEXT NOT NULL,            -- JSON
    location        TEXT NOT NULL,            -- JSON
    slo             TEXT,                     -- JSON
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    created_by      TEXT
);

-- services (aggregate multiple probes)
CREATE TABLE services (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    description     TEXT,
    team            TEXT,
    tier            INTEGER NOT NULL DEFAULT 2,  -- 1/2/3 criticality
    public_status_page INTEGER NOT NULL DEFAULT 0,
    tags            TEXT NOT NULL DEFAULT '{}',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

-- check results (time series)
CREATE TABLE probe_results (
    probe_id        TEXT NOT NULL,
    ts              INTEGER NOT NULL,         -- unix nanos
    location        TEXT NOT NULL,            -- agent-id / monitor / external-region
    state           TEXT NOT NULL,            -- ok / degraded / down / timeout / error
    latency_ms      INTEGER,
    status_code     INTEGER,                  -- HTTP/TLS/gRPC
    error           TEXT,
    PRIMARY KEY (probe_id, ts, location)
) WITHOUT ROWID;
CREATE INDEX idx_probe_results_ts ON probe_results(ts);

-- SLA rollup (one row per day)
CREATE TABLE probe_sla_daily (
    probe_id        TEXT NOT NULL,
    day             TEXT NOT NULL,            -- YYYY-MM-DD
    total_checks    INTEGER NOT NULL,
    ok_checks       INTEGER NOT NULL,
    degraded_checks INTEGER NOT NULL,
    down_checks     INTEGER NOT NULL,
    error_checks    INTEGER NOT NULL,
    avg_latency_ms  REAL,
    p95_latency_ms  REAL,
    p99_latency_ms  REAL,
    downtime_sec    INTEGER NOT NULL,
    PRIMARY KEY (probe_id, day)
);

-- incidents
CREATE TABLE incidents (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    probe_id        TEXT NOT NULL,
    state           TEXT NOT NULL,            -- open / acknowledged / resolved
    severity        TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    acknowledged_at INTEGER,
    resolved_at     INTEGER,
    duration_sec    INTEGER,
    oncall_user     TEXT,
    postmortem      TEXT,                     -- markdown
    timeline        TEXT NOT NULL DEFAULT '[]' -- JSON
);

-- maintenance windows
CREATE TABLE maintenance_windows (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    starts_at       INTEGER NOT NULL,
    ends_at         INTEGER NOT NULL,
    reason          TEXT,
    created_by      TEXT
);
```

## 12. Integration with the Alert Engine

Service health is one of the alert engine's data sources:

```
probe.state changes → alert engine evaluates rules → notify
                     ↓
                     incident manager opens/closes
                     ↓
                     SLA recalculates
                     ↓
                     status page updates
```

Alert rules can be based on:
- `probe.state == "down"`
- `probe.latency_ms > 500` for 5m
- `sla.error_budget_remaining < 10%`
- `incident.duration > 30m`

## 13. Implementation Roadmap

| Phase | Content | Estimate |
| --- | --- | --- |
| S-1 | Probe definitions + HTTP/TCP/gRPC execution + result storage | 1.5w |
| S-2 | Multi-step composition + thresholds + state machine | 1w |
| S-3 | SLA calculation + status page + public subscriptions | 1w |
| S-4 | Incident management + postmortems | 0.5w |
| S-5 | Integration with alert engine and cert-manager | 0.5w |

Total: ~4.5 weeks

## 14. Differentiation

| Product | Our Edge |
| --- | --- |
| Prometheus blackbox_exporter | Us: native service-health concept, SLA, status pages, incidents. Them: generic metrics. |
| Uptime Kuma | Us: MCP + certificate management + container management in one. Them: more probe types but lighter. |
| Better Uptime / Statuspage | Us: self-hosted + unified with monitoring/control. Them: SaaS only. |
| Datadog Synthetics | Us: open source + single binary. Them: commercial. |
| Checkly | Us: open source + self-hosted. Them: commercial. |

Core pitch: **monitoring + service health + certificates + incidents + status
page in a single binary**, with no external SaaS dependency.

---

**Summary**: Service health is no longer an "ancillary feature" — it's a core
observability capability of ZhiWei. It extends from "HTTP 200 detection" to a
full SRE practice loop (probe → state → SLA → incident → postmortem).