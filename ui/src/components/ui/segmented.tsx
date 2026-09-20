import { cn } from "@/lib/utils";

/** 分段切换（绝对值 / 占比、CPU / 内存）：项目里的统一控件外形 */
export function Segmented({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (v: string) => void;
  options: Array<{ value: string; label: string }>;
}) {
  return (
    <div className="inline-flex rounded-md border border-surface-3 dark:border-ink-700 p-0.5">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          onClick={() => onChange(o.value)}
          aria-pressed={value === o.value}
          className={cn(
            "px-2 py-1 rounded text-xs font-medium transition-colors",
            value === o.value
              ? "bg-brand-600 text-white"
              : "text-ink-500 hover:text-ink-900 dark:hover:text-surface-0",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
