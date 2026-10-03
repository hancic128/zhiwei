import * as React from "react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { cn } from "@/lib/utils";

/** Spec 7.15 + prohibited list: Tooltip uses fixed dark color, avoiding white text on white background in dark mode. */
export const TooltipProvider = TooltipPrimitive.Provider;

export function Tooltip({
  content,
  children,
  side = "right",
  wide = false,
}: {
  content: React.ReactNode;
  children: React.ReactNode;
  side?: "top" | "right" | "bottom" | "left";
  /** Wide tooltip (long commands / long paths): relax the width limit and allow line breaks */
  wide?: boolean;
}) {
  return (
    <TooltipPrimitive.Root delayDuration={300}>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          sideOffset={8}
          className={cn(
            // Spec 7.15.1: rounded / px-2 py-1 / text-xs / shadow-md; wide version relaxes max-w
            "z-50 rounded px-2 py-1 text-xs font-medium shadow-md",
            wide ? "max-w-[480px] break-words" : "max-w-[200px]",
            "animate-panel-slide",
          )}
          style={{ background: "#18181b", color: "#fff" }}
        >
          {content}
          <TooltipPrimitive.Arrow style={{ fill: "#18181b" }} width={8} height={4} />
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
  );
}
