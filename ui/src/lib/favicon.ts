/**
 * 标签页图标（favicon）跟随主题色。
 *
 * 静态文件 /favicon.svg 只能是单色的兜底（浏览器标签栏读不到 CSS 变量），
 * 所以进主题 / 切主题时用当前品牌色重新生成一份 data URI 塞回 <link rel="icon">。
 * 折线与 `components/ui/logo.tsx` 的 LogoMark 完全同一条（不加底板、不加边框），
 * 取色也跟侧边栏 logo 一致：浅色用 brand-600、深色用 brand-500。
 */

const GLYPH = "M7 21.5h4.5l2.5-6 3 11 3-14.5 2.5 9.5H25";
const VIEW_BOX = "3.6 8.6 24.8 21.2";

/** 兜底：与 :root 的 --brand-600 相同（朱砂） */
const FALLBACK = "#c73225";

function brandColor(): string {
  const root = document.documentElement;
  const dark = root.classList.contains("dark");
  const token = dark ? "--brand-500" : "--brand-600";
  const value = getComputedStyle(root).getPropertyValue(token).trim();
  return value || FALLBACK;
}

function svg(color: string): string {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${VIEW_BOX}">` +
    `<path d="${GLYPH}" fill="none" stroke="${color}" stroke-width="2.4" ` +
    `stroke-linecap="round" stroke-linejoin="round"/></svg>`
  );
}

export function applyFavicon() {
  const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (!link) return;
  const color = brandColor();
  link.type = "image/svg+xml";
  link.href = `data:image/svg+xml,${encodeURIComponent(svg(color))}`;
}
