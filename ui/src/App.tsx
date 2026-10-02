import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { NavLink, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  Activity,
  BellRing,
  Box,
  HelpCircle,
  Inbox,
  Menu,
  Rocket,
  Server,
  Settings as SettingsIcon,
  ShieldCheck,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { ApiError, api, clearAdminToken, getToken } from "@/api";
import { cn, formatTime, relativeTime, TIMEZONES } from "@/lib/utils";
import { APP_VERSION } from "@/lib/version";
import { usePrefs } from "@/components/prefs-provider";
import { Sidebar, useSidebarCollapsed } from "@/components/sidebar";
import { FloatingControls } from "@/components/floating-controls";
import {
  BreadcrumbOutlet,
  BreadcrumbProvider,
  HeaderActions,
} from "@/components/ui/breadcrumb";
import { HelpPage } from "@/pages/help";
import { Badge, DotBadge } from "@/components/ui/badge";
import { Tooltip, TooltipProvider } from "@/components/ui/tooltip";
import { LoginPage } from "@/pages/login";
import { Alerts } from "@/pages/alerts";
import { Certificates } from "@/pages/certificates";
import { Containers } from "@/pages/containers";
import { Todo } from "@/pages/todo";
import { Nodes } from "@/pages/nodes";
import { NodeDetail } from "@/pages/node-detail";
import { Settings } from "@/pages/settings";
import { Services } from "@/pages/services";

export default function App() {
  const [hasToken, setHasToken] = React.useState(() => !!getToken());

  if (!hasToken) {
    // 规范 07-3.3：登录页不显示导航栏，但**保留**右下角悬浮控制面板（主题 / 语言）
    return (
      <>
        <LoginPage onSubmit={() => setHasToken(true)} />
        <FloatingControls />
      </>
    );
  }

  return (
    <TooltipProvider>
      <BreadcrumbProvider>
        <Console onLogout={() => setHasToken(false)} />
      </BreadcrumbProvider>
    </TooltipProvider>
  );
}

function Console({ onLogout }: { onLogout: () => void }) {
  const { t } = useTranslation();
  const { timezone } = usePrefs();
  const { pathname } = useLocation();
  const { collapsed, toggle } = useSidebarCollapsed();
  const [drawer, setDrawer] = React.useState(false);

  const indexQ = useQuery({
    queryKey: ["index"],
    queryFn: api.index,
    refetchInterval: 10000,
  });

  // 最近部署时间 = monitor 进程启动时刻，由 /v1 带回来。拿不到（断网 / 老后端不认
  // 这个字段）就先不显示——顶栏宁可少一块，也不给一个猜出来的时间。
  const deployedAtMs = indexQ.data?.started_at_unix_nano
    ? Math.floor(indexQ.data.started_at_unix_nano / 1e6)
    : null;
  // Tooltip 里点明用的是哪个时区（"北京 (UTC+8)"）——顶栏时间跟着设置走，写出来
  // 就不用去设置页对照了。设置页那 6 个之外的值（理论上进不来）直接显示 IANA 名。
  const tzKey = TIMEZONES.find((z) => z.tz === timezone)?.key;
  const tzLabel = tzKey ? t(`tz.${tzKey}`) : timezone;

  // token 失效 → 清掉本地 token、回到登录页。
  // 受保护端点（/v1/todo 等）在 api.ts 里 dispatch `zhiwei:unauthorized`；
  // 这里挂监听器负责收尾——只清 token、不弹错误 toast（401 本身就是「请重登」的提示）。
  React.useEffect(() => {
    const handler = () => {
      clearAdminToken();
      onLogout();
    };
    window.addEventListener("zhiwei:unauthorized", handler);
    return () => window.removeEventListener("zhiwei:unauthorized", handler);
  }, [onLogout]);

  // /v1 永不返回 401（仅靠 body.authenticated 区分鉴权结果），
  // 但如果未来加了其它全局探测端点导致这里也回 401，仍按老逻辑兜底。
  React.useEffect(() => {
    if (indexQ.error instanceof ApiError && indexQ.error.status === 401) {
      clearAdminToken();
      onLogout();
    }
  }, [indexQ.error, onLogout]);

  const title = React.useMemo(() => {
    if (pathname.startsWith("/nodes/")) return t("detail.back");
    if (pathname === "/nodes") return t("nav.nodes");
    if (pathname === "/help") return t("help.title");
    if (pathname === "/") return t("nav.todo");
    const key = pathname.replace("/", "");
    return t(`nav.${key}`, { defaultValue: t("app.console") });
  }, [pathname, t]);

  return (
    <div className="flex h-screen overflow-hidden">
      <Sidebar collapsed={collapsed} onToggle={toggle} />

      {/* 手机端抽屉（规范 04：< md 侧边栏隐藏，汉堡菜单触发） */}
      <MobileDrawer open={drawer} onClose={() => setDrawer(false)} />

      <div className="flex-1 flex flex-col min-w-0">
        <header className="min-h-16 shrink-0 sticky top-0 z-30 bg-surface-0 dark:bg-ink-700 border-b border-surface-3 dark:border-ink-700 flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-2 md:px-6 lg:px-8">
          <div className="flex items-center gap-3 min-w-0 flex-1">
            <button
              type="button"
              className="md:hidden w-9 h-9 flex items-center justify-center rounded-lg hover:bg-surface-2 dark:hover:bg-ink-700/60"
              aria-label={t("nav.menu")}
              onClick={() => setDrawer(true)}
            >
              <Menu className="w-5 h-5 text-ink-700 dark:text-surface-4" aria-hidden="true" />
            </button>
            {/* 节点详情页把「Nodes / 家用电脑 · 状态 · 入网时间」投递到 header；
                其余页面继续显示单段标题，外观不变。 */}
            {pathname.startsWith("/nodes/") ? (
              <BreadcrumbOutlet />
            ) : (
              <div className="min-w-0">
                <h1 className="text-base font-semibold text-ink-900 dark:text-surface-0 truncate">
                  {title}
                </h1>
              </div>
            )}
          </div>

          {/* 页面自己的工具栏（节点详情：时间范围 / 刷新频率 / 节点操作）投递到这里，
              不再压在内容区顶部 —— 内容区从第一块卡片开始。
              窄屏（<lg）折成独立一行：横排在 390px 上会把标题和版本徽章顶出屏幕。 */}
          <HeaderActions className="order-last w-full justify-end shrink-0 gap-2 lg:order-none lg:w-auto" />

          <div className="flex items-center gap-2 shrink-0">
            {/* 最近部署时间：绝对时间按「设置 → 界面偏好 → 时区」的全局时区渲染
                （同一时刻，时区变了这里跟着变），相对时间放 Tooltip——相对时间
                与时区无关，不占顶栏宽度。窄屏隐藏，别把版本徽章挤走。 */}
            {deployedAtMs !== null && (
              <Tooltip
                side="bottom"
                content={t("app.deployedAtHint", {
                  ago: relativeTime(deployedAtMs, t),
                  tz: tzLabel,
                })}
              >
                <span className="hidden sm:inline-flex items-center gap-1 text-xs tabular-nums text-ink-400 dark:text-surface-4 cursor-default">
                  <Rocket className="w-3.5 h-3.5" aria-hidden="true" />
                  {t("app.deployedAt", {
                    time: formatTime(deployedAtMs, timezone),
                  })}
                </span>
              </Tooltip>
            )}
            {/* 版本徽章：写死的构建常量，不随在线状态波动 */}
            <Badge tone="neutral">{APP_VERSION}</Badge>
            <DotBadge
              tone={indexQ.isSuccess ? "success" : "danger"}
              pulse={indexQ.isSuccess}
            >
              {indexQ.isSuccess ? t("state.online") : t("err.server")}
            </DotBadge>

          </div>
        </header>

        <main className="flex-1 overflow-y-auto scrollbar-thin">
          <div className="max-w-7xl mx-auto px-4 md:px-6 lg:px-8 py-4 md:py-6 lg:py-8">
            <div className="space-y-4 md:space-y-8">
              <Routes>
                <Route path="/" element={<Todo />} />
                <Route path="/nodes" element={<Nodes />} />
                <Route path="/nodes/:id" element={<NodeDetail />} />
                <Route path="/services" element={<Services />} />
                <Route path="/help" element={<HelpPage />} />
                <Route path="/containers" element={<Containers />} />
                {/* 日志菜单已下线：容器日志在容器页行内，文件日志走容器页工具栏 */}
                <Route path="/logs" element={<Navigate to="/containers" replace />} />
                <Route path="/certificates" element={<Certificates />} />
                <Route path="/alerts" element={<Alerts />} />
                <Route path="/settings" element={<Settings />} />
                <Route path="*" element={<Navigate to="/" replace />} />
              </Routes>
            </div>
          </div>
        </main>
      </div>

      <FloatingControls />
    </div>
  );
}

/** 手机端抽屉导航 */
const MOBILE_NAV = [
  { to: "/", key: "todo", icon: Inbox },
  { to: "/nodes", key: "nodes", icon: Server },
  { to: "/services", key: "services", icon: Activity },
  { to: "/containers", key: "containers", icon: Box },
  { to: "/certificates", key: "certificates", icon: ShieldCheck },
  { to: "/alerts", key: "alerts", icon: BellRing },
  { to: "/settings", key: "settings", icon: SettingsIcon },
  { to: "/help", key: "help", icon: HelpCircle },
] as const;

function MobileDrawer({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <DialogPrimitive.Root open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-ink-900/50 backdrop-blur-sm md:hidden" />
        <DialogPrimitive.Content className="fixed left-0 top-0 bottom-0 z-50 w-60 md:hidden bg-surface-0 dark:bg-ink-700 border-r border-surface-3 dark:border-ink-700 animate-panel-slide">
          <div className="h-16 flex items-center justify-between px-6 border-b border-surface-3 dark:border-ink-700">
            <span className="text-sm font-semibold text-ink-900 dark:text-surface-0">
              {t("app.name")}
            </span>
            <DialogPrimitive.Close
              aria-label={t("action.close")}
              className="w-9 h-9 flex items-center justify-center rounded-lg hover:bg-surface-2"
            >
              <X className="w-5 h-5 text-ink-500" aria-hidden="true" />
            </DialogPrimitive.Close>
          </div>
          <nav className="p-3 space-y-1 overflow-y-auto scrollbar-thin">
            {MOBILE_NAV.map(({ to, key, icon: Icon }) => (
              <NavLink
                key={to}
                to={to}
                end={to === "/"}
                onClick={onClose}
                className={({ isActive }) =>
                  cn(
                    "flex items-center gap-3 px-3 py-2 rounded-lg text-sm font-medium transition-colors",
                    isActive
                      ? "bg-brand-50 text-brand-700 dark:bg-brand-900 dark:text-brand-100"
                      : "text-ink-700 hover:bg-surface-2 dark:text-surface-4",
                  )
                }
              >
                <Icon className="w-4 h-4" aria-hidden="true" />
                {t(`nav.${key}`)}
              </NavLink>
            ))}
          </nav>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
