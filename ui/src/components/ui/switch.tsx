import { cn } from "@/lib/utils";

/**
 * 二元开关（左关 / 右开）。
 *
 * 用来表达「一直存在的两种状态」，和 Segmented 的区别在语义：
 *   - Segmented 是「在几个视图/口径里选一个」（当前在看什么）；
 *   - Switch 是「这个对象的开关状态」（它是开着还是关着）。
 * 启停类操作（告警规则的启用 / 停用）用这个，别再拿一个会翻字的按钮顶替——
 * 「点一下会变成什么」要靠读文字才知道，开关一眼就看出来了。
 *
 * 受控组件：checked 由外部持有（数据源是服务端），点击只上报目标状态。
 * 无文字：状态靠位置 + 颜色表达，含义靠 aria-label（见各调用点）。
 */
export function Switch({
  checked,
  onCheckedChange,
  disabled,
  className,
  "aria-label": ariaLabel,
}: {
  checked: boolean;
  onCheckedChange: (next: boolean) => void;
  disabled?: boolean;
  className?: string;
  "aria-label": string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onCheckedChange(!checked)}
      className={cn(
        // p-0.5 + justify-* 让滑块靠内边距定位，不用手算 translate-x 的像素值
        "inline-flex h-6 w-11 shrink-0 items-center rounded-full p-0.5",
        "transition-colors duration-150",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40",
        checked
          ? "justify-end bg-emerald-600"
          // 关态在暗色下不能用 ink-700：卡片本身就是 ink-700，会糊成一片只看见滑块
          : "justify-start bg-surface-4 dark:bg-ink-500",
        disabled && "opacity-50 cursor-not-allowed",
        className,
      )}
    >
      <span
        aria-hidden="true"
        className="block h-5 w-5 rounded-full bg-white shadow-sm"
      />
    </button>
  );
}
