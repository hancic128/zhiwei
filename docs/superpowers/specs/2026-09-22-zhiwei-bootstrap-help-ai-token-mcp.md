# 知微:入网命令 / 帮助页改版 / AI Token / MCP Server

**日期**: 2026-09-22
**状态**: 已批准,待实现
**范围**: 4 个独立但同时落地的子特性

## 背景

zhiwei 当前定位 v0.1.0-alpha,要部署到 Render / Northflank 上做真实用户测试。
当前代码/文档有几处「假设用户下载了仓库代码」的假设,在托管平台上不再成立,
另外 AI 集成(MCP)虽然在 DESIGN.md 第 9 章有规划,但 token / 入口都还没做。

## 子特性 1:不假设用户下载代码(运行时入网命令)

### 问题

- UI 多处引导文案写死 `./scripts/dev.sh start`(common.json:`topNodesEmptyHint`,
  `emptyHint`,help 页 onboarding 章节)
- BootstrapTokens 当前只接受启动时的 `ZHIWEI_BOOTSTRAP_TOKEN` 环境变量,
  以及启动日志里印的一个 10 分钟一次性 token
- 部署到 Render/Northflank 后:用户没代码、日志够不着、env 由平台管理——三种路径都不通

### 方案

**后端**(`crates/monitor-server/src/routes.rs`):

- 复用 `BootstrapTokens`,扩展为支持「运行时 add by admin」(已存在 `add` 方法,
  只是没有 HTTP 入口)
- 新增端点(全部 admin 鉴权):
  - `POST /v1/enroll-tokens` body `{ttl_secs?: u64, label?: string}` →
    创建带 TTL 的临时 token,返回 `{token, expires_at_unix, enroll_command}`
  - `GET /v1/enroll-tokens` → 列出当前所有未过期 token 元信息
    (`{label, created_at_unix, expires_at_unix}`,**不**回显明文)
  - `DELETE /v1/enroll-tokens/:token_id` → 立即撤销
- `enroll_command` 由后端拼好:
  ```
  curl -sSL <monitor-url>/install-node.sh | \
    ZHIWEI_MONITOR_URL=<monitor-url> \
    ZHIWEI_BOOTSTRAP_TOKEN=<token> \
    bash -s
  ```
  - `<monitor-url>` 从请求 `Host` 头 + `X-Forwarded-Proto` 拼出
    (托管平台边缘会注入,自建部署用 `Host` 头回退)
- `BootstrapTokens` 扩展一个 `id` 字段(短串,展示用,不参与校验),
  现在的 token 字符串本身作为删除 key 也行,但 id 更友好

**新增脚本** `scripts/install-node.sh`:

- 从 GitHub release 下载 `zhiwei-node` 最新稳定版
- 检测平台(linux/amd64、linux/arm64、darwin 跳过 systemd 步骤)
- 写 `/etc/zhiwei-node.env`(ZHIWEI_MONITOR_URL + ZHIWEI_BOOTSTRAP_TOKEN)
- 写 systemd unit(`/etc/systemd/system/zhiwei-node.service`)并 enable
- `--uninstall` 子命令清理

**前端** `ui/src/pages/nodes.tsx`:

- 空状态:文案改成「还没有节点。生成入网命令,粘贴到目标机器运行。」
- 「生成入网命令」按钮 → 弹对话框:
  - 默认 TTL 24h,可调 1h/24h/7d
  - label 可选(给命令起个名字,比如「prod-web-01」)
  - 显示生成的命令 + 「复制」按钮 + 过期时间

**前端** `ui/src/pages/settings.tsx`:

- 新增「入网令牌」section,列出当前所有未过期 token,可撤销
- 跟「AI 令牌」分两块(语义不同)

**文案替换** `ui/locales/{zh-CN,en-US}/common.json`:

- `nodes.topNodesEmptyHint` / `nodes.emptyHint`:
  zh-CN: 「还没有节点。在『设置 → 入网令牌』生成命令,粘贴到目标机器运行。」
  en-US: 「No nodes yet. Generate an enroll command in Settings → Enroll Tokens,
         then run it on the target machine.」

## 子特性 2:帮助页改版(卡片 + Markdown)

### 方案

- 新增 `crates/monitor-server/assets/help.md`(中文版;英文版暂只标 TODO)
- 后端 `GET /v1/help`(admin 鉴权):启动时读到内存 `Arc<String>`,
  响应 `{locale: "zh-CN", body: "# ...\n..."}`
- 前端 `ui/src/pages/help.tsx` 重写:
  - 装 `react-markdown` + `remark-gfm`(若未装)
  - 内容区一节一卡(`<MarkdownCard>`),保留右侧目录
  - 卡片样式对齐 `pages/login.tsx`:bg-surface-0 rounded-xl border p-6 shadow-sm
- 删除 i18n 里 `help.*` key,内容从 `docs/HELP.md` 同步(真源)
- `docs/HELP.md` 是真源,build 时脚本同步到 `crates/monitor-server/assets/help.md`

### 假设

- v0.1.0-alpha 阶段不翻译(英文 locale 下也显示中文,顶部提示)
- 内容从 `docs/HELP.md` 同步,不引入新翻译体系

## 子特性 3:AI Token

### 存储

新增 SQLite 表(`crates/storage/src/migrations.rs`):

```sql
CREATE TABLE ai_tokens (
  id           TEXT PRIMARY KEY,         -- "ait_" + 12 hex
  token_hash   TEXT NOT NULL UNIQUE,     -- SHA-256(token),不存明文
  name         TEXT NOT NULL,
  created_at   INTEGER NOT NULL,         -- unix nano
  last_used_at INTEGER,                  -- unix nano
  revoked_at   INTEGER                   -- unix nano,非空即撤销
);
CREATE INDEX idx_ai_tokens_active ON ai_tokens(token_hash) WHERE revoked_at IS NULL;
```

### 端点(全部 admin 鉴权)

| 方法 | 路径 | 作用 |
|------|------|------|
| GET | `/v1/ai-tokens` | 列表(id + name + 时间戳,**不**含明文) |
| POST | `/v1/ai-tokens` `{name}` | 创建,响应 `{id, token}` 明文 **仅这一次** |
| DELETE | `/v1/ai-tokens/:id` | 撤销 |

### 鉴权层

新增 `pub(crate) fn read_auth_ok_v2(state, headers) -> ReadAuthKind`:

```rust
pub(crate) enum ReadAuthKind { Admin, AiToken(String), None }
```

- `read_auth_ok`(保留)改为调用 v2 然后 `matches!(kind, Admin | AiToken(_))`
- **写端点**(handler 里)显式 `if !matches!(kind, Admin) { return 401 }`:
  - `POST /v1/admin/token`
  - `POST /v1/ai-tokens` 等管理 AI token 自身的端点
- **本次范围**:当前所有受保护端点都是读端点,所以 AI token 实际权限
  = 当前 admin token 的全部权限(去掉 admin 改 admin.token 自身的能力)。
  D8 决策的 manage 端点(`enroll 审批`、`silence_alert`)本次不实现,
  等真实 manage 端点落地时,handler 加 `matches!(kind, Admin | AiToken)` 即可

### 前端

- `ui/src/pages/settings.tsx` 新增「AI 令牌」section:
  - 列表(name / created_at / last_used_at)
  - 「新建」按钮 → 输入 name → 创建 → 弹窗显示明文 token + 复制按钮
    + 「只显示一次」提示
- Node 空状态不出现 AI token(避免跟 enroll token 混)

### 假设

- AI token 不过期(只有撤销);要做过期再加一列
- 明文 token 仅创建响应返回一次,前端展示后用户需自己保存
- token 熵 ≥ 32 字节,base64url 编码,前缀 `ait_`

## 子特性 4:MCP Server

### 设计决策(本次)

- **暴露方式**:**只做 SSE 模式**,绑 monitor-server 现有端口
  (`https://<host>/mcp/sse`),无需新增二进制/端口
- **不实现 stdio 模式**:DESIGN.md 9.2 提到 stdio + unix socket,但 zhiwei 当前
  ops-server 还没准备好做 unix socket。本次仅 SSE,stdio 留 TODO
- **鉴权**:Bearer AI token(`Authorization: Bearer <ai-token>`),
  复用 `read_auth_ok_v2` 的 `ReadAuthKind::AiToken` 分支
- **CORS**:SSE 端点对跨域开放(`Access-Control-Allow-Origin: *`,
  `Access-Control-Allow-Headers: Authorization`)—— MCP 客户端通常跨域

### 实现路径

- 新增 crate `crates/mcp-server/`(纯库,被 monitor-server 引用):
  - `lib.rs`:MCP 协议实现(消息类型 / JSON-RPC 解析)
  - `sse.rs`:axum handler(`/mcp/sse` GET + `/mcp/message` POST),
    SSE 事件流 + 客户端消息接收
  - `tools.rs`:工具注册表,工具实现调 monitor HTTP API
- monitor-server:
  - `main.rs` 加 `Router::route("/mcp/sse", get(mcp_sse_handler))` 等
  - 启动时构造 `AppState` 的 mcp 部分(MCP tool 列表 + HTTP client)
- HTTP client:reqwest(workspace 已有依赖),调 monitor 自己的 REST API
  (`https://127.0.0.1:<port>/v1/...`),传 `Authorization: Bearer <ai-token>`

### MCP 工具集合(本次)

**只映射当前 monitor API 已有端点**(D8 决策里允许的 manage 端点,
zhiwei 还没实现,留 TODO 等真实端点落地):

| 工具 | 类型 | 调用的 monitor API | 备注 |
|------|------|---------------------|------|
| `list_nodes` | Read | `GET /v1/nodes` | |
| `get_node` | Read | `GET /v1/nodes/:id` | 详情(host_info + latest) |
| `get_telemetry` | Read | `GET /v1/nodes/:id/telemetry?limit=N` | |
| `list_alerts` | Read | `GET /v1/alerts` | 当前/历史 |
| `list_certs` | Read | `GET /v1/certs` | |
| `list_containers` | Read | `GET /v1/nodes/:id/containers` | 当前已有 inventory 端点?需确认 |
| `list_processes` | Read | `GET /v1/nodes/:id/processes` | 同上 |

**不实现**(`reboot/shutdown/kill/container_action/renew_cert/silence_alert`
等 D8 不允许或当前后端无对应端点)

### 协议实现范围

- MCP 协议核心子集(本次够用即可):
  - `initialize` / `initialized` 握手
  - `tools/list` / `tools/call`
  - `ping`
- 不实现:`resources/*`、`prompts/*`、`completion`、`sampling`
  (本次范围之外,YAGNI)

### 错误处理

- 工具调 monitor API 失败 → MCP `isError: true` + 错误 message
- 鉴权失败 → SSE 立即关闭 + HTTP 401(只在 POST message 时返回)
- 协议解析错误 → JSON-RPC `-32700 Parse error`

## 关键约束

- **不破坏现有 admin token 流程**:`read_auth_ok` 保留向后兼容
- **不引入新数据库迁移方式**:沿用 `crates/storage/src/migrations.rs`
  现有的 `sqlx::migrate!` 机制
- **前端不引入新的状态管理库**:沿用 `@tanstack/react-query` + 现有 i18n
- **不在本 PR 引入 mTLS / stdio MCP**:按 DESIGN.md 留 TODO
- **测试**:每个新端点加单测(monitor-server 现有模式:
  `mod xxx_tests` 在 routes.rs 底部);MCP 协议加一个 round-trip 单测

## 不做(明确)

- ops-server 改造(继续走 HTTP API)
- MCP stdio 模式
- MCP resources / prompts / sampling
- 翻译帮助页(英文)
- 入网令牌的租期/限额/作用域
- AI token 的过期(只有撤销)
- 节点批量入网 UI
- 新二进制(在 monitor-server 内加 SSE route)

---

## 实现说明（与上述设计的偏差）

以下偏离在设计评审后、实现过程中确定，理由记录在此：

1. **帮助页真源**：设计稿写「`docs/HELP.md` 是真源，build 时同步到 assets」。
   实际改为 **`crates/monitor-server/assets/help.md` 是唯一真源**——多一份
   `docs/HELP.md` 只会漂移，而 build 期同步脚本在 zhiwei 当前（无 build.rs）
   的形态下是额外的活动部件。内容直接 `include_str!` 进二进制。

2. **资源内嵌而非运行时读文件**：设计稿按「启动时读 `<ui_dir>/../assets/`」写。
   实际改为 `include_str!`。原因：Dockerfile 的运行镜像只拷
   `zhiwei-monitor` + `ui/dist`，不带 `crates/`，运行时按路径找必然是空的。
   内嵌同时让「单二进制、无外部资源」这个定位成立。

3. **入网脚本资产名**：设计稿假设新增 `zhiwei-node-<ver>-<triple>.tar.gz`。
   实际改为复用现有 release.yml 产出的 `zhiwei-<triple>.tar.gz`
   （内含 `zhiwei-node` + `zhiwei-monitor` + `VERSION`）。理由是零流水线改动、
   对已发布的 tag 也生效；代价是下载包略大（多一个 monitor 二进制）。

4. **`GET /install-node.sh` 不鉴权**：设计稿未提。目标机器执行
   `curl | bash` 时还没有任何凭据，真正的秘密是 enroll 命令里带过去的
   bootstrap token。脚本本身不含秘密，公开它等于公开安装方式（Tailscale 等同做法）。

5. **MCP `get_node` 实现**：后端没有 `GET /v1/nodes/:id` 端点。改为调
   `GET /v1/nodes` 后按 id 客户端过滤；找不到时返回 `isError: true`。

6. **MCP `list_alerts` 无 limit**：后端 `alerts_handler` 不接受 `limit`
   （固定「全部活跃 + 最近 50 条已解决」）。工具 schema 里去掉该参数，
   避免给出「能翻页」的错觉。

7. **MCP 传输**：只实现 `POST /mcp/sse`（同步返回 SSE 事件）。
   `GET /mcp/sse`（服务端主动推送）留 TODO——本次所有工具都是短操作。

8. **迁移编号**：AI token 表落在 **012**（设计稿未指定编号）。
