/**
 * 用率进度条：节点 / 容器列表共用同一份语义与配色阈值。
 * >80 rose、>60 amber、其余 emerald；null/undefined/NaN 不渲染，避免「0% 长条」的误导。
 * 入参是 0..100 的百分比；超出会被夹到 0..100。
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