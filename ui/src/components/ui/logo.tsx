import { cn } from "@/lib/utils";

/**
 * 知微标识 = public/favicon.svg 里的那条折线本身。
 *
 * favicon 是「朱砂底板 + 素色折线」，因为浏览器标签栏拿不到当前主题色，
 * 只能写死；界面里的 logo 不需要底板，改用 currentColor —— 于是颜色由
 * 调用点给的 `text-brand-*` 决定，跟着主题色（朱砂/靛蓝/翡翠…）与明暗走。
 *
 * viewBox 比 favicon 窄一圈：去掉底板后，原来为底板留的空白就显得 logo
 * 悬在左上角、偏小。这里裁到折线本身（含 2.4 描边的一半）留一点呼吸。
 */
const GLYPH = "M7 21.5h4.5l2.5-6 3 11 3-14.5 2.5 9.5H25";

export function LogoMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="3.6 8.6 24.8 21.2"
      className={cn("shrink-0", className)}
      aria-hidden="true"
    >
      <path
        d={GLYPH}
        fill="none"
        stroke="currentColor"
        strokeWidth={2.4}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
