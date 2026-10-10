import { useEffect, useRef, useState } from "react";
import * as echarts from "echarts/core";
import { LineChart as EChartsLine } from "echarts/charts";
import {
  GridComponent,
  LegendComponent,
  TooltipComponent,
  MarkLineComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import { usePrefs } from "@/components/prefs-provider";

echarts.use([
  EChartsLine,
  GridComponent,
  LegendComponent,
  TooltipComponent,
  MarkLineComponent,
  CanvasRenderer,
]);

/**
 * Spec 7.9.2 / 02 / 10: chart colors must come from tokens only — brand scale + emerald/amber/rose
 * three semantic colors + ink neutral, must not introduce non-token color names like violet / sky / fuchsia / lime.
 *
 * "One line per node" multi-series comparison needs more than 5 distinguishable colors; here we use
 * "brand primary + 3 semantic + 1 neutral + brand's three secondary shades" to make 8, all from tokens.
 * Beyond 8 series colors start to cycle, distinguishable via legend and tooltip — spec only allows token colors,
 * no relaxation for multi-series.
 */
const SEMANTIC_LIGHT = ["#059669", "#d97706", "#e11d48"]; // emerald-600 / amber-600 / rose-600
const SEMANTIC_DARK = ["#34d399", "#fbbf24", "#fb7185"]; // emerald-400 / amber-400 / rose-400
const NEUTRAL_LIGHT = "#71717a"; // ink-500
const NEUTRAL_DARK = "#a1a1aa"; // ink-400
/** Secondary shades (primary is already at palette index 1, here we only take the remaining brand shades) */
const BRAND_SHADE_VARS = ["--brand-500", "--brand-700", "--brand-900"];

/** Spec 7.9.1: only h-48 / h-64 / h-80 allowed */
export type ChartHeight = "sm" | "md" | "lg";
const HEIGHT_CLASS: Record<ChartHeight, string> = {
  sm: "h-48",
  md: "h-64",
  lg: "h-80",
};

export interface Series {
  name: string;
  data: Array<[number, number]>;
}

export function LineChart({
  series,
  height = "md",
  unit = "",
  decimals = 1,
  yMax,
  threshold,
  yAxisName,
  yFormatter,
  valueFormatter,
  xMin,
  xMax,
}: {
  series: Series[];
  height?: ChartHeight;
  unit?: string;
  decimals?: number;
  yMax?: number;
  threshold?: number;
  yAxisName?: string;
  /** Y-axis tick formatting (e.g. byte amounts shown as KiB / MiB / GiB) */
  yFormatter?: (v: number) => string;
  /** Tooltip value formatting, default `keep decimals digits + unit` */
  valueFormatter?: (v: number) => string;
  /** X-axis fixed range (matches selected time range, avoids only drawing the segment with data) */
  xMin?: number;
  xMax?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const { colorScheme } = usePrefs();
  const dark = colorScheme === "dark";

  /**
   * Legend selection state; null = user hasn't clicked yet (default all shown).
   * Stored in component state rather than ECharts's internal state: auto-refresh / window scroll
   * redraw (`notMerge` replaces entirely) won't clear it.
   * Double-clicking a legend item = invert: turn everything else off, keep only that one — fast focus on a single series.
   */
  const [legendSelected, setLegendSelected] = useState<Record<string, boolean> | null>(null);
  /** Record of the last "legend click", used to detect double-clicks on the same item within 350ms */
  const lastLegendClick = useRef<{ name: string; at: number } | null>(null);

  useEffect(() => {
    if (!ref.current) return;
    const chart = echarts.init(ref.current, undefined, { renderer: "canvas" });
    chartRef.current = chart;
    const ro = new ResizeObserver(() => chart.resize());
    ro.observe(ref.current);
    return () => {
      ro.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  // Legend interaction: single click uses ECharts's native toggle; double-click on the same item (twice within 350ms) inverts.
  // Listen to the "after selection state changes" event — params.selected is the complete state after the change.
  // Register once on mount: the event is bound to the chart instance and isn't affected by setOption rebuilds.
  useEffect(() => {
    const chart = chartRef.current;
    if (!chart) return;
    const onLegendSelected = (params: unknown) => {
      const ev = params as { name?: string; selected?: Record<string, boolean> };
      if (!ev.name) return;
      const now = Date.now();
      const prev = lastLegendClick.current;
      if (prev && prev.name === ev.name && now - prev.at <= 350) {
        // Double-click: invert — only the double-clicked item stays lit
        lastLegendClick.current = null;
        const names = Object.keys(ev.selected ?? {});
        setLegendSelected(
          Object.fromEntries(names.map((n) => [n, n === ev.name])),
        );
        return;
      }
      lastLegendClick.current = { name: ev.name, at: now };
      setLegendSelected(ev.selected ?? null);
    };
    chart.on("legendselected", onLegendSelected);
    return () => {
      chart.off("legendselected", onLegendSelected);
    };
  }, []);

  useEffect(() => {
    const chart = chartRef.current;
    if (!chart) return;

    const cssVar = (name: string) =>
      getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    const brand = cssVar("--brand-600");
    const accent = dark ? brand || "#6366f1" : brand || "#4f46e5";
    const palette = [
      accent,
      ...(dark ? SEMANTIC_DARK : SEMANTIC_LIGHT),
      dark ? NEUTRAL_DARK : NEUTRAL_LIGHT,
      ...BRAND_SHADE_VARS.map((v) => cssVar(v)).filter(Boolean),
    ];

    const axisColor = dark ? "#3f3f46" : "#e4e4e7";
    const gridColor = dark ? "#27272a" : "#f4f4f5";
    const labelColor = "#71717a";

    // Multi-series gets a legend; legend takes a row, so the grid top must leave room for it,
    // otherwise legend and curves overlap. `type: "scroll"` lets the legend scroll horizontally
    // when there are many nodes instead of wrapping into a stacked mess. Legend row height ~20px,
    // grid top margin 44px baseline — when exceeded, ECharts's pagination arrows won't touch the curves.
    const hasLegend = series.length > 1;

    // grid top must clear the legend AND have room above the legend so the axisPointer
    // tooltip doesn't sit on top of the legend row. With many series the legend grows
    // tall (pagination arrows + items wrap), and `grid.top: 44` would clip the tooltip
    // under the legend.
    const gridTop = hasLegend ? 56 : 16;

    chart.setOption(
      {
        animationDuration: 200,
        color: palette,
        // Spec 7.9.3: left 8, right 16, top/bottom 8
        grid: {
          left: 8,
          right: 16,
          top: gridTop,
          bottom: 8,
          containLabel: true,
        },
        tooltip: {
          trigger: "axis",
          // confine: tooltip stays inside the chart container, so a tall
          // tooltip (many series) doesn't escape into the sticky page header
          // above. Without this, ECharts's default behavior is to let the
          // tooltip overflow upward and overlap the header at the top of the viewport.
          confine: true,
          backgroundColor: "#18181b",
          borderWidth: 0,
          padding: [6, 10],
          textStyle: { color: "#fff", fontSize: 12 },
          axisPointer: {
            type: "line",
            lineStyle: { color: brand || "#6366f1", type: "dashed", width: 1 },
          },
          valueFormatter: (v: unknown) =>
            valueFormatter
              ? valueFormatter(Number(v))
              : `${Number(v).toFixed(decimals)}${unit}`,
        },
        legend: hasLegend
          ? {
              // scroll: legend in one row, scrolls left/right past the width — with many nodes it won't
              // wrap onto the curves or overlap them.
              type: "scroll",
              top: 0,
              left: 0,
              right: 0,
              icon: "roundRect",
              itemWidth: 12,
              itemHeight: 12,
              itemGap: 16,
              textStyle: { color: labelColor, fontSize: 12 },
              // After the user clicks the legend, persist the selection in component state; null (not yet interacted) is not passed,
              // falling back to ECharts's default of all-selected. Without this, `notMerge` would
              // reset legend selection to all-selected on every redraw, and the user's picks would be lost during auto-refresh.
              selected: legendSelected ?? undefined,
            }
          : undefined,
        xAxis: {
          type: "time",
          boundaryGap: false,
          min: xMin,
          max: xMax,
          axisLine: { lineStyle: { color: axisColor } },
          axisTick: { show: false },
          axisLabel: { color: labelColor, fontSize: 12, hideOverlap: true },
          splitLine: { show: false },
        },
        yAxis: {
          type: "value",
          name: yAxisName,
          nameTextStyle: { color: "#a1a1aa", fontSize: 12 },
          max: yMax,
          axisLine: { show: false },
          axisTick: { show: false },
          axisLabel: {
            color: labelColor,
            fontSize: 12,
            formatter: (v: number) =>
              yFormatter ? yFormatter(v) : v.toFixed(decimals >= 1 ? 0 : 1),
          },
          // Spec 7.9.3: horizontal grid lines dashed stroke-dasharray 4 4
          splitLine: { lineStyle: { color: gridColor, type: [4, 4] } },
        },
        series: series.map((s, i) => ({
          name: s.name,
          type: "line" as const,
          smooth: 0.25,
          showSymbol: false,
          data: s.data,
          lineStyle: { width: 1.6, color: palette[i % palette.length] },
          itemStyle: { color: palette[i % palette.length] },
          markLine:
            i === 0 && threshold !== undefined
              ? {
                  silent: true,
                  symbol: "none",
                  label: {
                    formatter: `${threshold}${unit}`,
                    color: labelColor,
                    fontSize: 12,
                    position: "insideEndTop" as const,
                  },
                  lineStyle: {
                    color: dark ? "#3f3f46" : "#d4d4d8",
                    type: "dashed" as const,
                    width: 1,
                  },
                  data: [{ yAxis: threshold }],
                }
              : undefined,
        })),
      },
      { notMerge: true },
    );
  }, [
    series,
    unit,
    decimals,
    threshold,
    yMax,
    yAxisName,
    yFormatter,
    valueFormatter,
    xMin,
    xMax,
    dark,
    legendSelected,
  ]);

  return <div ref={ref} className={`w-full ${HEIGHT_CLASS[height]}`} />;
}

/** Mini trend chart (inline in tables, no axes) */
export function Sparkline({ data }: { data: Array<[number, number]> }) {
  const ref = useRef<HTMLDivElement>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);

  useEffect(() => {
    if (!ref.current) return;
    const chart = echarts.init(ref.current, undefined, { renderer: "canvas" });
    chartRef.current = chart;
    const ro = new ResizeObserver(() => chart.resize());
    ro.observe(ref.current);
    return () => {
      ro.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    const chart = chartRef.current;
    if (!chart) return;
    const brand = getComputedStyle(document.documentElement)
      .getPropertyValue("--brand-600")
      .trim();
    chart.setOption(
      {
        animation: false,
        grid: { left: 0, right: 0, top: 2, bottom: 0 },
        xAxis: { type: "time", show: false, boundaryGap: false },
        yAxis: { type: "value", show: false, scale: true },
        series: [
          {
            type: "line",
            smooth: 0.3,
            showSymbol: false,
            data,
            lineStyle: { color: brand || "#4f46e5", width: 1.4 },
            itemStyle: { color: brand || "#4f46e5" },
          },
        ],
      },
      { notMerge: true },
    );
  }, [data]);

  return <div ref={ref} className="w-24 h-8" />;
}
