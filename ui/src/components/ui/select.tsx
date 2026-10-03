import * as React from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * Spec 06 component pattern: every dropdown in the project reuses this single shell
 * (same token set as Input). Uses native `<select>` for zero keyboard / accessibility cost;
 * styling entirely follows tokens, no browser-default look.
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
