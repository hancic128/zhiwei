import * as React from "react";
import {
  applyThemeEarly,
  loadColorScheme,
  loadLocale,
  loadTheme,
  loadTimezone,
  persist,
  type ColorScheme,
  type Locale,
  type Theme,
} from "@/lib/prefs";
import { switchLocale } from "@/i18n";
import { applyFavicon } from "@/lib/favicon";

interface PrefsValue {
  theme: Theme;
  colorScheme: ColorScheme;
  locale: Locale;
  timezone: string;
  setTheme: (t: Theme) => void;
  setColorScheme: (s: ColorScheme) => void;
  setLocale: (l: Locale) => void;
  setTimezone: (tz: string) => void;
}

const PrefsContext = React.createContext<PrefsValue | null>(null);

export function usePrefs() {
  const ctx = React.useContext(PrefsContext);
  if (!ctx) throw new Error("usePrefs must be used inside <PrefsProvider>");
  return ctx;
}

export function PrefsProvider({ children }: { children: React.ReactNode }) {
  const [theme, setThemeState] = React.useState<Theme>(() => loadTheme());
  const [colorScheme, setSchemeState] = React.useState<ColorScheme>(() =>
    loadColorScheme(),
  );
  const [locale, setLocaleState] = React.useState<Locale>(() => loadLocale());
  const [timezone, setTzState] = React.useState<string>(() => loadTimezone());

  // 首帧即应用，避免闪烁
  React.useEffect(() => {
    applyThemeEarly();
  }, []);

  React.useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
  }, [theme]);

  React.useEffect(() => {
    document.documentElement.classList.toggle("dark", colorScheme === "dark");
  }, [colorScheme]);

  // 标签页图标与侧边栏 logo 同色：主题色或明暗一变就重画一遍
  React.useEffect(() => {
    applyFavicon();
  }, [theme, colorScheme]);

  React.useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  const value = React.useMemo<PrefsValue>(
    () => ({
      theme,
      colorScheme,
      locale,
      timezone,
      setTheme: (t) => {
        setThemeState(t);
        persist.theme(t);
      },
      setColorScheme: (s) => {
        setSchemeState(s);
        persist.colorScheme(s);
      },
      setLocale: (l) => {
        setLocaleState(l);
        switchLocale(l);
      },
      setTimezone: (tz) => {
        setTzState(tz);
        persist.timezone(tz);
      },
    }),
    [theme, colorScheme, locale, timezone],
  );

  return (
    <PrefsContext.Provider value={value}>{children}</PrefsContext.Provider>
  );
}
