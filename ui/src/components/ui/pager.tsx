import * as React from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/select";
import { TableFooter } from "@/components/ui/table";

/** Spec 7.3.4: table pagination bar unified shape (page size + prev/next + page x of y) */
export const PAGE_SIZES = [10, 20, 50];

export function TablePager({
  page,
  pageCount,
  pageSize,
  onPage,
  onPageSize,
  left,
  className,
}: {
  page: number;
  pageCount: number;
  pageSize: number;
  onPage: (page: number) => void;
  /** When omitted, hide the "page size" selector (not needed on high information-density pages like the nodes list) */
  onPageSize?: (size: number) => void;
  /** Statistics text in the bottom-left */
  left?: React.ReactNode;
  className?: string;
}) {
  const { t } = useTranslation();
  const current = Math.min(Math.max(page, 1), Math.max(pageCount, 1));

  return (
    <TableFooter className={className ?? "flex-wrap gap-3"}>
      {left ?? <span />}
      <div className="flex items-center gap-2">
        {onPageSize && (
          <Select
            wrapperClassName="w-24"
            className="h-8 text-xs"
            value={pageSize}
            onChange={(e) => onPageSize(Number(e.target.value))}
            aria-label={t("pager.size")}
          >
            {PAGE_SIZES.map((s) => (
              <option key={s} value={s}>
                {t("pager.perPage", { n: s })}
              </option>
            ))}
          </Select>
        )}
        <Button
          variant="secondary"
          size="icon"
          aria-label={t("pager.prev")}
          disabled={current <= 1}
          onClick={() => onPage(current - 1)}
        >
          <ChevronLeft className="w-4 h-4" aria-hidden="true" />
        </Button>
        <span className="text-xs tabular-nums text-ink-500 min-w-12 text-center">
          {t("pager.pageOf", { page: current, pages: Math.max(pageCount, 1) })}
        </span>
        <Button
          variant="secondary"
          size="icon"
          aria-label={t("pager.next")}
          disabled={current >= pageCount}
          onClick={() => onPage(current + 1)}
        >
          <ChevronRight className="w-4 h-4" aria-hidden="true" />
        </Button>
      </div>
    </TableFooter>
  );
}

/** Slice an array into the current page + total page count */
export function paginate<T>(rows: T[], page: number, pageSize: number) {
  const pageCount = Math.max(1, Math.ceil(rows.length / pageSize));
  const current = Math.min(Math.max(page, 1), pageCount);
  return {
    pageCount,
    current,
    visible: rows.slice((current - 1) * pageSize, current * pageSize),
  };
}

/** Generic asc/desc value-getter for sorting lists */
export function sortRows<T>(
  rows: T[],
  dir: "asc" | "desc",
  value: (row: T) => number | string,
): T[] {
  const d = dir === "asc" ? 1 : -1;
  return [...rows].sort((a, b) => {
    const va = value(a);
    const vb = value(b);
    if (typeof va === "string" || typeof vb === "string") {
      return String(va).localeCompare(String(vb)) * d;
    }
    return (va - vb) * d;
  });
}
