import * as React from "react";
import { useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Box, FileText, RefreshCw } from "lucide-react";
import {
  containerBucket,
  containerLastChangeMs,
  containerTone,
  containersApi,
  shortId,
  type ContainerBucket,
  type ContainerGroup,
  type ContainerInfo,
} from "@/api";
import { ContainerActions } from "@/components/container-actions";
import { LogFetchDialog } from "@/components/log-fetch-dialog";
import { StatCards, type StatCard } from "@/components/stat-cards";
import { Badge, DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { SearchInput } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { SortHeader, type SortDir } from "@/components/ui/sort-header";
import {
  EmptyState,
  ErrorState,
  SearchEmptyState,
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
import { Tooltip } from "@/components/ui/tooltip";
import { usePrefs } from "@/components/prefs-provider";
import {
  cn,
  containerStateLabel,
  containerStatusLabel,
  formatCpuLimit,
  formatPercent,
  formatTime,
  formatUptime,
  formatUsagePair,
  friendlyError,
  nodeLabel,
  relativeTime,
} from "@/lib/utils";

type Filter = "all" | ContainerBucket;
type SortKey = "name" | "node" | "state" | "uptime" | "updated";

/** 状态排序：异常最前，其次运行中，最后已停止 */
const STATE_RANK: Record<string, number> = {
  dead: 0,
  restarting: 1,
  running: 2,
  exited: 3,
  created: 4,
  paused: 4,
};

interface Row extends ContainerInfo {
  nodeId: string;
  hostname: string;
  /** 节点别名（空串=未设置）；列展示与过滤都用 nodeLabel */
  alias: string;
}

export function Containers() {
  const { t } = useTranslation();
  const { timezone } = usePrefs();
  const qc = useQueryClient();
  const [params, setParams] = useSearchParams();

  const [q, setQ] = React.useState("");
  const [nodeFilter, setNodeFilter] = React.useState("all");
  const [appFilter, setAppFilter] = React.useState("all");
  const [sortKey, setSortKey] = React.useState<SortKey>("state");
  const [sortDir, setSortDir] = React.useState<SortDir>("asc");
  const [page, setPage] = React.useState(1);
  const [pageSize, setPageSize] = React.useState(10);
  const [fileLogsOpen, setFileLogsOpen] = React.useState(false);

  // 概览卡片会带 ?state=failed 进来，这里按 query 初始化筛选
  const [filter, setFilter] = React.useState<Filter>(() => {
    const raw = (params.get("state") ?? "").toLowerCase();
    return raw === "failed" || raw === "running" || raw === "stopped"
      ? (raw as Filter)
      : "all";
  });

  const selectFilter = React.useCallback(
    (next: Filter) => {
      setFilter(next);
      const sp = new URLSearchParams();
      if (next !== "all") sp.set("state", next);
      setParams(sp, { replace: true });
    },
    [setParams],
  );

  const groupsQ = useQuery({
    queryKey: ["containers"],
    queryFn: containersApi.all,
  });
  const groups: ContainerGroup[] = groupsQ.data ?? [];

  const rows: Row[] = React.useMemo(
    () =>
      groups.flatMap((g) =>
        g.containers.map((c) => ({
          ...c,
          nodeId: g.node_id,
          hostname: g.hostname,
          alias: g.alias ?? "",
        })),
      ),
    [groups],
  );

  /** 四张卡片：互斥分桶，相加 = 总数 */
  const counts = React.useMemo(() => {
    const c = { all: rows.length, running: 0, stopped: 0, failed: 0, other: 0 };
    for (const r of rows) c[containerBucket(r)] += 1;
    return c;
  }, [rows]);

  const nodes = React.useMemo(
    () =>
      groups
        .map((g) => ({
          id: g.node_id,
          hostname: g.hostname,
          alias: g.alias ?? "",
        }))
        .sort((a, b) => nodeLabel(a).localeCompare(nodeLabel(b))),
    [groups],
  );

  /** 应用 = 容器自带的 compose 项目标签。非 compose 起的容器为空，不进这个列表 */
  const apps = React.useMemo(
    () =>
      Array.from(
        new Set(rows.map((r) => r.compose_project ?? "").filter((p) => p !== "")),
      ).sort((a, b) => a.localeCompare(b)),
    [rows],
  );

  React.useEffect(() => {
    setPage(1);
  }, [q, filter, nodeFilter, appFilter, pageSize, sortKey, sortDir]);

  const filtered = React.useMemo(() => {
    const needle = q.trim().toLowerCase();
    return rows.filter((c) => {
      if (filter !== "all" && containerBucket(c) !== filter) return false;
      if (nodeFilter !== "all" && c.nodeId !== nodeFilter) return false;
      if (appFilter !== "all" && (c.compose_project ?? "") !== appFilter) return false;
      if (!needle) return true;
      return (
        c.name.toLowerCase().includes(needle) ||
        c.image.toLowerCase().includes(needle) ||
        c.id.toLowerCase().includes(needle) ||
        nodeLabel(c).toLowerCase().includes(needle) ||
        c.hostname.toLowerCase().includes(needle) ||
        c.nodeId.toLowerCase().includes(needle) ||
        (c.compose_project ?? "").toLowerCase().includes(needle)
      );
    });
  }, [rows, filter, nodeFilter, appFilter, q]);

  const sorted = React.useMemo(
    () =>
      sortRows(filtered, sortDir, (c: Row) => {
        switch (sortKey) {
          case "name":
            return c.name.toLowerCase();
          case "node":
            return nodeLabel(c).toLowerCase();
          case "uptime":
            return c.started_at_unix_nano ?? 0;
          case "updated":
            return containerLastChangeMs(c);
          case "state":
          default:
            return STATE_RANK[c.state] ?? 2;
        }
      }),
    [filtered, sortDir, sortKey],
  );

  const { pageCount, current, visible } = paginate(sorted, page, pageSize);
  const now = Date.now();
  const refreshing = groupsQ.isFetching;

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir(sortDir === "asc" ? "desc" : "asc");
      return;
    }
    setSortKey(key);
    setSortDir(key === "name" || key === "node" ? "asc" : "desc");
  };

  // 顶部大字卡片一律复用 shared StatCards（规范 7.2：禁止为不同卡片写不同外壳）
  const baseCards: Array<{
    key: Filter;
    label: string;
    value: number;
    tone: "neutral" | "success" | "danger";
    hint: string;
  }> = [
    {
      key: "all",
      label: t("containers.cardAll"),
      value: counts.all,
      tone: "neutral",
      hint: t("containers.cardNodes", { n: nodes.length }),
    },
    {
      key: "running",
      label: t("containers.cardRunning"),
      value: counts.running,
      tone: counts.running > 0 ? "success" : "neutral",
      hint: t("containers.filterRunning"),
    },
    {
      key: "stopped",
      label: t("containers.cardStopped"),
      value: counts.stopped,
      tone: "neutral",
      hint: t("containers.filterStopped"),
    },
    {
      key: "failed",
      label: t("containers.cardFailed"),
      value: counts.failed,
      tone: counts.failed > 0 ? "danger" : "neutral",
      hint: t("containers.filterFailed"),
    },
  ];
  const cards: StatCard[] = baseCards.map((c) => {
    const active = filter === c.key;
    return {
      ...c,
      active,
      onClick: () =>
        selectFilter(active && c.key !== "all" ? "all" : c.key),
    };
  });

  return (
    <>
      <StatCards cards={cards} cols={4} />

      <TableShell>
        <TableToolbar>
          <div>
            <h2 className="text-base font-semibold text-ink-900 dark:text-surface-0">
              {t("containers.title")}
            </h2>
            <p className="text-sm text-ink-500 mt-0.5">{t("containers.subtitle")}</p>
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            {/* 固定宽度：别名要能整段显示，长度不随内容变化 */}
            <Select
              wrapperClassName="w-64"
              value={nodeFilter}
              onChange={(e) => setNodeFilter(e.target.value)}
              aria-label={t("containers.filterNode")}
            >
              <option value="all">{t("containers.nodeAll")}</option>
              {nodes.map((n) => (
                <option key={n.id} value={n.id}>
                  {nodeLabel(n)}
                </option>
              ))}
            </Select>
            {apps.length > 0 && (
              <Select
                wrapperClassName="w-36"
                value={appFilter}
                onChange={(e) => setAppFilter(e.target.value)}
                aria-label={t("containers.filterApp")}
              >
                <option value="all">{t("containers.appAll")}</option>
                {apps.map((a) => (
                  <option key={a} value={a}>
                    {a}
                  </option>
                ))}
              </Select>
            )}
            <Select
              wrapperClassName="w-32"
              value={filter}
              onChange={(e) => selectFilter(e.target.value as Filter)}
              aria-label={t("containers.filterState")}
            >
              <option value="all">{t("containers.filterAll")}</option>
              <option value="running">{t("containers.filterRunning")}</option>
              <option value="stopped">{t("containers.filterStopped")}</option>
              <option value="failed">{t("containers.filterFailed")}</option>
            </Select>
            <SearchInput
              className="w-full sm:w-64"
              placeholder={t("containers.searchPlaceholder")}
              value={q}
              onChange={(e) => setQ(e.target.value)}
              aria-label={t("action.search")}
            />
            <Tooltip content={t("containers.fileLogs")}>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("containers.fileLogs")}
                onClick={() => setFileLogsOpen(true)}
              >
                <FileText className="w-4 h-4" aria-hidden="true" />
              </Button>
            </Tooltip>
            <Tooltip content={t("action.refresh")}>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("action.refresh")}
                onClick={() => void qc.invalidateQueries({ queryKey: ["containers"] })}
              >
                <RefreshCw
                  className={cn("w-4 h-4", refreshing && "animate-spin")}
                  aria-hidden="true"
                />
              </Button>
            </Tooltip>
          </div>
        </TableToolbar>

        {groupsQ.isLoading ? (
          <TableSkeleton rows={6} />
        ) : groupsQ.isError ? (
          <ErrorState
            message={t(friendlyError(groupsQ.error))}
            onRetry={() => void groupsQ.refetch()}
            retrying={refreshing}
          />
        ) : rows.length === 0 ? (
          <EmptyState
            icon={<Box className="w-12 h-12" aria-hidden="true" />}
            title={t("containers.empty")}
            description={t("containers.emptyHint")}
          />
        ) : sorted.length === 0 ? (
          <SearchEmptyState
            title={t("containers.searchEmpty")}
            description={t("containers.searchEmptyHint")}
          />
        ) : (
          <>
            <Table>
              <THead>
                <tr>
                  <SortHeader
                    label={t("containers.colName")}
                    k="name"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                  />
                  <SortHeader
                    label={t("containers.colNode")}
                    k="node"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    className="hidden md:table-cell"
                  />
                  <SortHeader
                    label={t("containers.colState")}
                    k="state"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                  />
                  <Th align="right" className="hidden md:table-cell">
                    {t("containers.colCpu")}
                  </Th>
                  <Th align="right" className="hidden xl:table-cell">
                    {t("containers.colMem")}
                  </Th>
                  <SortHeader
                    label={t("containers.colUptime")}
                    k="uptime"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    align="right"
                    className="hidden lg:table-cell"
                  />
                  <SortHeader
                    label={t("containers.colUpdated")}
                    k="updated"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    align="right"
                    className="hidden lg:table-cell"
                  />
                  <Th align="right">{t("containers.colActions")}</Th>
                </tr>
              </THead>
              <TBody>
                {visible.map((c) => {
                  const change = containerLastChangeMs(c);
                  return (
                    <Tr key={`${c.nodeId}-${c.id}`}>
                      <Td>
                        <div className="flex items-center gap-2">
                          <span className="text-sm font-medium text-ink-900 dark:text-surface-0 truncate max-w-[220px]">
                            {c.name}
                          </span>
                          {c.compose_project ? (
                            <Badge tone="neutral" className="shrink-0">
                              {c.compose_project}
                            </Badge>
                          ) : null}
                        </div>
                        <div className="text-xs text-ink-400 truncate max-w-[220px]">
                          {c.image}
                          <span className="ml-2 tabular-nums">{shortId(c.id)}</span>
                        </div>
                      </Td>
                      <Td className="hidden md:table-cell">
                        <span className="text-xs text-ink-500" title={c.hostname}>
                          {nodeLabel(c)}
                        </span>
                      </Td>
                      <Td>
                        <DotBadge tone={containerTone(c.state)}>
                          {containerStateLabel(c.state, t)}
                        </DotBadge>
                        <div
                          className="mt-1 text-xs text-ink-400 truncate max-w-[200px]"
                          title={c.status}
                        >
                          {containerStatusLabel(c, t)}
                        </div>
                      </Td>
                      {/* CPU：占用 + 限额两行；未运行 / 未上报按「—」处理 */}
                      <Td className="hidden md:table-cell" align="right">
                        <div className="text-sm tabular-nums text-ink-700 dark:text-ink-100">
                          {c.state === "running" &&
                          typeof c.cpu_percent === "number"
                            ? formatPercent(c.cpu_percent, 1)
                            : "—"}
                        </div>
                        <div className="text-xs text-ink-400 tabular-nums">
                          {c.state === "running"
                            ? c.cpu_limit_nano
                              ? t("containers.ofLimit", {
                                  limit: t("containers.unitCores", {
                                    n: formatCpuLimit(c.cpu_limit_nano),
                                  }),
                                })
                              : t("containers.limitUnlimited")
                            : ""}
                        </div>
                      </Td>
                      {/* 内存：用量 / 限额（限额 0 = 不限，只显示用量） */}
                      <Td className="hidden xl:table-cell" align="right">
                        <span
                          className="text-sm tabular-nums text-ink-700 dark:text-ink-100"
                          title={
                            c.state === "running" &&
                            !c.mem_usage_bytes &&
                            !c.mem_limit_bytes
                              ? t("containers.usageUnknownHint")
                              : undefined
                          }
                        >
                          {c.state === "running"
                            ? formatUsagePair(c.mem_usage_bytes, c.mem_limit_bytes)
                            : "—"}
                        </span>
                      </Td>
                      <Td className="hidden lg:table-cell" align="right">
                        <span className="text-sm tabular-nums text-ink-500">
                          {c.state === "running" && c.started_at_unix_nano
                            ? formatUptime(
                                Math.floor(now / 1000 - c.started_at_unix_nano / 1e9),
                                t,
                              )
                            : "—"}
                        </span>
                      </Td>
                      <Td className="hidden lg:table-cell" align="right">
                        <span
                          className="text-sm tabular-nums text-ink-500"
                          title={change ? formatTime(change, timezone) : undefined}
                        >
                          {change ? relativeTime(change, t) : "—"}
                        </span>
                      </Td>
                      <Td align="right">
                        <ContainerActions nodeId={c.nodeId} container={c} />
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
                  {t("containers.total", { n: sorted.length })}
                  {sorted.length !== rows.length && (
                    <span className="ml-1 text-ink-400">
                      {t("containers.totalAll", { n: rows.length })}
                    </span>
                  )}
                </p>
              }
            />
          </>
        )}
      </TableShell>

      <LogFetchDialog
        open={fileLogsOpen}
        onClose={() => setFileLogsOpen(false)}
        defaultSource="file"
      />
    </>
  );
}
