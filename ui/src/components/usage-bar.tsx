/**
 * Usage progress bar: shared semantics and color thresholds for node / container lists.
 * >80 rose, >60 amber, else emerald; null/undefined/NaN don't render to avoid misleading "0% bar".
 * Input is 0..100 percentage; values outside the range are clamped to 0..100.
 */
export function UsageBar({ pct }: { pct: number | null | undefined }) {
  if (pct === null || pct === undefined || Number.isNaN(pct)) return null;
  const clamped = Math.max(0, Math.min(100, pct));
  const fill =
    pct > 80
      ? "bg-rose-500"
      : pct > 60
        ? "bg-amber-500"
        : "bg-emerald-500";
  return (
    <div
      className="h-1 w-20 rounded-full bg-surface-2 dark:bg-ink-700 overflow-hidden"
      role="progressbar"
      aria-valuenow={Math.round(clamped)}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div
        className={`h-full rounded-full transition-all ${fill}`}
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}