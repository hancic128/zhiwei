import * as React from "react";
import { cn } from "@/lib/utils";

/** 规范 7.2：所有卡片复用同一外壳，禁止为不同卡片写不同样式。 */
export function Card({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "bg-surface-0 dark:bg-ink-700 rounded-xl border border-surface-3 dark:border-ink-700",
        className,
      )}
      {...props}
    />
  );
}

export function CardHeader({
  className,
  icon,
  title,
  description,
  action,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & {
  icon?: React.ReactNode;
  title: React.ReactNode;
  description?: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <div
      className={cn(
        "px-6 py-4 border-b border-surface-3 dark:border-ink-700 flex items-center justify-between gap-4",
        className,
      )}
      {...props}
    >
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          {icon}
          <h3 className="text-base font-semibold text-ink-900 dark:text-surface-0 truncate">
            {title}
          </h3>
        </div>
        {description && (
          <p className="text-sm text-ink-500 mt-0.5">{description}</p>
        )}
      </div>
      {action && <div className="flex items-center gap-2 shrink-0">{action}</div>}
    </div>
  );
}

export function CardBody({
  className,
  compact,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { compact?: boolean }) {
  return <div className={cn(compact ? "p-4" : "p-6", className)} {...props} />;
}

export function CardFooter({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "px-6 py-4 border-t border-surface-3 dark:border-ink-700 flex justify-end gap-2",
        className,
      )}
      {...props}
    />
  );
}

/** 规范 7.2.3：数据卡片（仅主体，无头部） */
export function StatCard({
  label,
  value,
  unit,
  hint,
  icon,
  tone = "default",
  loading,
}: {
  label: string;
  value: React.ReactNode;
  unit?: string;
  hint?: React.ReactNode;
  icon?: React.ReactNode;
  tone?: "default" | "brand";
  loading?: boolean;
}) {
  return (
    <Card className="p-6">
      <div className="flex items-center gap-2">
        {icon}
        <span className="text-sm text-ink-500">{label}</span>
      </div>
      {loading ? (
        <div className="mt-3 h-7 w-24 bg-surface-2 dark:bg-ink-700 animate-pulse rounded" />
      ) : (
        <div className="mt-1 flex items-baseline gap-1">
          <span
            className={cn(
              "text-xl font-bold tabular-nums",
              tone === "brand"
                ? "text-brand-600 dark:text-brand-500"
                : "text-ink-900 dark:text-surface-0",
            )}
          >
            {value}
          </span>
          {unit && <span className="text-sm text-ink-500">{unit}</span>}
        </div>
      )}
      {hint && <div className="mt-1 text-xs text-ink-400">{hint}</div>}
    </Card>
  );
}
