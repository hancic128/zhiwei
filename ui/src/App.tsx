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
    // Spec 07-3.3: login page hides the nav, but keeps the floating controls (theme / locale)
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

  // Latest deploy time = monitor process start time, returned by /v1. If unavailable
  // (network down / older backend that doesn't know this field), don't show it —
  // the header is better off with one less item than with a guessed timestamp.
  const deployedAtMs = indexQ.data?.started_at_unix_nano
    ? Math.floor(indexQ.data.started_at_unix_nano / 1e6)
    : null;
  // Tooltip makes it explicit which timezone is used ("Beijing (UTC+8)") — the header
  // time follows the setting, so showing it avoids having to compare in the settings page.
  // Timezone values outside the 6 in the settings page (shouldn't happen) display the IANA name.
  const tzKey = TIMEZONES.find((z) => z.tz === timezone)?.key;
  const tzLabel = tzKey ? t(`tz.${tzKey}`) : timezone;

  // Token expired → clear local token, return to login page.
  // Protected endpoints (/v1/todo etc.) dispatch `zhiwei:unauthorized` from api.ts;
  // this listener handles cleanup — only clears the token, no error toast (401 itself
  // is the "please re-login" prompt).
  React.useEffect(() => {
    const handler = () => {
      clearAdminToken();
      onLogout();
    };
    window.addEventListener("zhiwei:unauthorized", handler);
    return () => window.removeEventListener("zhiwei:unauthorized", handler);
  }, [onLogout]);

  // /v1 never returns 401 (it only uses body.authenticated to convey auth state),
  // but if a future global probe endpoint also returns 401, fall back to the old logic.
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

      {/* Mobile drawer (spec 04: < md sidebar hidden, hamburger menu triggers it) */}
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
            {/* Node detail page pushes "Nodes / Home PC · Status · Enroll time" into the header;
                Other pages keep showing a single title, no visual change. */}
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

          {/* The page's own toolbar (node detail: time range / refresh rate / node actions) pushes here,
              no longer stuck at the top of the content area — content starts from the first card.
              Narrow screens (<lg) wrap to a separate row: inline at 390px would push the title and
              version badge off-screen. */}
          <HeaderActions className="order-last w-full justify-end shrink-0 gap-2 lg:order-none lg:w-auto" />

          <div className="flex items-center gap-2 shrink-0">
            {/* Latest deploy time: absolute time rendered in the global timezone from
                "Settings → Interface preferences → Timezone" (the same instant changes here
                when the timezone changes), relative time goes in the Tooltip — relative time
                is timezone-agnostic and doesn't take header width. Hide on narrow screens so
                the version badge isn't pushed off. */}
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
            {/* Version badge: hard-coded build constant, doesn't fluctuate with online status */}
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
                {/* Logs menu removed: container logs are inline in the containers page, file logs via the containers page toolbar */}
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

/** Mobile drawer navigation */
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
