import * as React from "react";
import { Search } from "lucide-react";
import { cn } from "@/lib/utils";

const base =
  "flex h-9 w-full rounded-md border border-surface-3 bg-surface-0 px-3 py-1 text-sm text-ink-900 " +
  "placeholder:text-ink-400 " +
  "focus:border-brand-500 focus:ring-2 focus:ring-brand-500/20 focus:outline-none " +
  "disabled:cursor-not-allowed disabled:opacity-50 " +
  // dark border uses ink-500 not ink-700: dialog/card body is bg-ink-700, and a same-toned
  // border becomes invisible — interactive elements need to read as interactive against
  // any surface they sit on.
  "dark:bg-ink-700 dark:border-ink-500 dark:text-surface-0 dark:placeholder:text-ink-400";

export const Input = React.forwardRef<
  HTMLInputElement,
  React.InputHTMLAttributes<HTMLInputElement>
>(({ className, ...props }, ref) => (
  <input ref={ref} className={cn(base, className)} {...props} />
));
Input.displayName = "Input";

/** Spec 7.4.2: search input, Search icon on the left */
export const SearchInput = React.forwardRef<
  HTMLInputElement,
  React.InputHTMLAttributes<HTMLInputElement>
>(({ className, ...props }, ref) => (
  <div className={cn("relative", className)}>
    <Search
      className="absolute left-3 top-1/2 w-4 h-4 -translate-y-1/2 text-ink-400"
      aria-hidden="true"
    />
    <input ref={ref} className={cn(base, "pl-8")} type="search" {...props} />
  </div>
));
SearchInput.displayName = "SearchInput";
