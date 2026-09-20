import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import zhCN from "../../locales/zh-CN/common.json";
import enUS from "../../locales/en-US/common.json";
import { loadLocale, persist, type Locale } from "@/lib/prefs";

/**
 * 规范 05：中英双语，语言包存放 locales/{lang}/common.json，
 * 全部文案走 t('key')，禁止硬编码；切换即时生效并持久化。
 */
void i18n.use(initReactI18next).init({
  resources: {
    "zh-CN": { common: zhCN },
    "en-US": { common: enUS },
  },
  lng: loadLocale(),
  fallbackLng: "zh-CN",
  defaultNS: "common",
  ns: ["common"],
  interpolation: { escapeValue: false },
});

export function switchLocale(locale: Locale) {
  void i18n.changeLanguage(locale);
  persist.locale(locale);
  document.documentElement.lang = locale;
}

export default i18n;
