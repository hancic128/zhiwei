# 知微 API 参考

两套 API，鉴权方式不同：

| 面向 | 路径前缀 | 凭据 | 谁用 |
| --- | --- | --- | --- |
| 控制台 / 外部集成 | `/v1/*` | `Authorization: Bearer <admin token 或 AI token>` | 浏览器控制台、脚本、AI 客户端 |
| 节点 | `/v1/enroll`、`/v1/telemetry`、`/v1/inventory` | bootstrap token（enroll）/ Ed25519 请求签名（其余） | node-agent |

`admin.token` 是**单值**凭据（`<data-dir>/admin.token` 或 `ZHIWEI_ADMIN_TOKEN`）。
AI token 是**多值、可撤销**的读凭据，用于 MCP 与外部 AI 客户端。
详见 [§3 AI Token](#3-ai-token)。

---

## 1. 通用读端点

全部要求 `Authorization: Bearer <admin token 或 AI token>`。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/v1` | 总览：`{ service, version, authenticated, endpoints, nodes, telemetry_batches }` |
| GET | `/v1/nodes` | 节点列表（含 `host_info` 与最新一帧的关键指标） |
| GET | `/v1/nodes/:id/telemetry?limit=N` | 某节点最近的 telemetry 帧 |
| GET | `/v1/nodes/:id/series?range=…` | 时间序列（降采样后） |
| GET | `/v1/nodes/:id/containers` | 该节点最新容器快照 |
| GET | `/v1/nodes/:id/processes` | 该节点最新进程快照 |
| GET | `/v1/containers` | 全集群容器视图 |
| GET | `/v1/certificates` | 证书清单 |
| GET | `/v1/cert-sources` | 证书扫描来源（节点 + 路径） |
| GET | `/v1/alerts` | 告警实例（活跃 + 历史） |
| GET | `/v1/rules` | 告警规则 |
| GET | `/v1/channels` | 通知渠道 |
| GET | `/v1/services`、`/v1/probes` | 服务探活 |
| GET | `/v1/todo` | 待办聚合（按紧急度排序） |
| GET | `/v1/help` | 帮助页 markdown：`{ locale, body }` |
| GET | `/healthz` | 健康检查（**不需要鉴权**） |

## 2. 写端点（仅接受 admin token）

AI token **调不动**这些——调用会返回 401。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/v1/admin/token` | 改控制台凭据，body `{ current, new }` |
| POST | `/v1/ai-tokens` | 创建 AI token |
| DELETE | `/v1/ai-tokens/:id` | 撤销 AI token |
| POST | `/v1/enroll-tokens` | 创建一次性入网令牌 |
| DELETE | `/v1/enroll-tokens/:id` | 撤销入网令牌 |
| POST/PATCH/DELETE | `/v1/rules`、`/v1/channels`、`/v1/services`、`/v1/probes`、`/v1/cert-sources` | 各类配置增删改 |
| POST | `/v1/exec` | 下发命令给节点 |

## 3. AI Token

AI token 是**只能读**的凭据，专给 MCP / 外部 AI 客户端用。
和 admin token 完全隔离：它不能改任何配置，也不能自我复制。

### 创建

```sh
curl -X POST https://<monitor>/v1/ai-tokens \
  -H "Authorization: Bearer <admin token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "claude-desktop-home"}'
```

响应里 `token` 形如 `ait_<base64url>`，**仅此一次返回**：

```json
{
  "id": "ait-675cba",
  "name": "claude-desktop-home",
  "token": "ait_U-LXMwc2Hhf6-zSyXEQ5mXvRLCORlAoqfmNN1wuy0o8",
  "created_at_unix_nano": 1790038765833242000,
  "warning": "明文 token 仅返回一次，请立即复制保存"
}
```

### 列出 / 撤销

```sh
curl https://<monitor>/v1/ai-tokens -H "Authorization: Bearer <admin token>"

curl -X DELETE https://<monitor>/v1/ai-tokens/ait-675cba \
  -H "Authorization: Bearer <admin token>"
```

撤销后该 token **下一次请求**即返回 401。`last_used_at_unix_nano` 用于审计。

## 4. 入网令牌

运行时生成的**一次性** bootstrap token，配合 `install-node.sh` 使用。

| 方法 | 路径 | body / 说明 |
| --- | --- | --- |
| GET | `/v1/enroll-tokens` | 列出未过期令牌元信息（**不含明文**） |
| POST | `/v1/enroll-tokens` | `{ ttl_secs?, label? }` → 返回 `enroll_command` |
| DELETE | `/v1/enroll-tokens/:id` | 撤销 |

```sh
curl -X POST https://<monitor>/v1/enroll-tokens \
  -H "Authorization: Bearer <admin token>" \
  -H "Content-Type: application/json" \
  -d '{"ttl_secs": 86400, "label": "prod-web-01"}'
```

`enroll_command` 里的 monitor URL 由后端按 `X-Forwarded-Proto` + `Host` 推断。

## 5. MCP Server（给 AI 客户端）

MCP 走 **SSE transport**，端点：

```
POST https://<monitor>/mcp/sse
Authorization: Bearer <AI token>
Content-Type: application/json
```

响应是 `text/event-stream`，每个事件一条 JSON-RPC 2.0 消息：

```
event: message
data: {"jsonrpc":"2.0","id":1,"result":{...}}
```

协议版本 `2024-11-05`。支持 `initialize` / `notifications/initialized` /
`ping` / `tools/list` / `tools/call`。**不支持** `resources/*`、`prompts/*`。

> `admin token` 调 MCP 会被拒绝（403）——必须用 AI token。
> 这是刻意的：MCP 是常驻集成，应该用可单独撤销的凭据。

### 工具列表

| 工具 | 参数 | 底层端点 |
| --- | --- | --- |
| `list_nodes` | — | `GET /v1/nodes` |
| `get_node` | `node_id` | `GET /v1/nodes` 后按 id 过滤（后端无单节点端点） |
| `get_telemetry` | `node_id`, `limit?`（默认 100） | `GET /v1/nodes/:id/telemetry` |
| `list_alerts` | — | `GET /v1/alerts`（服务端固定窗口） |
| `list_certs` | — | `GET /v1/cert-sources` |
| `list_containers` | `node_id` | `GET /v1/nodes/:id/containers` |
| `list_processes` | `node_id`, `sort?`, `limit?` | `GET /v1/nodes/:id/processes` |

**范围边界**（DESIGN.md D8）：破坏性操作（`reboot` / `shutdown` /
`kill_process` / `container_action` / `renew_cert`）**不**暴露给 MCP。
MCP 只到「读 + 轻管理」中的读，manage 类等对应后端端点落地后再补。

### 客户端配置示例

Claude Desktop（`claude_desktop_config.json`）：

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

### 错误约定

| 情况 | 行为 |
| --- | --- |
| 未带 / 错 token | HTTP 401，JSON `{ error }` |
| 用 admin token | HTTP 403，提示改用 AI token |
| JSON 解析失败 | JSON-RPC `-32700` |
| 未知 method | JSON-RPC `-32601` |
| 参数缺失 | JSON-RPC `-32602` |
| 工具内部错误 | `result.isError = true` + 错误文本 |

## 6. 节点侧端点

| 方法 | 路径 | 凭据 |
| --- | --- | --- |
| POST | `/v1/enroll` | `Authorization: Bearer <bootstrap token>` + protobuf |
| POST | `/v1/telemetry` | Ed25519 请求签名 |
| POST | `/v1/inventory` | Ed25519 请求签名 |

签名校验：`crates/common/src/auth.rs`（时间窗 ±300s + nonce 防重放）。
