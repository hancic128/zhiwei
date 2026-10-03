import { cn } from "@/lib/utils";

/**
 * ZhiWei mark = the polyline itself from public/favicon.svg.
 *
 * The mark is just a polyline: no plate, no outline. The interface uses currentColor — the color is decided by
 * the `text-brand-*` class at the call site, following the theme color (vermilion / indigo / emerald...) and light/dark;
 * the tab icon is redrawn by `lib/favicon.ts` using the same rules (light brand-600 / dark brand-500),
 * so both stay in sync. If the polyline changes, favicon.svg and the GLYPH in lib/favicon.ts must change together.
 *
 * The viewBox is one inset tighter than the 32x32 square canvas: after removing the plate, the original padding
 * for the plate would leave the logo floating in the top-left corner, looking too small. Here we crop to the
 * polyline itself (including half of the 2.4 stroke) to leave a bit of breathing room.
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
