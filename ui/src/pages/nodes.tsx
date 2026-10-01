import * as React from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { HelpCircle, Pencil, Plus, Server, Trash2 } from "lucide-react";
import { api, osLabel, primaryIp, trendApi, type NodeView } from "@/api";
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

/** 存活状态排序：升序 = 正常在前（离线排最后） */
const LIVENESS_RANK: Record<string, number> = {
  online: 0,
  lagging: 1,
  offline: 2,
  unknown: 3,
};

/**
 * 用率阈值配色：≤60% 绿、>60% ≤80% 黄、>80% 红。
 * 入参为 0..100 的百分比，null / undefined 走「无数据」分支。
 */
function usageTone(pct: number | null | undefined): string {
  if (pct === null || pct === undefined) {
    return "text-ink-400";
  }
  if (pct > 80) return "text-rose-600 dark:text-rose-400";
  if (pct > 60) return "text-amber-600 dark:text-amber-400";
  return "text-emerald-600 dark:text-emerald-400";
}

export function Nodes() {
  const { t } = useTranslation();
  // 默认按状态排、有问题的在前（设计：默认排序＝最需要关注的在前）。
  const [page, setPage] = React.useState(1);
  const [range, setRange] = React.useState<TimeRange>(() => presetRange("3h"));
  /** 生成入网命令的弹窗——空状态点击按钮触发 */
  const [enrollDialogOpen, setEnrollDialogOpen] = React.useState(false);
  /** 「接入帮助」：自动生成并复制命令，点开即用 */
  const [onboardDialogOpen, setOnboardDialogOpen] = React.useState(false);
  /** 正在编辑别名 / 标签的节点（null = 关窗） */
  const [metaNode, setMetaNode] = React.useState<NodeView | null>(null);
  /** 待删除节点（null = 关窗） */
  const [deleteNode, setDeleteNode] = React.useState<NodeView | null>(null);
  /** 删除被 409 挡下（还有未发出的命令）后，把弹窗换成「作废并删除」再确认一次 */
  const [forceDelete, setForceDelete] = React.useState(false);
  const [deleting, setDeleting] = React.useState(false);
  const toast = useToast();
  const qc = useQueryClient();

  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes });
  const nodes: NodeView[] = nodesQ.data ?? [];

  const memOf = (n: NodeView) => {
    const used = n.latest?.mem_used_bytes ?? null;
    const total =
      n.latest?.mem_total_bytes ?? n.host_info.total_memory_bytes ?? null;
    const pct = used !== null && total ? (used / total) * 100 : null;
    return { used, total, pct };
  };

  // 磁盘：用量最高的挂载点（节点侧已算好），百分比直接用后端的读数；
  // 老节点没上报 host.disk.usage 时按 used / total 兜底，两个都没有就是「无数据」。
  const diskOf = (n: NodeView) => {
    const used = n.latest?.disk_used_bytes ?? null;
    const total = n.latest?.disk_total_bytes ?? null;
    const pct =
      n.latest?.disk_usage_percent ??
      (used !== null && total ? (used / total) * 100 : null);
    return { used, total, pct };
  };

  // 默认排序：状态 rank + 上次心跳降序（最久没上报的排后）。
  // 使用稳定的排序：优先保持节点在列表中的相对位置，除非节点状态真的变了
  const rows = React.useMemo(() => {
    const list = [...nodes].sort((a, b) => {
      const ra = LIVENESS_RANK[livenessOf(a.last_seen_ms)] ?? 9;
      const rb = LIVENESS_RANK[livenessOf(b.last_seen_ms)] ?? 9;
      if (ra !== rb) return ra - rb;
      return (b.last_seen_ms ?? 0) - (a.last_seen_ms ?? 0);
    });
    return list;
  }, [nodes]);

  // 每页 10 个，不显示切换控件
  const PAGE_SIZE = 10;
  const { pageCount, current, visible: visibleRows } = paginate(rows, page, PAGE_SIZE);

  // ── 顶部大字卡片：按存活状态分桶 ──
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
    {/* 删除节点：成功 / 404（已被别人删）都当作成功，关窗并刷新列表；失败只弹 toast。
        复用了 node-detail.tsx 同样的语义（detail.deleteNodeMessage 描述得很详细，
        不要在这里再简写文案，免得「删不干净的风险」前后描述不一致）。
        409 = 还有未发出的命令：后端默认 TTL 只有 60s，节点不再来拉的行永远有效不了，
        所以这里把弹窗换成「作废并删除」的二次确认（force 走 ?force=1），
        而不是丢一句用户没法执行的 toast。 */}
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
                      {/* 操作列：编辑（别名/标签）+ 删除。
                          删除用 rose 配色与「命令危险动作」语义对齐。 */}
                      <div className="inline-flex items-center gap-1">
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
    {/* 接入帮助：打开即生成命令并自动复制，省掉「先选 TTL 再点创建」 */}
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
 * 全部节点趋势：**一条线一个节点**，带图例。
 *
 * 抽稀与长窗口切源都由后端统一处理（`/v1/series/nodes`），这里只管画。
 * 系列名用别名（没设别名回落主机名），并且以**已入网节点**为准建系列——
 * 后端只返回有数据的节点，直接照抄会让离线 / 刚入网的机器整条从图例里消失。
 *
 * 指标切换：CPU / 内存 / 磁盘按 0–100% 画；网络（上行 / 下行）是计数器，
 * 切到 `rate=1` 让后端按相邻两点差分 / dt 转成 bytes/s，前端再按速率格式
 * 显示（避免「累计字节数」一直单调递增，曲线贴在 99% 附近没有信息量）。
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

  // 全都没数据时才给空状态：图例本身要一直显示（它承担「图上有哪些节点」），
  // 但一条点都没有时，一张空网格不如明说「这段时间没有数据」。
  const hasAnyPoint = series.some((s) => s.data.length > 0);

  // 网络指标的曲线没有自然上限（峰值依赖环境），让 Y 轴自动；
  // 百分比类指标固定 0–100。
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
