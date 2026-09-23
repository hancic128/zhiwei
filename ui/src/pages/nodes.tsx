import * as React from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { HelpCircle, Pencil, Plus, Server } from "lucide-react";
import { api, osLabel, primaryIp, trendApi, type NodeView } from "@/api";
import { EnrollTokenDialog } from "@/components/enroll-token-dialog";
import { NodeMetaDialog, TagList } from "@/components/node-meta-dialog";
import { LineChart } from "@/components/chart";
import { StatCards, type StatCard } from "@/components/stat-cards";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import { Segmented } from "@/components/ui/segmented";
import {
  TimeRangePicker,
  presetRange,
  type TimeRange,
} from "@/components/ui/date-range-picker";
import { SearchInput } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { SortHeader, type SortDir } from "@/components/ui/sort-header";
import { Tooltip } from "@/components/ui/tooltip";
import {
  EmptyState,
  ErrorState,
  SearchEmptyState,
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
import { TablePager, paginate, sortRows } from "@/components/ui/pager";
import {
  cn,
  formatBytes,
  formatPercent,
  formatUptime,
  friendlyError,
  livenessOf,
  nodeLabel,
} from "@/lib/utils";

type SortKey =
  | "hostname"
  | "status"
  | "system"
  | "mem"
  | "cpu"
  | "uptime";

/** 存活状态排序：升序 = 正常在前（离线排最后） */
const LIVENESS_RANK: Record<string, number> = {
  online: 0,
  lagging: 1,
  offline: 2,
  unknown: 3,
};

export function Nodes() {
  const { t } = useTranslation();
  const [q, setQ] = React.useState("");
  const [status, setStatus] = React.useState("all");
  const [system, setSystem] = React.useState("all");
  const [tag, setTag] = React.useState("all");
  // 默认按状态排、有问题的在前（设计：默认排序＝最需要关注的在前）。
  // 「上次更新」列已去掉，它不该再当默认排序键——那会变成一个看不见的排序依据。
  const [sortKey, setSortKey] = React.useState<SortKey>("status");
  const [sortDir, setSortDir] = React.useState<SortDir>("desc");
  const [page, setPage] = React.useState(1);
  const [pageSize, setPageSize] = React.useState(10);
  const [range, setRange] = React.useState<TimeRange>(() => presetRange("3h"));
  /** 生成入网命令的弹窗——空状态点击按钮触发 */
  const [enrollDialogOpen, setEnrollDialogOpen] = React.useState(false);
  /** 「接入帮助」：自动生成并复制命令，点开即用 */
  const [onboardDialogOpen, setOnboardDialogOpen] = React.useState(false);
  /** 正在编辑别名 / 标签的节点（null = 关窗） */
  const [metaNode, setMetaNode] = React.useState<NodeView | null>(null);

  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes });
  const nodes: NodeView[] = nodesQ.data ?? [];

  /** 系统下拉的候选值来自真实上报，不写死 */
  const systems = React.useMemo(() => {
    const set = new Set<string>();
    for (const n of nodes) {
      const label = osLabel(n.host_info);
      if (label) set.add(label);
    }
    return [...set].sort();
  }, [nodes]);

  /** 标签下拉的候选值同样来自真实数据，去重后排序 */
  const allTags = React.useMemo(() => {
    const set = new Set<string>();
    for (const n of nodes) for (const tag of n.tags ?? []) set.add(tag);
    return [...set].sort();
  }, [nodes]);

  const memOf = (n: NodeView) => {
    const used = n.latest?.mem_used_bytes ?? null;
    const total =
      n.latest?.mem_total_bytes ?? n.host_info.total_memory_bytes ?? null;
    const pct = used !== null && total ? (used / total) * 100 : null;
    return { used, total, pct };
  };

  const filtered = React.useMemo(() => {
    const needle = q.trim().toLowerCase();
    const list = nodes.filter((n) => {
      const live = livenessOf(n.last_seen_ms);
      if (status !== "all" && live !== status) return false;
      if (system !== "all" && osLabel(n.host_info) !== system) return false;
      if (tag !== "all" && !(n.tags ?? []).includes(tag)) return false;
      if (!needle) return true;
      // 搜索在**全量节点**上匹配别名 / 主机名 / 节点 ID / IP / 标签，
      // 命中的就是列表
      return (
        nodeLabel(n).toLowerCase().includes(needle) ||
        n.hostname.toLowerCase().includes(needle) ||
        n.id.toLowerCase().includes(needle) ||
        (n.tags ?? []).some((x) => x.toLowerCase().includes(needle)) ||
        primaryIp(n.host_info).toLowerCase().includes(needle)
      );
    });

    return sortRows(list, sortDir, (n: NodeView) => {
      switch (sortKey) {
        case "hostname":
          return nodeLabel(n).toLowerCase();
        case "status":
          return LIVENESS_RANK[livenessOf(n.last_seen_ms)] ?? 9;
        case "system":
          return osLabel(n.host_info).toLowerCase();
        case "mem":
          return n.latest?.mem_used_bytes ?? n.latest?.mem_total_bytes ?? -1;
        case "cpu":
          return n.latest?.cpu_percent ?? -1;
        case "uptime":
          return n.host_info.uptime_seconds ?? -1;
        default:
          return n.last_seen_ms ?? 0;
      }
    });
  }, [nodes, q, status, system, tag, sortKey, sortDir]);

  // 过滤 / 搜索 / 每页条数变化后回到第 1 页，否则会停在空页
  React.useEffect(() => {
    setPage(1);
  }, [q, status, system, tag, pageSize]);

  const { pageCount, current, visible: rows } = paginate(filtered, page, pageSize);

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir(sortDir === "asc" ? "desc" : "asc");
      return;
    }
    setSortKey(key);
    // 文字默认从 A 排，数值与时间默认从大排
    setSortDir(key === "hostname" || key === "system" ? "asc" : "desc");
  };

  // ── 顶部大字卡片：按存活状态分桶，点一下就地筛选下面那张表 ──
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
      active: status === "all",
      onClick: () => setStatus("all"),
    },
    {
      key: "online",
      label: t("state.online"),
      value: liveCount("online"),
      hint: t("nodes.cardOnlineHint"),
      tone: "success",
      active: status === "online",
      onClick: () => setStatus("online"),
    },
    {
      key: "lagging",
      label: t("state.lagging"),
      value: liveCount("lagging"),
      hint: t("nodes.cardLaggingHint"),
      tone: liveCount("lagging") > 0 ? "warn" : "neutral",
      active: status === "lagging",
      onClick: () => setStatus("lagging"),
    },
    {
      key: "offline",
      label: t("state.offline"),
      value: liveCount("offline"),
      hint: t("nodes.cardOfflineHint"),
      tone: liveCount("offline") > 0 ? "danger" : "neutral",
      active: status === "offline",
      onClick: () => setStatus("offline"),
    },
  ];

  return (
    <>
    <div className="space-y-4">
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
        <div className="flex flex-wrap items-center gap-2">
          <Select
            wrapperClassName="w-32"
            value={status}
            onChange={(e) => setStatus(e.target.value)}
            aria-label={t("nodes.filterStatus")}
          >
            <option value="all">{t("nodes.statusAll")}</option>
            <option value="online">{t("state.online")}</option>
            <option value="lagging">{t("state.lagging")}</option>
            <option value="offline">{t("state.offline")}</option>
          </Select>
          <Select
            wrapperClassName="w-40"
            value={system}
            onChange={(e) => setSystem(e.target.value)}
            aria-label={t("nodes.filterSystem")}
          >
            <option value="all">{t("nodes.systemAll")}</option>
            {systems.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
          <Select
            wrapperClassName="w-40"
            value={tag}
            onChange={(e) => setTag(e.target.value)}
            aria-label={t("nodes.filterTag")}
            disabled={allTags.length === 0}
          >
            <option value="all">{t("nodes.tagAll")}</option>
            {allTags.map((x) => (
              <option key={x} value={x}>
                {x}
              </option>
            ))}
          </Select>
          <SearchInput
            className="w-full sm:w-72 lg:w-80"
            placeholder={t("nodes.searchPlaceholder")}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            aria-label={t("action.search")}
          />
          <Tooltip content={t("nodes.onboardHelpHint")}>
            <Button
              variant="secondary"
              onClick={() => setOnboardDialogOpen(true)}
            >
              <HelpCircle className="w-4 h-4" aria-hidden="true" />
              {t("nodes.onboardHelp")}
            </Button>
          </Tooltip>
        </div>
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
      ) : filtered.length === 0 ? (
        <SearchEmptyState
          title={t("nodes.searchEmpty")}
          description={t("nodes.searchEmptyHint")}
        />
      ) : (
        <>
          <Table>
            <THead>
              <tr>
                <SortHeader
                  label={t("nodes.colHost")}
                  k="hostname"
                  sortKey={sortKey}
                  sortDir={sortDir}
                  onSort={onSort}
                />
                <SortHeader
                  label={t("nodes.colStatus")}
                  k="status"
                  sortKey={sortKey}
                  sortDir={sortDir}
                  onSort={onSort}
                  className="hidden md:table-cell"
                />
                <SortHeader
                  label={t("nodes.colOs")}
                  k="system"
                  sortKey={sortKey}
                  sortDir={sortDir}
                  onSort={onSort}
                  className="hidden lg:table-cell"
                />
                <Th className="hidden xl:table-cell">{t("nodes.colIp")}</Th>
                <SortHeader
                  label={t("nodes.colMem")}
                  k="mem"
                  sortKey={sortKey}
                  sortDir={sortDir}
                  onSort={onSort}
                  align="right"
                />
                <SortHeader
                  label={t("nodes.colCpu")}
                  k="cpu"
                  sortKey={sortKey}
                  sortDir={sortDir}
                  onSort={onSort}
                  align="right"
                />
                <SortHeader
                  label={t("nodes.colUptime")}
                  k="uptime"
                  sortKey={sortKey}
                  sortDir={sortDir}
                  onSort={onSort}
                  align="right"
                  className="hidden md:table-cell"
                />
              </tr>
            </THead>
            <TBody>
              {rows.map((node) => {
                const live = livenessOf(node.last_seen_ms);
                const { used, total, pct } = memOf(node);
                const cpu = node.latest?.cpu_percent ?? null;
                return (
                  <Tr key={node.id}>
                    <Td>
                      <div className="flex items-start gap-1">
                        <Link
                          to={`/nodes/${node.id}`}
                          className="block min-w-0 flex-1"
                        >
                          <div className="text-sm font-medium text-ink-900 dark:text-surface-0 truncate max-w-[200px]">
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
                        <Tooltip content={t("nodeMeta.edit")}>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="w-7 h-7 shrink-0"
                            aria-label={t("nodeMeta.edit")}
                            onClick={() => setMetaNode(node)}
                          >
                            <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
                          </Button>
                        </Tooltip>
                      </div>
                    </Td>

                    <Td className="hidden md:table-cell">
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
                      <div className="text-sm tabular-nums text-ink-900 dark:text-surface-0">
                        {pct === null ? "—" : formatPercent(pct, 0)}
                      </div>
                      <div className="text-xs tabular-nums text-ink-400">
                        {used === null
                          ? "—"
                          : `${formatBytes(used)} / ${formatBytes(total ?? 0)}`}
                      </div>
                    </Td>

                    <Td align="right">
                      <span
                        className={cn(
                          "text-sm tabular-nums",
                          cpu !== null && cpu > 80
                            ? "text-rose-600 dark:text-rose-400"
                            : "text-ink-900 dark:text-surface-0",
                        )}
                      >
                        {cpu === null ? "—" : formatPercent(cpu)}
                      </span>
                    </Td>

                    <Td className="hidden md:table-cell" align="right">
                      <span className="text-sm tabular-nums text-ink-500">
                        {formatUptime(node.host_info.uptime_seconds, t)}
                      </span>
                    </Td>

                  </Tr>
                );
              })}
            </TBody>
          </Table>
          <TablePager
            page={current}
            pageCount={pageCount}
            pageSize={pageSize}
            onPage={setPage}
            onPageSize={setPageSize}
            left={
              <p className="text-xs text-ink-500">
                {t("nodes.total", { n: filtered.length })}
                {filtered.length !== nodes.length && (
                  <span className="ml-1 text-ink-400">
                    {t("nodes.totalAll", { n: nodes.length })}
                  </span>
                )}
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
 */
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
  const [metric, setMetric] = React.useState<"cpu" | "mem">("cpu");
  const q = useQuery({
    queryKey: ["series-nodes", metric, range.from, range.to],
    queryFn: () =>
      trendApi.allNodes(
        metric === "cpu" ? "host.cpu.usage" : "host.mem.usage",
        { from: range.from, to: range.to, limit: 300 },
      ),
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
              onChange={(v) => setMetric(v as "cpu" | "mem")}
              options={[
                { value: "cpu", label: t("detail.kpiCpu") },
                { value: "mem", label: t("detail.kpiMem") },
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
        ) : (
          <LineChart
            series={series}
            height="md"
            unit="%"
            yMax={100}
            xMin={range.from}
            xMax={range.to}
          />
        )}
      </CardBody>
    </Card>
  );
}
