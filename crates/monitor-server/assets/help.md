# 知微 帮助

知微是一台服务器集群的可观测平台：本进程采集资源、服务健康、证书、告警，
一个 Web 控制台看全部，节点加进来不再写监控告警堆叠代码。

## 快速接入

### 1. 生成入网命令

打开「设置 → 入网令牌」，点「新建入网令牌」：
- TTL 选 24h（一次性用完即废）
- 名字可选，例如 `prod-web-01`

点「创建」后弹窗里会有一段形如下面的命令：

```
curl -sSL https://zhiwei.example.com/install-node.sh | \
  ZHIWEI_MONITOR_URL=https://zhiwei.example.com \
  ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
  bash -s
```

### 2. 在目标机器上跑

要求 root （会写 `/usr/local/bin` 和 systemd unit）。
脚本会：
- 从 GitHub release 下载 `zhiwei-node`
- 写 `/etc/zhiwei-node.env`
- 注册 systemd 服务 `zhiwei-node.service` 并启动
- 节点 30 秒内出现在「节点」列表里

### 3. （可选）反转入网

如果节点先于控制台运行（例如在 CI 里临时拉起一台机器），让它从环境
变量读 `ZHIWEI_BOOTSTRAP_TOKEN` 和 `ZHIWEI_MONITOR_URL`，
第一次心跳就会自动 enroll。

## AI 怎么用

知微暴露 MCP SSE 端点（`https://<host>/mcp/sse`），让 Claude Desktop、
Cursor、Cline 等能直接读集群数据。

### 1. 创建 AI Token

「设置 → AI 令牌」→「新建」→ 起个名字（例如 `claude-desktop-home`）→
复制明文 token（**仅显示一次**）。

### 2. 配置 MCP 客户端

在 Claude Desktop 的 `claude_desktop_config.json` 里加：

```json
{
  "mcpServers": {
    "zhiwei": {
      "url": "https://zhiwei.example.com/mcp/sse",
      "headers": {
        "Authorization": "Bearer ait_xxxxxxxxxxxxxxxx"
      }
    }
  }
}
```

### 3. 可用工具

- `list_nodes` — 集群节点总览
- `get_node` — 单节点详情（主机信息 / 最新指标）
- `get_telemetry` — 单节点时间序列
- `list_alerts` — 活跃 / 历史告警
- `list_certs` — 证书清单（含到期倒计时）

### 安全边界

AI token 只允许**读**。删除节点、重启服务、修改告警等写操作**不在 AI
token 权限范围**（需要控制台登录 + admin token）。

撤销 AI token 后，下一次请求立即返回 401。

## 部署与证书

知微自带 CA（首次启动时生成在 `<data-dir>/ca/`）：
- monitor server 证书 90 天有效，带 SAN
- 节点身份 = Ed25519 请求签名（不走客户端证书），CA 仅用于边缘 TLS 终结时
  服务端证书签发
- **换盘 = 所有节点掉线**：CA 私钥丢了再签发的证书，节点不会认

托管平台（Render / Railway / Northflank）上：
- TLS 由边缘终结，本进程监听明文 HTTP
- 必须设 `ZHIWEI_PLAIN_HTTP=1` 和 `ZHIWEI_LISTEN=0.0.0.0:<port>`
- Northflank 不会注入 `PORT`，必须显式设监听地址

自建主机：
- 本进程终结 TLS，浏览器直接 `https://monitor.example.com`
- 前面套 nginx/caddy 也行，但 monitor 自己终结的证书是自签 CA，
  浏览器会警告 → 把 CA 加进系统信任根

## 入网令牌 vs AI 令牌

二者**完全独立**，设计意图不同：

| | 入网令牌 | AI 令牌 |
|--|----------|---------|
| 用途 | 节点首次 enroll | MCP / 外部 AI 读数据 |
| 凭据对象 | `zhi-bt-...` | `ait_...` |
| 持久化 | 内存（重启丢） | SQLite（重启保留） |
| 数量 | 可多个 | 可多个 |
| 撤销 | 列表里点撤销 | 列表里点撤销 |
| 过期 | TTL（小时/天） | 不过期 |

