import * as React from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * 规范 06 组件模式：项目里所有下拉都复用这一个外壳（与 Input 同一套令牌）。
 * 用原生 `<select>` 是为了键盘 / 无障碍零成本；外观全部走令牌，不用浏览器默认样式。
 */
const base =
  "h-9 w-full rounded-md border border-surface-3 bg-surface-0 pl-3 pr-8 text-sm text-ink-900 " +
  "appearance-none cursor-pointer " +
  "focus:border-brand-500 focus:ring-2 focus:ring-brand-500/20 focus:outline-none " +
  "disabled:cursor-not-allowed disabled:opacity-50 " +
  "dark:bg-ink-700 dark:border-ink-700 dark:text-surface-0";

export const Select = React.forwardRef<
  HTMLSelectElement,
  React.SelectHTMLAttributes<HTMLSelectElement> & { wrapperClassName?: string }
>(({ className, wrapperClassName, children, ...props }, ref) => (
  <div className={cn("relative", wrapperClassName)}>
    <select ref={ref} className={cn(base, className)} {...props}>
      {children}
    </select>
    <ChevronDown
      className="absolute right-3 top-1/2 w-4 h-4 -translate-y-1/2 text-ink-400 pointer-events-none"
      aria-hidden="true"
    />
  </div>
));
Select.displayName = "Select";
