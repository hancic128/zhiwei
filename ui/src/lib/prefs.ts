/** Spec 03 / 05 / 10: theme color, light/dark, locale, timezone, sidebar state persisted uniformly.
 *  key naming follows the spec (theme / colorScheme / locale / timezone),
 *  sidebar collapse follows the spec's `tj_sidebar` prefix convention.
 */

import { TIMEZONES } from "@/lib/utils";

/** 6 selectable timezones (the settings page dropdown and UI rendering share this single source, don't re-copy it elsewhere) */
const SUPPORTED_TZ: readonly string[] = TIMEZONES.map((z) => z.tz);

export const THEMES = [
  "zhiwei",
  "indigo",
  "emerald",
  "rose",
  "amber",
  "slate",
] as const;
export type Theme = (typeof THEMES)[number];

export type ColorScheme = "light" | "dark";
export type Locale = "en-US" | "zh-CN";

export const KEYS = {
  theme: "theme",
  colorScheme: "colorScheme",
  locale: "locale",
  timezone: "timezone",
  sidebar: "tj_sidebar",
  adminToken: "zhiwei.adminToken",
} as const;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* In private mode localStorage may be unavailable, ignore */
  }
}

export function loadTheme(): Theme {
  const v = read(KEYS.theme);
  return (THEMES as readonly string[]).includes(v ?? "")
    ? (v as Theme)
    : "zhiwei";
}

export function loadColorScheme(): ColorScheme {
  const v = read(KEYS.colorScheme);
  if (v === "light" || v === "dark") return v;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

export function loadLocale(): Locale {
  const v = read(KEYS.locale);
  if (v === "en-US" || v === "zh-CN") return v;
  return "en-US";
}

export function loadTimezone(): string {
  const v = read(KEYS.timezone);
  if (v && SUPPORTED_TZ.includes(v)) return v;

  const browser = Intl.DateTimeFormat().resolvedOptions().timeZone;
  if (SUPPORTED_TZ.includes(browser)) return browser;

  // Fallback must land in the 6 options above: the settings page dropdown has exactly these 6 options,
  // and `<select>` silently returns to the first item for unrecognized values — so the settings page
  // shows "Beijing (UTC+8)" while the entire interface renders in the browser's timezone
  // (often UTC on containers / CI / servers), and the two don't match. When the browser timezone
  // isn't in the list, fall back to the **closest current UTC offset** option: wall-clock time
  // will be off by at most an hour or two, rather than a full rotation.
  const target = tzOffsetMinutes(browser);
  let best = SUPPORTED_TZ[0];
  let bestDiff = Number.POSITIVE_INFINITY;
  for (const tz of SUPPORTED_TZ) {
    const diff = Math.abs(tzOffsetMinutes(tz) - target);
    if (diff < bestDiff) {
      bestDiff = diff;
      best = tz;
    }
  }
  return best;
}

/**
 * An IANA timezone's current offset relative to UTC (in minutes).
 *
 * Doesn't use dayjs: `loadTimezone()` may be called during module initialization,
 * and dayjs's tz plugin is registered in utils.ts — we can't assume it's already active here.
 */
function tzOffsetMinutes(tz: string, at: Date = new Date()): number {
  try {
    const parts = new Intl.DateTimeFormat("en-US", {
      timeZone: tz,
      hour12: false,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }).formatToParts(at);
    const p: Record<string, string> = {};
    for (const part of parts) p[part.type] = part.value;
    const asUTC = Date.UTC(
      Number(p.year),
      Number(p.month) - 1,
      Number(p.day),
      Number(p.hour) % 24, // Some locales report midnight as "24"
      Number(p.minute),
      Number(p.second),
    );
    return Math.round((asUTC - at.getTime()) / 60000);
  } catch {
    // Browser doesn't recognize this timezone name → treat as UTC (continue with nearest-offset matching)
    return 0;
  }
}

/** Sidebar collapsed state: spec requires first-frame restoration to avoid refresh flicker */
export function loadSidebarCollapsed(): boolean {
  return read(KEYS.sidebar) === "collapsed";
}

/** Called before React mount to avoid theme / dark mode first-frame flicker */
export function applyThemeEarly(): void {
  const root = document.documentElement;
  root.setAttribute("data-theme", loadTheme());
  root.classList.toggle("dark", loadColorScheme() === "dark");
}

export const persist = {
  theme: (v: Theme) => write(KEYS.theme, v),
  colorScheme: (v: ColorScheme) => write(KEYS.colorScheme, v),
  locale: (v: Locale) => write(KEYS.locale, v),
  timezone: (v: string) => write(KEYS.timezone, v),
  sidebarCollapsed: (v: boolean) => write(KEYS.sidebar, v ? "collapsed" : "expanded"),
};

export const getAdminToken = () => read(KEYS.adminToken) ?? "";
/** Force trim on write: backend's `read_auth_ok` uses constant-time comparison with a fixed length,
 *  one extra space results in 401; the login and settings page entries already trim,
 *  but keeping the rule here is safer. */
export const setAdminToken = (v: string) => write(KEYS.adminToken, v.trim());
export const clearAdminToken = () => {
  try {
    localStorage.removeItem(KEYS.adminToken);
  } catch {
    /* ignore */
  }
};
