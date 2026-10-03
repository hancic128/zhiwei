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

/** Spec 05: timezone options (6) */
/**
 * The 6 timezones from Spec 05.
 *
 * `key` is the key in the locale pack (`tz.<key>`); **labels cannot be hardcoded** —
 * switching to English must also change the timezone list.
 */
export const TIMEZONES = [
  { key: "beijing", tz: "Asia/Shanghai" },
  { key: "tokyo", tz: "Asia/Tokyo" },
  { key: "singapore", tz: "Asia/Singapore" },
  { key: "london", tz: "Europe/London" },
  { key: "newYork", tz: "America/New_York" },
  { key: "losAngeles", tz: "America/Los_Angeles" },
  { key: "utc", tz: "UTC" },
] as const;

/** Spec 05: time uniformly YYYY-MM-DD HH:mm, 24-hour format, must include timezone */
export function formatTime(msOrDate: number | Date, tz: string): string {
  return dayjs(msOrDate).tz(tz).format("YYYY-MM-DD HH:mm");
}

export function formatClock(msOrDate: number | Date, tz: string): string {
  return dayjs(msOrDate).tz(tz).format("HH:mm:ss");
}

/** Relative time (used for "x minutes ago"), based on actual moment differences, independent of timezone */
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
 * CPU limit converted to cores. 1e9 nano = 1 core; 0 means unlimited (docker's `--cpus 0`).
 */
export function formatCpuLimit(nano: number): string {
  if (!Number.isFinite(nano) || nano <= 0) return "—";
  const cores = nano / 1e9;
  return cores >= 10 ? cores.toFixed(0) : cores.toFixed(2);
}

/**
 * "Usage / Limit" paired display, never blank when one side is missing: when limit = 0 (unlimited), show only usage.
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

/** Rate (bytes/s): `1.2 MB/s` */
export function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond)) return "—";
  return `${formatBytes(bytesPerSecond)}/s`;
}

/**
 * Uptime: days + hours (under a day gives "hours + minutes"), never show raw seconds.
 *
 * Both formats must go through the locale pack: previously the under-a-day case was hardcoded as `21h 36m`,
 * so the same page showed "100 days 9 hours" and "21h 36m" in two languages side by side
 * (and the reverse when switching to English).
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

/** Parse `vX.Y.Z` (allows missing segments and suffixes), returns null on failure */
function parseVersion(v: string | undefined): [number, number, number] | null {
  const m = /^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?/.exec((v ?? "").trim());
  return m ? [Number(m[1]), Number(m[2] ?? 0), Number(m[3] ?? 0)] : null;
}

/**
 * Whether the node Agent is older than the console.
 *
 * Version mismatch is the #1 cause of "container usage column blank" and "start/stop/log click has no response" —
 * the node is still running an old binary. If parsing fails, treat as not older — better to skip the hint
 * than to misreport.
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
 * Localization for container state badges. Unknown states display as-is
 * (never lets a `state.foo` key leak into the UI).
 */
export function containerStateLabel(state: string, t: TranslateFn): string {
  return t(`state.${state.toLowerCase()}`, { defaultValue: state });
}

/**
 * Localization for the container "running state" row.
 *
 * Docker returns English sentences (`Up 2 hours (healthy)` / `Exited (0) 3 hours ago`),
 * which feels jarring on a non-English interface. Here we recalculate duration from our own inspect timestamps,
 * then append health check and exit code — no information loss, language follows the UI;
 * the raw string is placed in `title` by the caller so it remains visible for verbatim verification.
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
 * Masked display of keys / tokens: keep only the first 8 and last 4 chars.
 * Full value never lands in the DOM (the copy button fetches the original when needed),
 * to prevent casual screenshots from leaking secrets.
 */
export function maskSecret(value: string | undefined | null): string {
  if (!value) return "—";
  if (value.length <= 16) return "……";
  return `${value.slice(0, 8)}……${value.slice(-4)}`;
}

/** Spec 10 / 08: forbidden to display raw HTTP status codes or raw exceptions to the user, unified friendly text */
export function friendlyError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err ?? "");
  if (/401|unauthor|unauthorised/i.test(raw)) return "err.unauthorized";
  // Backend's own ready-written messages (e.g. "ops-server unavailable: cannot connect …")
  // are more accurate than any mapping here, so let them through first: the transport-layer regex
  // matches "timeout / connection" too, and if it's eaten the user is left with a useless "network error".
  if (/network|fetch|ECONN|timeout/i.test(raw)) return "err.network";
  // Raw status codes (http_502 / 502 Bad Gateway) are not shown directly to the user
  if (/^http_\d+$/.test(raw) || /^\d{3}\b/.test(raw)) {
    return /^http_4/.test(raw) ? "err.generic" : "err.server";
  }
  // Server gives a human-readable sentence (e.g. "kill_process needs pid", "signal must be term or kill"),
  // use it directly — more useful than "something went wrong"; stack traces and status codes still don't leak.
  if (raw.trim()) return raw;
  return "err.generic";
}

/** Node liveness classification: online / lagging / offline */
export type Liveness = "online" | "lagging" | "offline" | "unknown";

export function livenessOf(lastSeenMs: number | null | undefined): Liveness {
  if (!lastSeenMs) return "unknown";
  const age = Date.now() - lastSeenMs;
  if (age < 60_000) return "online";
  if (age < 5 * 60_000) return "lagging";
  return "offline";
}

/**
 * Node display name: admin alias preferred, falls back to hostname.
 *
 * Dropdowns, list titles, and the log page's node selector all go through this —
 * the alias exists exactly for "recognize at a glance", so any one place missing it
 * would result in the same node having two names.
 */
export function nodeLabel(
  n: { alias?: string; hostname?: string } | null | undefined,
): string {
  if (!n) return "";
  const alias = (n.alias ?? "").trim();
  return alias || (n.hostname ?? "");
}

/**
 * Copy to clipboard, returns true on success.
 *
 * Tries the async Clipboard API first, falls back to a hidden textarea + `execCommand`
 * if unavailable or failed. The fallback isn't nostalgia: `navigator.clipboard` only exists
 * in **secure contexts** (https / localhost), but self-hosted deployments often open the console
 * at `http://<internal IP>:8443` where it's `undefined` — "click to copy" silently fails
 * and the user only sees "copy failed".
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Fall through to the fallback
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
