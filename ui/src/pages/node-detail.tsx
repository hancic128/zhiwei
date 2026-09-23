import * as React from "react";
import { useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient, keepPreviousData } from "@tanstack/react-query";
import {
  ArrowDown,
  ArrowUp,
  Clock,
  Copy,
  Cpu,
  HardDrive,
  Info,
  MemoryStick,
  Network,
  Pencil,
  Power,
  RefreshCw,
  RotateCw,
  Skull,
  Zap,
} from "lucide-react";
import {
  api,
  commandsApi,
  containersApi,
  hostAddresses,
  METRICS,
  osLabel,
  waitForCommand,
  type ProcessInfo,
} from "@/api";
import { LineChart, type Series } from "@/components/chart";
import { NodeContainers } from "@/components/node-containers";
import { NodeMetaDialog, TagList } from "@/components/node-meta-dialog";
import { NodeProbes } from "@/components/node-probes";
import { Badge, DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog } from "@/components/ui/dialog";
import {
  presetBounds,
  presetRange,
  TimeRangePicker,
  type TimeRange,
} from "@/components/ui/date-range-picker";
import { EmptyState, ErrorState, Skeleton } from "@/components/ui/feedback";
import { useBreadcrumb } from "@/components/ui/breadcrumb";
import { Select } from "@/components/ui/select";
import { Tooltip } from "@/components/ui/tooltip";
import { useToast } from "@/components/ui/toast";
import { usePrefs } from "@/components/prefs-provider";
import { APP_VERSION } from "@/lib/version";
import {
  cn,
  copyText,
  formatBytes,
  formatPercent,
  formatRate,
  formatTime,
  formatUptime,
  friendlyError,
  isAgentOlder,
  livenessOf,
  maskSecret,
  nodeLabel,
  relativeTime,
} from "@/lib/utils";

/** 刷新频率档位（需求点名的 5 档） */
const REFRESH_OPTIONS = [
  { ms: 5_000, label: "5s" },
  { ms: 10_000, label: "10s" },
  { ms: 30_000, label: "30s" },
  { ms: 60_000, label: "1min" },
  { ms: 300_000, label: "5min" },
];

type PendingAction =
  | { kind: "term"; pid: number; name: string }
  | { kind: "kill"; pid: number; name: string }
  | { kind: "restart" }
  | { kind: "shutdown" };

export function NodeDetail() {
  const { t } = useTranslation();
  const { id = "" } = useParams();
  const { timezone } = usePrefs();
  const qc = useQueryClient();
  const toast = useToast();

  const [range, setRange] = React.useState<TimeRange>(() => presetRange("1h"));
  const [refreshMs, setRefreshMs] = React.useState(30_000);
  const [showBasic, setShowBasic] = React.useState(false);
  const [memAbs, setMemAbs] = React.useState(false);
  const [diskAbs, setDiskAbs] = React.useState(false);
  const [cpuAbs, setCpuAbs] = React.useState(false);
  const [procSort, setProcSort] = React.useState<"cpu" | "mem">("cpu");
  const [pending, setPending] = React.useState<PendingAction | null>(null);
  const [busy, setBusy] = React.useState(false);
  /** 编辑别名 / 标签 */
  const [metaOpen, setMetaOpen] = React.useState(false);

  // 预设范围跟着刷新频率滚动（否则「最近 30 分钟」会一直停在首次选定的那一段）
  const [tick, setTick] = React.useState(() => Date.now());
  React.useEffect(() => {
    const h = window.setInterval(() => setTick(Date.now()), refreshMs);
    return () => window.clearInterval(h);
  }, [refreshMs]);
  const win = React.useMemo(
    () =>
      range.preset
        ? presetBounds(range.preset, tick)
        : { from: range.from, to: range.to },
    [range, tick],
  );

  const nodesQ = useQuery({
    queryKey: ["nodes"],
    queryFn: api.nodes,
    refetchInterval: refreshMs,
    // 刷新 / 切换范围时保留上一帧：整页内容不该因为一次后台请求而清空
    placeholderData: keepPreviousData,
  });
  const node = nodesQ.data?.find((n) => n.id === id);

  /**
   * 所有曲线都常驻拉取（含磁盘绝对值），切换「占比 / 绝对值」只是换个渲染，
   * 不触发任何请求——切换图表不该刷别的图表。
   *
   * `placeholderData` 让窗口滚动（预设范围按刷新频率重算，queryKey 随之变化）
   * 或手动刷新时继续用上一段数据渲染，而不是闪成骨架屏。
   */
  const seriesQ = (metric: string, rate = false) =>
    useQuery({
      queryKey: ["series", id, metric, win.from, win.to, rate],
      queryFn: () =>
        api.series(id, metric, { from: win.from, to: win.to, limit: 600, rate }),
      enabled: !!id,
      refetchInterval: refreshMs,
      placeholderData: keepPreviousData,
    });
  const cpuQ = seriesQ(METRICS.cpu);
  const memQ = seriesQ(METRICS.memUsed);
  const diskPctQ = seriesQ(METRICS.diskUsage);
  const diskUsedQ = seriesQ(METRICS.diskUsed);
  const netRxQ = seriesQ(METRICS.netRx, true);
  const netTxQ = seriesQ(METRICS.netTx, true);

  const procsQ = useQuery({
    queryKey: ["processes", id],
    queryFn: () => containersApi.processes(id),
    enabled: !!id,
    refetchInterval: refreshMs,
    placeholderData: keepPreviousData,
  });

  const cores = node?.host_info.cpu_cores ?? 0;
  const memTotal =
    node?.latest?.mem_total_bytes ?? node?.host_info.total_memory_bytes ?? 0;
  const memUsed = memQ.data?.latest ?? node?.latest?.mem_used_bytes ?? null;
  const memPct = memUsed !== null && memTotal > 0 ? (memUsed / memTotal) * 100 : null;
  const cpuPct = cpuQ.data?.latest ?? null;
  const diskPct = diskPctQ.data?.latest ?? node?.latest?.disk_usage_percent ?? null;
  const diskUsed = node?.latest?.disk_used_bytes ?? null;
  const diskTotal = node?.latest?.disk_total_bytes ?? null;
  const netUp = netTxQ.data?.latest ?? null;
  const netDown = netRxQ.data?.latest ?? null;
  const live = livenessOf(node?.last_seen_ms);
  const rel = (ms: number) => relativeTime(ms, t);

  const setCrumbs = useBreadcrumb();

  // 节点 Agent 比控制台旧时给个可点的提示：容器用量 / 启停日志「没反应」多半是它。
  const agentVersion = node?.host_info?.agent_version;
  const agentOutdated = isAgentOlder(agentVersion, APP_VERSION);

  // 「Nodes / 家用电脑 · Online · Enrolled 9h ago」放在 App 顶部 header 里，
  // 不再挤在内容区顶部。节点数据未回来时先只挂根级，避免叶子文字闪一下。
  React.useEffect(() => {
    setCrumbs([
      { to: "/nodes", label: t("nav.nodes") },
      {
        label: (
          <span className="flex items-center gap-2 min-w-0">
            <span className="text-base font-semibold text-ink-900 dark:text-surface-0 truncate">
              {node ? nodeLabel(node) : id}
            </span>
            <DotBadge
              tone={
                live === "online" ? "success" : live === "lagging" ? "warn" : "neutral"
              }
              pulse={live === "online"}
            >
              {t(`state.${live}`)}
            </DotBadge>
            {node && (
              <TagList
                tags={node.tags ?? []}
                className="flex items-center gap-1 shrink-0"
              />
            )}
            {agentOutdated && (
              <Tooltip
                content={t("containers.agentOutdatedHint", {
                  agent: agentVersion,
                  console: APP_VERSION,
                })}
              >
                <Badge tone="warn" className="shrink-0">
                  {t("containers.agentOutdated")}
                </Badge>
              </Tooltip>
            )}
            {node && (
              <span className="text-xs text-ink-400 whitespace-nowrap">
                {t("detail.enrolledAt")} {rel(node.enrolled_at_ms)}
              </span>
            )}
          </span>
        ),
      },
    ]);
  }, [setCrumbs, node, id, t, live, agentOutdated, agentVersion]);

  // 离开详情页时清掉，否则回到列表还会挂着上一个节点的面包屑
  React.useEffect(() => () => setCrumbs([]), [setCrumbs]);

  const processes: ProcessInfo[] = React.useMemo(() => {
    const list = [...(procsQ.data?.processes ?? [])];
    list.sort((a, b) =>
      procSort === "cpu"
        ? b.cpu_percent - a.cpu_percent
        : b.memory_bytes - a.memory_bytes,
    );
    return list.slice(0, 10);
  }, [procsQ.data, procSort]);

  // 曲线数据：useMemo 固定数组身份，避免无关的 state 变化（比如开对话框）
  // 让 ECharts 重画一遍
  const hostname = node?.hostname ?? "";
  const cpuSeries = React.useMemo<Series[]>(() => {
    const pts = cpuQ.data?.points ?? [];
    return cpuAbs
      ? [
          {
            name: t("detail.viewAbsolute"),
            data: pts.map((p) => [p.t, cores > 0 ? (p.v * cores) / 100 : 0] as [number, number]),
          },
        ]
      : [{ name: hostname, data: pts.map((p) => [p.t, p.v] as [number, number]) }];
  }, [cpuQ.data, cpuAbs, cores, hostname, t]);

  const memSeries = React.useMemo<Series[]>(() => {
    const pts = memQ.data?.points ?? [];
    return memAbs
      ? [{ name: t("detail.viewAbsolute"), data: pts.map((p) => [p.t, p.v] as [number, number]) }]
      : [
          {
            name: hostname,
            data: pts.map(
              (p) =>
                [p.t, memTotal > 0 ? (p.v / memTotal) * 100 : 0] as [number, number],
            ),
          },
        ];
  }, [memQ.data, memAbs, memTotal, hostname, t]);

  const diskSeries = React.useMemo<Series[]>(() => {
    const pts = diskAbs ? diskUsedQ.data?.points ?? [] : diskPctQ.data?.points ?? [];
    return [
      {
        name: diskAbs ? t("detail.viewAbsolute") : hostname,
        data: pts.map((p) => [p.t, p.v] as [number, number]),
      },
    ];
  }, [diskAbs, diskUsedQ.data, diskPctQ.data, hostname, t]);

  const netSeries = React.useMemo<Series[]>(
    () => [
      {
        name: t("detail.netUp"),
        data: (netTxQ.data?.points ?? []).map((p) => [p.t, p.v] as [number, number]),
      },
      {
        name: t("detail.netDown"),
        data: (netRxQ.data?.points ?? []).map((p) => [p.t, p.v] as [number, number]),
      },
    ],
    [netTxQ.data, netRxQ.data, t],
  );

  /** 下发命令并等回执：杀进程 / 重启 / 关机都走这一条路 */
  const runCommand = async (action: string, params: Record<string, unknown>) => {
    setBusy(true);
    try {
      const { command_id } = await commandsApi.exec(id, action, params);
      const row = await waitForCommand(command_id);
      if (!row) toast.push("warn", t("detail.cmdPending"));
      else if (row.result_ok) toast.push("success", row.result_text || t("detail.cmdOk"));
      else toast.push("error", row.result_error || t("detail.cmdFailed"));
    } catch (e) {
      toast.push("error", t(friendlyError(e)));
    } finally {
      setBusy(false);
      setPending(null);
      void qc.invalidateQueries({ queryKey: ["processes", id] });
    }
  };

  const copy = async (text: string) => {
    const ok = await copyText(text);
    toast.push(ok ? "success" : "error", t(ok ? "detail.copied" : "toast.copyFailed"));
  };

  const confirmCopy = (() => {
    switch (pending?.kind) {
      case "term":
        return {
          title: t("detail.killTitle", { pid: pending.pid }),
          message: t("detail.killTermMessage", { name: pending.name }),
          label: t("detail.killTerm"),
          danger: true,
        };
      case "kill":
        return {
          title: t("detail.killTitle", { pid: pending.pid }),
          message: t("detail.killKillMessage", { name: pending.name }),
          label: t("detail.killKill"),
          danger: true,
        };
      case "restart":
        return {
          title: t("detail.restart"),
          message: t("detail.restartMessage", { name: node?.hostname ?? id }),
          label: t("detail.restart"),
          danger: false,
        };
      case "shutdown":
        return {
          title: t("detail.shutdown"),
          message: t("detail.shutdownMessage", { name: node?.hostname ?? id }),
          label: t("detail.shutdown"),
          danger: true,
        };
      default:
        return null;
    }
  })();

  if (nodesQ.isError) {
    return (
      <Card>
        <ErrorState
          message={t(friendlyError(nodesQ.error))}
          onRetry={() => void nodesQ.refetch()}
          retrying={nodesQ.isFetching}
        />
      </Card>
    );
  }

  return (
    <>
      {/* 操作区：日期范围、刷新频率、基本信息 / 重启 / 关机 收在右侧。
          标题、状态、标签、入网时间已搬到 header 的面包屑里；节点 ID 进「Host info」。 */}
      <div className="flex flex-wrap items-center justify-end gap-2">
        <TimeRangePicker value={range} onChange={setRange} />
        <Tooltip content={t("detail.refresh")}>
          <div className="relative inline-flex items-center">
            <Clock
              className="absolute left-3 w-4 h-4 text-ink-400 pointer-events-none"
              aria-hidden="true"
            />
            <Select
              wrapperClassName="w-24"
              className="h-8 pl-8 text-xs tabular-nums"
              value={refreshMs}
              onChange={(e) => setRefreshMs(Number(e.target.value))}
              aria-label={t("detail.refresh")}
            >
              {REFRESH_OPTIONS.map((o) => (
                <option key={o.ms} value={o.ms}>
                  {o.label}
                </option>
              ))}
            </Select>
          </div>
        </Tooltip>
        <Tooltip content={t("detail.basicTitle")}>
          <Button
            variant={showBasic ? "primary" : "ghost"}
            size="icon"
            aria-label={t("detail.basicTitle")}
            aria-pressed={showBasic}
            onClick={() => setShowBasic(true)}
          >
            <Info className="w-4 h-4" aria-hidden="true" />
          </Button>
        </Tooltip>
        <Tooltip content={t("nodeMeta.edit")}>
          <Button
            variant="ghost"
            size="icon"
            aria-label={t("nodeMeta.edit")}
            disabled={!node}
            onClick={() => setMetaOpen(true)}
          >
            <Pencil className="w-4 h-4" aria-hidden="true" />
          </Button>
        </Tooltip>
        <Tooltip content={t("detail.restart")}>
          <Button
            variant="ghost"
            size="icon"
            aria-label={t("detail.restart")}
            onClick={() => setPending({ kind: "restart" })}
          >
            <RotateCw className="w-4 h-4" aria-hidden="true" />
          </Button>
        </Tooltip>
        <Tooltip content={t("detail.shutdown")}>
          <Button
            variant="ghost"
            size="icon"
            aria-label={t("detail.shutdown")}
            onClick={() => setPending({ kind: "shutdown" })}
            className="text-rose-600 dark:text-rose-400"
          >
            <Power className="w-4 h-4" aria-hidden="true" />
          </Button>
        </Tooltip>
      </div>

      {nodesQ.isLoading ? (
        <Skeleton className="h-24 w-full" />
      ) : !node ? (
        <Card>
          <EmptyState
            title={t("detail.notFound")}
            description={t("detail.notFoundHint")}
          />
        </Card>
      ) : (
        <>
          {/* 大字卡片：CPU / 内存 / 磁盘 / 网络（上下行） */}
          <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-4 gap-4">
            <MetricCard
              label={t("detail.kpiCpu")}
              icon={<Cpu className="w-4 h-4" aria-hidden="true" />}
              value={cpuPct === null ? "—" : formatPercent(cpuPct)}
              tone={cpuPct !== null && cpuPct > 80 ? "danger" : "default"}
              hint={cores > 0 ? t("detail.coresHint", { n: cores }) : undefined}
              loading={cpuQ.isLoading}
            />
            <MetricCard
              label={t("detail.kpiMem")}
              icon={<MemoryStick className="w-4 h-4" aria-hidden="true" />}
              value={memPct === null ? "—" : formatPercent(memPct)}
              tone={memPct !== null && memPct > 90 ? "danger" : "default"}
              hint={`${formatBytes(memUsed ?? 0)} / ${formatBytes(memTotal)}`}
              loading={memQ.isLoading}
            />
            <MetricCard
              label={t("detail.kpiDisk")}
              icon={<HardDrive className="w-4 h-4" aria-hidden="true" />}
              value={diskPct === null ? "—" : formatPercent(diskPct)}
              tone={diskPct !== null && diskPct > 85 ? "danger" : "default"}
              hint={
                diskUsed !== null && diskTotal
                  ? `${formatBytes(diskUsed)} / ${formatBytes(diskTotal)}`
                  : undefined
              }
              loading={diskPctQ.isLoading}
            />
            <MetricCard
              label={t("detail.kpiNet")}
              icon={<Network className="w-4 h-4" aria-hidden="true" />}
              rows={[
                {
                  up: true,
                  label: t("detail.netUp"),
                  value: netUp === null ? "—" : formatRate(netUp),
                },
                {
                  up: false,
                  label: t("detail.netDown"),
                  value: netDown === null ? "—" : formatRate(netDown),
                },
              ]}
              loading={netRxQ.isLoading || netTxQ.isLoading}
            />
          </div>

          {/* 趋势图：X 轴固定为所选时间范围，右上角可单独刷新这一张 */}
          <div className="grid grid-cols-1 xl:grid-cols-2 gap-4 md:gap-6">
            <TrendCard
              title={t("detail.cpuChart")}
              subtitle={t("detail.sampleCount", { n: cpuQ.data?.points.length ?? 0 })}
              query={cpuQ}
              toggle={{
                on: cpuAbs,
                set: setCpuAbs,
                offLabel: t("detail.viewRate"),
                onLabel: t("detail.viewAbsolute"),
              }}
            >
              {cpuAbs ? (
                <LineChart
                  series={cpuSeries}
                  height="md"
                  decimals={1}
                  unit={t("detail.unitCores")}
                  yMax={cores || undefined}
                  xMin={win.from}
                  xMax={win.to}
                />
              ) : (
                <LineChart
                  series={cpuSeries}
                  height="md"
                  unit="%"
                  yMax={100}
                  threshold={80}
                  xMin={win.from}
                  xMax={win.to}
                />
              )}
            </TrendCard>

            <TrendCard
              title={t("detail.memChart")}
              subtitle={t("detail.memChartSubtitle", {
                used: formatBytes(memUsed ?? 0),
                total: formatBytes(memTotal),
              })}
              query={memQ}
              toggle={{
                on: memAbs,
                set: setMemAbs,
                offLabel: t("detail.viewRate"),
                onLabel: t("detail.viewAbsolute"),
              }}
            >
              {memAbs ? (
                <LineChart
                  series={memSeries}
                  height="md"
                  yMax={memTotal > 0 ? memTotal : undefined}
                  yFormatter={formatBytes}
                  valueFormatter={formatBytes}
                  yAxisName={t("detail.axisBytes")}
                  xMin={win.from}
                  xMax={win.to}
                />
              ) : (
                <LineChart
                  series={memSeries}
                  height="md"
                  unit="%"
                  yMax={100}
                  threshold={90}
                  xMin={win.from}
                  xMax={win.to}
                />
              )}
            </TrendCard>

            <TrendCard
              title={t("detail.diskChart")}
              subtitle={
                diskUsed !== null && diskTotal
                  ? t("detail.memChartSubtitle", {
                      used: formatBytes(diskUsed),
                      total: formatBytes(diskTotal),
                    })
                  : undefined
              }
              query={diskAbs ? diskUsedQ : diskPctQ}
              toggle={{
                on: diskAbs,
                set: setDiskAbs,
                offLabel: t("detail.viewRate"),
                onLabel: t("detail.viewAbsolute"),
              }}
            >
              {diskAbs ? (
                <LineChart
                  series={diskSeries}
                  height="md"
                  yFormatter={formatBytes}
                  valueFormatter={formatBytes}
                  yAxisName={t("detail.axisBytes")}
                  xMin={win.from}
                  xMax={win.to}
                />
              ) : (
                <LineChart
                  series={diskSeries}
                  height="md"
                  unit="%"
                  yMax={100}
                  threshold={85}
                  xMin={win.from}
                  xMax={win.to}
                />
              )}
            </TrendCard>

            <TrendCard
              title={t("detail.netChart")}
              subtitle={t("detail.netChartSubtitle")}
              query={netRxQ}
              onRefresh={() => {
                void netRxQ.refetch();
                void netTxQ.refetch();
              }}
              refreshing={netRxQ.isFetching || netTxQ.isFetching}
            >
              <LineChart
                series={netSeries}
                height="md"
                yFormatter={formatBytes}
                valueFormatter={formatRate}
                yAxisName={t("detail.axisRate")}
                xMin={win.from}
                xMax={win.to}
              />
            </TrendCard>
          </div>

          {/* 进程 top10：按 CPU / 内存 切换，优雅杀 / 强杀（纯图标 + Tooltip） */}
          <Card>
            <CardHeader
              title={t("processes.title")}
              description={t("processes.topSubtitle", { n: processes.length })}
              action={
                <>
                  <Segmented
                    value={procSort}
                    onChange={(v) => setProcSort(v as "cpu" | "mem")}
                    options={[
                      { value: "cpu", label: t("processes.sortCpu") },
                      { value: "mem", label: t("processes.sortMem") },
                    ]}
                  />
                  <Tooltip content={t("action.refresh")}>
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={t("action.refresh")}
                      disabled={procsQ.isFetching}
                      onClick={() => void procsQ.refetch()}
                    >
                      <RefreshCw className="w-4 h-4" aria-hidden="true" />
                    </Button>
                  </Tooltip>
                </>
              }
            />
            <CardBody compact>
              {procsQ.isLoading ? (
                <Skeleton className="h-32 w-full" />
              ) : procsQ.isError ? (
                <ErrorState
                  compact
                  message={t(friendlyError(procsQ.error))}
                  onRetry={() => void procsQ.refetch()}
                  retrying={procsQ.isFetching}
                />
              ) : processes.length === 0 ? (
                <EmptyState
                  title={t("processes.empty")}
                  description={t("processes.emptyHint")}
                />
              ) : (
                <div className="overflow-x-auto scrollbar-thin">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-surface-3 dark:border-ink-700">
                        <th className="px-4 py-2 text-left text-xs font-semibold text-ink-500 uppercase tracking-wider">
                          {t("processes.colPid")}
                        </th>
                        <th className="px-4 py-2 text-left text-xs font-semibold text-ink-500 uppercase tracking-wider">
                          {t("processes.colName")}
                        </th>
                        <th className="px-4 py-2 text-right text-xs font-semibold text-ink-500 uppercase tracking-wider">
                          {t("processes.colCpu")}
                        </th>
                        <th className="px-4 py-2 text-right text-xs font-semibold text-ink-500 uppercase tracking-wider">
                          {t("processes.colMem")}
                        </th>
                        <th className="px-4 py-2 text-right text-xs font-semibold text-ink-500 uppercase tracking-wider">
                          {t("processes.colActions")}
                        </th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-surface-2 dark:divide-ink-700">
                      {processes.map((p) => (
                        <tr
                          key={p.pid}
                          className="hover:bg-surface-1 dark:hover:bg-ink-700/40 transition-colors"
                        >
                          <td className="px-4 py-2 text-xs tabular-nums text-ink-400">
                            {p.pid}
                          </td>
                          <td className="px-4 py-2">
                            <div className="text-sm text-ink-900 dark:text-surface-0 truncate max-w-[280px]">
                              {p.name}
                            </div>
                            {p.cmdline && (
                              <div className="text-xs text-ink-400 truncate max-w-[280px]">
                                {p.cmdline}
                              </div>
                            )}
                          </td>
                          <td className="px-4 py-2 text-right">
                            <span
                              className={cn(
                                "text-sm tabular-nums",
                                p.cpu_percent >= 50
                                  ? "text-rose-600 dark:text-rose-400"
                                  : "text-ink-900 dark:text-surface-0",
                              )}
                            >
                              {p.cpu_percent.toFixed(1)}%
                            </span>
                          </td>
                          <td className="px-4 py-2 text-right text-sm tabular-nums text-ink-500">
                            {formatBytes(p.memory_bytes)}
                          </td>
                          <td className="px-4 py-2">
                            <div className="flex items-center justify-end gap-1">
                              <Tooltip content={t("detail.killTerm")}>
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  aria-label={t("detail.killTerm")}
                                  onClick={() =>
                                    setPending({ kind: "term", pid: p.pid, name: p.name })
                                  }
                                >
                                  <Zap className="w-4 h-4" aria-hidden="true" />
                                </Button>
                              </Tooltip>
                              <Tooltip content={t("detail.killKill")}>
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  aria-label={t("detail.killKill")}
                                  onClick={() =>
                                    setPending({ kind: "kill", pid: p.pid, name: p.name })
                                  }
                                  className="text-rose-600 dark:text-rose-400"
                                >
                                  <Skull className="w-4 h-4" aria-hidden="true" />
                                </Button>
                              </Tooltip>
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </CardBody>
          </Card>

          {/* 容器组：列表 + 搜索/过滤/排序/分页 + 启停重启与最新日志 */}
          <NodeContainers nodeId={id} refreshMs={refreshMs} />

          {/* 服务探针的反向视图：这台机器在替谁探什么 */}
          <NodeProbes nodeId={id} refreshMs={refreshMs} />
        </>
      )}

      {/* 基本信息：对话框展示，不再占页面版面 */}
      <Dialog
        open={showBasic && !!node}
        onClose={() => setShowBasic(false)}
        size="xl"
        title={t("detail.basicTitle")}
        description={t("detail.basicSubtitle")}
      >
        {!node?.host_info.os_name ? (
          <EmptyState title={t("detail.basicEmpty")} />
        ) : (
          <dl className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-4 text-sm">
            <Field label={t("detail.system")} value={osLabel(node.host_info)} />
            <Field label={t("detail.kernel")} value={node.host_info.kernel_version ?? "—"} />
            <Field label={t("detail.arch")} value={node.host_info.arch ?? "—"} />
            <Field
              label={t("detail.cpu")}
              value={node.host_info.cpu_brand?.trim() ?? "—"}
            />
            <Field
              label={t("detail.cores")}
              value={node.host_info.cpu_cores ? String(node.host_info.cpu_cores) : "—"}
            />
            <Field
              label={t("detail.memory")}
              value={
                node.host_info.total_memory_bytes
                  ? formatBytes(node.host_info.total_memory_bytes)
                  : "—"
              }
            />
            <Field
              label={t("detail.uptime")}
              value={formatUptime(node.host_info.uptime_seconds, t)}
            />
            <Field
              label={t("detail.bootTime")}
              value={
                node.host_info.boot_time_unix_seconds
                  ? formatTime(node.host_info.boot_time_unix_seconds * 1000, timezone)
                  : "—"
              }
            />
            <Field
              label={t("detail.agentVersion")}
              value={node.host_info.agent_version ? `v${node.host_info.agent_version}` : "—"}
            />
            {/* 节点 ID：从标题区搬进来（header 只留「Nodes / 名称 · 状态 · 入网时间」） */}
            <div className="min-w-0">
              <dt className="text-xs text-ink-400">{t("detail.nodeId")}</dt>
              <dd className="mt-1 flex items-center gap-1">
                <span className="text-sm font-mono tabular-nums text-ink-900 dark:text-surface-0 truncate">
                  {id}
                </span>
                <Tooltip content={t("detail.copyKey")}>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label={t("detail.nodeId")}
                    onClick={() => void copy(id)}
                  >
                    <Copy className="w-4 h-4" aria-hidden="true" />
                  </Button>
                </Tooltip>
              </dd>
            </div>
            <div className="sm:col-span-2">
              <dt className="text-xs text-ink-400">{t("detail.addresses")}</dt>
              <dd className="mt-1 flex flex-wrap gap-1">
                {hostAddresses(node.host_info).length === 0 ? (
                  <span className="text-sm text-ink-500">
                    {t("detail.addressesEmpty")}
                  </span>
                ) : (
                  hostAddresses(node.host_info).map((a) => (
                    <DotBadge key={`${a.iface}-${a.addr}`} tone="neutral">
                      {a.iface} {a.addr}
                    </DotBadge>
                  ))
                )}
              </dd>
            </div>
            <div className="sm:col-span-2">
              <dt className="text-xs text-ink-400">{t("detail.nodeKey")}</dt>
              <dd className="mt-1 flex items-center gap-2">
                <span className="text-sm font-mono tabular-nums text-ink-900 dark:text-surface-0">
                  {maskSecret(node.public_key)}
                </span>
                <Tooltip content={t("detail.copyKey")}>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label={t("detail.copyKey")}
                    onClick={() => void copy(node.public_key)}
                  >
                    <Copy className="w-4 h-4" aria-hidden="true" />
                  </Button>
                </Tooltip>
                <span className="text-xs text-ink-400">{t("detail.keyMaskedHint")}</span>
              </dd>
            </div>
          </dl>
        )}
      </Dialog>

      {confirmCopy && pending && (
        <ConfirmDialog
          open
          danger={confirmCopy.danger}
          loading={busy}
          title={confirmCopy.title}
          message={confirmCopy.message}
          confirmLabel={confirmCopy.label}
          cancelLabel={t("action.cancel")}
          onCancel={() => setPending(null)}
          onConfirm={() => {
            if (pending.kind === "term")
              void runCommand("kill_process", { pid: pending.pid, signal: "term" });
            else if (pending.kind === "kill")
              void runCommand("kill_process", { pid: pending.pid, signal: "kill" });
            else if (pending.kind === "restart") void runCommand("restart_host", {});
            else void runCommand("shutdown_host", {});
          }}
        />
      )}

      <NodeMetaDialog
        node={metaOpen ? (node ?? null) : null}
        onClose={() => setMetaOpen(false)}
      />
    </>
  );
}

/** 大字卡片：单值（CPU/内存/磁盘）或两条值（网络上下行） */
function MetricCard({
  label,
  icon,
  value,
  unit,
  hint,
  tone = "default",
  loading,
  rows,
}: {
  label: string;
  icon?: React.ReactNode;
  value?: React.ReactNode;
  unit?: string;
  hint?: React.ReactNode;
  tone?: "default" | "danger";
  loading?: boolean;
  rows?: Array<{ up: boolean; label: string; value: string }>;
}) {
  return (
    <Card className="p-6">
      <div className="flex items-center gap-2 text-sm text-ink-500">
        {icon}
        <span>{label}</span>
      </div>
      {loading ? (
        <Skeleton className="mt-3 h-7 w-24" />
      ) : rows ? (
        <div className="mt-2 space-y-1">
          {rows.map((r) => (
            <div key={r.label} className="flex items-baseline gap-2">
              {r.up ? (
                <ArrowUp className="w-4 h-4 text-brand-600" aria-hidden="true" />
              ) : (
                <ArrowDown className="w-4 h-4 text-brand-600" aria-hidden="true" />
              )}
              <span className="text-xs text-ink-400">{r.label}</span>
              <span className="ml-auto text-xl font-bold tabular-nums text-ink-900 dark:text-surface-0">
                {r.value}
              </span>
            </div>
          ))}
        </div>
      ) : (
        <div className="mt-1 flex items-baseline gap-1">
          <span
            className={cn(
              "text-xl font-bold tabular-nums",
              tone === "danger"
                ? "text-rose-600 dark:text-rose-400"
                : "text-ink-900 dark:text-surface-0",
            )}
          >
            {value}
          </span>
          {unit && <span className="text-sm text-ink-500">{unit}</span>}
        </div>
      )}
      {hint && <div className="mt-1 text-xs text-ink-400">{hint}</div>}
    </Card>
  );
}

type QueryLike = {
  isLoading: boolean;
  isError: boolean;
  isFetching: boolean;
  error: unknown;
  refetch: () => void;
  data?: {
    points: Array<{ t: number; v: number }>;
    resolution?: "raw" | "hourly";
  };
};

/** 趋势图卡片：统一的 loading / 失败重试 / 空状态分支 + 单卡刷新按钮 */
function TrendCard({
  title,
  subtitle,
  query,
  toggle,
  onRefresh,
  refreshing,
  children,
}: {
  title: string;
  subtitle?: string;
  query: QueryLike;
  toggle?: { on: boolean; set: (v: boolean) => void; offLabel: string; onLabel: string };
  onRefresh?: () => void;
  refreshing?: boolean;
  children: React.ReactNode;
}) {
  const { t } = useTranslation();
  const points = query.data?.points.length ?? 0;
  const refresh = onRefresh ?? (() => query.refetch());
  // 窗口起点早于原始保留期时后端改走小时聚合——告诉用户这段不是原始 10 秒数据
  const hourly = query.data?.resolution === "hourly";
  return (
    <Card>
      <CardHeader
        title={title}
        description={
          hourly ? (
            <span>
              {subtitle}
              {subtitle ? " · " : ""}
              {t("settings.retentionHourlyHint")}
            </span>
          ) : (
            subtitle
          )
        }
        action={
          <>
            {toggle && (
              <Segmented
                value={toggle.on ? "abs" : "rate"}
                onChange={(v) => toggle.set(v === "abs")}
                options={[
                  { value: "rate", label: toggle.offLabel },
                  { value: "abs", label: toggle.onLabel },
                ]}
              />
            )}
            <Tooltip content={t("action.refresh")}>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("action.refresh")}
                disabled={refreshing ?? query.isFetching}
                onClick={refresh}
              >
                <RefreshCw className="w-4 h-4" aria-hidden="true" />
              </Button>
            </Tooltip>
          </>
        }
      />
      <CardBody compact>
        {query.isLoading ? (
          <Skeleton className="h-64 w-full" />
        ) : query.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(query.error))}
            onRetry={() => query.refetch()}
            retrying={query.isFetching}
          />
        ) : points > 1 ? (
          children
        ) : (
          <EmptyState title={t("detail.chartEmpty")} />
        )}
      </CardBody>
    </Card>
  );
}

/** 分段切换（绝对值 / 占比、按 CPU / 按内存）：项目里的统一控件外形 */
function Segmented({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (v: string) => void;
  options: Array<{ value: string; label: string }>;
}) {
  return (
    <div className="inline-flex rounded-md border border-surface-3 dark:border-ink-700 p-0.5">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          onClick={() => onChange(o.value)}
          className={cn(
            "px-2 py-1 rounded text-xs font-medium transition-colors",
            value === o.value
              ? "bg-brand-600 text-white"
              : "text-ink-500 hover:text-ink-900 dark:hover:text-surface-0",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** 详情页字段：标签在上、值在下 */
function Field({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs text-ink-400">{label}</dt>
      <dd className="mt-1 text-sm text-ink-900 dark:text-surface-0 truncate">
        {value || "—"}
      </dd>
    </div>
  );
}
