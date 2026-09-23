import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import dayjs from "dayjs";
import utc from "dayjs/plugin/utc";
import timezone from "dayjs/plugin/timezone";

dayjs.extend(utc);
dayjs.extend(timezone);

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** 规范 05：时区选项（6 个） */
/**
 * 规范 05 的 6 个时区。
 *
 * `key` 是语言包里的键（`tz.<key>`），**标签不能硬编码中文**——
 * 切到英文时时区列表也得跟着变。
 */
export const TIMEZONES = [
  { key: "beijing", tz: "Asia/Shanghai" },
  { key: "tokyo", tz: "Asia/Tokyo" },
  { key: "singapore", tz: "Asia/Singapore" },
  { key: "london", tz: "Europe/London" },
  { key: "newYork", tz: "America/New_York" },
  { key: "losAngeles", tz: "America/Los_Angeles" },
] as const;

/** 规范 05：时间统一 YYYY-MM-DD HH:mm，24 小时制，必须带时区 */
export function formatTime(msOrDate: number | Date, tz: string): string {
  return dayjs(msOrDate).tz(tz).format("YYYY-MM-DD HH:mm");
}

export function formatClock(msOrDate: number | Date, tz: string): string {
  return dayjs(msOrDate).tz(tz).format("HH:mm:ss");
}

/** 相对时间（用于「x 分钟前」），基于真实时刻差值，与时区无关 */
export function relativeTime(
  ms: number,
  t: (k: string, o?: Record<string, unknown>) => string,
): string {
  const diff = Math.max(0, Date.now() - ms);
  const s = Math.floor(diff / 1000);
  if (s < 60) return t("time.secondsAgo", { n: s });
  const m = Math.floor(s / 60);
  if (m < 60) return t("time.minutesAgo", { n: m });
  const h = Math.floor(m / 60);
  if (h < 24) return t("time.hoursAgo", { n: h });
  return t("time.daysAgo", { n: Math.floor(h / 24) });
}

export function formatPercent(n: number, digits = 1): string {
  if (!Number.isFinite(n)) return "—";
  return `${n.toFixed(digits)}%`;
}

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.min(
    Math.floor(Math.log(n) / Math.log(1024)),
    units.length - 1,
  );
  const v = n / Math.pow(1024, i);
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

/**
 * CPU 限额折算成核数。1e9 nano = 1 核；0 表示不限（docker 的 `--cpus 0`）。
 */
export function formatCpuLimit(nano: number): string {
  if (!Number.isFinite(nano) || nano <= 0) return "—";
  const cores = nano / 1e9;
  return cores >= 10 ? cores.toFixed(0) : cores.toFixed(2);
}

/**
 * 「用量 / 限额」配对展示，任一侧缺失都不留空：limit = 0（不限）时只给用量。
 */
export function formatUsagePair(
  usage: number | undefined,
  limit: number | undefined,
  format: (n: number) => string = formatBytes,
): string {
  const hasUsage = Number.isFinite(usage) && (usage ?? 0) > 0;
  const hasLimit = Number.isFinite(limit) && (limit ?? 0) > 0;
  if (!hasUsage && !hasLimit) return "—";
  if (!hasLimit) return format(usage ?? 0);
  if (!hasUsage) return `0 / ${format(limit ?? 0)}`;
  return `${format(usage ?? 0)} / ${format(limit ?? 0)}`;
}

/** 速率（bytes/s）：`1.2 MB/s` */
export function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond)) return "—";
  return `${formatBytes(bytesPerSecond)}/s`;
}

/**
 * 运行时长：天 + 小时（不足一天给「小时 + 分」），禁止裸显秒数。
 *
 * 两个档位都必须走语言包：以前不足一天那档硬编码成 `21h 36m`，于是同一个
 * 页面上「100 天 9 小时」和「21h 36m」两种语言并排出现（切到英文时反过来）。
 */
export function formatUptime(
  seconds: number | undefined | null,
  t: (k: string, o?: Record<string, unknown>) => string,
): string {
  if (!seconds || seconds <= 0) return "—";
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  if (d > 0) return t("detail.uptimeFormat", { d, h });
  const m = Math.floor((seconds % 3600) / 60);
  return t("detail.uptimeFormatHours", { h, m });
}

type TranslateFn = (k: string, o?: Record<string, unknown>) => string;

/** 解析 `vX.Y.Z`（允许缺省段与后缀），解析不了返回 null */
function parseVersion(v: string | undefined): [number, number, number] | null {
  const m = /^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?/.exec((v ?? "").trim());
  return m ? [Number(m[1]), Number(m[2] ?? 0), Number(m[3] ?? 0)] : null;
}

/**
 * 节点 Agent 是否比控制台旧。
 *
 * 版本对不上是「容器用量列空白」「启停/日志点了没反应」的头号原因——节点上跑的
 * 还是旧二进制。解析不了就当作不旧，宁可不提示也不误报。
 */
export function isAgentOlder(
  agent: string | undefined,
  consoleVersion: string,
): boolean {
  const a = parseVersion(agent);
  const c = parseVersion(consoleVersion);
  if (!a || !c) return false;
  for (let i = 0; i < 3; i++) {
    if (a[i] !== c[i]) return a[i] < c[i];
  }
  return false;
}

/**
 * 容器状态徽章的本地化。未知状态原样显示（绝不会把 `state.foo` 这种键漏到界面上）。
 */
export function containerStateLabel(state: string, t: TranslateFn): string {
  return t(`state.${state.toLowerCase()}`, { defaultValue: state });
}

/**
 * 容器「运行状态」行的本地化。
 *
 * Docker 给的是英文句子（`Up 2 hours (healthy)` / `Exited (0) 3 hours ago`），
 * 中文界面直接展示很割裂。这里用我们自己的 inspect 时间戳重算时长，再拼上
 * 健康检查与退出码——信息不丢，语言跟着界面走；原始字符串由调用方放进
 * `title`，需要逐字核对时仍能看到。
 */
export function containerStatusLabel(
  c: {
    state: string;
    status: string;
    started_at_unix_nano?: number;
    finished_at_unix_nano?: number;
  },
  t: TranslateFn,
): string {
  const raw = c.status ?? "";
  const lower = raw.toLowerCase();
  const state = c.state.toLowerCase();
  const health = lower.includes("unhealthy")
    ? t("containers.healthUnhealthy")
    : lower.includes("health: starting")
      ? t("containers.healthStarting")
      : lower.includes("(healthy)")
        ? t("containers.healthHealthy")
        : "";
  const withHealth = (text: string) => (health ? `${text} · ${health}` : text);

  if (state === "running") {
    const up = formatUptime(
      c.started_at_unix_nano
        ? Math.floor(Date.now() / 1000 - c.started_at_unix_nano / 1e9)
        : null,
      t,
    );
    return up === "—"
      ? withHealth(t(`state.${state}`))
      : withHealth(t("containers.statusUp", { uptime: up }));
  }
  if (state === "exited") {
    const code = /exited \((\d+)\)/.exec(lower)?.[1];
    const text = code
      ? t("containers.statusExitedCode", { code })
      : t(`state.${state}`);
    const ago = c.finished_at_unix_nano
      ? relativeTime(Math.floor(c.finished_at_unix_nano / 1e6), t)
      : "";
    return ago ? `${text} · ${ago}` : text;
  }
  if (state === "restarting") return t("containers.statusRestarting");
  return raw || t(`state.${state}`);
}

/**
 * 密钥 / 令牌的密文展示：只留头 8 位与尾 4 位。
 * 完整值不落在 DOM 里（复制按钮需要时才取原值），避免随手截图泄密。
 */
export function maskSecret(value: string | undefined | null): string {
  if (!value) return "—";
  if (value.length <= 16) return "……";
  return `${value.slice(0, 8)}……${value.slice(-4)}`;
}

/** 规范 10 / 08：禁止向用户裸显 HTTP 状态码或原始异常，统一友好文案 */
export function friendlyError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err ?? "");
  if (/401|unauthor|未授权/i.test(raw)) return "err.unauthorized";
  // 后端自己写好的中文提示（如「ops-server 不可用：连不上 …」）比这里的任何
  // 映射都准，先原样放行：传输层那条正则连「超时 / 连接」都会命中，
  // 一旦被吃掉，用户就只剩一句没用的「网络错误」。
  if (/[\u4e00-\u9fa5]/.test(raw)) return raw;
  if (/network|fetch|ECONN|timeout|超时/i.test(raw)) return "err.network";
  // 裸状态码（http_502 / 502 Bad Gateway）不直接给用户看
  if (/^http_\d+$/.test(raw) || /^\d{3}\b/.test(raw)) {
    return /^http_4/.test(raw) ? "err.generic" : "err.server";
  }
  // 服务端给的是一句人话（如「kill_process 需要 pid」「signal 只能是 term 或 kill」），
  // 直接用它——比「出错了」有用；异常栈与状态码仍然不外露。
  if (raw.trim()) return raw;
  return "err.generic";
}

/** 节点存活判定：在线 / 滞后 / 离线 */
export type Liveness = "online" | "lagging" | "offline" | "unknown";

export function livenessOf(lastSeenMs: number | null | undefined): Liveness {
  if (!lastSeenMs) return "unknown";
  const age = Date.now() - lastSeenMs;
  if (age < 60_000) return "online";
  if (age < 5 * 60_000) return "lagging";
  return "offline";
}

/**
 * 节点显示名：管理员别名优先，没有就用主机名。
 *
 * 下拉框、列表标题、日志页选节点统一走这里——别名本来就是给「一眼认出来」
 * 用的，单独一处漏掉就会出现同一个节点两个名字。
 */
export function nodeLabel(
  n: { alias?: string; hostname?: string } | null | undefined,
): string {
  if (!n) return "";
  const alias = (n.alias ?? "").trim();
  return alias || (n.hostname ?? "");
}

/**
 * 复制到剪贴板，成功返回 true。
 *
 * 先走异步 Clipboard API，不可用或失败时退回隐藏 textarea + `execCommand`。
 * 退路不是怀旧：`navigator.clipboard` 只在**安全上下文**（https / localhost）
 * 里存在，而自建部署常是 `http://<内网 IP>:8443` 打开控制台，那里它是
 * `undefined`——「点击即复制」会静默失效，用户只会看到「复制失败」。
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // 继续走退路
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}
