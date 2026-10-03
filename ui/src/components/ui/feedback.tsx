import * as React from "react";
import { AlertTriangle, Inbox, RefreshCw, SearchX } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { useTranslation } from "react-i18next";

/** Spec 7.13: skeleton block bg-surface-2 animate-pulse rounded */
export function Skeleton({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "bg-surface-2 dark:bg-ink-700 animate-pulse rounded",
        className,
      )}
      {...props}
    />
  );
}

export function TableSkeleton({ rows = 5 }: { rows?: number }) {
  return (
    <div className="px-4 py-4 space-y-3">
      {Array.from({ length: rows }).map((_, i) => (
        <Skeleton key={i} className="h-4 w-full" />
      ))}
    </div>
  );
}

/** Spec 08: empty state — icon w-12 h-12 text-ink-400 + title + description + action, py-16 */
export function EmptyState({
  icon,
  title,
  description,
  action,
}: {
  icon?: React.ReactNode;
  title: string;
  description?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center py-16 px-6 text-center">
      <div className="text-ink-400">
        {icon ?? <Inbox className="w-12 h-12" aria-hidden="true" />}
      </div>
      <h3 className="mt-4 text-base font-semibold text-ink-900 dark:text-surface-0">
        {title}
      </h3>
      {description && (
        <p className="mt-1 max-w-md text-sm text-ink-500">{description}</p>
      )}
      {action && <div className="mt-6">{action}</div>}
    </div>
  );
}

export function SearchEmptyState({ title, description }: { title: string; description?: string }) {
  return (
    <EmptyState
      icon={<SearchX className="w-12 h-12" aria-hidden="true" />}
      title={title}
      description={description}
    />
  );
}

/**
 * Spec 08 / 10: on load failure, forbidden to show raw status codes or raw exceptions;
 * unified friendly text + inline "Retry" button, failure state stays resident in this area.
 */
export function ErrorState({
  message,
  onRetry,
  retrying,
  compact,
}: {
  message: string;
  onRetry: () => void;
  retrying?: boolean;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center text-center",
        compact ? "py-8 px-6" : "py-16 px-6",
      )}
      role="alert"
    >
      <AlertTriangle className="w-12 h-12 text-amber-500" aria-hidden="true" />
      <p className="mt-4 text-sm text-ink-500">{message}</p>
      <Button
        variant="secondary"
        size="sm"
        className="mt-4"
        onClick={onRetry}
        loading={retrying}
      >
        {!retrying && <RefreshCw className="w-4 h-4" aria-hidden="true" />}
        {t("action.retry")}
      </Button>
    </div>
  );
}
