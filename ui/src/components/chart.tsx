import { useEffect, useRef } from "react";
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
 * 规范 7.9.2 / 02 / 10：图表配色只能取自令牌——brand 色阶 + emerald/amber/rose
 * 三个语义色 + ink 中性色，不得引入 violet / sky / fuchsia / lime 这类非令牌色名。
 *
 * 「一条线一个节点」的多系列对比需要多于 5 种可区分的颜色，这里用「brand 主色 +
 * 3 语义色 + 1 中性色 + brand 的三个次级色阶」凑满 8 个，颜色全部来自令牌。
 * 超过 8 个系列时颜色开始循环，靠图例与 tooltip 区分——规范只允许令牌色，
 * 不为多系列放宽。
 */
const SEMANTIC_LIGHT = ["#059669", "#d97706", "#e11d48"]; // emerald-600 / amber-600 / rose-600
const SEMANTIC_DARK = ["#34d399", "#fbbf24", "#fb7185"]; // emerald-400 / amber-400 / rose-400
const NEUTRAL_LIGHT = "#71717a"; // ink-500
const NEUTRAL_DARK = "#a1a1aa"; // ink-400
/** 次级色阶（主色已经在调色板第 1 位，这里只取剩下的品牌色阶） */
const BRAND_SHADE_VARS = ["--brand-500", "--brand-700", "--brand-900"];

/** 规范 7.9.1：仅允许 h-48 / h-64 / h-80 */
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
  /** Y 轴刻度格式化（如字节量显示 KiB / MiB / GiB） */
  yFormatter?: (v: number) => string;
  /** tooltip 取值格式化，默认 `保留 decimals 位 + unit` */
  valueFormatter?: (v: number) => string;
  /** X 轴固定范围（与所选时间范围一致，避免只画有数据的那一段） */
  xMin?: number;
  xMax?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const chartRef = useRef<echarts.ECharts | null>(null);
  const { colorScheme } = usePrefs();
  const dark = colorScheme === "dark";

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

    // 多系列才有图例；图例占一行，所以网格顶部要给它留出空间，
    // 否则图例与曲线互相遮盖。`type: "scroll"` 让节点多时图例横向滚动，
    // 而不是折行堆叠糊成一片。
    const hasLegend = series.length > 1;

    chart.setOption(
      {
        animationDuration: 200,
        color: palette,
        // 规范 7.9.3：左 8 右 16 上下 8
        grid: {
          left: 8,
          right: 16,
          top: hasLegend ? 34 : 16,
          bottom: 8,
          containLabel: true,
        },
        tooltip: {
          trigger: "axis",
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
              // scroll：图例排成一行、超出宽度就左右翻页——节点多时不会
              // 折行叠在曲线上，也不会互相遮盖。
              type: "scroll",
              top: 0,
              left: 0,
              right: 0,
              icon: "roundRect",
              itemWidth: 12,
              itemHeight: 12,
              itemGap: 16,
              textStyle: { color: labelColor, fontSize: 12 },
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
          // 规范 7.9.3：水平网格线虚线 stroke-dasharray 4 4
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
  ]);

  return <div ref={ref} className={`w-full ${HEIGHT_CLASS[height]}`} />;
}

/** 迷你走势图（表格内联用，无坐标轴） */
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
