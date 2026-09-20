# 知微（ZhiWei）架构设计

> 本文档是「知微」多主机可观测平台的完整架构设计。
> 不带历史包袱，从零起步，不考虑兼容旧 agent、旧 API、旧 DB。
> 覆盖：自动发现/注册/热更新、资源监控、容器与日志、节点管理、SSL 证书、告警、MCP。

**注意**：本文档原命名 `DESIGN-V3.md`、物理位置原在 `/Users/shark/Project/server-monitor/docs/`。2026-09-19 起随项目迁出至本目录并去 v3 印记，作为 zhiwei 自身的设计稿使用。后续如需重写或大幅调整，请在 zhiwei 仓库内独立演进，与其他独立项目无任何关联。

## 1. 设计原则

| # | 原则 | 体现 |
| --- | --- | --- |
| 1 | **零共享密钥** | 每节点独立身份（Ed25519 密钥对），杜绝「一台被攻陷传染全网」 |
| 2 | **最小权限** | 每个节点的能力集独立配置（能不能被 reboot、能不能启停容器） |
| 3 | **签名而非凭据** | 命令由 monitor 私钥签名，节点验签；token 泄露 ≠ 能发命令 |
| 4 | **TTL 一切** | 命令、token、能力全部带过期时间 |
| 5 | **能力可观测** | 所有写操作进审计链，所有控制流可追溯 |
| 6 | **数据/控制分离** | 监控数据走 telemetry 通道；远程操作走 control 通道；证书管理走 cert-mgmt 通道 |
| 7 | **可恢复** | 节点离线时本地缓存样本，恢复后批量回传 |
| 8 | **插件化** | 通知渠道、容器运行时、证书颁发机构都可替换 |

## 2. 总体架构

```
┌─────────────────────────────────────────────────────────────────────────┐
│                       Monitor (Go binary)                                │
│                                                                          │
│  ┌────────────────┐  ┌────────────────┐  ┌────────────────┐            │
│  │ Identity CA    │  │ Telemetry Sink │  │ Control Plane  │            │
│  │ (issue/revoke) │  │ (ingest/store) │  │ (sign/exec)    │            │
│  └────────────────┘  └────────────────┘  └────────────────┘            │
│  ┌────────────────┐  ┌────────────────┐  ┌────────────────┐            │
│  │ Alert Engine   │  │ Cert Manager   │  │ MCP Server     │            │
│  │                │  │ (ACME/CA)      │  │ (stdio/sse)    │            │
│  └────────────────┘  └────────────────┘  └────────────────┘            │
│  ┌────────────────┐  ┌────────────────┐  ┌────────────────┐            │
│  │ Web UI (SPA)   │  │ API Gateway    │  │ Audit Log      │            │
│  └────────────────┘  └────────────────┘  └────────────────┘            │
│           │                  │                  │                       │
│           └──────────────────┴──────────────────┘                       │
│                              │                                           │
│                     ┌────────┴────────┐                                  │
│                     │   Storage       │                                  │
│                     │  (TSDB + meta)  │                                  │
│                     └─────────────────┘                                  │
└─────────────────────────────────────────────────────────────────────────┘
                │ mTLS (HTTP/2)
                │
   ┌────────────┴────────────┬─────────────┬─────────────┐
   │                         │             │             │
┌──┴────────────┐   ┌────────┴──┐   ┌──────┴───┐   ┌─────┴────┐
│ Node A (Go)   │   │ Node B    │   │ Node C   │   │ Node D   │
│ - telemetry   │   │           │   │          │   │          │
│ - cert client │   │           │   │          │   │          │
│ - log agent   │   │           │   │          │   │          │
│ - container   │   │           │   │          │   │          │
└───────────────┘   └────────────┘   └──────────┘   └──────────┘
```

**Monitor**：单二进制 Go 服务，承载 CA、TSDB、控制平面、Web UI、MCP、证书管理、告警。

**Node**：单二进制 Go 服务（替代现在的 bash 脚本），可观测、可控制、可恢复。

## 3. 身份与注册

> **2026-09-19 修订（实现以此为准）**：节点身份从「mTLS 客户端证书」改为
> 「**Ed25519 请求签名**」。原因是托管平台（Render 等）在边缘终止 TLS、不向容器
> 转发客户端证书，证书模型在 PaaS 上直接失效；而签名模型与本文第 1 章原则
> #3「签名而非凭据」一致，且在明文 HTTP 边缘下鉴权强度不变。
>
> 落地形态：node 生成 Ed25519 密钥对（`signing.key`），enroll 时提交**公钥**
> （不再提交 CSR），此后每个请求带 `x-zhiwei-node/-timestamp/-nonce/-signature`
> 头，monitor 验签 + ±300s 时间窗 + nonce 防重放（`crates/common/src/auth.rs`）。
> 命令通道保持双向签名：ops 签命令、node 验签；node 签回执、monitor 验签。
> mTLS 由「必需」降为自建场景的可选加固。
>
> 下面的 §3.1 / §3.2 保留初版设计原貌，仅作沿革参考。

### 3.1 身份模型

```
每个 node 持有：
- node.key      Ed25519 私钥（仅本机 root 可读，0600）
- node.crt      由 monitor CA 签发的客户端证书（24h TTL，自动续签）

monitor 持有：
- ca.key        CA 私钥（仅 monitor root 可读）
- ca.crt        CA 证书（部署到所有 node）
- monitor.key   monitor 用于签命令的私钥
- monitor.crt   monitor 服务器证书
```

**关键不变量**：
- node 私钥**永不**离开 node（除了初始通过离线手段注入）
- monitor 私钥**永不**分发（node 用 monitor 的公钥验签命令）
- token 概念**完全消失** —— 取代以 mTLS + 签名

### 3.2 注册流程

#### 模式 A：自动注册（推荐 LAN/可信任环境）

```
1. operator 在 monitor 上生成一次性注册 token：
   monitor-cli enroll-token create --ttl 10m
   → 返回 token 字符串（仅显示一次）

2. node 首次启动，配置：
   monitor-url: https://monitor.lan:8443
   bootstrap-token: <上一步的 token>
   node-name: bj-1   （或自动取 hostname）

3. node 用 bootstrap token 调 /v1/enroll：
   POST /v1/enroll
   Authorization: Bearer <bootstrap-token>
   Body: { csr: <PKCS#10 CSR>, hostname, labels: {role: web, env: prod} }

4. monitor 验证 token → 签发证书：
   Response: { cert: <X.509>, ca: <CA cert>, monitor_cert: <server cert> }

5. node 保存 cert/CA，丢弃 bootstrap token，从此后只用 mTLS

6. monitor 自动登记 node 到 inventory 表，生成默认能力集（只读）
```

#### 模式 B：手动注册（生产 / 多租户）

```
1. operator 在 node 上生成密钥对：
   node-cli init --name bj-1

2. operator 把 CSR 提交给 monitor（CLI / Web UI）：
   monitor-cli node approve <csr-file> --capabilities read,kill,restart

3. monitor 签发证书，operator 导入：
   node-cli import-cert <cert-file>

4. 启动 node，开始 mTLS 通信
```

#### 自动发现

```
node 启动时可启用发现：
- DNS-SRV 查询 _monitor._tcp.example.com
- mDNS 查询 _monitor._tcp.local（仅 LAN）
- 静态配置兜底

找到 monitor URL 后，重复模式 A 的流程。
```

### 3.3 证书续签

```
node 启动后台协程，每 6h 检查证书：
- 剩余 TTL < 6h → 主动续签（POST /v1/renew，附旧 cert 验签）
- 续签失败 → 退避重试（30s/1m/5m）
- 续签成功 → 替换内存中的 cert，旧 cert 保留 5s 用于在途请求
```

### 3.4 节点下线

```
operator 命令：monitor-cli node decommission <node-id>
monitor:
  - CRL 加入该证书（24h 内所有 node 拒绝）
  - 清除 inventory、samples、audit 中可识别数据
  - 通知告警系统「node 已退役」
```

## 4. Telemetry（监控数据上报）

### 4.1 协议

```
POST /v1/telemetry
Content-Type: application/protobuf
mTLS (HTTP/2)

Body: TelemetryBatch {
  node_id:    string
  ts:         int64  // unix nanos
  interval:   int32  // 采样间隔（秒）
  metrics:    map<string, double>  // 标签化指标
  containers: []Container
  procs:      ProcessSnapshot
  logs?:      []LogChunk     // 主动 push 的系统日志（journald 风格）
}
```

### 4.2 指标清单

| 指标 | 单位 | 标签 | 采集方式 |
| --- | --- | --- | --- |
| `cpu_usage_pct` | 0-100 | `core=all` | /proc/stat 差分 |
| `mem_used_bytes` | bytes | — | /proc/meminfo |
| `mem_available_bytes` | bytes | — | /proc/meminfo |
| `disk_used_bytes` | bytes | `mount=/` | statvfs |
| `disk_total_bytes` | bytes | `mount=/` | statvfs |
| `net_rx_bytes` | bytes/sec | `iface=eth0` | /proc/net/dev 差分 |
| `net_tx_bytes` | bytes/sec | `iface=eth0` | 同上 |
| `load1` / `load5` / `load15` | float | — | /proc/loadavg |
| `uptime_seconds` | seconds | — | /proc/uptime |
| `proc_cpu_pct` | 0-100 | `pid,comm` | /proc/[pid]/stat |
| `proc_mem_bytes` | bytes | `pid,comm` | /proc/[pid]/status |
| `container_cpu_pct` | 0-100 | `id,name,image` | cgroup v2 |
| `container_mem_bytes` | bytes | `id,name,image` | 同上 |
| `cert_days_remaining` | int | `cn,san` | openssl x509 -enddate |

### 4.3 容器抽象

```go
type Container struct {
    ID      string
    Name    string
    Image   string
    State   string  // running/exited/...
    Status  string
    Runtime string  // docker/containerd/podman
    CPU     float64
    Memory  uint64
    Net     NetIO
    Created time.Time
}

type ContainerRuntime interface {
    List() ([]Container, error)
    Start(id string) error
    Stop(id string, timeout time.Duration) error
    Restart(id string) error
    Logs(id string, opts LogOpts) (LogStream, error)
}
```

默认实现：Docker；可选 containerd、Podman、CRI-O。

### 4.4 进程抽象

```go
type ProcessSnapshot struct {
    TopByMem []Process  // 前 10
    TopByCPU []Process
}

type Process struct {
    PID     int
    PPID    int
    Comm    string
    User    string
    RSS     uint64  // bytes
    CPUPct  float64
    State   string
    Cmd     string  // 可选，用于告警现场
}
```

### 4.5 日志流（实时 + 归档）

每个 node 暴露：
- `GET /v1/logs?container=...&since=...&follow=true`（SSE）
- `GET /v1/logs/journald?unit=...`（系统日志）

Monitor 转发给 Web UI（WebSocket）和 MCP（streamable）。

### 4.6 离线缓存

node 本地 SQLite WAL 缓冲样本（最多 100 MB / 24h）：
- 与 monitor 连接失败 → 写入本地
- 恢复后批量回传（按时间窗口，避免拥塞）
- 超过容量 → 丢弃最旧（FIFO，带告警）

## 5. Control Plane（远程操作）

### 5.1 命令协议

```protobuf
message Command {
  id:        string      // uuid
  action:    Action      // enum
  params:    bytes       // action-specific payload
  nonce:     bytes       // 16 bytes random
  issued_at: int64       // unix nanos
  ttl:       int32       // seconds, max 60
  signature: bytes       // ed25519(monitor.key, all fields above)
}

enum Action {
  REBOOT = 1;
  SHUTDOWN = 2;
  CONTAINER_START = 10;
  CONTAINER_STOP = 11;
  CONTAINER_RESTART = 12;
  PROCESS_KILL = 20;
  CERT_RENEW = 30;
  RELOAD_CONFIG = 40;
}
```

### 5.2 下发流程

```
operator（Web UI / CLI / MCP）：
  monitor-cli exec reboot <node-id> --reason "kernel update"

monitor：
  1. 检查 actor 权限（RBAC）
  2. 构造 Command payload
  3. 用 monitor.key 签 ed25519
  4. 写入 outbox（带 TTL）

node（每 5s 轮询）：
  GET /v1/commands?since=<last_id>
  返回 [{cmd, signature, monitor_cert}, ...]

node 验签：
  1. 用 monitor.crt 里的公钥验证 signature
  2. 校验 ttl + issued_at 未过期
  3. 校验 nonce 未用过（防重放）
  4. 校验 action 在 node 能力集内
  5. 全部通过 → 执行；否则 → 拒绝 + 审计日志
```

### 5.3 结果回报

```protobuf
message CommandResult {
  id:        string
  status:    enum { OK, ERROR, TIMEOUT, DENIED }
  output:    bytes       // stdout/stderr（截断 64KB）
  error:     string
  finished_at: int64
  signature: bytes       // ed25519(node.key, all fields above) —— 防 monitor 被篡改
}
```

```
node: POST /v1/commands/result
      Body: CommandResult
      Content-Type: application/protobuf

monitor: 验签 → 写审计 → 推送给等待的 operator（WebSocket / SSE）
```

### 5.4 RBAC（节点能力集）——**已定不做**

> 2026-09-19 拍板：**不需要用户与角色**。控制台只有单一 admin token，
> 没有「用户」这一层。本节保留为设计记录，落地时以
> [POSITIONING.md](./POSITIONING.md) 的排除清单为准。

```yaml
# monitor 配置
nodes:
  bj-1:
    capabilities: [read, container_start, container_stop, container_restart, process_kill]
    denied_capabilities: [reboot, shutdown]   # 这台是生产，不许远程重启

  lab-1:
    capabilities: [read, container_*, process_*, reboot, shutdown, cert_renew]
    # 测试机，所有能力
```

operator 角色：
- `viewer`：只读
- `operator`：除 reboot/shutdown 外所有
- `admin`：全部

### 5.5 二次确认（敏感操作）

```yaml
policies:
  reboot:
    require_approval: true
    approvers: ["oncall@example.com"]   # 邮件 / Webhook / Slack
    timeout: 5m
    # 流程：operator 申请 → 通知 approver → approver 二次确认 → 执行
```

## 6. 节点管理（开关机/容器）

### 6.1 节点操作

| 操作 | 命令 | 要求 |
| --- | --- | --- |
| 关机 | `monitor-cli node shutdown <id> --graceful` | capability + RBAC |
| 重启 | `monitor-cli node reboot <id> --reason "..."` | capability + RBAC |
| 容器启动 | `monitor-cli container start <node> <name>` | capability |
| 容器停止 | `monitor-cli container stop <node> <name> --timeout 30s` | capability |
| 容器重启 | `monitor-cli container restart <node> <name>` | capability |
| 进程 kill | `monitor-cli proc kill <node> <pid> --signal TERM` | capability |
| 进程 kill -9 | `monitor-cli proc kill <node> <pid> --signal KILL` | capability + admin |

所有操作在 Web UI 中均为「确认对话框 → 显示主机/目标/当前状态/影响」 → 执行。

### 6.2 批量操作

```bash
# 按标签批量
monitor-cli container restart --label env=staging
monitor-cli node reboot --label role=worker --reason "kernel CVE-2026-001"

# 滚动重启
monitor-cli node rolling-reboot --label role=worker --wait --batch-size 2
```

## 7. SSL 证书管理

### 7.1 能力

```
- 扫描：定期（每 6h）扫描所有 node 上的证书文件
- 列表：列出全部证书（CN、SAN、issuer、到期时间、所在 node/路径）
- 告警：到期前 30d / 7d / 1d 推送
- 续签：支持多种 CA 集成
- 推送：续签后自动推送到目标 node（通过 control plane）
- 回滚：失败时自动回滚到上一版本
```

### 7.2 CA 集成

```go
type CA interface {
    Issue(domain string) (*Cert, error)
    Renew(cert *Cert) (*Cert, error)
    Revoke(cert *Cert) error
}

实现：
- ACMEProvider:    Let's Encrypt / 自建 ACME（Buypass、ZeroSSL）
- SelfSignedCA:    monitor 自签 + 内部 PKI
- VaultProvider:   HashiCorp Vault PKI
- FileProvider:    直接读已有文件 + 外部脚本续签（如 certbot）
- CertManager:     外部 cert-manager（k8s 场景）
```

### 7.3 数据模型

```sql
CREATE TABLE certs (
    id          INTEGER PRIMARY KEY,
    node_id     TEXT NOT NULL,
    cn          TEXT NOT NULL,
    san         TEXT,           -- json 数组
    issuer      TEXT,
    path        TEXT NOT NULL,  -- /etc/ssl/certs/...
    issued_at   INTEGER,
    expires_at  INTEGER NOT NULL,
    status      TEXT,           -- ok/expiring/expired/renewing/error
    last_check  INTEGER,
    meta        TEXT            -- json: CA、续签策略等
);
CREATE INDEX idx_certs_expires ON certs(expires_at);
```

### 7.4 续签流程

```
monitor 检测到 cert X 剩余 < 30d：
  1. 调 ca.Issue(domain) → 新 cert
  2. 签名 Command{action: CERT_RENEW, params: {path, new_cert, old_backup}}
  3. 下发到 node
  4. node 验证 cert 有效性（openssl verify）
  5. node 备份旧 cert → 写新 cert → reload 服务
  6. node 回传 CommandResult + 新 cert 指纹
  7. monitor 更新 certs 表
  8. 失败 → 自动重试 1 次；仍失败 → 告警
```

## 8. 告警引擎

### 8.1 规则 DSL

```yaml
# /etc/monitor/alerts.yml
groups:
  - name: resources
    interval: 30s
    rules:
      - alert: HighCPU
        expr: cpu_usage_pct > 90
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "{{ $labels.node }} CPU > 90%"
          runbook: "https://wiki/..."

      - alert: DiskWillFillIn24h
        expr: predict_linear(disk_used_bytes, 24*3600) > disk_total_bytes
        for: 10m
        severity: critical

      - alert: CertExpiresSoon
        expr: cert_days_remaining < 30 and cert_days_remaining > 0
        for: 1h
        severity: warning

      - alert: CertExpired
        expr: cert_days_remaining <= 0
        for: 5m
        severity: critical
```

### 8.2 引擎实现

```
- 用 PromQL 子集或自定义 expr 解析
- 评估循环：每 30s 一次，对每条 rule 计算状态
- 状态机：inactive → pending → firing → resolved
- 抑制：父子规则、同 source 抑制
- 静默：manual silence（按 label 选择器）
```

### 8.3 通知渠道

```go
type Notifier interface {
    Send(ctx context.Context, alert Alert) error
}

实现：
- WebhookNotifier:     POST + Bearer/Signature
- SlackNotifier:       Slack incoming webhook
- EmailNotifier:       SMTP
- TelegramNotifier:    Bot API
- BarkNotifier:        iOS push
- FeishuNotifier:      飞书机器人
- PagerDutyNotifier:   PagerDuty Events API
```

### 8.4 路由

```yaml
routes:
  - match: { severity: critical }
    receivers: [pagerduty, slack-oncall]
    repeat_interval: 1h
  - match: { severity: warning }
    receivers: [slack-alerts]
    repeat_interval: 6h
  - match: { cert_days_remaining: "<7" }
    receivers: [email-ssl-team]
```

## 9. MCP 集成

### 9.1 工具列表

```
Read:
  list_nodes()                       → 节点清单 + 状态
  get_node(id)                       → 节点详情 + 最近指标
  get_telemetry(id, range)           → 时序数据
  list_containers(id)                → 容器列表
  container_logs(id, name, tail)     → 流式日志
  list_processes(id, sort)           → 进程 TopN
  list_alerts(state)                 → 活跃/历史告警
  list_certs(filter)                 → 证书清单

Manage:
  add_node(csr, capabilities)        → 审批入网
  remove_node(id, reason)            → 下线
  update_capabilities(id, caps)      → 修改能力
  shutdown_node(id, graceful)        → 关机
  reboot_node(id, reason)            → 重启
  container_action(id, name, action) → 启停容器
  kill_process(id, pid, signal)      → 杀进程
  renew_cert(id)                     → 触发续签
  silence_alert(rule, duration)      → 静默告警
  test_notifier(channel)             → 测试通知
```

### 9.2 鉴权

MCP 服务以 stdio 或 SSE 暴露：
- stdio：本地进程，unix socket 鉴权
- SSE：HTTPS + Bearer（用户的 monitor 登录 token）

工具调用走 monitor 的 RBAC，与 Web UI 同一套权限。

## 10. Web UI

### 10.1 信息架构

```
┌──────────────────────────────────────────────────────────────┐
│ Sidebar   │  Header（搜索、通知、用户菜单）                     │
│           ├──────────────────────────────────────────────────┤
│ 概览     │                                                      │
│ 节点     │                                                      │
│  容器    │              主内容区                                │
│  日志    │                                                      │
│  证书    │                                                      │
│ 告警     │                                                      │
│ 设置     │                                                      │
│           │                                                      │
└──────────────────────────────────────────────────────────────┘
```

### 10.2 核心页面

- **Overview**：全集群 KPI + 全局热点
- **Nodes**：节点列表/详情/拓扑
- **Containers**：按 node 分组的容器视图
- **Logs**：统一日志搜索（journald + 容器）
- **Certificates**：证书全景（到期倒计时）
- **Alerts**：活跃告警 + 静默管理
- **Settings**：节点能力、通知路由、CA 配置、用户/角色

### 10.3 实时性

- 指标：HTTP/2 SSE 流（每 node 独立 channel）
- 命令结果：WebSocket（双向）
- 日志：WebSocket（按 container 订阅）
- 告警：SSE 推送

### 10.4 技术栈

```
- React 18 + TypeScript
- TanStack Query（数据）
- Zustand（本地状态）
- Tailwind + shadcn/ui（组件）
- ECharts / uPlot（图表）
- Vite 构建
- 单一 SPA bundle（gzip < 200KB）
```

## 11. 存储

### 11.1 时序数据

```
方案 A：SQLite + 扩展（轻量、单文件、≤ 100 万指标/秒）
  CREATE TABLE metrics (
      ts INTEGER,        -- unix nanos
      node TEXT,
      name TEXT,         -- cpu_usage_pct
      labels TEXT,       -- json
      value REAL
  );
  CREATE INDEX idx_metrics_query ON metrics(name, node, ts);

方案 B：VictoriaMetrics 单二进制（推荐 > 50 节点或 > 1 万指标/秒）
方案 C：ClickHouse（多机/超大规模）
```

### 11.2 元数据

SQLite：
- `nodes`（inventory、能力、标签）
- `certificates`（见 7.3）
- `commands`（历史命令、审计）
- `alert_state`（抑制、静默）
- `users` / `roles` / `sessions`
- `audit_log`（append-only）

### 11.3 保留策略

```yaml
retention:
  default: 30d
  rules:
    - match: { name: "cpu_usage_pct" }    retention: 90d
    - match: { name: "cert_*" }           retention: 365d
    - match: { name: "audit_*" }          retention: forever
```

后台 compaction 任务每 6h 执行。

## 12. 部署形态

### 12.1 单二进制 + 配置

```bash
# monitor
monitor-server --config /etc/monitor/server.yml

# node
monitor-node --config /etc/monitor/node.yml
```

### 12.2 Docker

```yaml
# docker-compose.yml
services:
  monitor:
    image: monitor/server:latest
    ports: ["8443:8443"]
    volumes:
      - ./data:/var/lib/monitor
      - ./server.yml:/etc/monitor/server.yml
    restart: unless-stopped

  node:
    image: monitor/node:latest
    pid: host                    # 看 host 进程
    volumes:
      - /proc:/proc:ro
      - /sys:/sys:ro
      - /var/run/docker.sock:/var/run/docker.sock:ro
      - ./node.yml:/etc/monitor/node.yml
    restart: unless-stopped
```

### 12.3 Kubernetes

```
- monitor: Deployment + Service + PVC
- node: DaemonSet（每 node 一个）
- cert-manager 集成（可选）
```

## 13. 安全模型总结

| 攻击面 | 缓解 |
| --- | --- |
| Node token 泄露传染全网 | **不存在 token**；mTLS + 每 node 独立密钥对 |
| 命令伪造 | monitor 私钥签 ed25519，node 验签；TTL + nonce 防重放 |
| monitor 被攻陷发命令 | 私钥泄露确实可发命令；缓解：私钥放 HSM / 拆分 monitor 与 control plane（v4） |
| Web UI XSS | React 默认转义 + CSP + Trusted Types |
| SQL 注入 | 参数化查询；no string concat |
| SSRF（webhook 出站） | URL 白名单 + DNS 解析后 IP 黑名单（私网/loopback） |
| 重放攻击 | nonce + TTL + cert 时间戳 |
| 中间人 | mTLS + cert pinning（可选） |
| 拒绝服务 | 速率限制、连接限流、内存配额 |
| 日志泄露敏感信息 | 字段级 redact 配置；secret 字段永不记日志 |

## 14. 与 v1/v2 对比

| 维度 | 旧方案 | v2 计划 | 知微（本文） |
| --- | --- | --- | --- |
| 节点身份 | 共享 token | 拆 token + 签名 | mTLS + 每节点密钥对 |
| 命令安全 | token + 白名单 | HMAC 签名 | ed25519 签名 + TTL + nonce |
| 节点数量 | ~10 | ~30 | ~1000+ |
| 实时性 | 10s 轮询 | 10s 轮询 | SSE/WebSocket 流 |
| 日志查看 | 仅容器 | 仅容器 | 容器 + journald + 文件 |
| 证书管理 | 仅检测 | 仅检测 | 检测 + 续签 + 推送 + 回滚 |
| 通知渠道 | 1（webhook） | 1 | 6+（webhook/slack/email/...） |
| 二次确认 | 无 | 无 | OOB 审批（敏感操作） |
| 语言 | bash + Python | bash + Python | Go（单二进制，跨平台） |
| 部署 | systemd + Docker | 同 | 同 + K8s DaemonSet |
| 二进制大小 | bash 4KB + py 27MB | 同 | monitor 25MB / node 8MB |

## 15. 实施路线

如果从零开始：

| 阶段 | 内容 | 时间 |
| --- | --- | --- |
| 0 | proto 定义 + 存储 schema | 1w |
| 1 | CA + 注册 + mTLS + 最小 telemetry | 2w |
| 2 | 容器运行时 + 进程 + 日志采集 | 2w |
| 3 | Control plane + 节点管理 + RBAC | 2w |
| 4 | 证书管理（扫描 + 续签 + ACME） | 1w |
| 5 | 告警引擎 + 多通知渠道 | 2w |
| 6 | Web UI（React + 实时） | 3w |
| 7 | MCP + CLI 工具 | 1w |
| 8 | 文档 + Helm Chart + 迁移工具 | 1w |

**总计 ~ 15 周**

## 16. 决策记录

| # | 问题 | **决策** | 说明 |
| --- | --- | --- | --- |
| D1 | 语言栈 | **C. 全 Rust** | monitor / ops / node / cli 全部 Rust（tokio 异步，axum web，sqlx 存储） |
| D2 | 存储 | **A. SQLite 起步** | sqlx + SQLite + WAL；预留接口可换 VictoriaMetrics |
| D3 | 容器运行时 | **A. 仅 Docker** | bollard crate；v4 再扩展 |
| D4 | 前端框架 | **A. React** | 18 + TypeScript + Vite + Tailwind + shadcn/ui |
| D5 | 实时通道 | **A. SSE** + REST | 监控/alerts/日志走 SSE（text/event-stream）；命令下发与回执走 REST POST；mTLS 天然兼容 |
| D6 | 证书 CA | **A. ACME 优先** | 用 instant-acme 实现 Let's Encrypt / Buypass / ZeroSSL；预留 trait 给 Vault |
| D7 | 拓扑 | **B. 双进程** | `monitor`（数据）+ `ops`（控制平面，独立进程，独立 SSH 密钥，独立审计） |
| D8 | MCP 范围 | **B. read + manage** | list/get/approve/silence/renew 开放；reboot/shutdown/kill 不经 MCP |
| D9 | 过渡策略 | **A. 废弃重写** | v1/v2 不再维护；全新 Rust workspace |
| D10 | 多租户 | **A. 不需要** | 单组织模型；后续如要再加 |
| D11 | 节点身份 | **B. Ed25519 请求签名** | 2026-09-19 落地：托管平台边缘终止 TLS、不转发客户端证书，mTLS 不可用；改为「enroll 登记公钥 + 每请求签名 + 时间窗 + nonce」，与原则 #3「签名而非凭据」一致。mTLS 降为自建场景可选加固 |

### 16.1 由此衍生的关键架构决定

1. **两个可执行入口**：`monitor-server`（数据 + UI + MCP）和 `ops-server`（控制平面），通过 localhost 或 UNIX socket 通信
2. **node 三协程架构**：telemetry（push） + control-poll（拉命令） + cert-scan（扫描）
3. **每 node 一对密钥**：Ed25519；私钥 `/etc/monitor/node.key`（0600，root only）
4. **证书 TTL**：node client cert 24h，monitor server cert 90d；自动续签
5. **命令签名链**：ops 私钥签 ed25519，node 用 ops 公钥验签；TTL ≤ 60s；nonce 16 bytes 防重放

### 16.2 仓库结构

```
zhiwei/
├── Cargo.toml                          # workspace
├── proto/                              # protobuf 定义
│   ├── common.proto
│   ├── telemetry.proto
│   ├── control.proto
│   └── cert.proto
├── crates/
│   ├── common/                         # 共享类型、错误、加密、ID
│   ├── proto-gen/                      # 生成的 protobuf + tonic 服务
│   ├── monitor/                        # monitor-server 主进程
│   ├── ops/                            # ops-server 控制平面
│   ├── node/                           # 节点 agent 进程
│   ├── cli/                            # monitor-cli / node-cli
│   └── storage/                        # sqlx repos + migrations
├── ui/                                 # React 前端
│   ├── src/
│   ├── package.json
│   └── vite.config.ts
├── docker/
│   ├── Dockerfile.monitor
│   ├── Dockerfile.ops
│   └── Dockerfile.node
├── deploy/
│   ├── docker-compose.yml
│   ├── k8s/
│   └── systemd/
├── docs/                               # 设计文档
└── README.md
```

---

**总结**：知微把「共享 token」换成「每节点独立身份 + Ed25519 签名」（D11：原设计的 mTLS 在托管平台上不可用，故改为请求签名），把「轮询命令」换成「签名命令 + TTL + nonce」，把「bash 脚本」换成「Rust 单二进制」。D7 决定的双进程架构把数据/控制物理隔离；D5 用 SSE 简化实时通道；D8 把 MCP 限定在非破坏性操作。代价是放弃旧方案兼容、从零构建；收益是安全模型根本性改善、可扩展到 1000+ 节点、支持证书管理这类原本无法做的能力。
