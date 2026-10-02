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
 * 同步 <html lang> 与 <title>：浏览器标签页、书签、推送通知读的都是 <title>，
 * 必须跟着语言走（否则切到英文界面、标签页还写着「知微 · 控制台」）。
 * 首帧渲染前 index.html 里那份中文标题只是兜底。
 */
function syncDocumentMeta() {
  document.title = i18n.t("app.title", { defaultValue: i18n.t("app.name") });
  document.documentElement.lang = i18n.language;
}
// init 可能同步也可能延后完成（取决于是否走 backend），两条路都覆盖
i18n.on("initialized", syncDocumentMeta);
i18n.on("languageChanged", syncDocumentMeta);
if (i18n.isInitialized) syncDocumentMeta();

export function switchLocale(locale: Locale) {
  void i18n.changeLanguage(locale);
  persist.locale(locale);
}

export default i18n;
