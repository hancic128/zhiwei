import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

/**
 * Generic dialog (spec 7.10/7.14: must have backdrop-blur mask, title, close button).
 * Use this for read-only info (basic info, container logs); for destructive operations needing confirmation, use ConfirmDialog.
 */
export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  bodyClassName,
  size = "lg",
}: {
  open: boolean;
  onClose: () => void;
  title: React.ReactNode;
  description?: React.ReactNode;
  children: React.ReactNode;
  footer?: React.ReactNode;
  /** Classes appended to the body container (e.g. `space-y-4`); by default the body only has padding */
  bodyClassName?: string;
  size?: "md" | "lg" | "xl";
}) {
  const { t } = useTranslation();
  const width =
    size === "md" ? "max-w-md" : size === "xl" ? "max-w-3xl" : "max-w-lg";
  return (
    <DialogPrimitive.Root open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-[70] bg-ink-900/50 backdrop-blur-sm animate-panel-slide" />
        {/* Positioning and animation split across two layers: animate-panel-slide's transform overrides
            the -translate-* centering; putting them on the same element pushes the dialog off-screen */}
        <DialogPrimitive.Content
          className={cn(
            "fixed left-1/2 top-1/2 z-[70] w-full -translate-x-1/2 -translate-y-1/2 outline-none",
            width,
          )}
        >
          <div
            className={cn(
              "max-h-[90vh] flex flex-col overflow-hidden",
              "bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-700",
              "animate-panel-slide",
            )}
          >
            <div className="px-6 py-4 border-b border-surface-3 dark:border-ink-700 flex items-start justify-between gap-4">
              <div className="min-w-0">
                <DialogPrimitive.Title className="text-base font-semibold text-ink-900 dark:text-surface-0">
                  {title}
                </DialogPrimitive.Title>
                {description && (
                  <DialogPrimitive.Description className="mt-0.5 text-sm text-ink-500">
                    {description}
                  </DialogPrimitive.Description>
                )}
              </div>
              <DialogPrimitive.Close
                aria-label={t("action.close")}
                className="shrink-0 rounded p-1 text-ink-400 hover:text-ink-900 dark:hover:text-surface-0 transition-colors"
              >
                <X className="w-4 h-4" aria-hidden="true" />
              </DialogPrimitive.Close>
            </div>

            <div
              className={cn(
                "px-6 py-4 overflow-y-auto scrollbar-thin",
                bodyClassName,
              )}
            >
              {children}
            </div>

            {footer && (
              <div className="px-6 py-4 border-t border-surface-3 dark:border-ink-700 flex justify-end gap-2">
                {footer}
              </div>
            )}
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
