# 知微 入网命令/帮助页/AI Token/MCP Server 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development
> **Spec:** `docs/superpowers/specs/2026-09-22-zhiwei-bootstrap-help-ai-token-mcp.md`

**Goal:** 让 zhiwei 在 Render/Northflank 上能「独立部署」——admin 在 UI 里给目标机器生成入网命令,提供基于 Markdown 的帮助页,并通过 AI token + MCP SSE 让外部 AI 客户端读集群数据。

**Architecture:**
- 后端扩展 monitor-server(单一 Rust 进程),不加新二进制
- 鉴权层加 v2:ReadAuthKind {Admin, AiToken, None},沿用现有 read_auth_ok
- MCP SSE 端点复用 monitor 的 HTTP 客户端调自己的 REST API
- 前端不引入新依赖,沿用 @tanstack/react-query + 现有 i18n

**Tech Stack:** Rust(axum + sqlx + tokio)、React 18 + Vite + react-markdown + remark-gfm

---

## File Structure

| 文件 | 角色 |
|------|------|
| **后端(新增)** | |
| `crates/mcp-server/Cargo.toml` | 新 crate 清单 |
| `crates/mcp-server/src/lib.rs` | MCP 协议(消息类型 / JSON-RPC) |
| `crates/mcp-server/src/sse.rs` | SSE 路由 handler |
| `crates/mcp-server/src/tools.rs` | 工具实现(调 monitor HTTP API) |
| `crates/monitor-server/assets/help.md` | 帮助页 markdown 内容 |
| `crates/monitor-server/assets/install-node.sh` | 同步自 scripts/install-node.sh |
| `crates/storage/src/migrations/0002_ai_tokens.sql` | 新迁移 |
| `crates/storage/src/ai_tokens_repo.rs` | AI token repo |
| `scripts/install-node.sh` | 入网安装脚本(真源) |
| **后端(修改)** | |
| `crates/monitor-server/src/routes.rs` | 新增 enroll-tokens / ai-tokens / help / mcp-sse 路由 + ReadAuthKind v2 |
| `crates/monitor-server/src/main.rs` | 装载 MCP route + 加载 help.md |
| `crates/monitor-server/src/state.rs` | 加 AiTokens 字段 |
| `crates/storage/src/lib.rs` | 暴露 ai_tokens() |
| `crates/storage/src/migrations.rs` | 注册新迁移 |
| `crates/storage/Cargo.toml` | 暴露新模块 |
| `crates/common/src/auth.rs` | ct_eq 已经够用,不改 |
| `Cargo.toml` | 加 mcp-server 成员 |
| **前端(修改)** | |
| `ui/src/pages/nodes.tsx` | 空状态文案 + 生成入网命令对话框 |
| `ui/src/pages/settings.tsx` | 加「入网令牌」+「AI 令牌」两 section |
| `ui/src/pages/help.tsx` | 重写为 react-markdown 渲染 |
| `ui/src/api.ts` | 新增 enrollTokens / aiTokens / help 客户端 |
| `ui/src/components/markdown-card.tsx` | 新组件(卡片 + markdown) |
| `ui/src/components/enroll-token-dialog.tsx` | 新组件 |
| `ui/src/components/ai-token-dialog.tsx` | 新组件 |
| `ui/locales/{zh-CN,en-US}/common.json` | 文案替换 |
| `ui/package.json` | 加 react-markdown + remark-gfm |
| **文档** | |
| `docs/HELP.md` | 帮助页真源 |
| `docs/DEPLOY.md` | 入网命令用法 |
| `docs/api.md` | 新增 MCP 工具参考 |

---

## Task 拆分(可并行)

### Task 1:后端鉴权层 v2(ReadAuthKind)— 阻塞:无,先做

- `crates/monitor-server/src/routes.rs` 加 `pub(crate) enum ReadAuthKind { Admin, AiToken(String), None }`
- `read_auth_ok` 保留(改内部),所有 `read_auth_ok(...)` 调用点暂时不需要改语义(AI token 还没接)
- 加 `pub(crate) fn read_auth_ok_v2(...)` 返回枚举
- 加单测覆盖三种分支

**验收**: `cargo test -p zhiwei-monitor` 通过,所有现有测试不变

### Task 2:AI Token 表 + repo + 端点 — 依赖:Task 1

- 新迁移 `0002_ai_tokens.sql`
- `crates/storage/src/ai_tokens_repo.rs`(insert / list / get_by_hash / get_by_id / revoke / touch_last_used)
- `crates/storage/src/lib.rs` 加 `ai_tokens()` getter
- `crates/storage/src/migrations.rs` 注册
- `routes.rs` 加 `GET/POST/DELETE /v1/ai-tokens` 端点(写端点拒绝 AiToken)
- handler 内:AI token hash 用 SHA-256,token 形如 `ait_<base64url 32 字节>`
- 单测覆盖 CRUD + 撤销后立即失效

**验收**: curl 走完整 create → list → use(Bearer) → revoke → use 流程

### Task 3:运行时入网命令端点 — 依赖:Task 1(但与 Task 2 可并行)

- `BootstrapTokens` 扩展:`Entry { id, label, created_at_unix, expires_at_unix }`(`HashMap<token, Entry>`)
- 改 `add()` 接受 `label: Option<String>`,内部 mint id(短 hex)
- `routes.rs` 加 `GET/POST/DELETE /v1/enroll-tokens`(admin 鉴权)
- `enroll_command` 拼接:`curl -sSL <host>/install-node.sh | ZHIWEI_MONITOR_URL=<host> ZHIWEI_BOOTSTRAP_TOKEN=<token> bash -s`
  - `<host>` 优先 `X-Forwarded-Proto + Host`,回退 `request.scheme() + uri.authority()`(axum `ConnectInfo`)
- 单测覆盖 create/expire/list/revoke

**验收**: curl 创建 enroll-token,复制 install 命令,目标机器 enroll 成功

### Task 4:`scripts/install-node.sh` — 依赖:Task 3

- 从 GitHub release `https://github.com/<owner>/zhiwei/releases/latest/download/zhiwei-node-<ver>-<arch>.tar.gz` 下载
- 检测:`uname -s`(只支持 linux)+ `uname -m`(x86_64 / aarch64)
- 写 `/usr/local/bin/zhiwei-node`
- 写 `/etc/zhiwei-node.env`(mode 0600)
- linux:写 systemd unit + `systemctl enable --now`
- macOS:写到 `~/Library/LaunchAgents`,launchctl load
- `--uninstall` 反向操作
- 手测:在临时容器/Docker 内跑一遍

### Task 5:帮助页 markdown(后端)— 阻塞:无

- `crates/monitor-server/assets/help.md`(初始内容从 docs/HELP.md 复制,中文)
- `crates/monitor-server/assets/install-node.sh`(软链 / 复制自 scripts/install-node.sh)
- `main.rs`:启动时 `tokio::fs::read_to_string(...).await?` 到 `Arc<String>`,失败降级为空字符串
- `routes.rs`:`GET /v1/help` 返回 `{ locale: "zh-CN", body: "..." }`
- 单测:加载后内存里有内容

### Task 6:前端依赖 + 文案替换 — 阻塞:无,可最先做

- `ui/package.json` 加 `react-markdown` + `remark-gfm`(注意版本兼容 react 18)
- `ui/src/locales/{zh-CN,en-US}/common.json`:
  - `nodes.topNodesEmptyHint` 改成「还没有节点。在『设置 → 入网令牌』生成命令,粘贴到目标机器运行。」
  - `nodes.emptyHint` 同上
- 跑 `npm run check:locales` 通过

### Task 7:前端 UI(nodes / settings)— 依赖:Task 2、Task 3、Task 6

- `ui/src/api.ts` 新增:
  - `enrollTokens.list()` / `enrollTokens.create(ttl, label)` / `enrollTokens.revoke(id)`
  - `aiTokens.list()` / `aiTokens.create(name)` / `aiTokens.revoke(id)`
- `ui/src/components/enroll-token-dialog.tsx`:TTL select + label input + 显示生成命令 + 复制按钮
- `ui/src/components/ai-token-dialog.tsx`:name input + 显示明文 token(一次性)+ 复制按钮
- `pages/nodes.tsx`:空状态用对话框
- `pages/settings.tsx`:新增两 section(列表 + 「新建」按钮)
- 文案 i18n

### Task 8:前端 UI(help)— 依赖:Task 5、Task 6

- `ui/src/components/markdown-card.tsx`:卡片样式 + react-markdown 渲染(装 remark-gfm)
- `ui/src/pages/help.tsx` 重写:
  - 后端 `GET /v1/help` 拉内容(用 react-query)
  - 内容区按 `##` 切分,每节一卡(右侧目录保留)
  - 英文 locale 提示「翻译暂未提供」
- 删 i18n 里的 `help.*` key(删前跑 check:locales)

### Task 9:MCP Server(SSE)— 依赖:Task 1、Task 2

- 新 crate `crates/mcp-server/`:
  - `Cargo.toml` 依赖 axum / tokio / serde / reqwest / tokio-stream / tracing
  - `lib.rs`:JSON-RPC 2.0 消息类型(`JsonRpcRequest/Response/Notification/Error`)+ `parse_request` + `format_response`
  - `tools.rs`:`Tool` trait + `ToolRegistry`,每个工具是 async fn,接受 `serde_json::Value` 参数,返回 `Result<Vec<Content>, ToolError>`
  - `sse.rs`:两个 handler:
    - `GET /mcp/sse`:鉴权(ReadAuthKind::AiToken)→ 建立 SSE 连接 → 推送 `endpoint` 事件告知消息 URL
    - `POST /mcp/message?session_id=...`:接收客户端消息 → 通过 channel 发给对应 SSE handler → 工具执行 → 结果通过 SSE 推回
  - CORS:`tower_http::cors` 层,`allow_origin(Any)` + `allow_headers([AUTHORIZATION, CONTENT_TYPE])`
- monitor-server:
  - `Cargo.toml` workspace member 加 `mcp-server`
  - `main.rs`:`Router::route("/mcp/sse", get(mcp_sse)).route("/mcp/message", post(mcp_message))`
  - `AppState` 加 `mcp: Arc<McpServerHandle>`(内部含 HTTP client + tools + 活跃 session map)
- 工具实现(每个一行 `reqwest` GET → 把 JSON 转成 `Content::Json`):
  - `list_nodes` → `GET /v1/nodes`
  - `get_node` → `GET /v1/nodes/{id}`
  - `get_telemetry` → `GET /v1/nodes/{id}/telemetry?limit={n|default 100}`
  - `list_alerts` → `GET /v1/alerts`
  - `list_certs` → `GET /v1/certs`
  - `list_containers` / `list_processes`:若 inventory 端点已存在则接,否则本次跳过并返回 "not implemented"
- 单测:
  - JSON-RPC parse/format
  - 一个完整 round-trip:`initialize → tools/list → tools/call list_nodes`(用 mock HTTP server)

**验收**: `curl -N -H "Authorization: Bearer <ai-token>" https://<host>/mcp/sse` 拿到 SSE 流;Claude Desktop 配 SSE URL + token 能列出节点

### Task 10:文档 — 依赖:所有

- `docs/HELP.md`(真源,中文,覆盖原 help.tsx 内容)
- `docs/DEPLOY.md` 加一节「Render/Northflank 入网」:贴入网命令截图/示例 + install-node.sh 说明
- `docs/api.md`:补 MCP 工具参考(每个工具一段:描述 + 参数 schema + 返回示例)
- `docs/CHECKLIST.md` 把 MCP 那条 `docs/api.md` 划掉

### Task 11:端到端验证 — 依赖:所有

- `cd ui && npm run build` 通过(无 TS 错、无 i18n key 缺失)
- `cargo test --workspace` 通过
- `cargo build --release` 通过
- 起 monitor(`./scripts/dev.sh start`)+ curl 走完整链路:
  1. admin token 登录(`/v1`)
  2. 创建 enroll-token + 跑 install-node.sh(在临时 docker 容器里)
  3. 节点 enroll 成功,出现在 `/v1/nodes`
  4. 创建 ai-token
  5. 用 ai-token `GET /v1/sse` 拿到流
  6. 调 `tools/list`、`tools/call list_nodes` 成功
  7. 撤销 ai-token,再调立即 401
- 撤销 enroll-token,该 token enroll 立即被拒
- 帮助页前端 `npm run build` 后 dist 里的内容正确

---

## 执行顺序与并行

- **Phase A(并行)**:Task 1(鉴权 v2) + Task 6(前端 deps + 文案) + Task 5(help.md 后端)
- **Phase B(并行)**:Task 2(AI token) + Task 3(enroll tokens) + Task 8(help 前端,需 Task 5/6) + Task 7(nodes/settings UI,需 Task 2/3/6)
- **Phase C**:Task 4(install-node.sh,需 Task 3 端点固定)+ Task 9(MCP,需 Task 1/2)
- **Phase D**:Task 10(文档)+ Task 11(端到端)

实际执行时建议 2-3 个 subagent 并行做不同 task。

