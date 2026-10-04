/**
 * API client that talks to monitor.
 *
 * The console uses `Authorization: Bearer <admin token>`; nodes use Ed25519 request signatures.
 * The two credentials don't affect each other.
 *
 * Backend timestamps are Unix nanoseconds; here they're uniformly converted to milliseconds,
 * and the UI layer only handles milliseconds.
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
    // Protected endpoint 401: clear the token, let App.tsx return to the login page.
    // `/v1` itself always returns 200 (distinguishes via body.authenticated),
    // so this event won't be triggered by it.
    window.dispatchEvent(new CustomEvent("zhiwei:unauthorized"));
    throw new ApiError(401, "unauthorized");
  }
  if (!res.ok) {
    // Server gives a human-readable reason (e.g. "signal must be term or kill"), prefer it;
    // only fall back to status code if unavailable, which friendlyError translates to friendly text
    const text = await res.text().catch(() => "");
    let message = `http_${res.status}`;
    try {
      const body = JSON.parse(text) as { error?: unknown };
      if (typeof body?.error === "string" && body.error.trim()) {
        message = body.error.trim();
      }
    } catch {
      // Non-JSON response body: keep the status code
    }
    throw new ApiError(res.status, message);
  }
  // 204 and empty response bodies (e.g. DELETE / PATCH) have no JSON
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

/** Node host basic info (see crates/node-agent's build_host_info) */
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
  /** Unix seconds of the current node-agent process start; 0 or undefined = old agent / unknown */
  agent_started_at_unix_seconds?: number;
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
  command_channel?: RawCommandChannel;
}

/** Command channel state: when a node is online but doesn't pull commands, start/stop/log are dead buttons */
interface RawCommandChannel {
  state: string;
  last_poll_age_ms: number | null;
}

/** Key metrics in the latest frame (list page CPU / memory / disk columns use these directly, avoiding one request per node) */
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
  /** Admin-given short alias (≤10 chars), empty string means unset */
  alias: string;
  /** Admin-given tags (≤10) */
  tags: string[];
  enrolled_at_ms: number;
  last_seen_ms: number | null;
  host_info: HostInfo;
  latest: NodeLatest | null;
  /** Node identity public key (base64); displayed masked as "secret" on the UI */
  public_key: string;
  /** Command channel state; older monitors don't include this field, treat as unknown */
  command_channel: CommandChannel;
}

/**
 * Command channel state (see backend crates/monitor-server/src/control_channel.rs).
 * `down` = node is online but never pulls commands; its start/stop/restart/log all fail.
 */
export interface CommandChannel {
  state: "ok" | "down" | "unknown";
  /** Milliseconds since last command pull; null if this process has never seen it pull */
  last_poll_age_ms: number | null;
}

export interface IndexBody {
  service: string;
  version: string;
  authenticated: boolean;
  endpoints: string[];
  nodes: number;
  telemetry_batches: number;
  /** monitor process start time (= latest deploy / restart); header's "deployed at …" uses this */
  started_at_unix_nano: number;
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
  /** raw = raw 10-second data; hourly = hourly aggregate (window start before raw retention) */
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
      command_channel: {
        // Older monitors don't return this field: treat as "unknown", don't misreport a fault
        state: (n.command_channel?.state as CommandChannel["state"]) ?? "unknown",
        last_poll_age_ms: n.command_channel?.last_poll_age_ms ?? null,
      },
    }));
  },

  /** Update admin-maintained alias / tags (only pass fields to change) */
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

  /** Permanent node deletion: clears all telemetry / inventory / probe results / alerts.
   * Backend returns 204 on success; this endpoint only cares whether an error was thrown.
   *
   * Backend returns 409 when there are "unsent and unexpired" commands (default TTL is only 60s,
   * but the node may have been reinstalled / command channel is down, so these commands
   * never get pulled). force=true tells backend to invalidate them first (outcome=cancelled
   * is recorded in audit_log) before deletion. */
  deleteNode: (id: string, force = false) =>
    request<void>(`/v1/nodes/${encodeURIComponent(id)}${force ? "?force=1" : ""}`, {
      method: "DELETE",
    }),

  telemetry: (id: string, limit = 20) =>
    request<NodeTelemetryView>(
      `/v1/nodes/${encodeURIComponent(id)}/telemetry?limit=${limit}`,
    ),

  /** Trend chart data: `from`/`to` are millisecond time windows; `rate` does per-second differencing for cumulative metrics */
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

// ---------- Aggregated trends across nodes / services ----------

/** Trend for all nodes, same metric: one line per node */
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

/** Service health timeline: percentage of probes that are "ok" in each bucket */
export interface ServicesTimelineView {
  from_ms: number;
  to_ms: number;
  bucket_ms: number;
  level: "service" | "probe";
  /** service granularity: one line per service; probe granularity: one line per probe (name has service name prefix) */
  series: Array<{
    id: string;
    name: string;
    group: string | null;
    points: Array<{ t: number; v: number }>;
  }>;
}

/** Unified data source for cross-object trends */
export const trendApi = {
  allNodes: (metric: string, opts: SeriesQuery = {}) => {
    const params = new URLSearchParams({
      metric,
      limit: String(opts.limit ?? 300),
    });
    if (opts.from) params.set("from", String(Math.round(opts.from)));
    if (opts.to) params.set("to", String(Math.round(opts.to)));
    if (opts.rate) params.set("rate", "1");
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

/** Metric names currently collected in this project (see crates/node-agent's build_batch). */
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

/** Series conversions used in charts */
export const toPairs = (points: Array<{ t: number; v: number }>) =>
  points.map((p) => [p.t, p.v] as [number, number]);

export function getToken(): string | null {
  const t = getAdminToken();
  return t ? t : null;
}

export function setToken(token: string): void {
  localStorage.setItem("zhiwei.adminToken", token);
}

/** App.tsx calls this after receiving the `zhiwei:unauthorized` event, clearing the locally cached credentials */
export { clearAdminToken };

/** Display label for the system: prefer sysinfo's long version, fall back to name + version */
export function osLabel(host: HostInfo | undefined): string {
  if (!host) return "";
  const long = host.long_os_version?.trim();
  if (long) return long;
  const name = host.os_name?.trim() ?? "";
  const ver = host.os_version?.trim() ?? "";
  return `${name} ${ver}`.trim();
}

/** Primary IP: prefer non-loopback IPv4, then any non-loopback address */
export function primaryIp(host: HostInfo | undefined): string {
  if (!host?.interfaces) return "";
  const candidates = host.interfaces
    .filter((i) => !i.loopback)
    .flatMap((i) => i.addresses.map((a) => ({ ...a, iface: i.name })));
  const v4 = candidates.find((a) => !a.addr.includes(":"));
  const chosen = v4 ?? candidates.find((a) => !a.addr.startsWith("fe80"));
  return chosen ? chosen.addr : "";
}

/** Non-loopback network interface addresses, for the detail page */
export function hostAddresses(host: HostInfo | undefined) {
  if (!host?.interfaces) return [];
  return host.interfaces
    .filter((i) => !i.loopback)
    .flatMap((i) =>
      i.addresses
        .filter((a) => !a.addr.startsWith("fe80")) // Filter link-local addresses
        .map((a) => ({ iface: i.name, addr: a.addr, prefix: a.prefix })),
    );
}

// ---------- Containers and processes (from /v1/inventory, snapshot) ----------

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
  /** From per-container inspect; 0 = unknown (node agent didn't report) */
  started_at_unix_nano?: number;
  finished_at_unix_nano?: number;
  /** compose project / service (auto-tag, "application" dimension derived from it); non-compose containers have this empty */
  compose_project?: string;
  compose_service?: string;
  /** Memory usage (page cache deducted); 0 = not reported / not running */
  mem_usage_bytes?: number;
  /** Memory limit (docker HostConfig.Memory); 0 = unlimited */
  mem_limit_bytes?: number;
  /** CPU usage percentage (relative to all cores); 0 = not reported / not running */
  cpu_percent?: number;
  /** CPU limit (nanoseconds, 1e9 = 1 core); 0 = unlimited */
  cpu_limit_nano?: number;
}

/**
 * Container "failed" — matches the backend's /v1/overview logic:
 * dead / restarting count as failed; exited checks the exit code (anything other than `Exited (0)` counts);
 * running but unhealthy health check also counts.
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
 * Container bucketing: cards are **non-overlapping**, the four buckets sum to total containers.
 * "Failed" takes precedence (a container with exited non-0 is both "stopped" and "failed", only counted in failed),
 * so "stopped + failed" doesn't double-count the same container.
 */
export type ContainerBucket = "running" | "stopped" | "failed" | "other";

export function containerBucket(c: ContainerInfo): ContainerBucket {
  if (containerIsFailed(c)) return "failed";
  const state = c.state.toLowerCase();
  if (state === "running") return "running";
  if (state === "exited" || state === "created" || state === "paused") return "stopped";
  return "other";
}

/** Container "last update" = most recent state change (max of inspect's StartedAt / FinishedAt, in milliseconds) */
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
  /** Admin alias (empty string = unset); node dropdowns prefer it */
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

/** Container state → Badge semantic color */
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

/** Short form of Docker container ID (first 12 chars) */
export const shortId = (id: string) => id.slice(0, 12);

// ---------- Certificates (from /v1/inventory snapshot) ----------

export interface CertInfo {
  path: string;
  subject: string;
  issuer: string;
  not_after_unix_nano: number;
  not_before_unix_nano: number;
  domains: string[];
  serial: string;
  parse_error: boolean;
  /** Matched certificate source id; empty = built-in / scanned from command-line glob */
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

// ---------- Certificate path (source) configuration ----------

export interface CertSourceView {
  id: string;
  node_id: string;
  /** node_id is empty string: this source applies to all nodes */
  all_nodes: boolean;
  node_hostname: string | null;
  path: string;
  enabled: boolean;
  notify_enabled: boolean;
  notify_days_before: number;
  created_at_unix_nano: number;
  updated_at_unix_nano: number;
  /** Number of certificates matching this source in the latest snapshot */
  matched: number;
  /** Most recent snapshot time for this node */
  snapshot_at_unix_nano: number | null;
  /** Most urgent days-left among matched certificates */
  nearest_days_left: number | null;
}

export interface CertSourceInput {
  node_id: string;
  path: string;
  notify_enabled?: boolean;
  notify_days_before?: number;
}

export interface CertSourcePatch {
  /** Empty string = change to "all nodes" */
  node_id?: string;
  path?: string;
  enabled?: boolean;
  notify_enabled?: boolean;
  notify_days_before?: number;
}

/** "Test" receipt (JSON output from the node-side scan_certs command) */
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
  /** Let the node actually scan this path, returns command_id (use waitForCommand for the result) */
  test: (nodeId: string, path: string) =>
    request<{ command_id: string }>("/v1/cert-sources/test", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ node_id: nodeId, path }),
    }),
};

/** Days left (negative means already expired) */
export function daysLeft(notAfterNs: number): number {
  return (notAfterNs / 1e6 - Date.now()) / 86_400_000;
}

/** Expiry urgency: expired / within 30 days / normal */
export function expiryTone(
  days: number,
): "danger" | "warn" | "success" | "neutral" {
  if (!Number.isFinite(days)) return "neutral";
  if (days < 0) return "danger";
  if (days < 30) return "warn";
  return "success";
}

// ---------- Alerts ----------

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
  /** rule = metric rule alert; probe = service probe state alert */
  source: "rule" | "probe" | string;
  /** when source = probe, this is probe_id */
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
  /** `feishu` / `slack` / `bluebird` / `webhook` */
  kind: string;
  /** URL for Slack / bluebird generic source / generic webhook; not used for feishu (URL is determined by receive_id) */
  url: string;
  /** Reused by type: feishu = App Secret, bluebird / generic webhook = delivery Token */
  secret: string;
  /** Feishu app App ID */
  app_id: string;
  /** Feishu receive ID (group chat_id / user open_id etc.) */
  receive_id: string;
  /** Feishu receive ID type: chat_id / open_id / user_id / union_id / email */
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
  resolve: (id: number) =>
    request<unknown>(`/v1/alerts/${id}/resolve`, { method: "POST" }),
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
  updateRule: (id: number, body: Partial<{
    name: string;
    metric: string;
    op: string;
    threshold: number;
    duration_seconds: number;
    severity: string;
    enabled: boolean;
  }>) =>
    request<unknown>(`/v1/rules/${id}`, {
      method: "PATCH",
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

/** Builtin alert rule: node up/down, service probe, container, certificate events. Supports editing threshold and duration. */
export interface BuiltinAlertRule {
  id: string;
  name: string;
  enabled: boolean;
  threshold: number;
  duration_seconds: number;
  updated_at_unix_nano: number;
}

export const builtinAlertsApi = {
  list: () => request<BuiltinAlertRule[]>("/v1/builtin-alerts"),
  update: (id: string, updates: { enabled?: boolean; threshold?: number; duration_seconds?: number }) =>
    request<unknown>(`/v1/builtin-alerts/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(updates),
    }),
};

/** Data retention policy — numbers come from backend retention.rs, no hard-coding on the frontend */
export interface RetentionView {
  raw_days: number;
  hourly_days: number;
}

export const retentionApi = {
  get: () => request<RetentionView>("/v1/retention"),
};

/** Selectable alert metrics (matches the metric names collected by node-agent) */
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

// ---------- Control channel (commands) ----------

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
  /** Action output (log text etc.), UTF-8 */
  result_text: string | null;
  result_received_at_unix_nano: number | null;
}

/**
 * The console issues commands. Commands are signed with ops-server's private key
 * and verified by the node before execution; this only forwards the action and params,
 * then polls the execution result via command_id.
 */
export const commandsApi = {
  exec: (nodeId: string, action: string, params: Record<string, unknown> = {}) =>
    request<{ command_id: string }>("/v1/exec", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ node_id: nodeId, action, params }),
    }),
  history: () => request<CommandHistoryRow[]>("/v1/commands/history"),
  /** Current state of a single command (used when waiting for receipt, lighter than pulling entire history) */
  get: (id: string) =>
    request<CommandHistoryRow>(`/v1/commands/${encodeURIComponent(id)}`),
};

/**
 * Poll a command's execution result.
 *
 * Only **terminal states** (done / failed) count as finished: `delivered` only means "node picked it up";
 * slow actions (docker stop waits 10s for the signal timeout) will sit at this step for a while,
 * and bailing early would mistake "still running" for "execution failed".
 *
 * After nodes switched to hung long-polling, commands arrive in ~1s, so the first few rounds use 400ms
 * to probe quickly, then back off to 1.5s, max wait 60s (command TTL is also 60s). Once TTL passes
 * the node won't execute it anymore; return null as "dispatched, no receipt" rather than lie about success.
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

// ---------- CA / Settings ----------

export interface CaView {
  subject: string;
  not_before_unix_nano: number;
  not_after_unix_nano: number;
  serial: string;
  fingerprint_sha256: string;
  nodes_enrolled: number;
  /** Whether this process itself terminates TLS; false means this CA isn't used in the current deployment */
  tls_terminated_locally: boolean;
}

export const settingsApi = {
  ca: () => request<CaView>("/v1/ca"),
  channels: () => request<NotifyChannel[]>("/v1/channels"),
  createChannel: (body: {
    name: string;
    kind?: string;
    /** Slack / generic webhook URL; feishu leaves this empty */
    url?: string;
    /** feishu = App Secret, generic webhook = delivery Token (optional) */
    secret?: string;
    /** Feishu app App ID */
    app_id?: string;
    /** Feishu receive ID (group chat_id / user open_id etc.) */
    receive_id?: string;
    /** Feishu receive ID type, defaults to chat_id */
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
  /** Edit channel: omitted field = keep current value; secret passed as empty string also keeps the value (credentials can only be overwritten) */
  updateChannel: (
    id: number,
    body: {
      name?: string;
      url?: string;
      secret?: string;
      app_id?: string;
      receive_id?: string;
      receive_id_type?: string;
      min_severity?: string;
      enabled?: boolean;
    },
  ) =>
    request<unknown>(`/v1/channels/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  deleteChannel: (id: number) =>
    request<unknown>(`/v1/channels/${id}`, { method: "DELETE" }),
  /** Send a real test notification with the current parameters (can be triggered before saving) */
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
  /** Change console credentials: needs current credential, new credential must be at least 16 chars */
  changeToken: (current: string, next: string) =>
    request<{ ok: boolean }>("/v1/admin/token", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ current, new: next }),
    }),
};

// ---------- Control channel ----------

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
  /** Issue command (monitor forwards to ops-server for signing) */
  exec: (nodeId: string, action: string, params: unknown) =>
    request<{ command_id: string }>("/v1/exec", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ node_id: nodeId, action, params }),
    }),
  history: () => request<CommandHistoryItem[]>("/v1/commands/history"),
};

// ---------- Todo (default page) ----------

/** rule = metric rule; probe = service probe; cert = certificate expiry; node_offline = node offline */
export type TodoSource =
  | "rule"
  | "probe"
  | "cert"
  | "node_offline"
  | "command_channel";

export interface TodoItem {
  id: string;
  source: TodoSource;
  severity: string;
  /** Alert category is the rule name; node offline is the hostname */
  title: string;
  detail: string;
  /** What to do next, takes i18n key todo.hint.* */
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
  /** Number of healthy probes (per probe dimension) */
  probes_healthy: number;
  /** Total probes */
  probes_total: number;
  certs_total: number;
  containers_total: number;
  containers_failed: number;
}

export interface TodoPagination {
  page: number;
  page_size: number;
}

export interface TodoView {
  generated_at_unix_nano: number;
  summary: TodoSummary;
  counts: { now: number; watch: number; recovered: number; silenced: number };
  pagination?: TodoPagination;
  now: TodoItem[];
  watch: TodoItem[];
  recovered: TodoItem[];
}

export interface TodoQuery {
  page?: number;
  page_size?: number;
}

export const todoApi = {
  get: (query?: TodoQuery) => {
    const params = new URLSearchParams();
    if (query?.page !== undefined) params.set("page", String(query.page));
    if (query?.page_size !== undefined) params.set("page_size", String(query.page_size));
    const qs = params.toString();
    return request<TodoView>(`/v1/todo${qs ? `?${qs}` : ""}`);
  },
};

// ---------- Service health ----------

/** ok / degraded / down, never checked is unknown */
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
  /** Bound execution nodes; empty array = any node */
  node_ids: string[];
  /** Display name for bound nodes (alias preferred, falls back to hostname), same order as node_ids */
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

export interface ProbeInputBody {
  service_id: string;
  name: string;
  kind: string;
  target_json: string;
  expect_json: string;
  interval_seconds?: number;
  timeout_ms?: number;
  failure_threshold?: number;
  /** Empty array = any node */
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
  /** One-shot test when creating / editing a probe: monitor executes it immediately, not persisted */
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

/** Probe test result: state uses same names as probe state (ok / degraded / down) */
export interface ProbeTestResult {
  state: "ok" | "degraded" | "down";
  latency_ms: number | null;
  status_code: number | null;
  /** Machine-readable reason code, used to pick the text (ok / timeout / connect / status / body / ...) */
  reason: string;
  /** Arguments for the reason code (e.g. needle, threshold, target address) */
  args: Record<string, unknown>;
}

// ---------- Enrollment tokens (runtime-generated temporary bootstrap commands) ----------

/** Metadata returned by backend's BootstrapTokens::list_active */
export interface EnrollTokenMeta {
  id: string;
  label: string;
  created_at_unix: number;
  expires_at_unix: number;
  /** From ZHIWEI_BOOTSTRAP_TOKEN (never expires), distinguished from UI-generated tokens */
  permanent: boolean;
}

/** Returned by POST /v1/enroll-tokens. Also includes `enroll_command` as a complete copy-pasteable line. */
export interface EnrollTokenCreated extends EnrollTokenMeta {
  /** Plaintext token, shaped like `zhi-bt-<hex>` */
  token: string;
  /** Backend infers from X-Forwarded-Proto + Host, UI generally doesn't need it */
  monitor_url: string;
  /** Concatenated full install command */
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

// ---------- AI Token (for external agents, long-term) ----------

export interface AiTokenMeta {
  id: string;
  name: string;
  created_at_unix_nano: number;
  last_used_at_unix_nano: number | null;
  revoked_at_unix_nano: number | null;
}

export const aiTokens = {
  list: () => request<{ tokens: AiTokenMeta[] }>("/v1/ai-tokens"),
  /** Note: plaintext token is returned **only this one time** — user must copy it immediately after display */
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

// ---------- Help page markdown ----------

export const help = {
  /** Fetch help page markdown for the given locale; falls back to en-US. */
  fetch: (locale: string) =>
    request<{ locale: string; body: string }>(
      `/v1/help?locale=${encodeURIComponent(locale)}`,
    ),
};
