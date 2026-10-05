import * as React from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { HelpCircle, Pencil, Plus, Server, Trash2, Upload } from "lucide-react";
import { api, osLabel, primaryIp, trendApi, upgradeApi, type NodeView } from "@/api";
import { EnrollTokenDialog } from "@/components/enroll-token-dialog";
import { NodeMetaDialog, TagList } from "@/components/node-meta-dialog";
import { LineChart } from "@/components/chart";
import { StatCards, type StatCard } from "@/components/stat-cards";
import { DotBadge } from "@/components/ui/badge";
import { ChannelDownBadge } from "@/components/command-channel";
import { Button } from "@/components/ui/button";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import { Segmented } from "@/components/ui/segmented";
import {
  TimeRangePicker,
  presetRange,
  type TimeRange,
} from "@/components/ui/date-range-picker";
import { Tooltip } from "@/components/ui/tooltip";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { useToast } from "@/components/ui/toast";
import {
  EmptyState,
  ErrorState,
  Skeleton,
  TableSkeleton,
} from "@/components/ui/feedback";
import {
  Table,
  TableShell,
  TableToolbar,
  TBody,
  Td,
  Th,
  THead,
  Tr,
} from "@/components/ui/table";
import { TablePager, paginate } from "@/components/ui/pager";
import { UsageBar } from "@/components/usage-bar";
import {
  cn,
  formatBytes,
  formatPercent,
  formatRate,
  formatUptime,
  friendlyError,
  livenessOf,
  nodeLabel,
} from "@/lib/utils";

/** Liveness sort order: ascending = healthy first (node offline last) */
const LIVENESS_RANK: Record<string, number> = {
  online: 0,
  lagging: 1,
  offline: 2,
  unknown: 3,
};

/**
 * Usage threshold coloring: ≤60% green, >60% ≤80% yellow, >80% red.
 * Input is 0..100 percentage; null / undefined goes to "no data" branch.
 */
function usageTone(pct: number | null | undefined): string {
  if (pct === null || pct === undefined) {
    return "text-ink-400";
  }
  if (pct > 80) return "text-rose-600 dark:text-rose-400";
  if (pct > 60) return "text-amber-600 dark:text-amber-400";
  return "text-emerald-600 dark:text-emerald-400";
}

/** Check if a node needs upgrade: agent version < latest upgrade version */
function needsUpgrade(node: NodeView, latest: string | undefined): boolean {
  if (!latest || !node.host_info?.agent_version) return false;
  // Compare semver-ish versions: "1.2.3" vs "1.2.4"
  const nodeVer = node.host_info.agent_version.replace(/^v/, "");
  return nodeVer !== latest;
}

export function Nodes() {
  const { t } = useTranslation();
  // Default sort by status, problematic first (design: default sort = most attention-needed first).
  const [page, setPage] = React.useState(1);
  const [range, setRange] = React.useState<TimeRange>(() => presetRange("3h"));
  /** Dialog to generate enroll command — triggered by button on empty state */
  const [enrollDialogOpen, setEnrollDialogOpen] = React.useState(false);
  /** "Onboarding help": auto-generates and copies command, ready to use on open */
  const [onboardDialogOpen, setOnboardDialogOpen] = React.useState(false);
  /** Node currently editing alias / tags (null = dialog closed) */
  const [metaNode, setMetaNode] = React.useState<NodeView | null>(null);
  /** Node pending deletion (null = dialog closed) */
  const [deleteNode, setDeleteNode] = React.useState<NodeView | null>(null);
  /** When deletion is blocked by 409 (pending commands exist), switch dialog to "force delete" confirmation */
  const [forceDelete, setForceDelete] = React.useState(false);
  const [deleting, setDeleting] = React.useState(false);
  /** Upgrade dialog state: selected nodes for batch upgrade (null = dialog closed) */
  const [upgradeNodes, setUpgradeNodes] = React.useState<NodeView[] | null>(null);
  /** Single node upgrade dialog */
  const [upgradeNode, setUpgradeNode] = React.useState<NodeView | null>(null);
  const [upgrading, setUpgrading] = React.useState(false);
  const toast = useToast();
  const qc = useQueryClient();

  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes });
  const nodes: NodeView[] = nodesQ.data ?? [];

  // Fetch latest upgrade package info
  const latestUpgradeQ = useQuery({
    queryKey: ["latestUpgrade"],
    queryFn: () => upgradeApi.latest().catch(() => null),
    staleTime: 60_000,
  });
  const latestUpgrade = latestUpgradeQ.data;

  const memOf = (n: NodeView) => {
    const used = n.latest?.mem_used_bytes ?? null;
    const total =
      n.latest?.mem_total_bytes ?? n.host_info.total_memory_bytes ?? null;
    const pct = used !== null && total ? (used / total) * 100 : null;
    return { used, total, pct };
  };

  // Disk: the mount point with highest usage (precomputed on the node side); use the
  // percentage from the backend directly. Older nodes that don't report host.disk.usage
  // fall back to used / total; both missing = "no data".
  const diskOf = (n: NodeView) => {
    const used = n.latest?.disk_used_bytes ?? null;
    const total = n.latest?.disk_total_bytes ?? null;
    const pct =
      n.latest?.disk_usage_percent ??
      (used !== null && total ? (used / total) * 100 : null);
    return { used, total, pct };
  };

  // Default sort: status rank + last heartbeat descending (oldest heartbeat last).
  // Stable sort: preserve relative position unless the node's status actually changes.
  const rows = React.useMemo(() => {
    const list = [...nodes].sort((a, b) => {
      const ra = LIVENESS_RANK[livenessOf(a.last_seen_ms)] ?? 9;
      const rb = LIVENESS_RANK[livenessOf(b.last_seen_ms)] ?? 9;
      if (ra !== rb) return ra - rb;
      return (b.last_seen_ms ?? 0) - (a.last_seen_ms ?? 0);
    });
    return list;
  }, [nodes]);

  // 10 per page, no page-size switcher
  const PAGE_SIZE = 10;
  const { pageCount, current, visible: visibleRows } = paginate(rows, page, PAGE_SIZE);

  // ── Top large cards: bucketed by liveness ──
  const liveCount = (k: string) =>
    nodes.filter((n) => livenessOf(n.last_seen_ms) === k).length;
  const cards: StatCard[] = [
    {
      key: "all",
      label: t("nodes.cardAll"),
      value: nodes.length,
      hint: t("nodes.cardAllHint"),
      tone: "neutral",
      icon: Server,
    },
    {
      key: "online",
      label: t("state.online"),
      value: liveCount("online"),
      hint: t("nodes.cardOnlineHint"),
      tone: "success",
      icon: Server,
    },
    {
      key: "lagging",
      label: t("state.lagging"),
      value: liveCount("lagging"),
      hint: t("nodes.cardLaggingHint"),
      tone: liveCount("lagging") > 0 ? "warn" : "neutral",
      icon: Server,
    },
    {
      key: "offline",
      label: t("state.offline"),
      value: liveCount("offline"),
      hint: t("nodes.cardOfflineHint"),
      tone: liveCount("offline") > 0 ? "danger" : "neutral",
      icon: Server,
    },
  ];

  return (
    <>
    <div className="space-y-4">
    {/* Delete node: success / 404 (already deleted by someone else) both count as success,
        close dialog and refresh list; failure only shows toast.
        Reuses the same semantics as node-detail.tsx (detail.deleteNodeMessage describes this
        in detail — don't simplify the wording here to avoid inconsistency about
        "risk of incomplete deletion").
        409 = pending commands: the backend's default TTL is only 60s; if the node stops polling,
        those commands will never be consumed, so here we switch the dialog to a "force delete"
        second confirmation (force via ?force=1), rather than showing a toast the user can't act on. */}
    <ConfirmDialog
      open={deleteNode !== null}
      title={t(forceDelete ? "detail.deleteNodeForceTitle" : "detail.deleteNode")}
      message={t(
        forceDelete ? "detail.deleteNodeForceMessage" : "detail.deleteNodeMessage",
        { name: deleteNode?.alias || deleteNode?.hostname || deleteNode?.id || "" },
      )}
      confirmLabel={t(
        forceDelete ? "detail.deleteNodeForceConfirm" : "detail.deleteNodeConfirm",
      )}
      cancelLabel={t("action.cancel")}
      danger
      loading={deleting}
      onCancel={() => {
        if (!deleting) {
          setDeleteNode(null);
          setForceDelete(false);
        }
      }}
      onConfirm={async () => {
        if (!deleteNode) return;
        setDeleting(true);
        try {
          await api.deleteNode(deleteNode.id, forceDelete);
          toast.push("success", t("detail.deleteNodeOk"));
          setDeleteNode(null);
          setForceDelete(false);
          void qc.invalidateQueries({ queryKey: ["nodes"] });
        } catch (e) {
          const err = e as { status?: number; message?: string };
          if (err?.status === 404) {
            toast.push("success", t("detail.deleteNodeOk"));
            setDeleteNode(null);
            setForceDelete(false);
            void qc.invalidateQueries({ queryKey: ["nodes"] });
            return;
          }
          if (err?.status === 409 && !forceDelete) {
            setForceDelete(true);
            return;
          }
          toast.push("error", t(friendlyError(e)));
        } finally {
          setDeleting(false);
        }
      }}
    />

    {/* Batch upgrade dialog */}
    <ConfirmDialog
      open={upgradeNodes !== null}
      title={t("nodes.upgradeTitle")}
      message={t("nodes.upgradeMessage", {
        count: upgradeNodes?.length ?? 0,
        version: latestUpgrade?.version ?? "",
      })}
      confirmLabel={t("nodes.upgradeConfirm")}
      cancelLabel={t("action.cancel")}
      loading={upgrading}
      onCancel={() => !upgrading && setUpgradeNodes(null)}
      onConfirm={async () => {
        if (!upgradeNodes || !latestUpgrade) return;
        setUpgrading(true);
        const baseUrl = `${window.location.protocol}//${window.location.host}`;
        const downloadUrl = `${baseUrl}/v1/upgrade/${latestUpgrade.version}`;
        let success = 0;
        let failed = 0;
        for (const node of upgradeNodes) {
          try {
            await api.execCommand(node.id, "upgrade_agent", {
              version: latestUpgrade.version,
              download_url: downloadUrl,
              sha256: latestUpgrade.sha256,
              restart: true,
            });
            success++;
          } catch {
            failed++;
          }
        }
        toast.push(
          "success",
          t("nodes.upgradeResults", { success, failed }),
        );
        setUpgradeNodes(null);
        setUpgrading(false);
        void qc.invalidateQueries({ queryKey: ["nodes"] });
      }}
    />

    {/* Single node upgrade dialog */}
    <ConfirmDialog
      open={upgradeNode !== null}
      title={t("nodes.upgradeTitle")}
      message={t("nodes.upgradeSingleMessage", {
        node: upgradeNode?.alias || upgradeNode?.hostname || upgradeNode?.id || "",
        version: latestUpgrade?.version ?? "",
      })}
      confirmLabel={t("nodes.upgradeConfirm")}
      cancelLabel={t("action.cancel")}
      loading={upgrading}
      onCancel={() => !upgrading && setUpgradeNode(null)}
      onConfirm={async () => {
        if (!upgradeNode || !latestUpgrade) return;
        setUpgrading(true);
        const baseUrl = `${window.location.protocol}//${window.location.host}`;
        const downloadUrl = `${baseUrl}/v1/upgrade/${latestUpgrade.version}`;
        try {
          await api.execCommand(upgradeNode.id, "upgrade_agent", {
            version: latestUpgrade.version,
            download_url: downloadUrl,
            sha256: latestUpgrade.sha256,
            restart: true,
          });
          toast.push("success", t("nodes.upgradeSuccess"));
        } catch (e) {
          toast.push("error", t(friendlyError(e)));
        }
        setUpgradeNode(null);
        setUpgrading(false);
        void qc.invalidateQueries({ queryKey: ["nodes"] });
      }}
    />

    <StatCards cards={cards} />

    {!nodesQ.isLoading && nodes.length === 0 && (
      <div className="rounded-xl border border-dashed border-surface-3 dark:border-ink-700 bg-surface-0 dark:bg-ink-700 p-2">
        <EmptyState
          icon={<Server className="w-12 h-12" aria-hidden="true" />}
          title={t("overview.topNodesEmpty")}
          description={t("overview.topNodesEmptyHint")}
          action={
            <Button onClick={() => setEnrollDialogOpen(true)}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("dialog.createEnrollToken")}
            </Button>
          }
        />
      </div>
    )}

    {nodes.length > 0 && (
      <NodesTrend nodes={nodes} range={range} onRange={setRange} />
    )}

    <TableShell>
      <TableToolbar>
        <div>
          <h3 className="text-base font-semibold text-ink-900 dark:text-surface-0">
            {t("nodes.title")}
          </h3>
          <p className="text-sm text-ink-500 mt-0.5">{t("nodes.subtitle")}</p>
        </div>
        <Tooltip content={t("nodes.onboardHelpHint")}>
          <Button
            variant="ghost"
            onClick={() => setOnboardDialogOpen(true)}
          >
            <HelpCircle className="w-4 h-4" aria-hidden="true" />
            {t("nodes.onboardHelp")}
          </Button>
        </Tooltip>
        {latestUpgrade && (
          <Tooltip content={t("nodes.batchUpgradeHint", { version: latestUpgrade.version })}>
            <Button
              variant="primary"
              disabled={!upgradeNodes || upgradeNodes.length === 0}
              onClick={() => setUpgradeNodes(upgradeNodes)}
            >
              <Upload className="w-4 h-4" aria-hidden="true" />
              {t("nodes.batchUpgrade", {
                version: latestUpgrade.version,
                count: upgradeNodes?.length ?? 0,
              })}
            </Button>
          </Tooltip>
        )}
      </TableToolbar>

      {nodesQ.isLoading ? (
        <TableSkeleton rows={5} />
      ) : nodesQ.isError ? (
        <ErrorState
          message={t(friendlyError(nodesQ.error))}
          onRetry={() => void nodesQ.refetch()}
          retrying={nodesQ.isFetching}
        />
      ) : nodes.length === 0 ? (
        <EmptyState
          icon={<Server className="w-12 h-12" aria-hidden="true" />}
          title={t("nodes.empty")}
          description={t("nodes.emptyHint")}
          action={
            <Button onClick={() => setEnrollDialogOpen(true)}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("dialog.createEnrollToken")}
            </Button>
          }
        />
      ) : (
        <>
          <Table>
            <THead>
              <tr>
                <Th className="w-8">
                  <input
                    type="checkbox"
                    className="rounded border-ink-300 dark:border-ink-600"
                    checked={upgradeNodes?.length === nodes.length}
                    onChange={(e) =>
                      setUpgradeNodes(e.target.checked ? nodes : [])
                    }
                  />
                </Th>
                <Th>{t("nodes.colHost")}</Th>
                <Th className="hidden md:table-cell">{t("nodes.colStatus")}</Th>
                <Th className="hidden lg:table-cell">{t("nodes.colOs")}</Th>
                <Th className="hidden xl:table-cell">{t("nodes.colIp")}</Th>
                <Th align="right">{t("nodes.colMem")}</Th>
                <Th align="right">{t("nodes.colDisk")}</Th>
                <Th align="right">{t("nodes.colCpu")}</Th>
                <Th align="right" className="w-px whitespace-nowrap">
                  {t("nodes.colActions")}
                </Th>
              </tr>
            </THead>
            <TBody>
              {visibleRows.map((node) => {
                const live = livenessOf(node.last_seen_ms);
                const { used, total, pct } = memOf(node);
                const {
                  used: diskUsed,
                  total: diskTotal,
                  pct: diskPct,
                } = diskOf(node);
                const cpu = node.latest?.cpu_percent ?? null;
                return (
                  <Tr key={node.id}>
                    <Td>
                      <div className="flex items-start gap-1">
                        <Link
                          to={`/nodes/${node.id}`}
                          className="group/link block min-w-0 flex-1 rounded -mx-1 px-1 py-0.5 -my-0.5 cursor-pointer transition-colors hover:bg-surface-2 dark:hover:bg-ink-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500"
                        >
                          <div className="text-sm font-medium text-ink-900 dark:text-surface-0 group-hover/link:text-brand-700 dark:group-hover/link:text-brand-100 truncate max-w-[200px]">
                            {nodeLabel(node)}
                          </div>
                          <div className="text-xs text-ink-400 truncate max-w-[200px]">
                            {node.alias ? node.hostname : node.id}
                          </div>
                          <TagList
                            tags={node.tags ?? []}
                            className="mt-1 flex flex-wrap gap-1"
                          />
                        </Link>
                      </div>
                    </Td>

                    <Td>
                      <input
                        type="checkbox"
                        className="rounded border-ink-300 dark:border-ink-600"
                        checked={upgradeNodes?.some((n) => n.id === node.id) ?? false}
                        onChange={(e) => {
                          if (e.target.checked) {
                            setUpgradeNodes([...(upgradeNodes ?? []), node]);
                          } else {
                            setUpgradeNodes(
                              (upgradeNodes ?? []).filter((n) => n.id !== node.id),
                            );
                          }
                        }}
                      />
                    </Td>

                    <Td className="hidden md:table-cell">
                      <div className="flex flex-col gap-0.5">
                        <div className="flex items-center gap-1.5">
                          <DotBadge
                            tone={
                              live === "online"
                                ? "success"
                                : live === "lagging"
                                  ? "warn"
                                  : "neutral"
                            }
                            pulse={live === "online"}
                          >
                            {t(`state.${live}`)}
                          </DotBadge>
                          <ChannelDownBadge channel={node.command_channel} />
                        </div>
                        {node.host_info?.uptime_seconds != null ? (
                          <span className="text-xs tabular-nums text-ink-500 dark:text-surface-4">
                            {formatUptime(node.host_info.uptime_seconds, t)}
                          </span>
                        ) : null}
                      </div>
                    </Td>

                    <Td className="hidden lg:table-cell">
                      <span className="text-xs text-ink-500 dark:text-surface-4">
                        {osLabel(node.host_info) || "—"}
                      </span>
                    </Td>

                    <Td className="hidden xl:table-cell">
                      <span className="text-xs tabular-nums text-ink-500">
                        {primaryIp(node.host_info) || "—"}
                      </span>
                    </Td>

                    <Td align="right">
                      <div className="inline-flex flex-col items-end gap-1">
                        <div className={cn("text-sm tabular-nums", usageTone(pct))}>
                          {pct === null ? "—" : formatPercent(pct, 0)}
                        </div>
                        <div className="text-xs tabular-nums text-ink-400">
                          {used === null
                            ? "—"
                            : `${formatBytes(used)} / ${formatBytes(total ?? 0)}`}
                        </div>
                        <UsageBar pct={pct} />
                      </div>
                    </Td>

                    <Td align="right">
                      <div className="inline-flex flex-col items-end gap-1">
                        <div
                          className={cn(
                            "text-sm tabular-nums",
                            usageTone(diskPct),
                          )}
                        >
                          {diskPct === null ? "—" : formatPercent(diskPct, 0)}
                        </div>
                        <div className="text-xs tabular-nums text-ink-400">
                          {diskUsed === null
                            ? "—"
                            : `${formatBytes(diskUsed)} / ${formatBytes(diskTotal ?? 0)}`}
                        </div>
                        <UsageBar pct={diskPct} />
                      </div>
                    </Td>

                    <Td align="right">
                      <div className="inline-flex flex-col items-end gap-1">
                        <span className={cn("text-sm tabular-nums", usageTone(cpu))}>
                          {cpu === null ? "—" : formatPercent(cpu)}
                        </span>
                        <UsageBar pct={cpu} />
                      </div>
                    </Td>

                    <Td align="right">
                      {/* Actions column: edit (alias/tags) + delete + upgrade.
                          Delete uses rose color to align with "dangerous command" semantics.
                          Upgrade icon: RefreshCw, shown only when node agent version < latest upgrade. */}
                      <div className="inline-flex items-center gap-1">
                        {latestUpgrade && needsUpgrade(node, latestUpgrade.version) && (
                          <Tooltip content={t("nodes.upgradeNode", { version: latestUpgrade.version })}>
                            <Button
                              variant="ghost"
                              size="icon"
                              className="w-7 h-7 text-brand-600 hover:bg-brand-50 hover:text-brand-700 dark:text-brand-400 dark:hover:bg-brand-950/40 dark:hover:text-brand-300"
                              aria-label={t("nodes.upgradeNode", { version: latestUpgrade.version })}
                              onClick={() => setUpgradeNode(node)}
                            >
                              <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                                <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
                                <path d="M3 3v5h5" />
                                <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
                                <path d="M16 16h5v5" />
                              </svg>
                            </Button>
                          </Tooltip>
                        )}
                        <Tooltip content={t("nodeMeta.edit")}>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="w-7 h-7"
                            aria-label={t("nodeMeta.edit")}
                            onClick={() => setMetaNode(node)}
                          >
                            <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
                          </Button>
                        </Tooltip>
                        <Tooltip content={t("detail.deleteNode")}>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="w-7 h-7 text-rose-600 hover:bg-rose-50 hover:text-rose-700 dark:text-rose-400 dark:hover:bg-rose-950/40 dark:hover:text-rose-300"
                            aria-label={t("detail.deleteNode")}
                            onClick={() => setDeleteNode(node)}
                          >
                            <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                          </Button>
                        </Tooltip>
                      </div>
                    </Td>

                  </Tr>
                );
              })}
            </TBody>
          </Table>
          <TablePager
            page={current}
            pageCount={pageCount}
            pageSize={PAGE_SIZE}
            onPage={setPage}
            left={
              <p className="text-xs text-ink-500">
                {t("nodes.total", { n: rows.length })}
              </p>
            }
          />
        </>
      )}
    </TableShell>
    </div>

    <EnrollTokenDialog
      open={enrollDialogOpen}
      onClose={() => setEnrollDialogOpen(false)}
    />
    {/* Onboarding help: opens and immediately generates the command and copies it,
        skipping the "choose TTL then click create" step */}
    <EnrollTokenDialog
      autoCreate
      open={onboardDialogOpen}
      onClose={() => setOnboardDialogOpen(false)}
    />
    <NodeMetaDialog node={metaNode} onClose={() => setMetaNode(null)} />
    </>
  );
}

/**
 * All nodes trend: one line per node, with legend.
 *
 * Downsampling and long-window source switching are handled by the backend
 * (`/v1/series/nodes`); this component only draws.
 * Series names use the alias (falls back to hostname), and series are built
 * from **enrolled nodes** — the backend only returns nodes with data, so using
 * the response directly would make offline / newly-enrolled nodes disappear from the legend.
 *
 * Metric switching: CPU / memory / disk drawn as 0–100%; network (up/down) is a counter,
 * so switch to `rate=1` to let the backend convert adjacent-point differences / dt into
 * bytes/s; the frontend then displays as rate (to avoid the "cumulative bytes" line
 * monotonically increasing and sitting flat at 99%).
 */
type TrendMetric = "cpu" | "mem" | "disk" | "netUp" | "netDown";

const TREND_METRICS: Record<
  TrendMetric,
  { metric: string; rate: boolean; unit?: string }
> = {
  cpu: { metric: "host.cpu.usage", rate: false, unit: "%" },
  mem: { metric: "host.mem.usage", rate: false, unit: "%" },
  disk: { metric: "host.disk.usage", rate: false, unit: "%" },
  netUp: { metric: "host.net.tx_bytes", rate: true },
  netDown: { metric: "host.net.rx_bytes", rate: true },
};

function NodesTrend({
  nodes,
  range,
  onRange,
}: {
  nodes: NodeView[];
  range: TimeRange;
  onRange: (r: TimeRange) => void;
}) {
  const { t } = useTranslation();
  const [metric, setMetric] = React.useState<TrendMetric>("cpu");
  const cfg = TREND_METRICS[metric];
  const q = useQuery({
    queryKey: ["series-nodes", metric, range.from, range.to],
    queryFn: () =>
      trendApi.allNodes(cfg.metric, {
        from: range.from,
        to: range.to,
        limit: 300,
        rate: cfg.rate,
      }),
  });

  const series = React.useMemo(() => {
    const pointsByNode = new Map(
      (q.data?.nodes ?? []).map((n) => [n.node_id, n.points]),
    );
    return nodes.map((n) => ({
      name: nodeLabel(n),
      data: (pointsByNode.get(n.id) ?? []).map(
        (p) => [p.t, p.v] as [number, number],
      ),
    }));
  }, [nodes, q.data]);

  // Only show empty state when there's no data at all: legend should always be visible
  // (it carries "which nodes are on the chart"), but with zero points an empty grid
  // is worse than saying "no data in this window".
  const hasAnyPoint = series.some((s) => s.data.length > 0);

  // Network metric has no natural upper bound (peaks depend on environment), so let Y auto-scale;
  // percentage metrics fixed at 0–100.
  const isPercent = cfg.unit === "%";

  return (
    <Card>
      <CardHeader
        title={t("nodes.trendTitle")}
        description={
          q.data?.resolution === "hourly"
            ? t("nodes.trendHourly")
            : t("nodes.trendSubtitle")
        }
        action={
          <>
            <Segmented
              value={metric}
              onChange={(v) => setMetric(v as TrendMetric)}
              options={[
                { value: "cpu", label: t("detail.kpiCpu") },
                { value: "mem", label: t("detail.kpiMem") },
                { value: "disk", label: t("detail.kpiDisk") },
                { value: "netUp", label: t("nodes.trendNetUp") },
                { value: "netDown", label: t("nodes.trendNetDown") },
              ]}
            />
            <TimeRangePicker value={range} onChange={onRange} />
          </>
        }
      />
      <CardBody compact>
        {q.isPending ? (
          <Skeleton className="h-64 w-full" />
        ) : q.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : !hasAnyPoint ? (
          <EmptyState
            title={t("detail.chartEmpty")}
            description={t("detail.chartEmptyHint")}
          />
        ) : isPercent ? (
          <LineChart
            series={series}
            height="md"
            unit="%"
            yMax={100}
            xMin={range.from}
            xMax={range.to}
          />
        ) : (
          <LineChart
            series={series}
            height="md"
            yFormatter={formatBytes}
            valueFormatter={formatRate}
            yAxisName={t("detail.axisRate")}
            xMin={range.from}
            xMax={range.to}
          />
        )}
      </CardBody>
    </Card>
  );
}
