import * as React from "react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { cn } from "@/lib/utils";

/** 规范 7.15 + 禁止清单：Tooltip 使用固定深色，避免暗色模式下白字白底。 */
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
  /** 宽版提示（长命令 / 长路径）：放宽宽度上限并允许换行 */
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
            // 规范 7.15.1：rounded / px-2 py-1 / text-xs / shadow-md；宽版放宽 max-w
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
