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
  { to: "/services", key: "services", icon: Activity, ready: true },
  { to: "/containers", key: "containers", icon: Box, ready: true },
  { to: "/certificates", key: "certificates", icon: ShieldCheck, ready: true },
  { to: "/alerts", key: "alerts", icon: BellRing, ready: true },
  { to: "/settings", key: "settings", icon: Settings, ready: true },
  { to: "/help", key: "help", icon: HelpCircle, ready: true },
];

/**
 * 规范 7.6.1 / 7.17：
 *  - 宽度 w-60，折叠 4rem（w-16），仅图标
 *  - 导航区 flex-1 overflow-y-auto，折叠按钮 mt-auto 固定贴底
 *  - 折叠态持久化到 localStorage，首帧恢复
 */
export function Sidebar({
  collapsed,
  onToggle,
}: {
  collapsed: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  // 「现在要处理」的条数——为 0 时不显示徽标（0 是噪音，待办页会自己说「今天没事」）
  const todoQ = useQuery({ queryKey: ["todo"], queryFn: todoApi.get });
  const todoCount = todoQ.data?.counts.now ?? 0;

  /** 一组导航项——主列表与贴底的设置共用同一份渲染 */
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
        "border-r border-surface-3 dark:border-ink-700",
        "transition-[width] duration-200",
        collapsed ? "w-16" : "w-60",
      )}
    >
      {/* Logo 区 */}
      <div
        className={cn(
          "h-16 shrink-0 flex items-center border-b border-surface-3 dark:border-ink-700",
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

      {/* 导航区 */}
      <nav className="flex-1 p-3 space-y-1 overflow-y-auto scrollbar-thin">
        {renderItems(NAV)}
      </nav>

      {/* 折叠按钮：固定贴底，不随菜单滚动 */}
      <div className="shrink-0 mt-auto p-3 border-t border-surface-3 dark:border-ink-700">
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

/** 侧边栏折叠状态的读写（首帧恢复，避免刷新闪烁） */
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
