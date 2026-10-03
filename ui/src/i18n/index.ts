import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import enUS from "../../locales/en-US/common.json";
import zhCN from "../../locales/zh-CN/common.json";
import { loadLocale, persist, type Locale } from "@/lib/prefs";

void i18n.use(initReactI18next).init({
  resources: {
    "en-US": { common: enUS },
    "zh-CN": { common: zhCN },
  },
  lng: loadLocale(),
  fallbackLng: "en-US",
  defaultNS: "common",
  ns: ["common"],
  interpolation: { escapeValue: false },
});

/**
 * Sync <html lang> and <title> with the active locale: the browser tab,
 * bookmarks, and push notifications all read <title>, so it has to follow
 * the language (otherwise switching to English still leaves "ZhiWei Console"
 * on the tab). The pre-render title in index.html is just a fallback for
 * the first frame.
 */
function syncDocumentMeta() {
  document.title = i18n.t("app.title", { defaultValue: i18n.t("app.name") });
  document.documentElement.lang = i18n.language;
}
// init can finish synchronously or later (depending on whether a backend is used);
// cover both paths.
i18n.on("initialized", syncDocumentMeta);
i18n.on("languageChanged", syncDocumentMeta);
if (i18n.isInitialized) syncDocumentMeta();

export function switchLocale(locale: Locale) {
  persist.locale(locale);
  void i18n.changeLanguage(locale);
}

export default i18n;
