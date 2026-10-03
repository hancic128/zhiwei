import * as React from "react";
import { Link } from "react-router-dom";
import { cn } from "@/lib/utils";

/**
 * Top stat cards (overview-style metrics).
 *
 * The four pages Todo / Nodes / Services / Certificates share the same shell —
 * writing it per-page would let rules like "whole card is clickable" and "tone follows status"
 * gradually drift across four places.
 */
export interface StatCard {
  key: string;
  label: string;
  value: React.ReactNode;
  hint?: string;
  tone?: "neutral" | "danger" | "warn" | "success";
  /** With `to`, render as a link; otherwise pair with `onClick` to render as a button (used for inline filtering) */
  to?: string;
  onClick?: () => void;
  /** Currently filtering by this card */
  active?: boolean;
  icon?: React.ElementType;
}

const TONE_VALUE: Record<NonNullable<StatCard["tone"]>, string> = {
  neutral: "text-ink-900 dark:text-surface-0",
  danger: "text-rose-600 dark:text-rose-400",
  warn: "text-amber-600 dark:text-amber-400",
  success: "text-emerald-600 dark:text-emerald-400",
};

const TONE_HINT: Record<NonNullable<StatCard["tone"]>, string> = {
  neutral: "text-ink-400",
  danger: "text-rose-600 dark:text-rose-400",
  warn: "text-amber-600 dark:text-amber-400",
  success: "text-emerald-600 dark:text-emerald-400",
};

const COLS: Record<number, string> = {
  2: "grid-cols-2",
  3: "grid-cols-2 lg:grid-cols-3",
  4: "grid-cols-2 lg:grid-cols-4",
  5: "grid-cols-2 lg:grid-cols-5",
};

export function StatCards({
  cards,
  cols = 4,
}: {
  cards: StatCard[];
  cols?: 2 | 3 | 4 | 5;
}) {
  return (
    <div className={cn("grid gap-3 md:gap-4", COLS[cols])}>
      {cards.map((c) => {
        const Icon = c.icon;
        const tone = c.tone ?? "neutral";
        const shell = cn(
          "group text-left bg-surface-0 dark:bg-ink-700 rounded-xl border p-4 transition-colors",
          c.active
            ? "border-brand-500 ring-1 ring-brand-500/30"
            : "border-surface-3 dark:border-ink-700 hover:border-brand-500",
        );
        const body = (
          <>
            <div className="flex items-center justify-between gap-2">
              <span className="text-sm text-ink-500">{c.label}</span>
              {Icon && (
                <Icon className="w-4 h-4 shrink-0 text-ink-400" aria-hidden="true" />
              )}
            </div>
            <div
              className={cn(
                "mt-2 text-2xl font-semibold tabular-nums",
                TONE_VALUE[tone],
              )}
            >
              {c.value}
            </div>
            {c.hint && (
              <div className={cn("mt-1 text-xs", TONE_HINT[tone])}>{c.hint}</div>
            )}
          </>
        );

        if (c.to) {
          return (
            <Link key={c.key} to={c.to} className={shell}>
              {body}
            </Link>
          );
        }
        return (
          <button
            key={c.key}
            type="button"
            onClick={c.onClick}
            aria-pressed={c.active}
            className={cn(shell, "cursor-pointer")}
          >
            {body}
          </button>
        );
      })}
    </div>
  );
}
