import * as React from "react";
import { useNavigate, useParams } from "react-router-dom";
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
  Trash2,
  Upload,
  Zap,
} from "lucide-react";
import {
  api,
  commandsApi,
  containersApi,
  hostAddresses,
  METRICS,
  osLabel,
  upgradeApi,
  waitForCommand,
  type ProcessInfo,
} from "@/api";
import { LineChart, type Series } from "@/components/chart";
import { NodeContainers } from "@/components/node-containers";
import { NodeCerts } from "@/components/node-certs";
import { ChannelDownBadge } from "@/components/command-channel";
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
import { useBreadcrumb, useHeaderActions } from "@/components/ui/breadcrumb";
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
} from "@/lib/utils";

/** Refresh rate tiers (5 tiers as required) */
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
  | { kind: "shutdown" }
  | { kind: "deleteNode" }
  | { kind: "upgrade"; version: string; sha256: string }
  /** Upgrade after 409: first void the node's pending commands (write audit) then delete */
  | { kind: "forceDeleteNode" };

export function NodeDetail() {
  const { t } = useTranslation();
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { timezone } = usePrefs();
  const qc = useQueryClient();
  const toast = useToast();

  const [range, setRange] = React.useState<TimeRange>(() => presetRange("3h"));
  // Node detail page defaults to 5s: the detail page is for viewing real-time curves / processes / containers,
  // 30s would feel "frozen"; other pages keep their original frequencies.
  const [refreshMs, setRefreshMs] = React.useState(5_000);
  const [showBasic, setShowBasic] = React.useState(false);
  const [memAbs, setMemAbs] = React.useState(false);
  const [diskAbs, setDiskAbs] = React.useState(false);
  const [cpuAbs, setCpuAbs] = React.useState(false);
  const [procSort, setProcSort] = React.useState<"cpu" | "mem">("cpu");
  const [pending, setPending] = React.useState<PendingAction | null>(null);
  const [busy, setBusy] = React.useState(false);
  /** Editing alias / tags */
  const [metaOpen, setMetaOpen] = React.useState(false);

  // Preset range scrolls with the refresh rate (otherwise "last 30 minutes" stays stuck at the first selected window)
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
    // Keep previous frame on refresh / range switch: the entire page content shouldn't clear for a single background request
    placeholderData: keepPreviousData,
  });
  const node = nodesQ.data?.find((n) => n.id === id);

  // Fetch latest upgrade package info (for upgrade button)
  const latestUpgradeQ = useQuery({
    queryKey: ["latestUpgrade"],
    queryFn: () => upgradeApi.latest().catch(() => null),
    staleTime: 60_000, // Cache for 1 minute
  });
  const latestUpgrade = latestUpgradeQ.data;

  /**
   * All curves are always polled (including absolute disk values); switching "ratio / absolute"
   * only changes rendering, doesn't trigger any request — switching a chart shouldn't refresh others.
   *
   * `placeholderData` keeps the previous window's data rendering while the window scrolls
   * (preset range recomputed by refresh rate, queryKey changes accordingly) or on manual refresh,
   * instead of flashing to a skeleton.
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

  const setCrumbs = useBreadcrumb();

  // When node Agent is older than the console, show a clickable hint: container usage / start-stop logs
  // "not responding" is usually this.
  const agentVersion = node?.host_info?.agent_version;
  const agentOutdated = isAgentOlder(agentVersion, APP_VERSION);

  // "Nodes / home pc · Online" goes into the App header, not the content area top.
  // Enroll / deploy time doesn't go in the header (required on 2026-09-29); the detail dialog has the full timestamps.
  // Before node data loads, only set the root crumb to avoid leaf text flashing.
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
            <ChannelDownBadge
              channel={node?.command_channel}
              className="shrink-0 inline-flex"
            />
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
          </span>
        ),
      },
    ]);
  }, [setCrumbs, node, id, t, live, agentOutdated, agentVersion]);

  // Clear crumbs when leaving the detail page, otherwise the list page keeps the previous node's crumbs
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

  // Curve data: useMemo stabilizes array identity, avoiding unrelated state changes (like opening dialog)
  // causing ECharts to redraw
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

  /** Send command and wait for receipt: kill process / restart / shutdown all go through this */
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

  /** Send upgrade_agent command */
  const runUpgradeCommand = async (version: string, sha256: string) => {
    setBusy(true);
    try {
      // Get monitor base URL from current location
      const baseUrl = `${window.location.protocol}//${window.location.host}`;
      const downloadUrl = `${baseUrl}/v1/upgrade/${version}`;
      const { command_id } = await commandsApi.exec(id, "upgrade_agent", {
        version,
        download_url: downloadUrl,
        sha256,
        restart: true,
      });
      const row = await waitForCommand(command_id);
      if (!row) toast.push("warn", t("detail.cmdPending"));
      else if (row.result_ok) {
        toast.push("success", row.result_text || t("detail.upgradeAgentOk"));
        void qc.invalidateQueries({ queryKey: ["nodes"] });
      } else toast.push("error", row.result_error || t("detail.upgradeAgentFailed"));
    } catch (e) {
      toast.push("error", t(friendlyError(e)));
    } finally {
      setBusy(false);
      setPending(null);
    }
  };

  /** Node deletion: on success, navigate back to list + invalidate nodes cache. Failure only shows toast.
   *  404 is treated as "deleted by someone else" and handled as success, redirect to list to avoid leaving
   *  user confused staring at a corpse.
   *  409 = pending commands: switch to "force delete" confirmation, `force` via ?force=1.
   *  Backend's default TTL is only 60s; if the node is reinstalled / command channel is down,
   *  those commands will never be pulled — "wait for the node to pull them" is impossible.
   *  So we must provide a path that actually works. */
  const runDeleteNode = async (force = false) => {
    setBusy(true);
    try {
      await api.deleteNode(id, force);
      toast.push("success", t("detail.deleteNodeOk"));
      void qc.invalidateQueries({ queryKey: ["nodes"] });
      setPending(null);
      navigate("/nodes");
    } catch (e) {
      const err = e as { status?: number; message?: string };
      if (err?.status === 404) {
        toast.push("success", t("detail.deleteNodeOk"));
        void qc.invalidateQueries({ queryKey: ["nodes"] });
        setPending(null);
        navigate("/nodes");
        return;
      }
      if (err?.status === 409 && !force) {
        setPending({ kind: "forceDeleteNode" });
        return;
      }
      toast.push("error", t(friendlyError(e)));
      setPending(null);
    } finally {
      setBusy(false);
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
      case "deleteNode":
        return {
          title: t("detail.deleteNode"),
          message: t("detail.deleteNodeMessage", { name: node?.hostname ?? id }),
          label: t("detail.deleteNodeConfirm"),
          danger: true,
        };
      case "forceDeleteNode":
        return {
          title: t("detail.deleteNodeForceTitle"),
          message: t("detail.deleteNodeForceMessage", { name: node?.hostname ?? id }),
          label: t("detail.deleteNodeForceConfirm"),
          danger: true,
        };
      case "upgrade":
        return {
          title: t("detail.upgradeAgent"),
          message: t("detail.upgradeAgentMessage", {
            version: pending.version,
            name: node?.hostname ?? id,
          }),
          label: t("detail.upgradeAgent"),
          danger: false,
        };
      default:
        return null;
    }
  })();

  // Toolbar (time range / refresh rate / basic info / restart / shutdown / delete) no longer pinned at the
  // content area top; instead delivered to the app header's actions area (the header is outside <Routes>,
  // can only receive from below).
  const headerActions = useHeaderActions(
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
      {latestUpgrade && (
        <Tooltip content={t("detail.upgradeAgent", { version: latestUpgrade.version })}>
          <Button
            variant="ghost"
            size="icon"
            aria-label={t("detail.upgradeAgent", { version: latestUpgrade.version })}
            onClick={() =>
              setPending({
                kind: "upgrade",
                version: latestUpgrade.version,
                sha256: latestUpgrade.sha256,
              })
            }
          >
            <Upload className="w-4 h-4" aria-hidden="true" />
          </Button>
        </Tooltip>
      )}
      <Tooltip content={t("detail.deleteNode")}>
        <Button
          variant="ghost"
          size="icon"
          aria-label={t("detail.deleteNode")}
          onClick={() => setPending({ kind: "deleteNode" })}
          className="text-rose-600 dark:text-rose-400"
        >
          <Trash2 className="w-4 h-4" aria-hidden="true" />
        </Button>
      </Tooltip>
    </div>,
  );

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
      {headerActions}

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
          {/* Large cards: CPU / memory / disk / network (up + down) */}
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

          {/* Trend charts: X axis fixed at the selected time range, individual refresh button at top-right */}
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

          {/* Process top10: switch by CPU / memory, graceful kill / force kill (icon-only with Tooltip) */}
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
                      <tr className="border-b border-surface-3 dark:border-ink-500">
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
                    <tbody className="divide-y divide-surface-2 dark:divide-ink-500">
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
                              // Hover shows the full command (wide tooltip, wraps);
                              // Clicking the command itself copies the whole line (command line may be truncated, tooltip can't show all)
                              <Tooltip content={p.cmdline} wide>
                                <button
                                  type="button"
                                  className="block max-w-[280px] truncate text-left text-xs text-ink-400 hover:text-brand-600 dark:hover:text-brand-400 transition-colors"
                                  aria-label={t("processes.copyCmd")}
                                  onClick={() => void copy(p.cmdline)}
                                >
                                  {p.cmdline}
                                </button>
                              </Tooltip>
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

          {/* Container group: list + search/filter/sort/pagination + start/stop/restart and latest logs */}
          <NodeContainers nodeId={id} refreshMs={refreshMs} />

          {/* Certificates: TLS certificates discovered on this machine, reuses the certificates page styling */}
          <NodeCerts nodeId={id} refreshMs={refreshMs} hostLabel={nodeLabel(node)} />

          {/* Reverse view of service probes: what this machine is probing on whose behalf */}
          <NodeProbes nodeId={id} refreshMs={refreshMs} />
        </>
      )}

      {/* Basic info: shown in a dialog, no longer takes page space */}
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
            <Field
              label={t("detail.alias")}
              value={node.alias?.trim() ? node.alias : t("detail.aliasEmpty")}
            />
            <div>
              <dt className="text-xs text-ink-400">{t("detail.tags")}</dt>
              <dd className="mt-1">
                {node.tags && node.tags.length > 0 ? (
                  <TagList tags={node.tags} />
                ) : (
                  <span className="text-sm text-ink-500">{t("detail.tagsEmpty")}</span>
                )}
              </dd>
            </div>
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
              label={t("detail.enrolledAt")}
              value={
                node.enrolled_at_ms
                  ? formatTime(node.enrolled_at_ms, timezone)
                  : "—"
              }
            />
            <Field
              label={t("detail.agentVersion")}
              value={node.host_info.agent_version ? `v${node.host_info.agent_version}` : "—"}
            />
            {/* Node ID: moved here from the title area (header only keeps "Nodes / name · status") */}
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
            else if (pending.kind === "shutdown") void runCommand("shutdown_host", {});
            else if (pending.kind === "upgrade")
              void runUpgradeCommand(pending.version, pending.sha256);
            else void runDeleteNode(pending.kind === "forceDeleteNode");
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

/** Large card: single value (CPU/memory/disk) or two values (network up/down) */
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

/** Trend chart card: unified loading / failure-retry / empty-state branches + per-card refresh button */
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
  // When the window start is earlier than the original retention period, the backend switches to hourly
  // aggregation — tell the user this isn't raw 10s data
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
        {query.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(query.error))}
            onRetry={() => query.refetch()}
            retrying={query.isFetching}
          />
        ) : !query.data ? (
          // Only show skeleton on initial load (no placeholder data) — subsequent window scrolls / auto-refresh
          // rely on placeholderData: keepPreviousData to hold the old curve, shouldn't flash a skeleton.
          <Skeleton className="h-64 w-full" />
        ) : points > 1 ? (
          <div className="relative">
            {children}
            {query.isFetching && (
              <div
                className="pointer-events-none absolute right-2 top-2 inline-flex items-center gap-1 rounded-full bg-surface-0/80 px-2 py-0.5 text-[10px] text-ink-400 shadow-sm backdrop-blur dark:bg-ink-800/80"
                aria-live="polite"
              >
                <RefreshCw className="h-3 w-3 animate-spin" aria-hidden="true" />
                <span>{t("action.refreshing")}</span>
              </div>
            )}
          </div>
        ) : (
          <EmptyState title={t("detail.chartEmpty")} />
        )}
      </CardBody>
    </Card>
  );
}

/** Segmented switch (absolute / ratio, by CPU / by memory): the project's unified control appearance */
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
    <div className="inline-flex rounded-md border border-surface-3 dark:border-ink-500 p-0.5">
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

/** Detail page field: label on top, value below */
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
