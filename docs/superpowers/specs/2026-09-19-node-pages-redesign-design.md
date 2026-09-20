# 节点页面 / 节点详情页改版 · 设计

日期：2026-09-19
状态：按本人给出的条目实现（歧义处按下表取定，已在同轮回复里逐条点明）
上位文档：`docs/DESIGN.md`、Trilium「前端约束规范」（令牌唯一来源 / 组件优先 / 禁止清单）

## 1. 需求原文与落点

### 节点页面

| 需求 | 落点 |
| --- | --- |
| 列表支持分页（默认 10）、排序、过滤（状态、系统） | 前端分页 + 列头排序 + 状态/系统两个下拉过滤 |
| 移除列：批次、走势 | 删 `colBatches` / `colTrend`（同时去掉每节点一次 series 请求，列表从 N+1 次请求降为 1 次） |
| 添加列：内存、uptime、上次更新 | 新列；内存显示 `已用/总量` + 百分比，uptime 显示「天 + 小时」，上次更新相对时间（hover 出绝对时间） |
| 搜索主机时，显示匹配列表 | 搜索在**全量节点**上匹配（主机名 / 节点 ID / IP），命中结果即列表；显示命中条数，翻页回到第 1 页 |

### 节点详情

| 需求 | 落点 |
| --- | --- |
| 时间范围选择 + 快捷选择（30m/1h/3h/12h/1d/3d/7d/30d） | 右上控件区 `TimeRangePicker`：左侧快捷范围列 + 右侧自定义起止（月历 + 时:分，禁用原生日期控件） |
| 刷新频率 5s/10s/30s/1min/5min | 控件区刷新下拉，落到 react-query `refetchInterval`；默认 30s |
| 大字卡片：CPU、内存、磁盘、网络（上下行） | 4 张大字卡片（磁盘 = 使用率最高的挂载点；网络 = 当前上行/下行速率） |
| 基本信息卡片，IP 算基本信息；默认不展示，点 SVG 按钮查看 | 卡片默认折叠，标题区右侧「基本信息」按钮（`Info` SVG）切换；IP 地址并入基本信息 |
| 趋势图：CPU（绝对值、率）、内存（绝对值、率）、磁盘（绝对值、率）、网络 | 4 张趋势图，CPU/内存/磁盘带「绝对值 / 占比」切换，网络固定上下行两条线 |
| 进程 top10（内存/CPU），可杀（强杀/优雅杀） | 进程卡片带 CPU/内存 排序切换，取前 10，每行两个操作按钮（SIGTERM = 优雅杀，SIGKILL = 强杀），走签名命令通道 + 二次确认 |
| 不需要网络接口、最近一次上报、磁盘卡片、磁盘挂载、上报间隔、批次、采样数 | 删除对应卡片/字段/列 |
| 节点 token 密文显示 | 基本信息里的节点身份（Ed25519 公钥）默认掩码显示 `b3f1a9c2……8e4d`，可复制全量 |
| 内存使用 Y 轴单位 | 图表 Y 轴支持自定义格式化，内存/磁盘/网络绝对量按 B/KiB/MiB/GiB 显示 |
| 关机、重启按钮和基本信息按钮放一块，都在右上角 | 右上角操作组：基本信息 / 重启 / 关机（后两者走 ConfirmDialog） |

## 2. 关键决策

| 决策 | 选择 | 理由 |
| --- | --- | --- |
| 需求 3 与需求 7 的「磁盘」冲突 | 保留**磁盘大字卡片**（需求 3），删除**磁盘挂载明细卡片**（需求 7 的「磁盘挂载」） | 需求 7 的语境是「不要明细卡片」，需求 3 明确列了 4 张大字卡片 |
| 「绝对值 / 率」的含义 | 绝对值 = 物理量（核数 / 字节），率 = 占比（%） | 内存/磁盘的 Y 轴单位需求（需求 8）只有在绝对值模式下才有意义，两者互为补充 |
| CPU 的绝对值 | 使用核数 = `使用率% × 核数 / 100`（核数取 host_info.cpu_cores） | CPU 没有字节量，核数是唯一有意义的绝对量；单核机器两视图数值相同 |
| 网络 | 上下行**速率**（bytes/s），由服务端对网卡累计计数做差分 | 累计字节数没有可读性；差分放服务端可复用，也能喂告警 |
| 列表分页/排序/过滤位置 | 前端（`/v1/nodes` 一次取全量，带最近一帧指标） | 节点规模是「几台 ~ 几十台」；服务端分页会牺牲「搜索即全量匹配」。节点上千时再补服务端分页 |
| 列表指标来源 | `/v1/nodes` 顺带返回每个节点最新一帧的 CPU/内存/磁盘（新增 `latest` 字段） | 避免每节点一次 series 请求（原走势列就是 N+1） |
| 时间范围参数 | `/v1/nodes/:id/series?metric=&from=&to=&limit=&rate=` | `from`/`to` 是毫秒时间戳；`rate=1` 表示对累计型指标做每秒差分 |
| 杀进程 / 关机 / 重启的实现 | 复用既有「ops 签名 → 节点验签 → 执行 → 回执」命令通道，新增 3 个动作 | 不新开特权通道；白名单、TTL、回执签名全都复用 |
| 关机 / 重启的安全边界 | 二次确认弹窗必须手输动作语义之外，节点侧仍按白名单执行 | 保留审计链路（命令历史页可见） |

## 3. 后端改动

### 3.1 采集（node-agent）

- `build_batch` 增补 3 个指标，供绝对值视图与大字卡片直接取用：
  - `host.disk.used_bytes`：使用率最高的挂载点的已用字节
  - `host.disk.total_bytes`：同一挂载点的总字节
  - （网络不加指标，`network[]` 已含每网卡累计 rx/tx，服务端按需差分）
- `snapshot_processes` 取「CPU 前 20 ∪ 内存前 20」的并集（去重），让前端的「按内存排序 top10」有数据可用。

### 3.2 存储（storage）

- `telemetry_repo.range(node_id, from_ns, to_ns, limit)`：按时间窗取（升序），替代只有 `limit` 的取法。
- `telemetry_repo.latest_per_node()`：每节点最新一帧（`MAX(id) GROUP BY node_id`），给 `/v1/nodes` 用。

### 3.3 接口（monitor-server）

- `GET /v1/nodes`：每个节点加 `latest` 块（`ts_unix_nano` + cpu/mem/disk 指标），`host_info` 里已有 uptime 与 IP。
- `GET /v1/nodes/:id/series`：
  - 支持 `from` / `to`（ms）；
  - `rate=1` 时对累计型指标做差分（bytes/s）；
  - 指标抽取统一走 `extract_metric(batch, name)`：先查 `metrics[]`，再兜底派生 `host.net.rx_bytes` / `host.net.tx_bytes`（跨网卡求和）。
- 命令动作新增（`proto/control.proto`）：`ACTION_KILL_PROCESS = 3`、`ACTION_RESTART_HOST = 4`、`ACTION_SHUTDOWN_HOST = 5`。
  - `kill_process` 参数 `{pid, signal: "term" | "kill"}`；`restart_host` / `shutdown_host` 无参数。
- ops-server 与节点侧白名单同步放行，节点侧执行：
  - 杀进程：`kill -TERM <pid>` / `kill -KILL <pid>`
  - 重启：`shutdown -r now`（失败再退 `systemctl reboot`）
  - 关机：`shutdown -h now`（失败再退 `systemctl poweroff`）

## 4. 前端改动

- 新增组件：`ui/src/components/ui/select.tsx`（令牌化的原生下拉）、
  `ui/src/components/ui/date-range-picker.tsx`（快捷范围 + 月历自定义，禁原生日期控件）。
- `ui/src/components/chart.tsx`：`LineChart` 增加 `yFormatter` / `tooltipFormatter`，时间轴随范围自适应。
- `ui/src/pages/nodes.tsx` 重写：列 = 主机 / 状态 / 系统 / 标签 / 内存 / CPU / uptime / 上次更新；排序、过滤、分页、搜索命中提示。
- `ui/src/pages/node-detail.tsx` 重写：右上操作组、时间范围 + 刷新频率、4 张大字卡片、4 张趋势图、可折叠基本信息、进程 top10（含杀进程）。
- 语言包中英同步补齐（`npm run build` 会校验 key 对齐与单花括号）。

## 5. 验证

- `cargo fmt --check` / `clippy -D warnings` / `cargo test`；新增解析与差分逻辑的单测。
- `npm run build`（含语言包体检）。
- 本地预览（`./scripts/dev.sh start`）+ Playwright：列表分页/排序/过滤/搜索、详情页时间范围与刷新频率、4 卡与 4 图出数、基本信息折叠、进程 top10 与「杀进程」真跑一次（对 `sleep` 进程发 SIGTERM），关机/重启只验证按钮与确认弹窗、不实际执行。
