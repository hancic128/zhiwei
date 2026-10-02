/** 规范 03 / 05 / 10：主题色、明暗、语言、时区、侧边栏状态统一持久化。
 *  key 命名沿用规范（theme / colorScheme / locale / timezone），
 *  侧边栏折叠沿用规范给出的 `tj_sidebar` 约定前缀。
 */

import { TIMEZONES } from "@/lib/utils";

/** 6 个可选时区（设置页下拉与界面渲染共用同一份，别在别处再抄一遍） */
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
export type Locale = "en-US";

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
    /* 隐私模式下 localStorage 可能不可用，忽略 */
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
  if (v === "en-US") return v;
  return "en-US";
}

export function loadTimezone(): string {
  const v = read(KEYS.timezone);
  if (v && SUPPORTED_TZ.includes(v)) return v;

  const browser = Intl.DateTimeFormat().resolvedOptions().timeZone;
  if (SUPPORTED_TZ.includes(browser)) return browser;

  // 兜底必须落在上面这 6 项里：设置页的下拉就这 6 个选项，`<select>` 拿到不认识的
  // 值会静默退回首项——于是「设置里显示北京 (UTC+8)」而整个界面按浏览器时区
  // 渲染（容器 / CI / 服务器上常是 UTC），两边对不上。浏览器时区不在列表里时，
  // 换成**当前 UTC 偏移最接近**的那一项：墙上时间最多差一两个小时，而不是差一整圈。
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
 * 某 IANA 时区此刻相对 UTC 的偏移（分钟）。
 *
 * 不走 dayjs：`loadTimezone()` 在模块初始化时就可能被调用，而 dayjs 的 tz 插件
 * 是在 utils.ts 里注册的，这里不能假设它已经生效。
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
      Number(p.hour) % 24, // 某些区域把午夜给成 "24"
      Number(p.minute),
      Number(p.second),
    );
    return Math.round((asUTC - at.getTime()) / 60000);
  } catch {
    // 浏览器不认这个时区名 → 当 UTC 处理（继续走最近偏移匹配）
    return 0;
  }
}

/** 侧边栏折叠态：规范要求首帧恢复，避免刷新闪烁 */
export function loadSidebarCollapsed(): boolean {
  return read(KEYS.sidebar) === "collapsed";
}

/** 在 React 挂载前调用，避免主题/暗色首帧闪烁 */
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
/** 写入时强制 trim：后端 `read_auth_ok` 用长度恒定的常量时间比较，
 *  多一个空格就 401；登录页和设置页的入口 trim 一遍，但把规则放在这里更稳。 */
export const setAdminToken = (v: string) => write(KEYS.adminToken, v.trim());
export const clearAdminToken = () => {
  try {
    localStorage.removeItem(KEYS.adminToken);
  } catch {
    /* ignore */
  }
};
