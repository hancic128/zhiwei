import * as React from "react";
import { createPortal } from "react-dom";
import { ChevronRight } from "lucide-react";
import { Link } from "react-router-dom";
import { cn } from "@/lib/utils";

/**
 * 顶部 header 里的面包屑。
 *
 * 页面把「当前层级」写进 context，header 里的 <BreadcrumbOutlet/> 负责渲染——
 * App 的 header 在 <Routes> 之外，页面拿不到它的位置，只能这样反向投递。
 * 最深层（叶子）是当前页，不可点；更浅的层级渲染成路由链接。
 * 叶子允许放任意节点（节点详情页要把状态徽章、入网时间一起放进 header）。
 */
export interface BreadcrumbSegment {
  /** 链接目标；省略 = 当前页（叶子） */
  to?: string;
  label: React.ReactNode;
}

const SetContext = React.createContext<(segments: BreadcrumbSegment[]) => void>(
  () => undefined,
);
const ReadContext = React.createContext<BreadcrumbSegment[]>([]);

/** 页面侧：投递 / 清空面包屑。没有 Provider（登录页等）时是 no-op。 */
export function useBreadcrumb() {
  return React.useContext(SetContext);
}

// ---- header 操作区（页面工具栏的反向投递）----
// 页面把工具栏 portal 进 header 里的 <HeaderActions/>。之所以用 portal 而不是
// 把 ReactNode 存进 state：工具栏跟着页面状态频繁重渲染，节点身份每次都是新的，
// 存 state 会变成「effect → setState → 再渲染」的死循环。
const ActionsHostContext = React.createContext<HTMLElement | null>(null);
const SetActionsHostContext = React.createContext<
  React.Dispatch<React.SetStateAction<HTMLElement | null>>
>(() => undefined);

export function BreadcrumbProvider({ children }: { children: React.ReactNode }) {
  const [segments, setSegments] = React.useState<BreadcrumbSegment[]>([]);
  const [actionsHost, setActionsHost] = React.useState<HTMLElement | null>(null);
  return (
    <SetContext.Provider value={setSegments}>
      <ReadContext.Provider value={segments}>
        <ActionsHostContext.Provider value={actionsHost}>
          <SetActionsHostContext.Provider value={setActionsHost}>
            {children}
          </SetActionsHostContext.Provider>
        </ActionsHostContext.Provider>
      </ReadContext.Provider>
    </SetContext.Provider>
  );
}

/** header 里的操作区占位；页面用 useHeaderActions 往里投递工具栏。 */
export function HeaderActions({ className }: { className?: string }) {
  const setHost = React.useContext(SetActionsHostContext);
  return <div ref={setHost} className={cn("flex items-center min-w-0", className)} />;
}

/** 页面侧：把工具栏投递到 header 的操作区。必须把返回值渲染出来（portal 元素）。 */
export function useHeaderActions(node: React.ReactNode) {
  const host = React.useContext(ActionsHostContext);
  return host ? createPortal(node, host) : null;
}

export function BreadcrumbOutlet({ className }: { className?: string }) {
  const segments = React.useContext(ReadContext);
  if (segments.length === 0) return null;
  return (
    <nav
      aria-label="breadcrumb"
      className={cn("flex items-center gap-2 min-w-0", className)}
    >
      {segments.map((seg, i) => {
        const last = i === segments.length - 1;
        return (
          <React.Fragment key={i}>
            {seg.to && !last ? (
              <Link
                to={seg.to}
                className="text-sm text-ink-500 hover:text-ink-900 dark:hover:text-surface-0 transition-colors truncate"
              >
                {seg.label}
              </Link>
            ) : (
              <span className="min-w-0">{seg.label}</span>
            )}
            {!last && (
              <ChevronRight
                className="w-4 h-4 shrink-0 text-ink-400"
                aria-hidden="true"
              />
            )}
          </React.Fragment>
        );
      })}
    </nav>
  );
}
