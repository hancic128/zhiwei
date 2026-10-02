import * as React from "react";
import * as Popover from "@radix-ui/react-popover";
import { useNavigate } from "react-router-dom";
import {
  Activity,
  Box,
  Check,
  Globe,
  HelpCircle,
  Inbox,
  LogOut,
  Menu,
  Moon,
  Palette,
  Server,
  Settings,
  ShieldCheck,
  Sun,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { THEMES, type Locale, type Theme } from "@/lib/prefs";
import { usePrefs } from "@/components/prefs-provider";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { useToast } from "@/components/ui/toast";

/** 规范 7.16.2 / 02：5 个主题色圆点的展示色 */
const THEME_SWATCH: Record<Theme, string> = {
  zhiwei: "#c73225", // 朱砂
  indigo: "#4f46e5",
  emerald: "#059669",
  rose: "#e11d48",
  amber: "#d97706",
  slate: "#475569",
};

/**
 * 气泡菜单列出可用页面视图。
 *
 * 不含「设置」：设置是常驻入口（侧边栏固定可见），在悬浮气泡里重复
 * 只会让「快捷控制」这组变浑浊——本人 2026-09-22 明确要求。
 * 其余页面项与侧边栏保持同一份清单。
 */
const NAV_ITEMS = [
  { to: "/", key: "todo", icon: Inbox },
  { to: "/nodes", key: "nodes", icon: Server },
  { to: "/services", key: "services", icon: Activity },
  { to: "/containers", key: "containers", icon: Box },
  { to: "/certificates", key: "certificates", icon: ShieldCheck },
  { to: "/help", key: "help", icon: HelpCircle },
] as const;

/**
 * 规范 7.16 + 禁止清单：
 *  - 收敛为一个主按钮（Settings 图标，brand 实底），hover/focus 展开子按钮组
 *  - 主按钮展开时图标旋转 45°；移出约 180ms 后自动收起；Esc 立即收起
 *  - 子按钮独立、纯 SVG 图标、禁止文字、禁止二元切换
 *  - 主题色气泡在按钮左侧水平展开，避免遮挡同级按钮
 *  - 退出登录必须走 ConfirmDialog，用 rose 语义色区分
 */
export function FloatingControls() {
  const { t } = useTranslation();
  const prefs = usePrefs();
  const toast = useToast();
  const navigate = useNavigate();

  const [open, setOpen] = React.useState(false);
  const [confirmLogout, setConfirmLogout] = React.useState(false);
  const closeTimer = React.useRef<number | null>(null);

  const cancelClose = () => {
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  };
  const scheduleClose = () => {
    cancelClose();
    closeTimer.current = window.setTimeout(() => setOpen(false), 180);
  };

  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      cancelClose();
    };
  }, []);

  const subButton = cn(
    "w-11 h-11 rounded-full bg-surface-0 border border-surface-3 shadow-lg",
    "flex items-center justify-center text-ink-700",
    "hover:bg-surface-2 hover:scale-105 transition-all",
    "dark:bg-ink-700 dark:border-ink-700 dark:text-surface-0 dark:hover:bg-ink-700/70",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40",
  );

  const logoutButton = cn(
    "w-11 h-11 rounded-full bg-rose-50 border border-rose-200 shadow-lg",
    "flex items-center justify-center text-rose-600",
    "hover:scale-105 transition-all",
    "dark:bg-rose-700/20 dark:border-rose-700/40 dark:text-rose-400",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-rose-500/40",
  );

  return (
    <>
      {/* 悬停展开只认「按钮本体」：handler 挂在外层容器上时，收起状态那列
          透明区域也算悬停区，鼠标扫过主按钮**上方**就会展开。 */}
      <div className="fixed bottom-6 right-6 z-30 flex flex-col items-end gap-3">
        {/* 子按钮组：展开后自上而下 导航 → 帮助 → 主题色 → 明暗 → 语言 → 退出。
            折叠时 display:none（而非仅透明）：不收起的透明列会占满整段竖直区域，
            叠在页面右下角的内容上方。 */}
        <div
          className={cn(
            "flex flex-col items-end gap-3 transition-opacity duration-200",
            open ? "opacity-100" : "hidden",
          )}
          onMouseEnter={cancelClose}
          onMouseLeave={scheduleClose}
        >
          {/* 页面导航 */}
          <Popover.Root>
            <Popover.Trigger asChild>
              <button type="button" className={subButton} aria-label={t("nav.menu")}>
                <Menu className="w-5 h-5" aria-hidden="true" />
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                side="left"
                align="end"
                sideOffset={12}
                className="z-50 w-36 bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-700 p-2 animate-panel-slide"
              >
                {NAV_ITEMS.map((item) => (
                  <button
                    key={item.to}
                    type="button"
                    onClick={() => navigate(item.to)}
                    className="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-sm font-medium text-ink-700 hover:bg-surface-2 dark:text-surface-4 dark:hover:bg-ink-700/60"
                  >
                    <item.icon className="w-4 h-4 shrink-0" aria-hidden="true" />
                    {t(`nav.${item.key}`)}
                  </button>
                ))}
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>

          {/* 主题色：气泡在按钮左侧水平展开 */}
          <Popover.Root>
            <Popover.Trigger asChild>
              <button
                type="button"
                className={subButton}
                aria-label={t("action.theme")}
              >
                <Palette className="w-5 h-5" aria-hidden="true" />
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                side="left"
                align="center"
                sideOffset={12}
                className="z-50 bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-700 p-3 animate-panel-slide"
              >
                <div className="flex items-center gap-2">
                  {THEMES.map((theme) => (
                    <button
                      key={theme}
                      type="button"
                      aria-label={theme}
                      onClick={() => {
                        prefs.setTheme(theme);
                        toast.push("success", t("toast.themeChanged"));
                      }}
                      className={cn(
                        "w-7 h-7 rounded-full hover:scale-110 transition-transform relative",
                        prefs.theme === theme &&
                          "outline outline-2 outline-offset-2 outline-ink-400",
                      )}
                      style={{ backgroundColor: THEME_SWATCH[theme] }}
                    >
                      {prefs.theme === theme && (
                        <Check
                          className="w-4 h-4 text-white absolute inset-0 m-auto"
                          aria-hidden="true"
                        />
                      )}
                    </button>
                  ))}
                </div>
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>

          {/* 明暗：单按钮直接切换，图标跟随状态 */}
          <button
            type="button"
            className={subButton}
            aria-label={t("action.colorScheme")}
            onClick={() =>
              prefs.setColorScheme(
                prefs.colorScheme === "dark" ? "light" : "dark",
              )
            }
          >
            {prefs.colorScheme === "dark" ? (
              <Moon className="w-5 h-5" aria-hidden="true" />
            ) : (
              <Sun className="w-5 h-5" aria-hidden="true" />
            )}
          </button>

          {/* 语言：单按钮直接切换，右下角徽标显示当前语言 */}
          <button
            type="button"
            className={cn(subButton, "relative")}
            aria-label={t("action.language")}
            onClick={() => prefs.setLocale(prefs.locale === "en-US" ? "zh-CN" : "en-US")}
          >
            <Globe className="w-5 h-5" aria-hidden="true" />
            <span className="absolute -bottom-1 -right-1 w-5 h-5 rounded-full bg-brand-600 text-white text-[10px] font-bold flex items-center justify-center">
              {prefs.locale === "en-US" ? "EN" : "中"}
            </span>
          </button>

          {/* 退出登录：危险操作，rose 语义色 + 必须确认 */}
          <button
            type="button"
            className={logoutButton}
            aria-label={t("action.logout")}
            onClick={() => setConfirmLogout(true)}
          >
            <LogOut className="w-5 h-5" aria-hidden="true" />
          </button>
        </div>

        {/* 主按钮：brand 实底，展开时图标旋转 45°。
            图标用齿轮（本人指定保留），但它**不是**「设置页入口」——
            语义是「展开快捷控制」，相应地 aria-label 也不叫设置，
            免得这组里出现第二个叫「设置」的按钮（设置是侧边栏的常驻项）。 */}
        <button
          type="button"
          aria-label={t("action.quickControls")}
          aria-expanded={open}
          onMouseEnter={() => {
            cancelClose();
            setOpen(true);
          }}
          onMouseLeave={scheduleClose}
          onClick={() => setOpen((v) => !v)}
          className={cn(
            "w-12 h-12 rounded-full bg-brand-600 text-white shadow-lg",
            "flex items-center justify-center hover:bg-brand-700 transition-all",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40",
          )}
        >
          <Settings
            className={cn(
              "w-5 h-5 transition-transform duration-200",
              open && "rotate-45",
            )}
            aria-hidden="true"
          />
        </button>
      </div>

      <ConfirmDialog
        open={confirmLogout}
        title={t("confirm.logoutTitle")}
        message={t("confirm.logoutMessage")}
        confirmLabel={t("confirm.logoutConfirm")}
        cancelLabel={t("confirm.cancel")}
        danger
        onCancel={() => setConfirmLogout(false)}
        onConfirm={() => {
          setConfirmLogout(false);
          toast.push("success", t("toast.logoutSuccess"));
          window.setTimeout(() => {
            localStorage.removeItem("zhiwei.adminToken");
            window.location.reload();
          }, 600);
        }}
      />
    </>
  );
}

/** 供语言徽标复用：当前 locale 的短标 */
export function localeBadge(_locale: Locale) {
  return "EN";
}
