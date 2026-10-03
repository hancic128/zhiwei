import { ArrowDown, ArrowUp, ArrowUpDown } from "lucide-react";
import { Th } from "@/components/ui/table";
import { cn } from "@/lib/utils";

export type SortDir = "asc" | "desc";

/** Sortable header (spec 7.3.1 Th appearance + sort icons, click toggles asc/desc) */
export function SortHeader<K extends string>({
  label,
  k,
  sortKey,
  sortDir,
  onSort,
  align = "left",
  className,
}: {
  label: string;
  k: K;
  sortKey: K;
  sortDir: SortDir;
  onSort: (k: K) => void;
  align?: "left" | "right";
  className?: string;
}) {
  const active = sortKey === k;
  // Spec 7.3.2: unsorted ArrowUpDown / asc ArrowUp / desc ArrowDown, size w-3 h-3
  const Icon = !active ? ArrowUpDown : sortDir === "asc" ? ArrowUp : ArrowDown;
  return (
    <Th
      className={className}
      align={align}
      aria-sort={
        active ? (sortDir === "asc" ? "ascending" : "descending") : "none"
      }
    >
      <button
        type="button"
        onClick={() => onSort(k)}
        className={cn(
          "inline-flex items-center gap-1 uppercase tracking-wider transition-colors hover:text-ink-900 dark:hover:text-surface-0",
          active && "text-ink-900 dark:text-surface-0",
        )}
      >
        {label}
        <Icon
          className={cn("w-3 h-3", active && "text-brand-600")}
          aria-hidden="true"
        />
      </button>
    </Th>
  );
}
