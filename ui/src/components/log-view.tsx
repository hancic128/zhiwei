import * as React from "react";
import { ArrowDown } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

/**
 * Log view (shared by container / file logs).
 *
 * Three concerns:
 *  1. Coloring: prefer ANSI colors embedded in the log (many programs write
 *     TTY colors to container stdout); if no escape sequences are present,
 *     fall back to per-line color by level keywords like ERROR / WARN /
 *     INFO / DEBUG.
 *  2. Stick-to-bottom: while following, new content auto-scrolls to the
 *     bottom; once the user scrolls up, we stop grabbing the scrollbar and
 *     surface a "back to bottom" affordance.
 *  3. Monospace dark layout + auto-wrap (long lines don't push a horizontal
 *     scrollbar).
 *
 * This component only renders text. Fetching / follow cadence is the
 * caller's responsibility — the command channel has to wait for the node's
 * poll, and blind concurrent fetches just queue commands up.
 */
const BOTTOM_EPS = 24;

/** Basic 16 foreground colors → tailwind classes; dark background uses lighter shades. */
const ANSI_FG: Record<number, string> = {
  30: "text-ink-400",
  31: "text-rose-400",
  32: "text-emerald-400",
  33: "text-amber-300",
  34: "text-sky-400",
  35: "text-fuchsia-400",
  36: "text-cyan-300",
  37: "text-surface-0",
  90: "text-ink-500",
  91: "text-rose-300",
  92: "text-emerald-300",
  93: "text-amber-200",
  94: "text-sky-300",
  95: "text-fuchsia-300",
  96: "text-cyan-200",
  97: "text-white",
};

const OTHER_ESCAPES = /\u001b\[[0-9;?]*[A-Za-z]/g;

type Seg = { text: string; cls: string };

/** If the line has ANSI SGR sequences, split it into styled segments; otherwise return null (fall through to level-based coloring). */
function ansiSegments(line: string): Seg[] | null {
  if (!line.includes("\u001b[")) return null;
  const segs: Seg[] = [];
  const re = /\u001b\[([0-9;]*)m/g;
  let cls = "";
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(line)) !== null) {
    if (m.index > last) segs.push({ text: line.slice(last, m.index), cls });
    const codes = m[1] === "" ? [0] : m[1].split(";").map((c) => Number(c) || 0);
    for (const c of codes) {
      if (c === 0) cls = "";
      else if (ANSI_FG[c]) cls = ANSI_FG[c];
      else if (c === 1) cls = cn(cls, "font-semibold");
      else if (c === 22) cls = cls.split(" ").filter((x) => x !== "font-semibold").join(" ");
      // Ignore other attributes (background / underline): they don't render readably on a dark background
    }
    last = re.lastIndex;
  }
  if (last < line.length) segs.push({ text: line.slice(last), cls });
  return segs
    .map((s) => ({ ...s, text: s.text.replace(OTHER_ESCAPES, "") }))
    .filter((s) => s.text !== "");
}

/** No ANSI color present — color the whole line by level keywords. */
function lineClass(line: string): string {
  if (/\b(?:ERROR|ERR|FATAL|PANIC|CRITICAL|CRIT|SEVERE)\b/i.test(line) || /error|fail|exception/i.test(line))
    return "text-rose-400";
  if (/\b(?:WARN|WARNING)\b/i.test(line) || /warn/i.test(line)) return "text-amber-300";
  if (/\b(?:INFO|NOTICE)\b/i.test(line)) return "text-sky-300";
  if (/\b(?:DEBUG|TRACE|VERBOSE)\b/i.test(line)) return "text-ink-400";
  return "";
}

export function LogView({
  text,
  follow,
  heightClass = "max-h-[60vh]",
  className,
}: {
  text: string;
  /** While following: as long as the view is pinned to the bottom, stick to bottom whenever new content arrives */
  follow: boolean;
  heightClass?: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const ref = React.useRef<HTMLDivElement | null>(null);
  const stick = React.useRef(true);
  const [atBottom, setAtBottom] = React.useState(true);

  const lines = React.useMemo(
    () =>
      text
        .replace(/\r\n?/g, "\n")
        .replace(/\n$/, "")
        .split("\n")
        .map((line) => ({ raw: line, segs: ansiSegments(line), cls: lineClass(line) })),
    [text],
  );

  const scrollToBottom = React.useCallback((smooth = false) => {
    const el = ref.current;
    if (!el) return;
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? "smooth" : "auto" });
    stick.current = true;
    setAtBottom(true);
  }, []);

  // Content change / follow turned on: continue sticking to the bottom unless the user has interrupted it.
  // Content changes in non-follow mode always come from a user-initiated fetch — jump to bottom (that's what they want to see).
  // While following, respect upward scrolling (otherwise historical lines are unreachable).
  const wasFollow = React.useRef(follow);
  React.useLayoutEffect(() => {
    const turnedOn = follow && !wasFollow.current;
    wasFollow.current = follow;
    if (!follow || turnedOn) stick.current = true;
    if (stick.current) scrollToBottom();
  }, [text, follow, scrollToBottom]);

  const onScroll = () => {
    const el = ref.current;
    if (!el) return;
    const dist = el.scrollHeight - el.scrollTop - el.clientHeight;
    const bottom = dist <= BOTTOM_EPS;
    stick.current = bottom;
    setAtBottom((prev) => (prev === bottom ? prev : bottom));
  };

  return (
    <div className={cn("relative", className)}>
      <div
        ref={ref}
        onScroll={onScroll}
        className={cn(
          "overflow-auto scrollbar-thin rounded-lg bg-ink-900 px-4 py-3 text-xs font-mono leading-relaxed text-surface-3 whitespace-pre-wrap break-all",
          heightClass,
        )}
      >
        {lines.map((l, i) => (
          <div key={i} className={l.segs ? undefined : l.cls}>
            {l.segs
              ? l.segs.map((s, j) => (
                  <span key={j} className={s.cls}>
                    {s.text}
                  </span>
                ))
              : l.raw || "\u00a0"}
          </div>
        ))}
      </div>
      {!atBottom && (
        <button
          type="button"
          onClick={() => scrollToBottom(true)}
          className="absolute bottom-3 right-3 inline-flex items-center gap-1 rounded-full bg-brand-600 px-3 py-1.5 text-xs font-medium text-white shadow-lg hover:bg-brand-700 transition-colors"
        >
          <ArrowDown className="w-3.5 h-3.5" aria-hidden="true" />
          {t("logs.backToBottom")}
        </button>
      )}
    </div>
  );
}
