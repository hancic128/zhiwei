/**
 * Tab icon (favicon) follows the theme color.
 *
 * The static /favicon.svg file can only be a single-color fallback (browser tab bar can't read CSS variables),
 * so on page load and theme change we regenerate a data URI in the current brand color and inject it
 * back into <link rel="icon">. The polyline is identical to `components/ui/logo.tsx`'s LogoMark
 * (no plate, no outline), and the color matches the sidebar logo: light uses brand-600, dark uses brand-500.
 */

const GLYPH = "M7 21.5h4.5l2.5-6 3 11 3-14.5 2.5 9.5H25";
const VIEW_BOX = "3.6 8.6 24.8 21.2";

/** Fallback: same as :root's --brand-600 (vermilion) */
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
