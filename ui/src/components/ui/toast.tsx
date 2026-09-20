import * as React from "react";
import {
  AlertTriangle,
  CheckCircle,
  Info,
  X,
  XCircle,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

/**
 * 规范 7.11：所有异步操作必须通过 Toast 反馈，禁止 alert()。
 * success / info 3 秒自动消失；error / warn 必须手动关闭；最多同时 3 条。
 */
export type ToastLevel = "success" | "error" | "warn" | "info";

export interface ToastItem {
  id: number;
  level: ToastLevel;
  message: string;
}

const LEVELS: Record<
  ToastLevel,
  { icon: React.ElementType; bg: string; text: string; bar: string }
> = {
  success: {
    icon: CheckCircle,
    bg: "bg-emerald-50 dark:bg-emerald-700/20",
    text: "text-emerald-700 dark:text-emerald-400",
    bar: "bg-emerald-600",
  },
  error: {
    icon: XCircle,
    bg: "bg-rose-50 dark:bg-rose-700/20",
    text: "text-rose-700 dark:text-rose-400",
    bar: "bg-rose-600",
  },
  warn: {
    icon: AlertTriangle,
    bg: "bg-amber-50 dark:bg-amber-700/20",
    text: "text-amber-700 dark:text-amber-400",
    bar: "bg-amber-600",
  },
  info: {
    icon: Info,
    bg: "bg-brand-50 dark:bg-brand-900",
    text: "text-brand-700 dark:text-brand-100",
    bar: "bg-brand-600",
  },
};

interface ToastContextValue {
  push: (level: ToastLevel, message: string) => void;
}

const ToastContext = React.createContext<ToastContextValue | null>(null);

export function useToast() {
  const ctx = React.useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used inside <ToastProvider>");
  return ctx;
}

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const { t } = useTranslation();
  const [items, setItems] = React.useState<ToastItem[]>([]);
  const nextId = React.useRef(1);

  const remove = React.useCallback((id: number) => {
    setItems((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const push = React.useCallback(
    (level: ToastLevel, message: string) => {
      const id = nextId.current++;
      // 最多 3 条，超出时最早的一条被移除
      setItems((prev) => [...prev, { id, level, message }].slice(-3));
      if (level === "success" || level === "info") {
        window.setTimeout(() => remove(id), 3000);
      }
    },
    [remove],
  );

  const value = React.useMemo(() => ({ push }), [push]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div
        className={cn(
          "fixed top-4 left-1/2 -translate-x-1/2 z-[60] flex flex-col gap-2",
          // 规范 7.11.3：桌面 min 320 / max 480，移动端收窄到视口内（避免窄屏横向溢出）
          "w-[min(480px,calc(100vw-2rem))] min-w-[320px]",
        )}
        role="status"
        aria-live="polite"
      >
        {items.map((item) => {
          const cfg = LEVELS[item.level];
          const Icon = cfg.icon;
          return (
            <div
              key={item.id}
              className={cn(
                "flex items-start gap-3 rounded-lg px-4 py-3 shadow-md animate-panel-slide overflow-hidden relative",
                cfg.bg,
              )}
            >
              <span
                className={cn("absolute left-0 top-0 bottom-0 w-1", cfg.bar)}
                aria-hidden="true"
              />
              <Icon
                className={cn("w-5 h-5 shrink-0 mt-0.5", cfg.text)}
                aria-hidden="true"
              />
              <p className={cn("flex-1 text-sm", cfg.text)}>{item.message}</p>
              <button
                type="button"
                onClick={() => remove(item.id)}
                aria-label={t("action.close")}
                className={cn("shrink-0 opacity-70 hover:opacity-100", cfg.text)}
              >
                <X className="w-4 h-4" aria-hidden="true" />
              </button>
            </div>
          );
        })}
      </div>
    </ToastContext.Provider>
  );
}
