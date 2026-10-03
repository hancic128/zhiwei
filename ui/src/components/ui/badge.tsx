import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

/** Spec 7.5: only 4 semantic color combinations; no custom colors allowed. */
const badgeVariants = cva(
  "inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium whitespace-nowrap",
  {
    variants: {
      tone: {
        success:
          "bg-emerald-50 text-emerald-700 dark:bg-emerald-700/20 dark:text-emerald-400",
        warn: "bg-amber-50 text-amber-700 dark:bg-amber-700/20 dark:text-amber-400",
        danger: "bg-rose-50 text-rose-700 dark:bg-rose-700/20 dark:text-rose-400",
        brand:
          "bg-brand-50 text-brand-700 dark:bg-brand-900 dark:text-brand-100",
        neutral:
          "bg-surface-2 text-ink-500 dark:bg-ink-700 dark:text-surface-4",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

export function Badge({ className, tone, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />;
}

/** Spec 7.5.3: Badge with dot */
export function DotBadge({
  tone = "neutral",
  children,
  pulse = false,
}: {
  tone?: "success" | "warn" | "danger" | "brand" | "neutral";
  children: React.ReactNode;
  pulse?: boolean;
}) {
  const dot =
    tone === "success"
      ? "bg-emerald-600"
      : tone === "warn"
        ? "bg-amber-600"
        : tone === "danger"
          ? "bg-rose-600"
          : tone === "brand"
            ? "bg-brand-600"
            : "bg-ink-400";
  return (
    <Badge tone={tone}>
      <span
        className={cn(
          "w-1.5 h-1.5 rounded-full shrink-0",
          dot,
          pulse && "animate-pulse",
        )}
        aria-hidden="true"
      />
      {children}
    </Badge>
  );
}
