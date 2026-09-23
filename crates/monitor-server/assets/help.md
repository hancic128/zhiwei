# 知微 帮助

知微是一台服务器集群的可观测平台：本进程采集资源、服务健康、证书、告警，
一个 Web 控制台看全部，节点加进来不再写监控告警堆叠代码。

## 快速接入

### 1. 拿入网命令

打开「节点」页，点右上角「接入帮助」：控制台会现签一个一次性入网令牌，
弹窗里给出整段命令并**自动复制到剪贴板**，粘到目标机器的 shell 里跑一次即可。

```
curl -sSL https://zhiwei.example.com/install-node.sh | \
  ZHIWEI_MONITOR_URL=https://zhiwei.example.com \
  ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
  bash -s
```

- 命令开头那一段（scheme + 主机 + 端口）就是控制台当前的访问地址，
  所以先确认你是用哪个地址打开控制台的——内网 IP + 明文部署时它就是
  `http://10.0.0.5:8443/...`，照着跑才不会连错。
- **命令里含明文 token**，等于一把「入网钥匙」：别转发到公开渠道，
  过期时间（默认 24h）到了自动失效，用完也可以去
  「设置 → 入网令牌」列表里点撤销。
- 想显式管理令牌（起名字、设 TTL、看还剩几个、撤销）就去
  「设置 → 入网令牌」→「新建入网令牌」。

### 2. 在目标机器上跑

要求 root（会写 `/usr/local/bin` 和 systemd unit）。脚本会：
- 下载 `zhiwei-node`（默认 GitHub Releases；隔离网络可设 `ZHIWEI_BASE_URL`
  指向自建镜像 / 制品库）
- 写 `/etc/zhiwei-node.env`
- 注册 systemd 服务 `zhiwei-node.service` 并启动
- 节点 30 秒内出现在「节点」列表里

想在这台机器**第一次入网时就带上名字**，给脚本加两个参数（也可用环境变量
`ZHIWEI_NODE_ALIAS` / `ZHIWEI_NODE_TAGS`）：

```
curl -sSL https://zhiwei.example.com/install-node.sh | \
  ZHIWEI_MONITOR_URL=https://zhiwei.example.com \
  ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
  bash -s -- --alias 北京入口 --tags "prod bj 入口"
```

标签用空格 / 逗号 / 顿号分隔都行（同控制台的输入框）。它们**只在第一次入网
时上报**：机器已经有 `node.id` 就不会再 enroll，改它们要去控制台，或删掉
`/var/lib/zhiwei-node` 重新入网。

### 3. （可选）反转入网

如果节点先于控制台运行（例如在 CI 里临时拉起一台机器），让它从环境
变量读 `ZHIWEI_BOOTSTRAP_TOKEN` 和 `ZHIWEI_MONITOR_URL`，
第一次心跳就会自动 enroll。

### 4. 起个名字：别名与标签

入网只看主机名（一长串 `VM-16-12-opencloudos` 那种）不好认，控制台里可以
给每台节点补两层元数据——列表里「主机」列右侧的编辑按钮打开即可；也可以在
**第一次入网时**就让节点自己带上（`install-node.sh --alias/--tags`，见第 2 节）：

| | 别名 | 标签 |
|--|------|------|
| 数量 | 每台 1 个 | 每台最多 10 个 |
| 长度 | 最多 10 个字符 | 每个最多 24 个字符 |
| 用途 | 代替主机名显示 | 分组 / 过滤 |

- **别名优先**：「节点」列表、证书页、容器页、节点详情标题，以及所有
  节点下拉框（看容器日志、证书来源、容器过滤）都显示别名，没设才退回主机名。
  搜索框对别名、主机名、ID、IP、标签一起匹配。
- **标签过滤**：「节点」页顶部的「全部标签」下拉按标签筛，标签列也进排序 /
  搜索。标签是按含义自己定的（例如 `prod` / `bj` / `入口`），没有预置值，
  输入框里回车或用逗号分隔就能一次加多个。
- 别名 / 标签只存在控制台记录里，不参与节点身份，改它们不影响已发的证书、
  告警和命令通道。

## 排查（常见报错）

### 「ops-server 不可用：连不上 …：该地址上没有进程在监听」

看容器日志 / 文件日志、杀进程、重启主机、续签证书这些写操作要先送到
`zhiwei-ops`（控制平面）签名，monitor（数据平面）只是转发。这条报错的意思是
monitor 连不上 ops：

- **自建 / 裸机**：`zhiwei-ops` 没起。它是独立进程，默认只监听
  `127.0.0.1:8444`，monitor 通过 `ZHIWEI_OPS_URL` 找它。**装的如果是发行包**
  （`install.sh --bin monitor` 会把 `zhiwei-ops` 装在 `zhiwei-monitor` 旁边），
  monitor 启动时会发现该端口没人监听、并自动把同目录的 `zhiwei-ops` 拉起来；
  手工只留了 `zhiwei-monitor` 一个二进制、或路径不在一起时，用
  `ZHIWEI_OPS_BIN=/path/to/zhiwei-ops` 指一下，或自己按 systemd unit 起
  `zhiwei-ops`。
- **容器 / 托管平台**：本该由镜像的 entrypoint 在同一个容器里一并拉起
  `zhiwei-ops`。如果你用面板的 Command 字段覆盖了 entrypoint，且没让
  `zhiwei-ops` 与 `zhiwei-monitor` 待在同一目录，就只剩数据平面在跑——
  去掉覆盖，或自己把 `zhiwei-ops &` 加进启动命令。
- 只想要 telemetry + 探活 + 证书扫描（不需要远程命令）：报错可以无视，
  也可以显式 `ZHIWEI_OPS_DISABLE=1`，控制台会改报「命令通道未启用」。

### 「未持有 ops 公钥，控制通道不会拉取命令」

节点侧的安全默认：拿不到 ops 公钥就拒不执行任何来自 monitor 的写命令。
多半是 monitor 的数据目录里没有 `ops.pub`（没起 ops、或数据目录没挂持久卷）。
修法同上；注意**不挂持久卷时**每次冷启动都会换签名密钥，已入网的节点会拒绝
新命令，表观是「命令发出去没有回执」。

### 机器装完了，控制台里却一直没有这台节点

两种常见原因：
- **`node.id` 是旧的**：节点只认 `<state-dir>/node.id` 在不在。控制台侧换过
  数据目录 / 数据库（或对端数据被重置）后，旧身份会一直 401「节点签名校验失败」。
  删掉节点上的 `node.id`（以及 `ops.pub`）重装一次即可。
- **下载源不通**：脚本没报错但节点没起来时，先看
  `systemctl status zhiwei-node` 与 `/etc/zhiwei-node.env`；隔离网络记得设
  `ZHIWEI_BASE_URL`。

### 容器日志 / 容器列表是空的

两者走的不是同一条路，排查方式也不同：

- **列表**：节点低频上报的「快照」里带容器清单（主机信息 + 容器 + 进程 +
  证书），monitor 只留最新一份。节点上没装 `docker`（或读不到 socket）时
  清单就是空的——这是正常结果，不报错。
- **日志**：属于点击触发的写操作，要经 `zhiwei-ops` 签名下发命令、节点执行
  `docker logs` 再回传。所以列表有数据、日志报「ops-server 不可用」说明
  数据平面正常、只是命令通道没起来（见上一条）。
- 日志只按需拉取、不落库：能看到的范围取决于容器自身是否还在（容器被
  重建 / 删除后旧日志查不到）。

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

- `list_nodes` — 集群节点总览（含别名字段 `alias` 与 `tags`，AI 读到的名字
  和你控制台里看到的一致）
- `get_node` — 单节点详情（host_info + 最新指标）
- `get_telemetry` — 单节点时间序列
- `list_alerts` — 全部活跃告警 + 最近 50 条已解决
- `list_certs` — 证书扫描来源（「节点 + 路径」配置，不含证书内容）
- `list_containers` — 单节点最新容器快照
- `list_processes` — 单节点最新进程快照 TopN

容器 / 进程快照来自节点低频上报的 inventory，所以要求该节点已经报过一次。

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
- **数据目录要挂持久卷**：CA 私钥、`ops.key`、SQLite 都在这里
- 镜像的 entrypoint 会在同一个容器里先起 `zhiwei-ops` 再起
  `zhiwei-monitor`（可用 `ZHIWEI_OPS_DISABLE=1` 关掉、`ZHIWEI_OPS_WAIT=<秒>`
  调等待）。**别在面板里覆盖 Command / entrypoint**，否则命令通道（看日志、
  杀进程、重启服务、续签证书）会用不了，报「ops-server 不可用」。

自建主机：
- 本进程终结 TLS，浏览器直接 `https://monitor.example.com`
- 前面套 nginx/caddy 也行，但 monitor 自己终结的证书是自签 CA，
  浏览器会警告 → 把 CA 加进系统信任根
- `zhiwei-ops` 不是 monitor 的一部分，要单独起一个进程（同容器也行）；
  没起时数据平面照常工作，只是命令通道不可用
