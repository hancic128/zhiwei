import { cn } from "@/lib/utils";

/** Segmented switch (absolute / percentage, CPU / memory): unified control shape used across the project */
export function Segmented({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (v: string) => void;
  options: Array<{ value: string; label: string }>;
}) {
  // See input.tsx: dialogs are bg-ink-700, so the border must be one tone lighter
  // (ink-500) to read as an interactive control inside a modal.
  return (
    <div className="inline-flex rounded-md border border-surface-3 dark:border-ink-500 p-0.5">
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
