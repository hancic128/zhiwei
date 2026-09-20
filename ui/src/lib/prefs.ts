/** 规范 03 / 05 / 10：主题色、明暗、语言、时区、侧边栏状态统一持久化。
 *  key 命名沿用规范（theme / colorScheme / locale / timezone），
 *  侧边栏折叠沿用规范给出的 `tj_sidebar` 约定前缀。
 */

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
export type Locale = "zh-CN" | "en-US";

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
  if (v === "zh-CN" || v === "en-US") return v;
  return navigator.language?.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
}

export function loadTimezone(): string {
  return read(KEYS.timezone) || Intl.DateTimeFormat().resolvedOptions().timeZone;
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
