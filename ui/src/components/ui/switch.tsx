import { cn } from "@/lib/utils";

/**
 * Binary switch (left off / right on).
 *
 * Used to express "two persistent states"; semantic difference from Segmented:
 *   - Segmented is "pick one of several views/perspectives" (what you're looking at);
 *   - Switch is "the on/off state of this object" (whether it's on or off).
 * Enable/disable operations (alert rule enable/disable) use this — don't substitute a button whose label flips;
 * "what does clicking do" requires reading text, while a switch is recognizable at a glance.
 *
 * Controlled component: checked is held externally (data source is the server); click only reports the target state.
 * No text: state expressed by position + color, meaning carried by aria-label (see each call site).
 */
export function Switch({
  checked,
  onCheckedChange,
  disabled,
  className,
  "aria-label": ariaLabel,
}: {
  checked: boolean;
  onCheckedChange: (next: boolean) => void;
  disabled?: boolean;
  className?: string;
  "aria-label": string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onCheckedChange(!checked)}
      className={cn(
        // p-0.5 + justify-* lets the thumb position itself by padding, no manual translate-x pixel math
        "inline-flex h-6 w-11 shrink-0 items-center rounded-full p-0.5",
        "transition-colors duration-150",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40",
        checked
          ? "justify-end bg-emerald-600"
          // Off state can't use ink-700 in dark mode: the card itself is ink-700, the thumb would be all that's visible
          : "justify-start bg-surface-4 dark:bg-ink-500",
        disabled && "opacity-50 cursor-not-allowed",
        className,
      )}
    >
      <span
        aria-hidden="true"
        className="block h-5 w-5 rounded-full bg-white shadow-sm"
      />
    </button>
  );
}
