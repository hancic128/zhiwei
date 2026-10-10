import * as React from "react";
import { NavLink } from "react-router-dom";
import { Link } from "react-router-dom";
import {
  Activity,
  BellRing,
  Box,
  ChevronsLeft,
  ChevronsRight,
  HelpCircle,
  Inbox,
  Server,
  Settings,
  ShieldCheck,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { todoApi } from "@/api";
import { cn } from "@/lib/utils";
import { loadSidebarCollapsed, persist } from "@/lib/prefs";
import { LogoMark } from "@/components/ui/logo";
import { Tooltip } from "@/components/ui/tooltip";

interface NavItem {
  to: string;
  key: string;
  icon: React.ElementType;
  ready: boolean;
}

const NAV: NavItem[] = [
  { to: "/", key: "todo", icon: Inbox, ready: true },
  { to: "/nodes", key: "nodes", icon: Server, ready: true },
  { to: "/probes", key: "probes", icon: Activity, ready: true },
  { to: "/containers", key: "containers", icon: Box, ready: true },
  { to: "/certificates", key: "certificates", icon: ShieldCheck, ready: true },
  { to: "/alerts", key: "alerts", icon: BellRing, ready: true },
  { to: "/settings", key: "settings", icon: Settings, ready: true },
  { to: "/help", key: "help", icon: HelpCircle, ready: true },
];

/**
 * Spec 7.6.1 / 7.17:
 *  - Width w-60, collapsed 4rem (w-16), icons only
 *  - Nav area flex-1 overflow-y-auto, collapse button mt-auto pinned to bottom
 *  - Collapsed state persists to localStorage, restored on first frame
 */
export function Sidebar({
  collapsed,
  onToggle,
}: {
  collapsed: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  // Count of "things to handle now" — don't show badge when 0 (0 is noise, the todo page says "nothing today" itself)
  const todoQ = useQuery({ queryKey: ["todo"], queryFn: () => todoApi.get() });
  const todoCount = todoQ.data?.counts.now ?? 0;

  /** A group of nav items — main list and bottom settings share the same rendering */
  const renderItems = (items: NavItem[]) =>
    items.map((item) => {
      const Icon = item.icon;
      const label = t(`nav.${item.key}`);
      const link = (
        <NavLink
          key={item.to}
          to={item.to}
          end={item.to === "/"}
          className={({ isActive }) =>
            cn(
              "relative flex items-center rounded-lg px-3 py-2 text-sm font-medium transition-colors",
              collapsed ? "justify-center" : "gap-3",
              isActive
                ? "bg-brand-50 text-brand-700 dark:bg-brand-900 dark:text-brand-100"
                : "text-ink-700 hover:bg-surface-2 dark:text-surface-4 dark:hover:bg-ink-700/60",
            )
          }
        >
          <Icon className="w-4 h-4 shrink-0" aria-hidden="true" />
          {!collapsed && <span className="truncate">{label}</span>}
          {item.key === "todo" && todoCount > 0 && (
            <span
              className={cn(
                "inline-flex items-center justify-center rounded-full bg-rose-600 text-xs font-medium text-white tabular-nums",
                collapsed
                  ? "absolute top-1.5 right-1.5 w-2 h-2 min-w-0 p-0"
                  : "ml-auto min-w-5 h-5 px-1.5",
              )}
              aria-label={t("todo.badgeLabel", { n: todoCount })}
            >
              {collapsed ? "" : todoCount}
            </span>
          )}
          {!collapsed && !item.ready && (
            <span className="ml-auto text-xs text-ink-400">{t("nav.pending")}</span>
          )}
        </NavLink>
      );
      return collapsed ? (
        <Tooltip key={item.to} content={label}>
          {link}
        </Tooltip>
      ) : (
        link
      );
    });

  return (
    <aside
      className={cn(
        "hidden md:flex flex-col shrink-0 bg-surface-0 dark:bg-ink-700",
        "border-r border-surface-3 dark:border-ink-500",
        "transition-[width] duration-200",
        collapsed ? "w-16" : "w-60",
      )}
    >
      {/* Logo area */}
      <div
        className={cn(
          "h-16 shrink-0 flex items-center border-b border-surface-3 dark:border-ink-500",
          collapsed ? "justify-center px-3" : "px-6",
        )}
      >
        <Link
          to="/"
          className="flex items-center gap-3 hover:opacity-80 transition-opacity"
        >
          <LogoMark className="w-7 h-7 text-brand-600 dark:text-brand-500" />
          {!collapsed && (
            <span className="text-sm font-semibold text-ink-900 dark:text-surface-0 truncate">
              {t("app.name")}
            </span>
          )}
        </Link>
      </div>

      {/* Nav area */}
      <nav className="flex-1 p-3 space-y-1 overflow-y-auto scrollbar-thin">
        {renderItems(NAV)}
      </nav>

      {/* Collapse button: pinned to bottom, doesn't scroll with the menu */}
      <div className="shrink-0 mt-auto p-3 border-t border-surface-3 dark:border-ink-500">
        <button
          type="button"
          onClick={onToggle}
          aria-label={collapsed ? t("nav.expand") : t("nav.collapse")}
          className={cn(
            "w-full flex items-center rounded-lg px-3 py-2 text-sm font-medium",
            "text-ink-500 hover:bg-surface-2 hover:text-ink-900 transition-colors",
            "dark:text-surface-4 dark:hover:bg-ink-700/60 dark:hover:text-surface-0",
            collapsed && "justify-center",
          )}
        >
          {collapsed ? (
            <ChevronsRight className="w-[18px] h-[18px]" aria-hidden="true" />
          ) : (
            <>
              <ChevronsLeft className="w-[18px] h-[18px]" aria-hidden="true" />
              <span className="ml-3">{t("nav.collapse")}</span>
            </>
          )}
        </button>
      </div>
    </aside>
  );
}

/** Sidebar collapsed state read/write (restored on first frame to avoid refresh flicker) */
export function useSidebarCollapsed() {
  const [collapsed, setCollapsed] = React.useState(() =>
    loadSidebarCollapsed(),
  );
  const toggle = React.useCallback(() => {
    setCollapsed((prev) => {
      persist.sidebarCollapsed(!prev);
      return !prev;
    });
  }, []);
  return { collapsed, toggle };
}
