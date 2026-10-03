import * as React from "react";
import { createPortal } from "react-dom";
import { ChevronRight } from "lucide-react";
import { Link } from "react-router-dom";
import { cn } from "@/lib/utils";

/**
 * Breadcrumb in the top header.
 *
 * The page writes "current level" into context, and <BreadcrumbOutlet/> in the header renders it —
 * App's header is outside <Routes>, pages can't reach it, so we push from the page side.
 * The deepest level (leaf) is the current page and is not clickable; shallower levels render as route links.
 * The leaf allows arbitrary nodes (the node detail page puts status badge, enroll time, etc. into the header).
 */
export interface BreadcrumbSegment {
  /** Link target; omitted = current page (leaf) */
  to?: string;
  label: React.ReactNode;
}

const SetContext = React.createContext<(segments: BreadcrumbSegment[]) => void>(
  () => undefined,
);
const ReadContext = React.createContext<BreadcrumbSegment[]>([]);

/** Page side: push / clear breadcrumb. No-op without a Provider (e.g. login page). */
export function useBreadcrumb() {
  return React.useContext(SetContext);
}

// ---- header actions area (reverse push of page toolbar) ----
// Pages portal the toolbar into <HeaderActions/> in the header. Why portal instead of
// storing the ReactNode in state: the toolbar re-renders frequently with page state, every render produces
// a new node identity, and storing in state would create an "effect → setState → re-render" infinite loop.
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

/** Placeholder for the actions area in the header; pages use useHeaderActions to push toolbars into it. */
export function HeaderActions({ className }: { className?: string }) {
  const setHost = React.useContext(SetActionsHostContext);
  return <div ref={setHost} className={cn("flex items-center min-w-0", className)} />;
}

/** Page side: pushes the toolbar into the header actions area. Must render the return value (the portal element). */
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
