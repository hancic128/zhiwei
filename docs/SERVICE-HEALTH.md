# 服务健康度监控（Service Health）

> v1/v2 的「服务健康」只支持 agent 本机 HTTP/HTTPS 探测 + 证书检查。
> 知微把它扩展为完整的可观测能力：多探针类型、SLA 跟踪、状态页、事件管理。

## 1. 设计目标

| 维度 | 旧方案 | 知微扩展 |
| --- | --- | --- |
| 探针类型 | HTTP / HTTPS | HTTP / HTTPS / TCP / gRPC / DNS / ICMP / TLS / 命令 / 多步 |
| 探针执行方 | agent（单点） | agent / monitor / 外部（geo-distributed） |
| 调度 | 单一间隔 | 间隔 + 重试 + 退避 + 维护窗口 |
| 阈值 | 单一（异常/正常） | warning + critical（延迟、错误率、丢包率） |
| SLA | 无 | uptime % + MTTR + MTBF + 错误预算 |
| 状态页 | 无 | 公共/私有状态页 + 订阅 |
| 事件 | 无 | 事故时间线 + 影响范围 + 事后总结 |
| 多租户 | 无 | per-team 探针 + RBAC |

## 2. 探针类型

### 2.1 HTTP / HTTPS

```yaml
type: http
method: GET                  # GET / POST / PUT / HEAD
url: https://api.example.com/health
headers:
  Authorization: "Bearer xxx"
  X-Region: us-west
body: |                      # 可选，POST 时使用
  {"ping": "ok"}
expect:
  status: [200, 204]         # 期望状态码（列表 = 任一即可）
  body_match:                # 响应体包含（任一即通过）
    - '"status":"ok"'
    - '"alive":true'
  body_regex: '"version":\s*"[0-9]+\.[0-9]+"'  # 完整 regex
  headers_match:
    Content-Type: application/json
  max_latency_ms: 500        # warning 阈值
  min_latency_ms: 0
tls:
  verify: true               # false = 跳过证书校验（自签场景）
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
  banner_match: "^.*PostgreSQL.*$"   # 可选 banner 正则
  max_latency_ms: 200
```

### 2.3 gRPC

```yaml
type: grpc
address: grpc.example.com:443
service: grpc.health.v1.Health        # 标准 health check
method: Check
metadata:
  Authorization: "Bearer xxx"
expect:
  status: SERVING                     # grpc-health 的标准状态
  max_latency_ms: 300
tls:
  verify: true
```

### 2.4 DNS

```yaml
type: dns
server: 1.1.1.1                       # 可选，默认系统 resolver
domain: example.com
query_type: A                         # A / AAAA / CNAME / MX / TXT / NS / SOA
expect:
  answers_include: ["1.2.3.4"]        # 必须包含的记录
  answers_exclude: ["0.0.0.0"]        # 必须不包含
  max_latency_ms: 100
```

### 2.5 ICMP（ping）

```yaml
type: icmp
host: 1.2.3.4
count: 3
expect:
  packet_loss_pct: 0                  # warning: 1-5; critical: >5
  max_avg_latency_ms: 50
```

### 2.6 TLS handshake（独立检查）

```yaml
type: tls
host: api.example.com
port: 443
expect:
  min_tls_version: "1.2"
  cert_cn: api.example.com
  cert_expires_in_days: 30            # warning 阈值（独立于 cert 监控）
  max_latency_ms: 500
```

### 2.7 自定义命令

```yaml
type: command
# 仅在指定 node 上执行（agent 白名单）
node: bj-1
command: ["redis-cli", "-h", "localhost", "ping"]
expect:
  exit_code: 0
  stdout_match: "PONG"
  max_latency_ms: 100
```

> 自定义命令受 agent 白名单 + 服务端 RBAC 双重管控。详见 SECURITY.md。

### 2.8 多步组合（probe composition）

```yaml
type: sequence
steps:
  - type: dns
    domain: api.example.com
    query_type: A
    # 把答案提取到变量
    extract: ip_address
  - type: http
    url: "http://{{ .ip_address }}:8080/health"   # 用上一步的变量
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
  # 全 ok 才 ok；任一 warning 则 degraded；任一 critical 则 critical
  strategy: worst_of
```

## 3. 探针调度

```yaml
schedule:
  interval: 30s              # 检查频率
  jitter: 5s                 # 随机抖动，避免惊群
  timeout: 5s                # 单次超时
  retries: 2                 # 失败重试次数（指数退避）
  retry_backoff: exponential # fixed / linear / exponential
  # 维护窗口：暂停探测 + 不告警
  maintenance_windows:
    - start: "2026-01-15T02:00:00Z"
      end:   "2026-01-15T04:00:00Z"
      reason: "数据库迁移"
```

## 4. 执行位置

```yaml
location:
  # 三选一或多选（多选 = 多点探测）
  agent: "lab-1"             # 在该 agent 上执行（适合内网）
  monitor: true              # 在 monitor 进程内执行（适合公网/少量）
  external_probes:
    - region: us-east-1      # 多地理区域
      url: https://probe-us-east.zhiwei.io
    - region: eu-west-1
      url: https://probe-eu-west.zhiwei.io
```

多点探测结果聚合：
- `best_of`：任一 ok 即 ok（公网可达性）
- `worst_of`：全部 ok 才 ok（强一致）
- `majority`：半数以上 ok 即 ok
- `quorum`：自定义阈值

## 5. SLA 与 SLO

### 5.1 SLO 配置

```yaml
slo:
  availability:
    target: 99.9%            # 月度目标
    window: 30d
    error_budget: 43m        # 30 天允许多少停机
  
  latency:
    p95_target: 200ms
    window: 7d
    burn_rate_alert: 14.4x   # 多窗口 burn rate
```

### 5.2 实时计算

```
uptime_pct = (checks_ok + checks_warning) / total_checks
error_budget_remaining = error_budget_total - downtime_paid
burn_rate = (1 - current_availability) / (1 - target_availability)
```

### 5.3 Burn rate 告警

参考 Google SRE workbook：
- 2% 预算在 1h 内烧完 → page
- 5% 预算在 6h 内烧完 → page
- 10% 预算在 3d 内烧完 → ticket

## 6. 状态机

```
            ┌─────────┐
            │  OK     │◄────────────────────┐
            └────┬────┘                     │
                 │ check fails N times       │
                 ▼                           │
            ┌─────────┐                     │
            │ DEGRADED│ warning 阈值         │
            └────┬────┘                     │
                 │ check fails more          │
                 ▼                           │
            ┌─────────┐                     │
            │ DOWN    │ critical 阈值        │
            └────┬────┘                     │
                 │ check recovers            │
                 │ + 进入 cooldown           │
                 ▼                           │
            ┌─────────┐                     │
            │ RECOVER │                     │
            └────┬────┘                     │
                 │ cooldown 结束 + ok        │
                 └──────────────────────────┘
```

时间线：
- `last_state_change_at`
- `consecutive_failures`
- `total_downtime_in_window`
- `incidents`（合并的连续故障事件）

## 7. 状态页

### 7.1 内部状态页

```
/health/services
  - 按 group / team / region 分组
  - 当前状态 + 最近事件 + SLA 仪表盘
  - RBAC 控制可见性
```

### 7.2 公共状态页（可选）

```
/status/<tenant>/
  - 极简 UI（status.zhiwei.io 风格）
  - 当前状态（up / degraded / down / maintenance）
  - 90 天 uptime 柱状图
  - 事故时间线
  - 订阅：email / RSS / webhook / Slack
```

### 7.3 嵌入

```html
<iframe src="https://status.example.com/embed/services" 
        width="100%" height="200"></iframe>
```

## 8. 事故管理

### 8.1 自动开事故

```
当 service 从 OK → DEGRADED/DOWN：
  - 自动创建 incident（open）
  - 通知 on-call（按 service 的 on-call schedule）
  - 关联最近 N 条失败的探针数据
当 service 回到 OK 且 cooldown 结束：
  - incident 标记为 resolved
  - 计算 downtime + 影响
  - 生成 postmortem 草稿（可手动补充）
```

### 8.2 事后总结模板

```markdown
# Incident: <service> DOWN

## Summary
- 开始：2026-01-15 02:14 UTC
- 结束：2026-01-15 02:47 UTC
- 持续：33 分钟
- 影响：99.9% SLO 消耗 4% 错误预算

## Timeline
- 02:14:00 探针连续失败 3 次
- 02:14:30 alert 触发，page on-call
- 02:18:00 on-call 确认，DB 主节点 CPU 100%
- 02:32:00 杀掉长事务
- 02:47:00 探针恢复

## Root Cause
DB 主节点被一个未优化的批量查询占满 CPU。

## Action Items
- [ ] 给查询加索引（owner: alice, due: 2026-01-22）
- [ ] 给 DB 加 CPU 阈值告警（owner: bob, due: 2026-01-18）
- [ ] 复盘 runbook（owner: alice, due: 2026-01-25）
```

## 9. 通知路由

```yaml
# 与告警规则共享通知配置
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
    🔴 {{ .service.name }} 状态变更
    {{ .from }} → {{ .to }}
    持续：{{ .duration }}
    最近一次检查：HTTP {{ .last_check.status }} · {{ .last_check.latency_ms }}ms
```

## 10. RBAC

> 2026-09-19 拍板：**不需要用户与角色**，本节不落地。见
> [POSITIONING.md](./POSITIONING.md) 的排除清单。

```
roles:
  viewer:     list / status page / read SLA dashboards
  editor:     创建/修改/启停探针 / 配置 SLO
  oncall:     editor + 认领 incident / 触发 maintenance window
  admin:      全部 + 删除 service / 管理 RBAC
```

## 11. 数据模型

```sql
-- 探针定义
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

-- 服务（聚合多个探针）
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

-- 检查结果（时序）
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

-- SLA 计算结果（每天一行）
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

-- 事故
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

-- 维护窗口
CREATE TABLE maintenance_windows (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    starts_at       INTEGER NOT NULL,
    ends_at         INTEGER NOT NULL,
    reason          TEXT,
    created_by      TEXT
);
```

## 12. 与告警引擎集成

服务健康是告警引擎的「数据源」之一：

```
probe.state changes → alert engine evaluates rules → notify
                     ↓
                     incident manager opens/closes
                     ↓
                     SLA recalculates
                     ↓
                     status page updates
```

告警规则可基于：
- `probe.state == "down"`
- `probe.latency_ms > 500` for 5m
- `sla.error_budget_remaining < 10%`
- `incident.duration > 30m`

## 13. 实施路线

| 阶段 | 内容 | 估时 |
| --- | --- | --- |
| S-1 | 探针定义 + HTTP/TCP/gRPC 执行 + 结果入库 | 1.5w |
| S-2 | 多步组合 + 阈值 + 状态机 | 1w |
| S-3 | SLA 计算 + 状态页 + 公共订阅 | 1w |
| S-4 | 事故管理 + 事后总结 | 0.5w |
| S-5 | 与告警引擎、cert-manager 联动 | 0.5w |

合计 ~ 4.5 周

## 14. 与现有产品的差异化

| 产品 | 我们的差异化 |
| --- | --- |
| Prometheus blackbox_exporter | 我们：原生服务健康概念、SLA、状态页、事故管理；它：通用 metric |
| Uptime Kuma | 我们：MCP + 证书管理 + 容器管理一体化；它：探针种类更多但轻量 |
| Better Uptime / Statuspage | 我们：自托管 + 与监控/控制统一；它们：SaaS |
| Datadog Synthetics | 我们：开源 + 单二进制；它：商业 |
| Checkly | 我们：开源 + 自托管；它：商业 |

核心卖点：**「监控 + 服务健康 + 证书 + 事故 + 状态页」全部在一个二进制里**，不依赖外部 SaaS。

---

**总结**：服务健康度不再是旧方案的「附带功能」，而是知微的核心可观测能力之一。
它从「HTTP 200 检测」扩展为完整的 SRE 实践闭环（探针 → 状态 → SLA → 事故 → 复盘）。
