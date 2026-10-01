import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { BellOff, Loader2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** 静默时长档位（小时），按业务诉求定为 1/3/12/24 */
const PRESETS_HOURS = [1, 3, 12, 24] as const;

/**
 * 把 Date 截到分钟，再转成本地时区 ISO 串供 `<input type="datetime-local">` 使用。
 * `<input>` 的 value 期望形如 `YYYY-MM-DDTHH:mm`（无时区后缀）。
 */
function toLocalInputValue(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` +
    `T${pad(d.getHours())}:${pad(d.getMinutes())}`
  );
}

/** 把 datetime-local 输入框的字符串转回 Date；输入空 / 非法时返回 null。 */
function fromLocalInputValue(v: string): Date | null {
  if (!v) return null;
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? null : d;
}

/**
 * 告警静默对话框（规范 §7.10）。
 *
 * - 顶部说明「这条告警将静默到 xx:xx」
 * - 中间 4 个预设按钮：1 小时 / 3 小时 / 12 小时 / 24 小时；点选即同步到自定义时间
 * - 底部一个「截止到」`<input type="datetime-local">`，可手填任意未来时间
 *   - 用户改了预设：截止时间立刻同步
 *   - 用户改了自定义：清除预设的「选中」标记（表示偏离了预设）
 * - 「确认静默」按钮在截止时间非未来时禁用
 * - 截止时间换算成「整数分钟」调后端，保证和服务端的契约一致
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

  // 初始值 = 当前 + 1 小时；预设默认选中 1h
  const initialUntil = React.useMemo(() => {
    const d = new Date();
    d.setHours(d.getHours() + 1, 0, 0, 0);
    return d;
  }, [open]);
  const [until, setUntil] = React.useState<Date>(() => initialUntil);
  const [selectedPreset, setSelectedPreset] = React.useState<number | null>(1);

  // 每次打开对话框时重置：默认 1 小时
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
    // 自定义时间 → 清除预设高亮（用户改的不是这套档位）
    setSelectedPreset(null);
  };

  const minutes = Math.max(
    1,
    Math.round((until.getTime() - Date.now()) / 60_000),
  );
  const isFuture = until.getTime() > Date.now();
  const canConfirm = isFuture && minutes > 0;

  // 预设按钮旁的「绝对时间」小字（让用户不必脑算）
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
              "bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-700",
              "animate-panel-slide",
            )}
          >
            <div className="px-6 py-4 border-b border-surface-3 dark:border-ink-700 flex items-start justify-between gap-3">
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

              {/* 预设档位：4 个胶囊按钮；选中态用 primary 高亮 */}
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
                          : "border-surface-3 dark:border-ink-700 text-ink-700 dark:text-ink-100 hover:border-brand-300",
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

              {/* 自定义截止时间：datetime-local；非法值（过去 / 空）时禁用确认按钮 */}
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
                    "w-full rounded-lg border border-surface-3 dark:border-ink-700 bg-surface-0 dark:bg-ink-800",
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

            <div className="px-6 py-4 border-t border-surface-3 dark:border-ink-700 flex justify-end gap-2">
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