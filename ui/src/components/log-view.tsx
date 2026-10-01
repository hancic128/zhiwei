import * as React from "react";
import { ArrowDown } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

/**
 * 日志视图（容器 / 文件日志共用）。
 *
 * 三件事：
 *  1. 着色：优先还原日志里自带的 ANSI 颜色（很多程序往容器 stdout 写 TTY 色彩），
 *     没有转义序列时按 ERROR / WARN / INFO / DEBUG 这类级别关键字整行着色。
 *  2. 粘底：跟随刷新时新内容自动滚到最底；用户手动上滑就不再抢滚动条，
 *     右下角出现「回到底部」，点一下恢复粘底。
 *  3. 等宽深底排版 + 自动换行（长行不撑出横向滚动条）。
 *
 * 只负责渲染文本，拉取 / 跟随的频率由调用方决定（命令通道要等节点轮询，
 * 盲目并发拉取只会把命令堆在队列里）。
 */
const BOTTOM_EPS = 24;

/** 基础 16 色（前景）→ tailwind 类；深色底所以用浅一档的色值。 */
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

/** 有 ANSI SGR 就把一行切成带样式的片段；没有则返回 null（交给级别着色）。 */
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
      // 背景色 / 下划线等其余属性忽略：深色底上渲染不出可读效果
    }
    last = re.lastIndex;
  }
  if (last < line.length) segs.push({ text: line.slice(last), cls });
  return segs
    .map((s) => ({ ...s, text: s.text.replace(OTHER_ESCAPES, "") }))
    .filter((s) => s.text !== "");
}

/** 没有 ANSI 颜色时按级别整行着色。 */
function lineClass(line: string): string {
  if (/\b(?:ERROR|ERR|FATAL|PANIC|CRITICAL|CRIT|SEVERE)\b/i.test(line) || /错误|失败|异常/.test(line))
    return "text-rose-400";
  if (/\b(?:WARN|WARNING)\b/i.test(line) || /警告/.test(line)) return "text-amber-300";
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
  /** 跟随中：只要视图贴着底，新内容到达就继续贴住 */
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

  // 内容变化 / 打开跟随：贴底状态没被用户打断就继续贴底。
  // 非跟随状态下的内容变化都来自用户主动「拉取」，直接跳到底（新内容才是他要看的）；
  // 跟随中则尊重用户上滑（否则永远读不到历史）。
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
