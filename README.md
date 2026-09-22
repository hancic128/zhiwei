# 知微 / ZhiWei

> 见微知著，守正待时。
> Know the subtle, await the right moment.

**给独立开发者与一人公司：一个人加一个 Agent，就能管住 N 台机器。**

*For indie developers and one-person companies: one human plus one agent, managing N machines.*

单二进制 Rust 实现的多主机可观测平台——资源监控 · 服务健康 · 证书管理 · 远程操作 · 告警 · MCP。
自托管，SQLite，不需要 Prometheus / Grafana / K8s / Postgres。

定位、第一用户与边界见 [docs/POSITIONING.md](./docs/POSITIONING.md)；想贡献代码先读
[CONTRIBUTING.md](./CONTRIBUTING.md)。

## 状态

**v0.1.0-alpha · P2-1 范围**（节点身份 + 入网 + mTLS + 最简 telemetry 上报）

设计稿见 [`docs/`](./docs)：
- [POSITIONING.md](./docs/POSITIONING.md) — **定位：为谁做、凭什么、不做什么**（最上游文档）
- [DESIGN.md](./docs/DESIGN.md) — 整体架构
- [NAMING.md](./docs/NAMING.md) — 命名理据与视觉基线
- [CHECKLIST.md](./docs/CHECKLIST.md) — 开源准备清单
- [SERVICE-HEALTH.md](./docs/SERVICE-HEALTH.md) — 服务健康度扩展设计
- [DEPLOY.md](./docs/DEPLOY.md) — **部署指南（含托管平台与 mTLS 的冲突分析）**

## 架构

```
┌──────────────────────────────────────────────────────────────────────────┐
│  monitor-server  (data plane)                                            │
│  ────────────────────────────────                                        │
│  • 自建部署：本进程终结 TLS；托管平台：TLS 由前置边缘终结                │
│  • 节点身份 = Ed25519 请求签名（不再依赖客户端证书）                     │
│  • SQLx + SQLite + WAL · Local CA 首启自生成                             │
│  • Routes:                                                               │
│      GET  /                 (总览 JSON)                                  │
│      POST /v1/enroll        (bootstrap token + 节点公钥 → node_id)       │
│      POST /v1/telemetry     (请求签名 + protobuf TelemetryBatch)         │
│      POST /v1/inventory     (请求签名 + 容器/进程/证书快照)              │
│      GET  /v1/nodes         (Bearer admin token, JSON)                   │
│      GET  /v1/nodes/:id/telemetry?limit=N   (Bearer, 已解码)             │
│      GET  /healthz                                                       │
└──────────────────────────────────────────────────────────────────────────┘
                                  ▲ HTTPS（自建）或前置边缘 TLS（托管平台）
                                  │  每次请求带 Ed25519 签名
┌──────────────────────────────────────────────────────────────────────────┐
│  node-agent  (every host)                                                │
│  ──────────────────────────                                              │
│  • Ed25519 签名密钥（signing.key, 0600）→ 每次请求签名                   │
│  • enroll 时存下 ops 公钥（TOFU），用于验证命令签名                      │
│  • sysinfo 采集 CPU / 内存 / 磁盘 / 网络 / 进程                          │
│  • 30s 上报一次（可配）；inventory 每 5 分钟                             │
└──────────────────────────────────────────────────────────────────────────┘
```

## 仓库结构

```
.
├── Cargo.toml              # workspace
├── proto/                  # .proto 定义（common / telemetry）
├── crates/
│   ├── common/             # NodeId / Timestamp / Error / KeyPair
│   ├── proto/              # prost-build 生成的 Rust 类型
│   ├── storage/            # SQLx + SQLite + repos
│   ├── monitor-server/     # data plane binary
│   └── node-agent/         # node binary
├── docs/                   # 设计稿
└── config/                 # 示例配置
```

## 控制台（Web UI）

monitor 在 `ui/dist` 存在时把它挂在 `/`：静态文件按扩展名给 MIME，其余路径
回落 `index.html`（**200**，支持前端深链），未知 `/v1/*` 仍返回 JSON 404。

## 服务健康度（服务探活）

服务（service）1:N 探针（probe），探针在**节点侧**执行（能探 localhost、容器端口、
内网），monitor 负责下发配置、收敛状态、触发告警。第一版支持 `http` / `tcp` / `tls`
三类探针；调度与阈值（间隔 / 超时 / 连续失败几次判 down）都是每条探针的字段。

```
节点 GET  /v1/probe-config?node_id=<id>   拉取本机探针（Ed25519 请求签名，只能拉自己）
节点 POST /v1/probe-results               批量上报结果（签名 + JSON）
控制台 /v1/services  /v1/probes           服务与探针 CRUD（admin token）
控制台 /v1/probes/:id/results             最近结果明细
控制台 /v1/todo                           待办聚合（现在要处理 / 留意 / 已恢复 + 状态摘要）
```

状态机：`ok → degraded → down`，连续失败达 `failure_threshold`（默认 3）才判 down，
一次成功即回 ok。进入 down 写一条 `source = probe` 的告警并走既有 webhook 通知，
恢复时自动关闭。结果明细滚动保留 7 天（启动清一次，之后每 6 小时一次）。

**前端实现严格遵循 Trilium「前端约束规范」**（12 章：总则/令牌/主题/响应式/国际化/组件/布局/交互/代码/禁止清单）：

| 规范要求 | 实现 |
| --- | --- |
| 令牌唯一来源 | 只用 `brand-*` / `surface-0..4` / `ink-900..400` + 3 语义色；无硬编码色值 |
| 间距 / 字号 / 圆角 / 阴影 | 仅用规范允许的档位（4px 网格；5 级字号；4 档圆角；3 级阴影） |
| 图标纯 SVG | 全部 Lucide React，`w-4/5/6/12`，禁止 emoji |
| 双轴主题 | 5 主题色（indigo/emerald/rose/amber/slate）× 明暗，独立切换并持久化 |
| 首帧恢复 | `index.html` 内联脚本在 React 挂载前应用主题，避免闪烁 |
| 侧边栏折叠 | `w-60 ⇄ w-16`，折叠按钮固定贴底不随菜单滚动，状态存 `tj_sidebar` |
| 右下角悬浮按钮组 | 收敛为一个 Settings 主按钮，hover/focus 展开；主按钮展开时图标旋转 45°；Esc 或移出 180ms 收起；主题色气泡向左展开 |
| 危险操作确认 | 退出登录走 ConfirmDialog，rose 语义色区分 |
| 中英双语 | i18next + `locales/{lang}/common.json`，全部文案走 `t()` |
| 时区 | dayjs + 6 个 IANA 时区，统一 `YYYY-MM-DD HH:mm` 24 小时制 |
| 交互与动效 | 颜色 150ms / 位移缩放 200ms，无超 300ms 动画 |
| 失败友好化 | 禁止裸显状态码，友好文案 + 就地「重试」，失败态常驻 |
| 空状态 | Lucide `w-12 h-12 text-ink-400` + 标题 + 描述，`py-16` |

开发：

```sh
cd ui && npm install && npm run build   # 产出 ui/dist
cd .. && ./scripts/dev.sh start         # monitor 自动托管 ui/dist
```

默认明文 HTTP（TLS 交给前置边缘，与托管平台一致），打开 <http://127.0.0.1:8443/>；
想试内置 TLS 就用 `ZHIWEI_DEV_TLS=1 ./scripts/dev.sh start`（自签证书需在浏览器确认一次）。
控制台填 admin token：

```sh
cat data/admin.token
```

> 控制台用 Bearer admin token；节点用 Ed25519 请求签名，两套凭据互不影响。

已实现页面：**概览**、**节点**、**节点详情**、**容器**、**日志**、**证书**、**告警**、**设置**。

## 快速开始

### 本地试验环境（推荐）

一条命令起 monitor + 自动入网一个本地 node：

```sh
./scripts/dev.sh start     # 构建并启动，自动 enroll
./scripts/dev.sh query     # 列出已入网节点
./scripts/dev.sh watch     # 轮询某节点最新 telemetry
./scripts/dev.sh status    # 进程与日志
./scripts/dev.sh stop      # 停掉
./scripts/dev.sh clean     # 停掉并删数据（含 CA）
```

只读接口要 `Authorization: Bearer <admin token>`（脚本已带上）；节点那一侧走请求签名，
与脚本无关。

### 手工跑

前置：Rust 1.75+（macOS / Linux）

#### 启动 monitor-server

```sh
cargo run --bin zhiwei-monitor
```

首次启动会：
1. 在 `data/` 下生成自签名 root CA（10 年有效期）
2. 颁发 monitor server cert（90 天有效期，CA 签名）
3. 创建 SQLite 数据库 `data/monitor.db`
4. 生成一次性 bootstrap token 并打印到日志（10 分钟有效）

把 token 复制下来给 node 用。设了 `ZHIWEI_BOOTSTRAP_TOKEN` 环境变量的话，
就用你设的那个、长期有效，不再随机生成（托管平台没 Shell 时用这个）。

### 启动 node-agent

装了预编译二进制的话（见下方「[部署](#部署)」的安装脚本），直接跑：

```sh
ZHIWEI_MONITOR_URL=https://monitor.example.com \
ZHIWEI_BOOTSTRAP_TOKEN=<入网令牌> \
zhiwei-node --state-dir /var/lib/zhiwei-node
```

从源码跑：

```sh
ZHIWEI_MONITOR_URL=https://127.0.0.1:8443 \
ZHIWEI_BOOTSTRAP_TOKEN=<上一步的 token> \
cargo run --bin zhiwei-node
```

首次启动会：
1. 生成 Ed25519 签名密钥（持久化到 `data/node/signing.key`，0600）
2. POST `/v1/enroll`：提交公钥 + bootstrap token，换取 node_id（同时存下 ops 公钥与 monitor CA）
3. 持久化 `data/node/{node.id,ca.crt.pem,ops.pub}`
4. 进入稳态：30 秒一轮，采集 → 用自己私钥签名 → POST `/v1/telemetry`；
   另有一条控制通道轮询命令（验 ops 签名后执行，回执再签名回传）

后续启动跳过 enroll（`node.id` 已在），直接签名上报。

### 数据落库位置

- `data/monitor.db` — SQLite，含 `nodes` 与 `telemetry_batches` 两张表
- `data/ca/ca.{crt,key}.pem` — monitor CA
- `data/monitor.{crt,key}.pem` — monitor server cert
- `data/node/...` — node-agent 状态

## 部署

### 装节点 / 服务端二进制（预编译）

打 tag（`v0.1.0` 这种）后 release 工作流会产出 linux（x86_64 / aarch64 × musl / gnu）
与 macOS（arm64 / x86_64）的预编译包。安装脚本自动识别系统架构、校验 SHA256：

```sh
curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install.sh | sh
```

装 node-agent 是默认行为；装 monitor 加 `-s -- --bin monitor`。

**国内服务器**（连不上 GitHub Releases / 托管平台）加一个 `ZHIWEI_BASE_URL` 即可走
自建源，不需要改代码：

```sh
curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install.sh \
  | ZHIWEI_BASE_URL=https://artifacts.hancic.site/releases/hancic128/zhiwei sh
```

节点入网同理（`install-node.sh` 也认这个变量）。完整方案见
[docs/DEPLOY.md 第 9 节「国内 / 隔离网络部署」](./docs/DEPLOY.md#9-国内--隔离网络部署)。

> 仓库目前是私有的——开源之前这条匿名 `curl` 走不通，需要
> `export GITHUB_TOKEN=<PAT>`，或把仓库设为 public。详见
> [docs/DEPLOY.md](./docs/DEPLOY.md#装二进制一行命令)。

### Docker

```sh
docker build -t zhiwei-monitor .
docker run -d -p 8443:8443 -v zhiwei-data:/var/lib/zhiwei zhiwei-monitor
```

国内网络构建慢可加 `--build-arg CARGO_MIRROR=https://rsproxy.cn/index/`。

### 托管平台（Render / Railway / Northflank）

托管平台在边缘终止 TLS、不向容器转发客户端证书，因此 monitor 需要以明文 HTTP 监听
（`--plain-http`）——节点身份由请求签名承担，不依赖传输层，鉴权强度不受影响。
部署步骤与取舍见 **[docs/DEPLOY.md](./docs/DEPLOY.md)**。

平台只注入 `PORT`，monitor 已支持（无显式配置时绑定 `0.0.0.0:$PORT`）。

### 持久化（必须）

`ZHIWEI_DATA_DIR` 里存着 **CA 私钥**与 SQLite。不挂持久卷的话，每次重新部署都会
生成新 CA，**所有节点全部掉线**。

## 安全模型

- **签名而非凭据**：节点入网时登记 Ed25519 公钥，此后每个请求都由节点私钥签名，
  monitor 验签 + 时间窗（±300s）+ nonce 防重放（见 `crates/common/src/auth.rs`）
- /v1/enroll 用一次性 bootstrap token（10 分钟 TTL），入网后丢弃
- 命令反向签名：ops 私钥签命令、节点验签；节点私钥签回执、monitor 验签——
  monitor 被攻陷也伪造不出命令，传输层被攻陷也伪造不出回执
- 托管平台上 TLS 由边缘终结，节点仍全程签名：**传输层不再承担身份**
- 控制台只读接口用 Bearer admin token（浏览器无法持有节点密钥）

## 当前限制

- 证书管理只做「扫描 + 到期展示」，续签 / ACME 未实装（P2-4）
- 告警只做阈值规则 + webhook 通知，暂无多渠道路由（P2-5）
- 前端首屏 gzip 约 320 KB，超 DESIGN.md 定的 200 KB（换 uPlot 可砍约 130 KB）
- 老节点（mTLS 时代入网、库里没有公钥）需要重新 enroll

**按定位决定不做的**（不是「还没做」，理由见 [docs/POSITIONING.md](./docs/POSITIONING.md)
与 [CONTRIBUTING.md](./CONTRIBUTING.md)）：用户 / 角色 / RBAC、多租户、SSO / LDAP、
HA / 集群 / 分布式存储、图表拖拽与自定义大屏、K8s 深度集成、长周期海量指标。

## License

Apache-2.0
