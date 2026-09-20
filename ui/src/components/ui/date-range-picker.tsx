import * as React from "react";
import * as PopoverPrimitive from "@radix-ui/react-popover";
import dayjs, { type Dayjs } from "dayjs";
import { Calendar, ChevronDown, ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/select";
import { usePrefs } from "@/components/prefs-provider";
import { cn } from "@/lib/utils";

/**
 * 快捷范围：原需求点名的 8 档，加两档长窗口。
 *
 * 90d / 365d 走小时聚合（原始 10 秒数据只保留 14 天，见
 * crates/monitor-server/src/retention.rs）——这是「能看三个月趋势」的前提。
 */
export const RANGE_PRESETS = [
  { key: "30m", minutes: 30 },
  { key: "1h", minutes: 60 },
  { key: "3h", minutes: 180 },
  { key: "12h", minutes: 720 },
  { key: "1d", minutes: 1440 },
  { key: "3d", minutes: 4320 },
  { key: "7d", minutes: 10080 },
  { key: "30d", minutes: 43200 },
  { key: "90d", minutes: 129600 },
  { key: "365d", minutes: 525600 },
] as const;

export type RangePresetKey = (typeof RANGE_PRESETS)[number]["key"];

export interface TimeRange {
  /** 预设档位；有值时 from/to 由调用方按「当前时刻」滚动计算，图表才会跟着刷新走 */
  preset?: RangePresetKey;
  from: number;
  to: number;
}

export const presetRange = (key: RangePresetKey): TimeRange => {
  const { from, to } = presetBounds(key, Date.now());
  return { preset: key, from, to };
};

/** 预设档位在 `now` 时刻的起止（调用方按刷新频率重算，范围就跟着现在走） */
export const presetBounds = (key: RangePresetKey, now: number) => {
  const preset = RANGE_PRESETS.find((p) => p.key === key)!;
  return { from: now - preset.minutes * 60_000, to: now };
};

const MINUTE_STEPS = [0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55];

/**
 * 时间范围控件（规范：日期范围必须带左侧快捷范围列，禁止原生日期控件）。
 *
 * 左列快捷范围；右侧月历自定义起止——标题点一次进月份视图、再点一次进年份视图，
 * 与「日/月/年三级快速跳转」的要求对应。起止时刻各带时:分下拉。
 */
export function TimeRangePicker({
  value,
  onChange,
}: {
  value: TimeRange;
  onChange: (range: TimeRange) => void;
}) {
  const { t } = useTranslation();
  const { timezone } = usePrefs();
  const [open, setOpen] = React.useState(false);
  const [view, setView] = React.useState<"day" | "month" | "year">("day");
  const [cursor, setCursor] = React.useState<Dayjs>(() =>
    dayjs(value.from).tz(timezone).startOf("month"),
  );
  const [draftFrom, setDraftFrom] = React.useState<number | null>(value.from);
  const [draftTo, setDraftTo] = React.useState<number | null>(value.to);

  // 打开面板时把草稿对齐到当前取值，避免上次未应用的改动残留
  React.useEffect(() => {
    if (!open) return;
    setDraftFrom(value.from);
    setDraftTo(value.to);
    setCursor(dayjs(value.from).tz(timezone).startOf("month"));
    setView("day");
  }, [open, value.from, value.to, timezone]);

  const fmt = (ms: number) => dayjs(ms).tz(timezone).format("MM-DD HH:mm");
  const label = value.preset ? t(`range.${value.preset}`) : `${fmt(value.from)} → ${fmt(value.to)}`;

  const pickDay = (d: Dayjs) => {
    if (draftFrom === null || draftTo !== null) {
      setDraftFrom(d.startOf("day").valueOf());
      setDraftTo(null);
      return;
    }
    if (d.isBefore(dayjs(draftFrom).tz(timezone), "day")) {
      setDraftFrom(d.startOf("day").valueOf());
      return;
    }
    setDraftTo(d.endOf("day").valueOf());
  };

  const setTime = (which: "from" | "to", hour: number, minute: number) => {
    const cur = which === "from" ? draftFrom : draftTo;
    if (cur === null) return;
    const next = dayjs(cur).tz(timezone).hour(hour).minute(minute).second(0).valueOf();
    if (which === "from") setDraftFrom(next);
    else setDraftTo(next);
  };

  const canApply =
    draftFrom !== null && draftTo !== null && draftTo > draftFrom;

  return (
    <PopoverPrimitive.Root open={open} onOpenChange={setOpen}>
      <PopoverPrimitive.Trigger asChild>
        <Button variant="secondary" size="sm" aria-label={t("range.title")}>
          <Calendar className="w-4 h-4" aria-hidden="true" />
          <span className="tabular-nums">{label}</span>
          <ChevronDown className="w-4 h-4 text-ink-400" aria-hidden="true" />
        </Button>
      </PopoverPrimitive.Trigger>
      <PopoverPrimitive.Portal>
        <PopoverPrimitive.Content
          align="end"
          sideOffset={8}
          className={cn(
            "z-50 rounded-xl border border-surface-3 dark:border-ink-700 bg-surface-0 dark:bg-ink-700",
            "shadow-lg animate-panel-slide p-4 flex flex-col sm:flex-row gap-4",
          )}
        >
          {/* 左：快捷范围列 */}
          <div className="sm:w-32 shrink-0">
            <p className="text-xs font-semibold text-ink-400 uppercase tracking-wider mb-2">
              {t("range.quick")}
            </p>
            <div className="flex flex-wrap sm:flex-col gap-1">
              {RANGE_PRESETS.map((p) => (
                <button
                  key={p.key}
                  type="button"
                  onClick={() => {
                    onChange(presetRange(p.key));
                    setOpen(false);
                  }}
                  className={cn(
                    "px-2 py-1.5 rounded-md text-left text-sm transition-colors",
                    value.preset === p.key
                      ? "bg-brand-50 text-brand-700 dark:bg-brand-900 dark:text-brand-100"
                      : "text-ink-700 dark:text-surface-0 hover:bg-surface-2 dark:hover:bg-ink-700/70",
                  )}
                >
                  {t(`range.${p.key}`)}
                </button>
              ))}
            </div>
          </div>

          {/* 右：自定义范围（月历 + 时:分） */}
          <div>
            <div className="flex items-center justify-between gap-2 mb-2">
              <Button
                variant="secondary"
                size="icon"
                aria-label={t("range.prevMonth")}
                onClick={() => {
                  if (view === "day") setCursor(cursor.subtract(1, "month"));
                  else if (view === "month") setCursor(cursor.subtract(1, "year"));
                  else setCursor(cursor.subtract(12, "year"));
                }}
              >
                <ChevronLeft className="w-4 h-4" aria-hidden="true" />
              </Button>
              <button
                type="button"
                className="text-sm font-semibold text-ink-900 dark:text-surface-0 hover:text-brand-600 transition-colors"
                onClick={() =>
                  setView(view === "day" ? "month" : view === "month" ? "year" : "day")
                }
              >
                {view === "day"
                  ? t("range.yearMonth", { y: cursor.year(), m: cursor.month() + 1 })
                  : t("range.year", { y: cursor.year() })}
              </button>
              <Button
                variant="secondary"
                size="icon"
                aria-label={t("range.nextMonth")}
                onClick={() => {
                  if (view === "day") setCursor(cursor.add(1, "month"));
                  else if (view === "month") setCursor(cursor.add(1, "year"));
                  else setCursor(cursor.add(12, "year"));
                }}
              >
                <ChevronRight className="w-4 h-4" aria-hidden="true" />
              </Button>
            </div>

            {view === "day" && (
              <DayGrid
                cursor={cursor}
                timezone={timezone}
                draftFrom={draftFrom}
                draftTo={draftTo}
                onPick={pickDay}
              />
            )}
            {view === "month" && (
              <div className="grid grid-cols-3 gap-1 w-[252px]">
                {Array.from({ length: 12 }, (_, i) => (
                  <button
                    key={i}
                    type="button"
                    onClick={() => {
                      setCursor(cursor.month(i));
                      setView("day");
                    }}
                    className={cn(
                      "py-2 rounded-md text-sm transition-colors",
                      cursor.month() === i
                        ? "bg-brand-600 text-white"
                        : "text-ink-700 dark:text-surface-0 hover:bg-surface-2 dark:hover:bg-ink-700/70",
                    )}
                  >
                    {t("range.month", { m: i + 1 })}
                  </button>
                ))}
              </div>
            )}
            {view === "year" && (
              <div className="grid grid-cols-3 gap-1 w-[252px]">
                {Array.from({ length: 12 }, (_, i) => {
                  const y = cursor.year() - 5 + i;
                  return (
                    <button
                      key={y}
                      type="button"
                      onClick={() => {
                        setCursor(cursor.year(y));
                        setView("month");
                      }}
                      className={cn(
                        "py-2 rounded-md text-sm tabular-nums transition-colors",
                        cursor.year() === y
                          ? "bg-brand-600 text-white"
                          : "text-ink-700 dark:text-surface-0 hover:bg-surface-2 dark:hover:bg-ink-700/70",
                      )}
                    >
                      {y}
                    </button>
                  );
                })}
              </div>
            )}

            <div className="mt-3 grid grid-cols-2 gap-3">
              <TimeField
                label={t("range.start")}
                value={draftFrom}
                timezone={timezone}
                onChange={(h, m) => setTime("from", h, m)}
              />
              <TimeField
                label={t("range.end")}
                value={draftTo}
                timezone={timezone}
                onChange={(h, m) => setTime("to", h, m)}
              />
            </div>

            <div className="mt-3 flex justify-end gap-2">
              <Button variant="secondary" size="sm" onClick={() => setOpen(false)}>
                {t("action.cancel")}
              </Button>
              <Button
                size="sm"
                disabled={!canApply}
                onClick={() => {
                  if (draftFrom === null || draftTo === null) return;
                  onChange({ from: draftFrom, to: draftTo });
                  setOpen(false);
                }}
              >
                {t("action.apply")}
              </Button>
            </div>
          </div>
        </PopoverPrimitive.Content>
      </PopoverPrimitive.Portal>
    </PopoverPrimitive.Root>
  );
}

function DayGrid({
  cursor,
  timezone,
  draftFrom,
  draftTo,
  onPick,
}: {
  cursor: Dayjs;
  timezone: string;
  draftFrom: number | null;
  draftTo: number | null;
  onPick: (d: Dayjs) => void;
}) {
  const { t } = useTranslation();
  const first = cursor.startOf("month");
  // 周一开头：dayjs 的 day() 周日是 0
  const lead = (first.day() + 6) % 7;
  const start = first.subtract(lead, "day");
  const days = Array.from({ length: 42 }, (_, i) => start.add(i, "day"));
  const weekdays = [0, 1, 2, 3, 4, 5, 6].map((i) => t(`range.weekday.${i}`));

  const sameDay = (a: number | null, d: Dayjs) =>
    a !== null && dayjs(a).tz(timezone).isSame(d, "day");
  const inRange = (d: Dayjs) => {
    if (draftFrom === null || draftTo === null) return false;
    const day = d.tz(timezone).endOf("day").valueOf();
    const startOfDay = d.tz(timezone).startOf("day").valueOf();
    return startOfDay >= draftFrom && day <= draftTo;
  };

  return (
    <div className="w-[252px]">
      <div className="grid grid-cols-7 mb-1">
        {weekdays.map((w) => (
          <span key={w} className="text-center text-xs text-ink-400 py-1">
            {w}
          </span>
        ))}
      </div>
      <div className="grid grid-cols-7 gap-y-0.5">
        {days.map((d) => {
          const isStart = sameDay(draftFrom, d);
          const isEnd = sameDay(draftTo, d);
          const muted = d.month() !== cursor.month();
          return (
            <button
              key={d.valueOf()}
              type="button"
              onClick={() => onPick(d)}
              className={cn(
                "h-8 rounded-md text-xs tabular-nums transition-colors",
                isStart || isEnd
                  ? "bg-brand-600 text-white"
                  : inRange(d)
                    ? "bg-brand-50 text-brand-700 dark:bg-brand-900 dark:text-brand-100"
                    : muted
                      ? "text-ink-400 hover:bg-surface-2 dark:hover:bg-ink-700/70"
                      : "text-ink-700 dark:text-surface-0 hover:bg-surface-2 dark:hover:bg-ink-700/70",
              )}
            >
              {d.date()}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function TimeField({
  label,
  value,
  timezone,
  onChange,
}: {
  label: string;
  value: number | null;
  timezone: string;
  onChange: (hour: number, minute: number) => void;
}) {
  const d = value === null ? null : dayjs(value).tz(timezone);
  const minute = d ? Math.round(d.minute() / 5) * 5 % 60 : 0;
  return (
    <div>
      <p className="text-xs text-ink-400 mb-1">{label}</p>
      <div className="flex items-center gap-1">
        <Select
          className="h-8 px-2 pr-8 text-xs tabular-nums"
          value={d ? d.hour() : ""}
          disabled={!d}
          onChange={(e) => onChange(Number(e.target.value), minute)}
          aria-label={label}
        >
          {d === null && <option value="">--</option>}
          {Array.from({ length: 24 }, (_, h) => (
            <option key={h} value={h}>
              {String(h).padStart(2, "0")}
            </option>
          ))}
        </Select>
        <span className="text-ink-400">:</span>
        <Select
          className="h-8 px-2 pr-8 text-xs tabular-nums"
          value={d ? minute : ""}
          disabled={!d}
          onChange={(e) => onChange(d ? d.hour() : 0, Number(e.target.value))}
          aria-label={label}
        >
          {d === null && <option value="">--</option>}
          {MINUTE_STEPS.map((m) => (
            <option key={m} value={m}>
              {String(m).padStart(2, "0")}
            </option>
          ))}
        </Select>
      </div>
      <p className="mt-1 text-xs text-ink-400 tabular-nums">
        {d ? d.format("YYYY-MM-DD") : "—"}
      </p>
    </div>
  );
}
