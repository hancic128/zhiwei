import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { BellOff, Loader2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** Silence duration tiers (in hours), set to 1/3/12/24 per business requirement */
const PRESETS_HOURS = [1, 3, 12, 24] as const;

/**
 * Truncate a Date to the minute, then convert it to a local-time-zone ISO
 * string for `<input type="datetime-local">`. The `<input>` value is expected
 * to look like `YYYY-MM-DDTHH:mm` (no timezone suffix).
 */
function toLocalInputValue(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` +
    `T${pad(d.getHours())}:${pad(d.getMinutes())}`
  );
}

/** Convert a datetime-local input string back to Date; returns null on empty / invalid input. */
function fromLocalInputValue(v: string): Date | null {
  if (!v) return null;
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? null : d;
}

/**
 * Alert silence dialog (spec §7.10).
 *
 * - Header explains "this alert will be silenced until xx:xx"
 * - Middle row: 4 preset buttons (1 hour / 3 hours / 12 hours / 24 hours);
 *   picking one syncs to the custom time field
 * - Bottom: an "until" `<input type="datetime-local">` for any future time
 *   - User changed a preset: the until time syncs immediately
 *   - User changed the custom time: clears the "selected" marker on the
 *     presets (meaning it has deviated from them)
 * - "Confirm silence" button is disabled when the until time isn't in the future
 * - Until time is converted to whole minutes before calling the backend,
 *   to match the server contract
 */
export function SilenceDialog({
  open,
  loading = false,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  loading?: boolean;
  onConfirm: (minutes: number) => void;
  onCancel: () => void;
}) {
  const { t, i18n } = useTranslation();
  const locale = i18n.language || "en-US";

  // Initial value = now + 1 hour; default preset is 1h
  const initialUntil = React.useMemo(() => {
    const d = new Date();
    d.setHours(d.getHours() + 1, 0, 0, 0);
    return d;
  }, [open]);
  const [until, setUntil] = React.useState<Date>(() => initialUntil);
  const [selectedPreset, setSelectedPreset] = React.useState<number | null>(1);

  // Reset on every dialog open: default 1 hour
  React.useEffect(() => {
    if (!open) return;
    const d = new Date();
    d.setHours(d.getHours() + 1, 0, 0, 0);
    setUntil(d);
    setSelectedPreset(1);
  }, [open]);

  const pickPreset = (h: number) => {
    const d = new Date();
    d.setHours(d.getHours() + h, 0, 0, 0);
    setUntil(d);
    setSelectedPreset(h);
  };

  const onCustomChange = (v: string) => {
    const d = fromLocalInputValue(v);
    if (!d) return;
    setUntil(d);
    // Custom time -> clear preset highlight (user picked something outside this tier set)
    setSelectedPreset(null);
  };

  const minutes = Math.max(
    1,
    Math.round((until.getTime() - Date.now()) / 60_000),
  );
  const isFuture = until.getTime() > Date.now();
  const canConfirm = isFuture && minutes > 0;

  // "Absolute time" caption next to each preset button (so users don't have to do the math)
  const presetUntilLabel = (h: number) => {
    const d = new Date();
    d.setHours(d.getHours() + h, 0, 0, 0);
    return d.toLocaleTimeString(locale, {
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });
  };

  return (
    <DialogPrimitive.Root open={open} onOpenChange={(o) => !o && onCancel()}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-[70] bg-ink-900/50 backdrop-blur-sm animate-panel-slide" />
        <DialogPrimitive.Content className="fixed left-1/2 top-1/2 z-[70] w-full max-w-md -translate-x-1/2 -translate-y-1/2 outline-none">
          <div
            className={cn(
              "bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-500",
              "animate-panel-slide",
            )}
          >
            <div className="px-6 py-4 border-b border-surface-3 dark:border-ink-500 flex items-start justify-between gap-3">
              <div className="flex items-start gap-2 min-w-0">
                <BellOff
                  className="w-5 h-5 shrink-0 mt-0.5 text-ink-500"
                  aria-hidden="true"
                />
                <DialogPrimitive.Title className="text-base font-semibold text-ink-900 dark:text-surface-0">
                  {t("alerts.silenceTitle")}
                </DialogPrimitive.Title>
              </div>
              <DialogPrimitive.Close
                aria-label={t("action.close")}
                className="shrink-0 rounded p-1 text-ink-400 hover:text-ink-900 dark:hover:text-surface-0 transition-colors"
              >
                <X className="w-4 h-4" aria-hidden="true" />
              </DialogPrimitive.Close>
            </div>

            <div className="px-6 py-4 space-y-4">
              <DialogPrimitive.Description className="text-sm text-ink-500">
                {t("alerts.silenceDesc")}
              </DialogPrimitive.Description>

              {/* Preset tiers: 4 pill buttons; selected state highlighted via primary color */}
              <div
                className="grid grid-cols-4 gap-2"
                role="radiogroup"
                aria-label={t("alerts.silenceTitle")}
              >
                {PRESETS_HOURS.map((h) => {
                  const active = selectedPreset === h;
                  return (
                    <button
                      key={h}
                      type="button"
                      role="radio"
                      aria-checked={active}
                      onClick={() => pickPreset(h)}
                      className={cn(
                        "rounded-lg border px-2 py-2 text-sm transition-colors",
                        "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500",
                        active
                          ? "border-brand-500 bg-brand-50 text-brand-700 dark:bg-brand-900/30 dark:text-brand-100"
                          : "border-surface-3 dark:border-ink-500 text-ink-700 dark:text-ink-100 hover:border-brand-300",
                      )}
                    >
                      <div className="font-medium tabular-nums">
                        {t("alerts.silenceHours", { n: h, count: h })}
                      </div>
                      <div className="text-[11px] tabular-nums text-ink-400 mt-0.5">
                        {presetUntilLabel(h)}
                      </div>
                    </button>
                  );
                })}
              </div>

              {/* Custom until time: datetime-local; confirm disabled on invalid value (past / empty) */}
              <div>
                <label
                  htmlFor="silence-until"
                  className="block text-xs font-medium text-ink-500 mb-1"
                >
                  {t("alerts.silenceUntil")}
                </label>
                <input
                  id="silence-until"
                  type="datetime-local"
                  value={toLocalInputValue(until)}
                  onChange={(e) => onCustomChange(e.target.value)}
                  className={cn(
                    "w-full rounded-lg border border-surface-3 dark:border-ink-500 bg-surface-0 dark:bg-ink-800",
                    "px-3 py-2 text-sm text-ink-900 dark:text-surface-0",
                    "focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500",
                  )}
                />
                {!isFuture && (
                  <p className="mt-1 text-xs text-rose-600 dark:text-rose-400">
                    {t("alerts.silenceUntilPast")}
                  </p>
                )}
              </div>
            </div>

            <div className="px-6 py-4 border-t border-surface-3 dark:border-ink-500 flex justify-end gap-2">
              <Button variant="secondary" onClick={onCancel} disabled={loading}>
                {t("action.cancel")}
              </Button>
              <Button
                variant="primary"
                onClick={() => onConfirm(minutes)}
                disabled={!canConfirm || loading}
                autoFocus
              >
                {loading && (
                  <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
                )}
                {t("alerts.silenceConfirm", {
                  at: until.toLocaleString(locale, {
                    month: "short",
                    day: "numeric",
                    hour: "2-digit",
                    minute: "2-digit",
                    hour12: false,
                  }),
                })}
              </Button>
            </div>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}