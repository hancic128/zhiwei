import { cn } from "@/lib/utils";

/**
 * 知微标识 = public/favicon.svg 里的那条折线本身。
 *
 * 标识只有折线：不加底板、不加描边。界面里用 currentColor —— 颜色由调用点给的
 * `text-brand-*` 决定，跟着主题色（朱砂/靛蓝/翡翠…）与明暗走；标签页图标由
 * `lib/favicon.ts` 按同一套规则（浅色 brand-600 / 深色 brand-500）重画，
 * 两边始终同色。折线若改，favicon.svg 与 lib/favicon.ts 的 GLYPH 要一起改。
 *
 * viewBox 比 32x32 的方形画布窄一圈：去掉底板后，原来为底板留的空白会让 logo
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
