/**
 * 与 monitor 通信的 API 客户端。
 *
 * 控制台走 `Authorization: Bearer <admin token>`；节点走 Ed25519 请求签名，
 * 两套凭据互不影响。
 *
 * 后端时间戳是 Unix 纳秒，这里统一转成毫秒，UI 层只处理毫秒。
 */

import { clearAdminToken, getAdminToken } from "@/lib/prefs";

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getAdminToken();
  const res = await fetch(path, {
    ...init,
    headers: {
      ...(init?.headers ?? {}),
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
  });

  if (res.status === 401) {
    // 受保护端点 401：清掉 token，让 App.tsx 回到登录页。
    // `/v1` 自身永远 200（仅靠 body.authenticated 区分），所以这条事件
    // 不会被自己误触发。
    window.dispatchEvent(new CustomEvent("zhiwei:unauthorized"));
    throw new ApiError(401, "unauthorized");
  }
  if (!res.ok) {
    // 服务端会给人话原因（如「signal 只能是 term 或 kill」），优先展示它；
    // 拿不到才退回状态码，交给 friendlyError 翻译成友好文案
    const text = await res.text().catch(() => "");
    let message = `http_${res.status}`;
    try {
      const body = JSON.parse(text) as { error?: unknown };
      if (typeof body?.error === "string" && body.error.trim()) {
        message = body.error.trim();
      }
    } catch {
      // 非 JSON 响应体：保持状态码
    }
    throw new ApiError(res.status, message);
  }
  // 204 与空响应体（如 DELETE / PATCH）没有 JSON
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export interface IpAddress {
  addr: string;
  prefix: number;
}

export interface InterfaceInfo {
  name: string;
  addresses: IpAddress[];
  loopback: boolean;
}

/** 节点主机基本信息（见 crates/node-agent 的 build_host_info） */
export interface HostInfo {
  hostname?: string;
  os_name?: string;
  os_version?: string;
  long_os_version?: string;
  kernel_version?: string;
  arch?: string;
  cpu_brand?: string;
  cpu_cores?: number;
  total_memory_bytes?: number;
  uptime_seconds?: number;
  boot_time_unix_seconds?: number;
  interfaces?: InterfaceInfo[];
  agent_version?: string;
}

interface RawNode {
  id: string;
  hostname: string;
  labels: Record<string, string>;
  alias?: string;
  tags?: string[];
  enrolled_at_unix_nano: number;
  last_seen_unix_nano: number | null;
  host_info?: HostInfo;
  latest?: NodeLatest | null;
  public_key?: string;
}

/** 最新一帧里的关键指标（列表页 CPU / 内存 / 磁盘列直接用，免去每节点一次请求） */
export interface NodeLatest {
  ts_unix_nano: number;
  cpu_percent: number | null;
  mem_used_bytes: number | null;
  mem_total_bytes: number | null;
  disk_usage_percent: number | null;
  disk_used_bytes: number | null;
  disk_total_bytes: number | null;
  net_rx_bytes: number | null;
  net_tx_bytes: number | null;
}

export interface NodeView {
  id: string;
  hostname: string;
  labels: Record<string, string>;
  /** 管理员给的简短别称（≤10 字符），空串表示未设置 */
  alias: string;
  /** 管理员给的标签（≤10 个） */
  tags: string[];
  enrolled_at_ms: number;
  last_seen_ms: number | null;
  host_info: HostInfo;
  latest: NodeLatest | null;
  /** 节点身份公钥（base64）；界面上按「密文」掩码展示 */
  public_key: string;
}

export interface IndexBody {
  service: string;
  version: string;
  authenticated: boolean;
  endpoints: string[];
  nodes: number;
  telemetry_batches: number;
}

export interface MetricView {
  name: string;
  value: number;
}

export interface BatchView {
  ts_unix_nano: number;
  interval_seconds: number;
  bytes: number;
  metrics: MetricView[];
  disks: string[];
  network_interfaces: string[];
  signature_bytes: number;
}

export interface NodeTelemetryView {
  node_id: string;
  returned: number;
  batches: BatchView[];
}

export interface SeriesView {
  node_id: string;
  metric: string;
  points: Array<{ t: number; v: number }>;
  latest: number | null;
  /** raw = 原始 10 秒数据；hourly = 小时聚合（窗口起点早于原始保留期） */
  resolution?: "raw" | "hourly";
}

const nsToMs = (ns: number | null) => (ns === null ? null : Math.floor(ns / 1e6));

export const api = {
  index: () => request<IndexBody>("/v1"),

  nodes: async (): Promise<NodeView[]> => {
    const raw = await request<RawNode[]>("/v1/nodes");
    return raw.map((n) => ({
      id: n.id,
      hostname: n.hostname,
      labels: n.labels ?? {},
      alias: n.alias ?? "",
      tags: n.tags ?? [],
      enrolled_at_ms: nsToMs(n.enrolled_at_unix_nano) ?? 0,
      last_seen_ms: nsToMs(n.last_seen_unix_nano),
      host_info: n.host_info ?? {},
      latest: n.latest ?? null,
      public_key: n.public_key ?? "",
    }));
  },

  /** 改管理员维护的别名 / 标签（只传要改的字段） */
  updateNode: (
    id: string,
    patch: { alias?: string; tags?: string[] },
  ) =>
    request<{ id: string; alias: string; tags: string[] }>(
      `/v1/nodes/${encodeURIComponent(id)}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(patch),
      },
    ),

  telemetry: (id: string, limit = 20) =>
    request<NodeTelemetryView>(
      `/v1/nodes/${encodeURIComponent(id)}/telemetry?limit=${limit}`,
    ),

  /** 趋势图取数：`from`/`to` 是毫秒时间窗；`rate` 对累计型指标做每秒差分 */
  series: (id: string, metric: string, opts: SeriesQuery = {}) => {
    const params = new URLSearchParams({
      metric,
      limit: String(opts.limit ?? 600),
    });
    if (opts.from) params.set("from", String(Math.round(opts.from)));
    if (opts.to) params.set("to", String(Math.round(opts.to)));
    if (opts.rate) params.set("rate", "1");
    return request<SeriesView>(
      `/v1/nodes/${encodeURIComponent(id)}/series?${params.toString()}`,
    );
  },
};

export interface SeriesQuery {
  from?: number;
  to?: number;
  limit?: number;
  rate?: boolean;
}

// ---------- 跨节点 / 跨服务的聚合趋势 ----------

/** 全部节点同一指标的趋势：一条线一个节点 */
export interface AllNodesSeriesView {
  metric: string;
  resolution: "raw" | "hourly";
  nodes: Array<{
    node_id: string;
    hostname: string;
    latest: number | null;
    points: Array<{ t: number; v: number }>;
  }>;
}

/** 服务健康时间线：每桶里「有多少比例的探测是 ok 的」（百分比） */
export interface ServicesTimelineView {
  from_ms: number;
  to_ms: number;
  bucket_ms: number;
  level: "service" | "probe";
  /** service 粒度时一条线一个服务；probe 粒度时一条线一个探针（name 带服务名前缀） */
  series: Array<{
    id: string;
    name: string;
    group: string | null;
    points: Array<{ t: number; v: number }>;
  }>;
}

/** 跨对象趋势的统一取数口 */
export const trendApi = {
  allNodes: (metric: string, opts: SeriesQuery = {}) => {
    const params = new URLSearchParams({
      metric,
      limit: String(opts.limit ?? 300),
    });
    if (opts.from) params.set("from", String(Math.round(opts.from)));
    if (opts.to) params.set("to", String(Math.round(opts.to)));
    return request<AllNodesSeriesView>(`/v1/series/nodes?${params.toString()}`);
  },
  servicesTimeline: (
    from: number,
    to: number,
    buckets = 60,
    level: "service" | "probe" = "service",
  ) =>
    request<ServicesTimelineView>(
      `/v1/services/timeline?from=${Math.round(from)}&to=${Math.round(to)}&buckets=${buckets}&level=${level}`,
    ),
};

/** 本项目当前采集的指标名（见 crates/node-agent 的 build_batch）。 */
export const METRICS = {
  cpu: "host.cpu.usage",
  memUsed: "host.mem.used_bytes",
  memTotal: "host.mem.total_bytes",
  diskUsage: "host.disk.usage",
  diskUsed: "host.disk.used_bytes",
  diskTotal: "host.disk.total_bytes",
  netRx: "host.net.rx_bytes",
  netTx: "host.net.tx_bytes",
} as const;

/** 图表里用到的序列转换 */
export const toPairs = (points: Array<{ t: number; v: number }>) =>
  points.map((p) => [p.t, p.v] as [number, number]);

export function getToken(): string | null {
  const t = getAdminToken();
  return t ? t : null;
}

export function setToken(token: string): void {
  localStorage.setItem("zhiwei.adminToken", token);
}

/** App.tsx 在收到 `zhiwei:unauthorized` 事件后调用，清掉本地缓存的凭据 */
export { clearAdminToken };

/** 展示用系统标签：优先 sysinfo 的长版本号，回落到 name + version */
export function osLabel(host: HostInfo | undefined): string {
  if (!host) return "";
  const long = host.long_os_version?.trim();
  if (long) return long;
  const name = host.os_name?.trim() ?? "";
  const ver = host.os_version?.trim() ?? "";
  return `${name} ${ver}`.trim();
}

/** 主 IP：优先非回环 IPv4，其次任意非回环地址 */
export function primaryIp(host: HostInfo | undefined): string {
  if (!host?.interfaces) return "";
  const candidates = host.interfaces
    .filter((i) => !i.loopback)
    .flatMap((i) => i.addresses.map((a) => ({ ...a, iface: i.name })));
  const v4 = candidates.find((a) => !a.addr.includes(":"));
  const chosen = v4 ?? candidates.find((a) => !a.addr.startsWith("fe80"));
  return chosen ? chosen.addr : "";
}

/** 非回环网卡的地址列表，用于详情页 */
export function hostAddresses(host: HostInfo | undefined) {
  if (!host?.interfaces) return [];
  return host.interfaces
    .filter((i) => !i.loopback)
    .flatMap((i) =>
      i.addresses
        .filter((a) => !a.addr.startsWith("fe80")) // 过滤链路本地地址
        .map((a) => ({ iface: i.name, addr: a.addr, prefix: a.prefix })),
    );
}

// ---------- 容器与进程（来自 /v1/inventory，快照） ----------

export type ContainerState =
  | "running"
  | "exited"
  | "created"
  | "paused"
  | "restarting"
  | "dead"
  | string;

export interface ContainerInfo {
  id: string;
  name: string;
  image: string;
  state: ContainerState;
  status: string;
  runtime: string;
  created_at_unix_nano: number;
  /** 逐容器 inspect 得到；0 = 未知（节点 agent 未上报） */
  started_at_unix_nano?: number;
  finished_at_unix_nano?: number;
  /** compose 项目 / 服务（自动标签，「应用」维度由它推导）；非 compose 容器为空 */
  compose_project?: string;
  compose_service?: string;
  /** 内存用量（已扣 page cache）；0 = 未上报 / 未运行 */
  mem_usage_bytes?: number;
  /** 内存限额（docker HostConfig.Memory）；0 = 不限 */
  mem_limit_bytes?: number;
  /** CPU 占用百分比（相对全部核心）；0 = 未上报 / 未运行 */
  cpu_percent?: number;
  /** CPU 限额（纳秒，1e9 = 1 核）；0 = 不限 */
  cpu_limit_nano?: number;
}

/**
 * 容器「异常」——与后端 /v1/overview 的判定保持一致：
 * dead / restarting 算异常；exited 看退出码（`Exited (0)` 之外都算）；
 * running 但健康检查 unhealthy 也算。
 */
export function containerIsFailed(c: ContainerInfo): boolean {
  const state = c.state.toLowerCase();
  const status = c.status.toLowerCase();
  if (state === "dead" || state === "restarting") return true;
  if (state === "exited") return !status.startsWith("exited (0)");
  if (state === "running") return status.includes("unhealthy");
  return false;
}

/**
 * 容器分桶：卡片之间**互不重叠**，四桶相加 = 容器总数。
 * 异常优先（exited 非 0 的容器同时是「已停止」与「异常」，只计入异常），
 * 这样「已停止 + 异常」不会把同一台容器数两遍。
 */
export type ContainerBucket = "running" | "stopped" | "failed" | "other";

export function containerBucket(c: ContainerInfo): ContainerBucket {
  if (containerIsFailed(c)) return "failed";
  const state = c.state.toLowerCase();
  if (state === "running") return "running";
  if (state === "exited" || state === "created" || state === "paused") return "stopped";
  return "other";
}

/** 容器「上次更新」= 最近一次状态变化（inspect 的 StartedAt / FinishedAt 取大者，毫秒） */
export function containerLastChangeMs(c: ContainerInfo): number {
  return Math.max(
    Math.floor((c.finished_at_unix_nano ?? 0) / 1e6),
    Math.floor((c.started_at_unix_nano ?? 0) / 1e6),
  );
}

export interface ProcessInfo {
  pid: number;
  name: string;
  cmdline: string;
  user: string;
  cpu_percent: number;
  memory_bytes: number;
}

export interface ContainerGroup {
  node_id: string;
  hostname: string;
  /** 管理员别名（空串=未设置）；节点下拉优先显示它 */
  alias: string;
  ts_unix_nano: number | null;
  containers: ContainerInfo[];
}

export interface NodeProcessesView {
  node_id: string;
  ts_unix_nano: number | null;
  processes: ProcessInfo[];
}

export const containersApi = {
  all: () => request<ContainerGroup[]>("/v1/containers"),
  processes: (id: string) =>
    request<NodeProcessesView>(
      `/v1/nodes/${encodeURIComponent(id)}/processes`,
    ),
  byNode: (id: string) =>
    request<{ node_id: string; ts_unix_nano: number | null; containers: ContainerInfo[] }>(
      `/v1/nodes/${encodeURIComponent(id)}/containers`,
    ),
};

/** 容器状态 → Badge 语义色 */
export function containerTone(
  state: ContainerState,
): "success" | "warn" | "danger" | "neutral" {
  switch (state) {
    case "running":
      return "success";
    case "restarting":
    case "paused":
    case "created":
      return "warn";
    case "dead":
      return "danger";
    default:
      return "neutral";
  }
}

/** Docker 容器 ID 短写（前 12 位） */
export const shortId = (id: string) => id.slice(0, 12);

// ---------- 证书（来自 /v1/inventory 快照） ----------

export interface CertInfo {
  path: string;
  subject: string;
  issuer: string;
  not_after_unix_nano: number;
  not_before_unix_nano: number;
  domains: string[];
  serial: string;
  parse_error: boolean;
  /** 命中的证书路径来源 id；空 = 内置 / 命令行 glob 扫到的 */
  source_id?: string;
}

export interface NodeCertsGroup {
  node_id: string;
  hostname: string;
  ts_unix_nano: number | null;
  certificates: CertInfo[];
}

export const certsApi = {
  all: () => request<NodeCertsGroup[]>("/v1/certificates"),
};

// ---------- 证书路径（来源）配置 ----------

export interface CertSourceView {
  id: string;
  node_id: string;
  /** node_id 为空串：这条来源作用于所有节点 */
  all_nodes: boolean;
  node_hostname: string | null;
  path: string;
  enabled: boolean;
  notify_enabled: boolean;
  notify_days_before: number;
  created_at_unix_nano: number;
  updated_at_unix_nano: number;
  /** 最新快照里命中这条来源的证书数 */
  matched: number;
  /** 该节点最近一次快照时间 */
  snapshot_at_unix_nano: number | null;
  /** 命中证书里最紧急的剩余天数 */
  nearest_days_left: number | null;
}

export interface CertSourceInput {
  node_id: string;
  path: string;
  notify_enabled?: boolean;
  notify_days_before?: number;
}

export interface CertSourcePatch {
  /** 空串 = 改成「所有节点」 */
  node_id?: string;
  path?: string;
  enabled?: boolean;
  notify_enabled?: boolean;
  notify_days_before?: number;
}

/** 「测试」回执（节点侧 scan_certs 命令的 JSON 产物） */
export interface CertScanResult {
  path: string;
  patterns: string[];
  matched: number;
  certs: Array<{
    path: string;
    subject: string;
    issuer: string;
    domains: string[];
    not_after_unix_nano: number;
    parse_error: boolean;
    error: string;
  }>;
}

export const certSourcesApi = {
  list: () => request<CertSourceView[]>("/v1/cert-sources"),
  create: (body: CertSourceInput) =>
    request<{ id: string }>("/v1/cert-sources", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  patch: (id: string, body: CertSourcePatch) =>
    request<void>(`/v1/cert-sources/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  remove: (id: string) =>
    request<void>(`/v1/cert-sources/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  /** 让节点真扫一遍这条路径，返回 command_id（用 waitForCommand 等结果） */
  test: (nodeId: string, path: string) =>
    request<{ command_id: string }>("/v1/cert-sources/test", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ node_id: nodeId, path }),
    }),
};

/** 剩余天数（负数表示已过期） */
export function daysLeft(notAfterNs: number): number {
  return (notAfterNs / 1e6 - Date.now()) / 86_400_000;
}

/** 到期紧急度：过期 / 30 天内 / 正常 */
export function expiryTone(
  days: number,
): "danger" | "warn" | "success" | "neutral" {
  if (!Number.isFinite(days)) return "neutral";
  if (days < 0) return "danger";
  if (days < 30) return "warn";
  return "success";
}

// ---------- 告警 ----------

export interface Alert {
  id: number;
  rule_id: number;
  rule_name: string;
  node_id: string;
  hostname: string;
  severity: "warning" | "critical" | string;
  metric: string;
  op: string;
  threshold: number;
  value: number;
  message: string;
  started_at_unix_nano: number;
  resolved_at_unix_nano: number | null;
  silenced_until_unix_nano: number | null;
  /** rule = 指标规则告警；probe = 服务探针状态告警 */
  source: "rule" | "probe" | string;
  /** source = probe 时为 probe_id */
  source_ref: string;
}

export interface AlertsView {
  open: Alert[];
  resolved: Alert[];
}

export interface AlertRule {
  id: number;
  name: string;
  metric: string;
  op: "gt" | "gte" | "lt" | "lte" | "eq" | string;
  threshold: number;
  duration_seconds: number;
  severity: "warning" | "critical" | string;
  enabled: boolean;
  created_at_unix_nano: number;
  updated_at_unix_nano: number;
}

export interface NotifyChannel {
  id: number;
  name: string;
  /** `feishu` / `slack` / `webhook` */
  kind: string;
  /** Slack / 通用 webhook 的地址；飞书不用（地址由 receive_id 决定） */
  url: string;
  /** 按类型复用：飞书 = App Secret，通用 webhook = 投递 Token */
  secret: string;
  /** 飞书应用 App ID */
  app_id: string;
  /** 飞书接收 ID（群 chat_id / 用户 open_id 等） */
  receive_id: string;
  /** 飞书接收 ID 类型：chat_id / open_id / user_id / union_id / email */
  receive_id_type: string;
  enabled: boolean;
  min_severity: string;
}

export const alertsApi = {
  list: () => request<AlertsView>("/v1/alerts"),
  silence: (id: number, minutes = 60) =>
    request<unknown>(`/v1/alerts/${id}/silence`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ minutes }),
    }),
  rules: () => request<AlertRule[]>("/v1/rules"),
  createRule: (body: {
    name: string;
    metric: string;
    op: string;
    threshold: number;
    duration_seconds: number;
    severity: string;
  }) =>
    request<{ id: number }>("/v1/rules", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  toggleRule: (id: number, enabled: boolean) =>
    request<unknown>(`/v1/rules/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled }),
    }),
  deleteRule: (id: number) =>
    request<unknown>(`/v1/rules/${id}`, { method: "DELETE" }),
  channels: () => request<NotifyChannel[]>("/v1/channels"),
};

/** 数据留存策略——数字来自后端 retention.rs，前端不硬编码 */
export interface RetentionView {
  raw_days: number;
  hourly_days: number;
}

export const retentionApi = {
  get: () => request<RetentionView>("/v1/retention"),
};

/** 可选的告警指标（与 node-agent 采集的指标名一致） */
export const ALERT_METRICS = [
  "host.cpu.usage",
  "host.mem.usage",
  "host.disk.usage",
] as const;

export const OP_LABEL: Record<string, string> = {
  gt: ">",
  gte: "≥",
  lt: "<",
  lte: "≤",
  eq: "=",
};

// ---------- 控制通道（命令）----------

export interface CommandHistoryRow {
  id: string;
  node_id: string;
  node_hostname: string;
  action: string;
  params_json: string;
  state: string;
  issued_at_unix_nano: number;
  ttl_seconds: number;
  result_ok: boolean | null;
  result_error: string | null;
  /** 动作产物（日志文本之类），UTF-8 */
  result_text: string | null;
  result_received_at_unix_nano: number | null;
}

/**
 * 控制台发起命令。命令由 ops-server 私钥签名、节点验签后执行，
 * 这里只负责把动作与参数递进去，拿到 command_id 后轮询执行结果。
 */
export const commandsApi = {
  exec: (nodeId: string, action: string, params: Record<string, unknown> = {}) =>
    request<{ command_id: string }>("/v1/exec", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ node_id: nodeId, action, params }),
    }),
  history: () => request<CommandHistoryRow[]>("/v1/commands/history"),
  /** 单条命令的当前状态（等回执时用，比拉整段 history 轻得多） */
  get: (id: string) =>
    request<CommandHistoryRow>(`/v1/commands/${encodeURIComponent(id)}`),
};

/**
 * 轮询某条命令的执行结果。
 *
 * 只有 **终态**（done / failed）才算结束：`delivered` 只是「节点已取走」，
 * 慢动作（docker stop 要等 10s 信号超时）会长时间停在这一步，早退会把
 * 「还在跑」误判成「执行失败」。
 *
 * 节点改成挂起式长轮询后命令几乎是秒到，所以前几轮用 400ms 快速试探，
 * 之后逐步退避到 1.5s，最长等 60s（命令 TTL 也是 60s）。TTL 一过节点不会
 * 再执行，直接按「已下发，未见回执」返回 null，不谎报成功。
 */
export async function waitForCommand(
  commandId: string,
  timeoutMs = 60_000,
): Promise<CommandHistoryRow | null> {
  const deadline = Date.now() + timeoutMs;
  let interval = 400;
  for (;;) {
    const row = await commandsApi
      .get(commandId)
      .catch(() => null as CommandHistoryRow | null);
    if (row) {
      if (row.state === "done" || row.state === "failed") return row;
      const expired =
        Date.now() > row.issued_at_unix_nano / 1e6 + row.ttl_seconds * 1000;
      if (expired) return null;
    }
    if (Date.now() >= deadline) return null;
    await new Promise((r) => setTimeout(r, interval));
    interval = Math.min(Math.round(interval * 1.5), 1500);
  }
}

// ---------- CA / 设置 ----------

export interface CaView {
  subject: string;
  not_before_unix_nano: number;
  not_after_unix_nano: number;
  serial: string;
  fingerprint_sha256: string;
  nodes_enrolled: number;
  /** 本进程是否自己终结 TLS；false 表示这个 CA 在当前部署里用不到 */
  tls_terminated_locally: boolean;
}

export const settingsApi = {
  ca: () => request<CaView>("/v1/ca"),
  channels: () => request<NotifyChannel[]>("/v1/channels"),
  createChannel: (body: {
    name: string;
    kind?: string;
    /** Slack / 通用 webhook 的地址；飞书留空 */
    url?: string;
    /** 飞书 = App Secret，通用 webhook = 投递 Token（可选） */
    secret?: string;
    /** 飞书应用 App ID */
    app_id?: string;
    /** 飞书接收 ID（群 chat_id / 用户 open_id 等） */
    receive_id?: string;
    /** 飞书接收 ID 类型，缺省 chat_id */
    receive_id_type?: string;
    min_severity?: string;
  }) =>
    request<{ id: number }>("/v1/channels", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ kind: "webhook", ...body }),
    }),
  toggleChannel: (id: number, enabled: boolean) =>
    request<unknown>(`/v1/channels/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled }),
    }),
  deleteChannel: (id: number) =>
    request<unknown>(`/v1/channels/${id}`, { method: "DELETE" }),
  /** 拿当前填的参数真发一条测试通知（保存之前就能点） */
  testChannel: (body: {
    kind: string;
    url?: string;
    secret?: string;
    app_id?: string;
    receive_id?: string;
    receive_id_type?: string;
  }) =>
    request<{ ok: boolean; detail: string }>("/v1/channels/test", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  /** 改控制台凭据：要带当前凭据，新凭据至少 16 字符 */
  changeToken: (current: string, next: string) =>
    request<{ ok: boolean }>("/v1/admin/token", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ current, new: next }),
    }),
};

// ---------- 控制通道 ----------

export interface CommandHistoryItem {
  id: string;
  node_id: string;
  node_hostname: string;
  action: string;
  params_json: string;
  state: "pending" | "delivered" | "done" | "failed" | string;
  issued_at_unix_nano: number;
  ttl_seconds: number;
  result_ok: boolean | null;
  result_error: string | null;
  result_text: string | null;
  result_received_at_unix_nano: number | null;
}

export const controlApi = {
  /** 发起命令（monitor 转给 ops-server 签名） */
  exec: (nodeId: string, action: string, params: unknown) =>
    request<{ command_id: string }>("/v1/exec", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ node_id: nodeId, action, params }),
    }),
  history: () => request<CommandHistoryItem[]>("/v1/commands/history"),
};

// ---------- 待办（默认页） ----------

/** rule = 指标规则；probe = 服务探活；cert = 证书到期；node_offline = 节点离线 */
export type TodoSource = "rule" | "probe" | "cert" | "node_offline";

export interface TodoItem {
  id: string;
  source: TodoSource;
  severity: string;
  /** 告警类是规则名；节点离线是主机名 */
  title: string;
  detail: string;
  /** 下一步做什么，取 i18n 键 todo.hint.* */
  hint_key: string;
  node_id: string;
  hostname: string;
  since_unix_nano: number;
  resolved_at_unix_nano: number | null;
  link: string;
}

export interface TodoSummary {
  nodes_online: number;
  nodes_total: number;
  services_healthy: number;
  services_total: number;
  certs_total: number;
  containers_total: number;
  containers_failed: number;
}

export interface TodoView {
  generated_at_unix_nano: number;
  summary: TodoSummary;
  counts: { now: number; watch: number; recovered: number; silenced: number };
  now: TodoItem[];
  watch: TodoItem[];
  recovered: TodoItem[];
}

export const todoApi = {
  get: () => request<TodoView>("/v1/todo"),
};

// ---------- 服务健康度 ----------

/** ok / degraded / down，尚未检查过为 unknown */
export type ProbeStateName = "ok" | "degraded" | "down" | "unknown";

export interface ProbeStateView {
  probe_id: string;
  state: ProbeStateName;
  consecutive_failures: number;
  last_change_at_unix_nano: number;
  last_check_at_unix_nano: number;
  last_latency_ms: number | null;
  last_error: string;
}

export interface ProbeView {
  id: string;
  service_id: string;
  service_name: string;
  name: string;
  kind: "http" | "tcp" | "tls" | string;
  target_json: string;
  expect_json: string;
  interval_seconds: number;
  timeout_ms: number;
  failure_threshold: number;
  /** 绑定的执行节点；空数组 = 任意节点 */
  node_ids: string[];
  /** 绑定节点的展示名（别名优先，回落主机名），与 node_ids 同序 */
  node_labels: string[];
  location: string;
  enabled: boolean;
  created_at_unix_nano: number;
  updated_at_unix_nano: number;
  state: ProbeStateView;
}

export interface ServiceView {
  id: string;
  name: string;
  description: string;
  group_name: string;
  tier: number;
  enabled: boolean;
  created_at_unix_nano: number;
  updated_at_unix_nano: number;
  health: ProbeStateName;
  probes: ProbeView[];
}

export interface ProbeResultView {
  ts_unix_nano: number;
  state: ProbeStateName;
  latency_ms: number | null;
  status_code: number | null;
  error: string;
}

export interface ProbeInputBody {
  service_id: string;
  name: string;
  kind: string;
  target_json: string;
  expect_json: string;
  interval_seconds?: number;
  timeout_ms?: number;
  failure_threshold?: number;
  /** 空数组 = 任意节点 */
  node_ids?: string[];
  enabled?: boolean;
}

export const servicesApi = {
  list: () => request<ServiceView[]>("/v1/services"),
  create: (body: {
    name: string;
    description?: string;
    group_name?: string;
    tier?: number;
  }) =>
    request<ServiceView>("/v1/services", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  update: (
    id: string,
    body: Partial<{
      name: string;
      description: string;
      group_name: string;
      tier: number;
      enabled: boolean;
    }>,
  ) =>
    request<unknown>(`/v1/services/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  remove: (id: string) =>
    request<unknown>(`/v1/services/${id}`, { method: "DELETE" }),

  createProbe: (body: ProbeInputBody) =>
    request<ProbeView>("/v1/probes", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  updateProbe: (
    id: string,
    body: Partial<Omit<ProbeInputBody, "service_id">>,
  ) =>
    request<unknown>(`/v1/probes/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  removeProbe: (id: string) =>
    request<unknown>(`/v1/probes/${id}`, { method: "DELETE" }),
  results: (id: string, limit = 50) =>
    request<ProbeResultView[]>(`/v1/probes/${id}/results?limit=${limit}`),
  /** 新建 / 编辑探针时的一次性测试：由 monitor 立即执行一遍，不落库 */
  test: (body: {
    kind: string;
    target_json: string;
    expect_json: string;
    timeout_ms?: number;
  }) =>
    request<ProbeTestResult>("/v1/probes/test", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
};

/** 探针测试结果：state 与探针状态同名（ok / degraded / down） */
export interface ProbeTestResult {
  state: "ok" | "degraded" | "down";
  latency_ms: number | null;
  status_code: number | null;
  /** 机器可读的原因码，用来选文案（ok / timeout / connect / status / body / ...） */
  reason: string;
  /** 原因码的参数（如 needle、阈值、目标地址） */
  args: Record<string, unknown>;
}

// ---------- 入网令牌（运行时生成的临时 bootstrap 命令） ----------

/** 后端 BootstrapTokens::list_active 返回的元信息 */
export interface EnrollTokenMeta {
  id: string;
  label: string;
  created_at_unix: number;
  expires_at_unix: number;
  /** 来自 ZHIWEI_BOOTSTRAP_TOKEN（永不失效），与 UI 临时生成的区分 */
  permanent: boolean;
}

/** POST /v1/enroll-tokens 返回。多带 `enroll_command` 完整一行复制可用的命令。 */
export interface EnrollTokenCreated extends EnrollTokenMeta {
  /** 明文 token，形如 `zhi-bt-<hex>` */
  token: string;
  /** 后端从 X-Forwarded-Proto + Host 推断，UI 一般用不到 */
  monitor_url: string;
  /** 拼接好的整段安装命令 */
  enroll_command: string;
}

export const enrollTokens = {
  list: () =>
    request<{ tokens: EnrollTokenMeta[] }>("/v1/enroll-tokens"),
  create: (body: { ttl_secs?: number; label?: string }) =>
    request<EnrollTokenCreated>("/v1/enroll-tokens", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  revoke: (id: string) =>
    request<{ ok: boolean; id: string }>(
      `/v1/enroll-tokens/${encodeURIComponent(id)}`,
      { method: "DELETE" },
    ),
};

// ---------- AI Token（外部 agent 用，长期） ----------

export interface AiTokenMeta {
  id: string;
  name: string;
  created_at_unix_nano: number;
  last_used_at_unix_nano: number | null;
  revoked_at_unix_nano: number | null;
}

export const aiTokens = {
  list: () => request<{ tokens: AiTokenMeta[] }>("/v1/ai-tokens"),
  /** 注意：明文 token **只这一次**返回——展示后必须立即让用户复制下来 */
  create: (name: string) =>
    request<{
      id: string;
      name: string;
      token: string;
      created_at_unix_nano: number;
      warning: string;
    }>("/v1/ai-tokens", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    }),
  revoke: (id: string) =>
    request<{ ok: boolean; id: string }>(
      `/v1/ai-tokens/${encodeURIComponent(id)}`,
      { method: "DELETE" },
    ),
};

// ---------- 帮助页 markdown ----------

export const help = {
  /** locale 现在恒为 zh-CN（后端只准备了中文版），保留字段方便将来扩展 */
  fetch: () =>
    request<{ locale: string; body: string }>("/v1/help"),
};
