import * as React from "react";
import { cn } from "@/lib/utils";

/** Spec 7.3: all tables reuse the same template. */
export function TableShell({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "bg-surface-0 dark:bg-ink-700 rounded-xl border border-surface-3 dark:border-ink-700 overflow-hidden",
        className,
      )}
      {...props}
    />
  );
}

/** Table toolbar: px-6 py-4 border-b, filters on left, search on right */
export function TableToolbar({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "px-6 py-4 border-b border-surface-3 dark:border-ink-700 flex items-center justify-between gap-4 flex-wrap",
        className,
      )}
      {...props}
    />
  );
}

export function Table({ className, ...props }: React.HTMLAttributes<HTMLTableElement>) {
  return (
    <div className="overflow-x-auto scrollbar-thin">
      <table className={cn("w-full text-sm", className)} {...props} />
    </div>
  );
}

export function THead({
  className,
  ...props
}: React.HTMLAttributes<HTMLTableSectionElement>) {
  return (
    <thead
      className={cn(
        "border-b border-surface-3 dark:border-ink-700 bg-surface-1 dark:bg-ink-700/40",
        className,
      )}
      {...props}
    />
  );
}

/** Spec 7.3.1: header px-4 py-3, text-xs font-semibold text-ink-500 uppercase */
export function Th({
  className,
  align = "left",
  ...props
}: React.ThHTMLAttributes<HTMLTableCellElement> & {
  align?: "left" | "right" | "center";
}) {
  return (
    <th
      className={cn(
        "px-4 py-3 text-xs font-semibold text-ink-500 uppercase tracking-wider",
        align === "right" && "text-right",
        align === "center" && "text-center",
        align === "left" && "text-left",
        className,
      )}
      {...props}
    />
  );
}

export function TBody({
  className,
  ...props
}: React.HTMLAttributes<HTMLTableSectionElement>) {
  return (
    <tbody
      className={cn("divide-y divide-surface-2 dark:divide-ink-700", className)}
      {...props}
    />
  );
}

/** Spec 7.3.3: row px-4 py-4, hover:bg-surface-1 */
export function Tr({
  className,
  ...props
}: React.HTMLAttributes<HTMLTableRowElement>) {
  return (
    <tr
      className={cn(
        "hover:bg-surface-1 dark:hover:bg-ink-700/40 transition-colors",
        className,
      )}
      {...props}
    />
  );
}

export function Td({
  className,
  align = "left",
  ...props
}: React.TdHTMLAttributes<HTMLTableCellElement> & {
  align?: "left" | "right" | "center";
}) {
  return (
    <td
      className={cn(
        "px-4 py-4",
        align === "right" && "text-right",
        align === "center" && "text-center",
        className,
      )}
      {...props}
    />
  );
}

/** Pagination bar: px-6 py-4 border-t */
export function TableFooter({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "px-6 py-4 border-t border-surface-3 dark:border-ink-700 flex items-center justify-between",
        className,
      )}
      {...props}
    />
  );
}
