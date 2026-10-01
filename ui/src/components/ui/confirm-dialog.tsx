import * as DialogPrimitive from "@radix-ui/react-dialog";
import { AlertTriangle, Loader2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";

/**
 * 规范 7.10：所有危险/不可逆操作必须复用此模板。
 * 必须带 backdrop-blur 遮罩；确认按钮文案描述动作，禁止「确定」。
 */
export function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel,
  cancelLabel,
  danger = false,
  loading = false,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
  danger?: boolean;
  loading?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  return (
    <DialogPrimitive.Root open={open} onOpenChange={(o) => !o && onCancel()}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay
          className="fixed inset-0 z-[70] bg-ink-900/50 backdrop-blur-sm animate-panel-slide"
        />
        {/* 定位与动画分两层：动画的 transform 会覆盖 -translate-*，同层会跑到视口外 */}
        <DialogPrimitive.Content className="fixed left-1/2 top-1/2 z-[70] w-full max-w-md -translate-x-1/2 -translate-y-1/2 outline-none">
          <div
            className={cn(
              "bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-700",
              "animate-panel-slide",
            )}
          >
            {/* 规范 7.10.2/7.10.3：头部 = 图标 + 标题 + 右上角关闭（X / ESC / 遮罩三种关闭方式并存） */}
            <div className="px-6 py-4 border-b border-surface-3 dark:border-ink-700 flex items-start justify-between gap-3">
              <div className="flex items-start gap-2 min-w-0">
                {danger && (
                  <AlertTriangle
                    className="w-5 h-5 shrink-0 mt-0.5 text-rose-600 dark:text-rose-400"
                    aria-hidden="true"
                  />
                )}
                <DialogPrimitive.Title className="text-base font-semibold text-ink-900 dark:text-surface-0">
                  {title}
                </DialogPrimitive.Title>
              </div>
              <DialogPrimitive.Close
                aria-label={t("action.close")}
                className="shrink-0 rounded p-1 text-ink-400 hover:text-ink-900 dark:hover:text-surface-0 transition-colors"
              >
                <X className="w-4 h-4" aria-hidden="true" />
              </DialogPrimitive.Close>
            </div>

            <div className="px-6 py-4">
              <DialogPrimitive.Description className="text-sm text-ink-500">
                {message}
              </DialogPrimitive.Description>
            </div>

            <div className="px-6 py-4 border-t border-surface-3 dark:border-ink-700 flex justify-end gap-2">
              <Button variant="secondary" onClick={onCancel} disabled={loading}>
                {cancelLabel}
              </Button>
              <Button
                variant={danger ? "danger" : "primary"}
                onClick={onConfirm}
                disabled={loading}
                autoFocus
              >
                {loading && (
                  <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
                )}
                {confirmLabel}
              </Button>
            </div>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
