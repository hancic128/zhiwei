# 服务健康度（服务探活）+ 概览页改版 · 设计

日期：2026-09-19
状态：已与本人对齐，按本设计实现第一版
上位文档：`docs/SERVICE-HEALTH.md`（完整蓝图，本版只取其中 S-1 一段）

## 1. 范围

**做**

- 服务探活：服务（service）1:N 探针（probe），节点侧执行，状态机 ok / degraded / down，状态翻转进告警引擎并走既有 webhook 通知。
- 探针类型三种：`http`、`tcp`、`tls`。
- 控制台新增「服务」页：服务列表与状态、探针 CRUD、探针最近结果。
- 概览页改版：5 张大字卡片（可点击跳转）、最近告警列表、Top N 节点（CPU / 内存）。

**不做（本版明确排除）**

- gRPC / DNS / ICMP / 自定义命令 / 多步组合探针。
- monitor 侧执行与多地区外部探针（数据模型预留 `location` 字段）。
- SLA / 错误预算 / 公共状态页 / 事故管理与复盘。
- RBAC（角色仍未实现，沿用 admin token）。

## 2. 关键决策

| 决策 | 选择 | 理由 |
| --- | --- | --- |
| 探针执行位置 | 节点侧（`location = node`） | 能探 localhost、容器端口、内网；三台主机的服务都覆盖得到。monitor 部署到 PaaS 后探不到内网 |
| 配置下发方式 | 节点每 30s 拉一次 `/v1/probe-config`（Ed25519 请求签名） | 复用既有签名鉴权与传输层，无需新的推送通道 |
| 结果上报方式 | 节点 `POST /v1/probe-results`（签名 + JSON 批量） | 与 telemetry / inventory 同构；JSON 便于定义演进 |
| 服务与探针关系 | 两层：服务 1:N 探针 | 以后做状态页 / 事故 / SLA 不用改模型 |
| 服务状态 | 该服务下探针状态的最差值（worst_of） | 与 SERVICE-HEALTH.md 的 aggregate 语义一致 |
| 状态机 | `ok → degraded → down`，连续失败达 `failure_threshold`（默认 3）才 down；一次成功即回 ok | 避免单次抖动误报 |
| 告警联动 | 状态翻转写既有 `alerts` 表（`metric = probe.state`），通知复用 webhook 渠道 | 不引入第二套通知栈 |

## 3. 数据模型（迁移 008）

```sql
CREATE TABLE services (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    group_name TEXT NOT NULL DEFAULT '',
    tier INTEGER NOT NULL DEFAULT 2,          -- 1/2/3 重要度
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at_unix_nano INTEGER NOT NULL,
    updated_at_unix_nano INTEGER NOT NULL
);

CREATE TABLE probes (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,                       -- http | tcp | tls
    target_json TEXT NOT NULL,                -- 类型相关目标（URL / host:port / 域名）
    expect_json TEXT NOT NULL DEFAULT '{}',   -- 类型相关期望（状态码 / body 包含 / 阈值）
    interval_seconds INTEGER NOT NULL DEFAULT 60,
    timeout_ms INTEGER NOT NULL DEFAULT 5000,
    failure_threshold INTEGER NOT NULL DEFAULT 3,
    node_id TEXT,                             -- 归属节点；NULL = 任意节点（本版未用）
    location TEXT NOT NULL DEFAULT 'node',    -- node | monitor（monitor 预留）
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at_unix_nano INTEGER NOT NULL,
    updated_at_unix_nano INTEGER NOT NULL
);
CREATE INDEX idx_probes_node ON probes(node_id, enabled);

CREATE TABLE probe_state (
    probe_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,                      -- ok | degraded | down
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_change_at_unix_nano INTEGER NOT NULL,
    last_check_at_unix_nano INTEGER NOT NULL,
    last_latency_ms REAL,
    last_error TEXT NOT NULL DEFAULT '',
    updated_at_unix_nano INTEGER NOT NULL
);

CREATE TABLE probe_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    probe_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    ts_unix_nano INTEGER NOT NULL,
    state TEXT NOT NULL,
    latency_ms REAL,
    status_code INTEGER,
    error TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_probe_results_probe_ts ON probe_results(probe_id, ts_unix_nano);
```

明细保留 7 天：monitor 启动时跑一次清理，之后每 6 小时清理一次（只清 `probe_results`，telemetry 仍维持现状）。

### 3.1 探针配置形态

`target_json` / `expect_json` 按 `kind` 解释：

| kind | target | expect |
| --- | --- | --- |
| `http` | `{"url": "...", "method": "GET", "headers": {...}, "body": "..."}` | `{"status": [200,204], "body_contains": ["ok"], "max_latency_ms": 500, "tls_verify": true}` |
| `tcp` | `{"host": "...", "port": 5432}` | `{"max_latency_ms": 200, "banner_contains": "PostgreSQL"}` |
| `tls` | `{"host": "...", "port": 443, "sni": "..."}` | `{"min_days_valid": 30, "max_latency_ms": 1000, "verify": true}` |

## 4. 接口

节点侧（Ed25519 请求签名）：

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/v1/probe-config?node_id=<id>` | 返回该节点启用的探针配置（含服务名），`{"probes": [...]}` |
| POST | `/v1/probe-results` | 批量上报结果，`{"results": [{probe_id, ts_unix_nano, state, latency_ms, status_code, error}]}`，返回 204 |

控制台（`Authorization: Bearer <admin token>`）：

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET/POST | `/v1/services` | 服务列表（含健康汇总）/ 新建 |
| PATCH/DELETE | `/v1/services/:id` | 改名 / 描述 / 启停 / 删除（连带探针与状态） |
| GET/POST | `/v1/probes` | 探针列表（含最近一次结果）/ 新建 |
| PATCH/DELETE | `/v1/probes/:id` | 修改 / 删除 |
| GET | `/v1/probes/:id/results?limit=N` | 最近 N 条结果（默认 50） |
| GET | `/v1/overview` | 概览聚合：5 项统计 + 最近告警 + Top 节点（含 60 点趋势） |

## 5. 状态机与告警

节点每次上报一条结果（`state` 由节点按期望判定为 `ok` / `degraded` / `down`），monitor 侧维护 `probe_state`：

- 结果 `ok` → `state = ok`，`consecutive_failures = 0`；若上一状态非 ok 则写 resolved 告警。
- 结果非 ok → `consecutive_failures += 1`；达到 `failure_threshold` 则 `state = down`，否则 `degraded`。
- 状态从 ok 变为非 ok，或从非 ok 变为 down / ok 时，写一条状态变更告警（`opening` 用 `probe.state`，severity：down = critical，degraded = warning）。

告警内容形如 `服务 kimi 的探针 kimi-auth /health 异常：HTTP 503`。

## 6. 概览页

| 区域 | 内容 | 跳转 |
| --- | --- | --- |
| 卡片 1 | 节点：在线 / 总数 | `/nodes` |
| 卡片 2 | 服务：健康 / 总数 | `/services` |
| 卡片 3 | 证书：最近到期 X 天 | `/certificates?filter=expiring` |
| 卡片 4 | 容器：异常 N | `/containers?state=failed` |
| 卡片 5 | 告警：未解决 N | `/alerts` |
| 最近告警 | 最近 5 条未解决告警（严重度 / 规则 / 节点 / 值 / 开始时间 / 静默） | `/alerts` |
| Top N 节点 | CPU / 内存 可切换，每行带趋势 Sparkline | `/nodes/:id` |

容器的「异常」定义：`state ∈ {dead, restarting}`，或 `state = exited` 且 status 不以 `Exited (0)` 开头，或 `state = running` 且 status 含 `unhealthy`。

数据来源为单个 `/v1/overview` 请求；原「集群 CPU 曲线」卡片移除（Top N 节点已带趋势）。

## 7. 测试与验收

- `cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test` 全绿。
- 存储层单测：状态机推进（ok → degraded → down → ok）、服务健康汇总（worst_of）、容器异常判定。
- 端到端（真实进程）：起 monitor + ops + node，新建一个指向本机 monitor 的健康探针，观察节点拉配置 → 执行 → 上报 → 服务页与告警页出数据；再新建一个必失败探针验证 down 与告警，最后恢复验证 resolved。
- 界面：浏览器逐页确认（服务页 CRUD、概览 5 卡跳转、最近告警、Top N 切换）。
