import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

/** Spec 7.1: only 4 variants, no extension allowed. */
const buttonVariants = cva(
  // Spec 08: hover color change 150ms + click active:scale-95.
  // Two transition utility classes would override each other, so we list the needed properties explicitly.
  "inline-flex items-center justify-center gap-2 rounded-lg text-sm font-medium " +
    "transition-[color,background-color,border-color,transform] duration-150 " +
    "active:scale-95 " +
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40 " +
    "disabled:opacity-50 disabled:cursor-not-allowed",
  {
    variants: {
      variant: {
        primary: "bg-brand-600 text-white hover:bg-brand-700 dark:bg-brand-500",
        secondary:
          "bg-surface-0 border border-surface-3 text-ink-700 hover:bg-surface-2 " +
          "dark:bg-ink-700 dark:border-ink-700 dark:text-surface-0 dark:hover:bg-ink-700/70",
        ghost: "text-brand-600 hover:text-brand-700 dark:text-brand-500",
        danger: "bg-rose-600 text-white hover:bg-rose-700",
      },
      size: {
        default: "px-4 py-2 text-sm",
        sm: "px-3 py-1.5 text-xs",
        icon: "w-9 h-9 flex items-center justify-center",
      },
    },
    defaultVariants: { variant: "primary", size: "default" },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  /** Spec 08: when loading, replace text with Loader2 + animate-spin, button disabled */
  loading?: boolean;
}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  (
    { className, variant, size, loading, disabled, children, ...props },
    ref,
  ) => (
    <button
      ref={ref}
      className={cn(buttonVariants({ variant, size }), className)}
      disabled={disabled || loading}
      aria-disabled={disabled || loading || undefined}
      {...props}
    >
      {loading && (
        <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
      )}
      {children}
    </button>
  ),
);
Button.displayName = "Button";
