# 容器页改版 · 日志菜单收敛 · 证书路径管理 · 设计

日期：2026-09-19
状态：按本人给出的条目实现（歧义处按下表取定，已在同轮回复里逐条点明）
上位文档：`docs/DESIGN.md`、Trilium「前端约束规范」（令牌唯一来源 / 组件优先 / 禁止清单）

## 1. 需求原文与落点

### 容器页

| 需求 | 落点 |
| --- | --- |
| 容器列表支持分页、排序、搜索、过滤 | 跨节点**扁平表**：分页（默认 10，可选 20/50）、5 列排序（名称 / 节点 / 状态 / 启动时长 / 上次更新）、搜索（名称 · 镜像 · 容器 ID · 主机名 · 节点 ID）、状态与节点两个过滤 |
| 顶部显示大字卡片：不同状态的容器数 | 4 张卡：总容器数 / 运行中 / 已停止 / 异常；**互斥分桶**（异常优先，其次运行中，再看已停止），点卡片即按该桶过滤（`?state=` 带进 URL，概览页跳转继续可用） |
| 不需要单独的日志菜单 | 侧栏与路由移除 `/logs`；容器日志落到容器页行内操作；文件日志保留在容器页工具栏的「查看文件日志」对话框（能力不丢） |

### 证书页

| 需求 | 落点 |
| --- | --- |
| 支持新增 SSL 证书路径 | 新增「证书路径」配置区：`节点 + 路径` 的增删改；路径接受**目录**（自动展开为 `目录/*.pem` `*.crt`）、**单个文件**或 **glob** |
| 指定节点 + 目录 | 每条来源归属一个节点（必填）；节点被删除时来源保留但标记「节点不存在」 |
| 可以测试 | 表单内与列表行都有「测试」：走签名命令通道让**节点真的扫一遍**并回结果，界面列出命中的证书（域名/主体/到期/剩余天数）或失败原因 |
| 设置到期前多久通知 | 每条来源一个 `到期前 N 天`（默认 30，范围 1–365） |
| 通知开关等 | 每条来源一个 `通知` 开关 + 一个 `启用` 开关（关闭启用 = 节点不再扫这条路径） |

## 2. 关键决策

| 决策 | 选择 | 理由 |
| --- | --- | --- |
| 证书路径存放在哪 | 存 monitor 侧（新表 `cert_sources`），节点每次采集快照前拉一次 `/v1/cert-config` | 与既有 `probe-config` 同构；路径属于「用户配置」而非节点本地状态，放在服务端才能在控制台增删改 |
| 内置默认扫描路径怎么办 | 保留为**基线**（节点侧 `--cert-globs` 与内置 glob 仍照旧扫），服务端配置的路径叠加在其上；同一路径被两边命中时以服务端来源为准 | 不破坏既有行为（升级后老用户不会突然扫不到 letsencrypt），且新增路径可以有自己的通知设置 |
| 命中数怎么算 | 用最新快照 + 来源的展开 glob 做服务端匹配 | 不新增协议字段、不新增状态表；「上次扫描」直接取该节点快照时间 |
| 测试怎么实现 | 新增命令动作 `scan_certs`（参数 = 单条路径），节点扫描后把结果 JSON 放进命令回执 | 路径是否存在只有节点知道；复用既有 ops 签名 → 节点验签 → 回执通道，无需新接口 |
| 到期通知怎么发 | 快照落库后就地跑一遍证书评估：`source=cert` 写既有 `alerts` 表，过期 critical、临期 warning，续签/删来源后自动 resolved | 与探针告警同一套表 + 同一套通知渠道与严重度过滤，告警页/静默/Webhook 全部复用 |
| 容器分桶口径 | 异常 = 既有 `containerIsFailed`（dead / restarting / exited 非 0 / running 但 unhealthy）；运行中 = running 且非异常；已停止 = exited(0) / created / paused；其余进「其他」并计入总数 | 卡片之间互不重叠，四张卡相加 = 总数，避免「已停止 + 异常」重复计数 |
| 容器页操作范围 | 与节点详情容器组一致（启动 / 关闭 / 重启 / 查看日志） | 日志菜单下线后，容器页就是容器的唯一入口；操作通道与二次确认沿用既有实现 |

## 3. 数据与接口

### 存储（迁移 010）

```sql
CREATE TABLE cert_sources (
    id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL,
    path TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    notify_enabled INTEGER NOT NULL DEFAULT 1,
    notify_days_before INTEGER NOT NULL DEFAULT 30,
    created_at_unix_nano INTEGER NOT NULL,
    updated_at_unix_nano INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_cert_sources_node_path ON cert_sources(node_id, path);
```

### 协议

- `proto/telemetry.proto`：`CertInfo` 增 `string source_id = 9`（空 = 来自内置/命令行 glob 的基线扫描）
- `proto/control.proto`：新增 `ACTION_SCAN_CERTS = 10`（参数 JSON `{ "path": "..." }`）

### 接口

| 方法 | 路径 | 调用方 | 说明 |
| --- | --- | --- | --- |
| GET | `/v1/cert-config?node_id=` | 节点（签名） | 拉取本节点启用的证书路径（只能拉自己） |
| GET | `/v1/cert-sources` | 控制台 | 来源列表 + 节点主机名 + 命中数 + 快照时间 |
| POST | `/v1/cert-sources` | 控制台 | 新增（节点必须已入网；校验路径） |
| PATCH | `/v1/cert-sources/:id` | 控制台 | 改路径 / 启用 / 通知开关 / 到期前天数 |
| DELETE | `/v1/cert-sources/:id` | 控制台 | 删除 |
| POST | `/v1/cert-sources/test` | 控制台 | 发起 `scan_certs` 命令，返回 `command_id`（界面轮询回执） |

新增 / 修改 / 删除来源后，monitor 会向该节点补发一次 `refresh_inventory`（best effort），
让节点立刻按新配置重扫，不必等 5 分钟采集周期。

## 4. 边界与不做

- 不做 ACME / 自动续签（沿用 P2-4 范围）；不做证书内容展示（只做路径、域名、到期）。
- 不做「所有节点」来源（必须指定节点）——避免与「节点侧看不到的路径」混淆。
- 不做来源级 RBAC；不做告警静默的自定义规则（复用既有静默）。
- 文件日志对话框只做「按需拉取」，不做实时跟随（与原日志页一致，但收敛到容器页）。
