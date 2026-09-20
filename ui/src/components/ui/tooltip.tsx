import * as React from "react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { cn } from "@/lib/utils";

/** 规范 7.15 + 禁止清单：Tooltip 使用固定深色，避免暗色模式下白字白底。 */
export const TooltipProvider = TooltipPrimitive.Provider;

export function Tooltip({
  content,
  children,
  side = "right",
}: {
  content: React.ReactNode;
  children: React.ReactNode;
  side?: "top" | "right" | "bottom" | "left";
}) {
  return (
    <TooltipPrimitive.Root delayDuration={300}>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          sideOffset={8}
          className={cn(
            // 规范 7.15.1：rounded / px-2 py-1 / text-xs / shadow-md / max-w-[200px]
            "z-50 rounded px-2 py-1 text-xs font-medium shadow-md max-w-[200px]",
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
