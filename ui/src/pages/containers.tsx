import * as React from "react";
import { useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Box } from "lucide-react";
import {
  containerBucket,
  containerTone,
  containersApi,
  shortId,
  type ContainerBucket,
  type ContainerGroup,
  type ContainerInfo,
} from "@/api";
import { ContainerActions } from "@/components/container-actions";
import { StatCards, type StatCard } from "@/components/stat-cards";
import { Badge, DotBadge } from "@/components/ui/badge";
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
import { UsageBar } from "@/components/usage-bar";
import {
  containerStateLabel,
  containerStatusLabel,
  formatCpuLimit,
  formatPercent,
  formatUsagePair,
  friendlyError,
  nodeLabel,
} from "@/lib/utils";

type Filter = "all" | ContainerBucket;
type SortKey = "name" | "node" | "state" | "cpu" | "mem";

/** State sort order: failed first, then running, then stopped */
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
  /** Node alias (empty string = unset); both column display and filtering use nodeLabel */
  alias: string;
}

export function Containers() {
  const { t } = useTranslation();
  const [params, setParams] = useSearchParams();

  const [q, setQ] = React.useState("");
  const [nodeFilter, setNodeFilter] = React.useState("all");
  const [appFilter, setAppFilter] = React.useState("all");
  const [sortKey, setSortKey] = React.useState<SortKey>("state");
  const [sortDir, setSortDir] = React.useState<SortDir>("asc");
  const [page, setPage] = React.useState(1);
  const [pageSize, setPageSize] = React.useState(10);

  // Overview cards arrive with ?state=failed; initialize filter from the query
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
    refetchInterval: 15000,
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

  /** Four cards: mutually exclusive buckets, sum equals the total */
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

  /** App = the compose project label carried by the container. Containers not started via compose have no label and don't appear in this list */
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
          case "cpu":
            // Not running / no report = 0, normalized to -1 so it sinks (default desc: highest usage first)
            return c.cpu_percent ?? -1;
          case "mem":
            return c.mem_usage_bytes ?? -1;
          case "state":
          default:
            return STATE_RANK[c.state] ?? 2;
        }
      }),
    [filtered, sortDir, sortKey],
  );

  const { pageCount, current, visible } = paginate(sorted, page, pageSize);
  const refreshing = groupsQ.isFetching;

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir(sortDir === "asc" ? "desc" : "asc");
      return;
    }
    setSortKey(key);
    setSortDir(key === "name" || key === "node" ? "asc" : "desc");
  };

  // Top large cards always reuse the shared StatCards (spec 7.2: forbid different shells for different cards)
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
            {/* Fixed width: the alias must display in full, length does not vary with content */}
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
                  <SortHeader
                    label={t("containers.colCpu")}
                    k="cpu"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    align="right"
                    className="hidden md:table-cell"
                  />
                  <SortHeader
                    label={t("containers.colMem")}
                    k="mem"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    align="right"
                    className="hidden xl:table-cell"
                  />
                  <Th align="right">{t("containers.colActions")}</Th>
                </tr>
              </THead>
              <TBody>
                {visible.map((c) => {
                  const memPct =
                    c.state === "running" && c.mem_limit_bytes
                      ? ((c.mem_usage_bytes ?? 0) / c.mem_limit_bytes) * 100
                      : null;
                  return (
                    <Tr key={`${c.nodeId}-${c.id}`}>
                      <Td>
                        {/* Name + ID in the same column: name as the header, ID shown in mono as an identifier so it's clear at a glance "which node is this" */}
                        <div className="flex items-center gap-2 min-w-0">
                          <span className="text-sm font-medium text-ink-900 dark:text-surface-0 truncate max-w-[220px]">
                            {c.name}
                          </span>
                          {c.compose_project ? (
                            <Badge tone="neutral" className="shrink-0">
                              {c.compose_project}
                            </Badge>
                          ) : null}
                        </div>
                        <div className="mt-1 flex items-center gap-2 min-w-0">
                          <span
                            className="font-mono text-[11px] tabular-nums text-ink-500 dark:text-ink-400 shrink-0"
                            title={c.id}
                          >
                            {shortId(c.id)}
                          </span>
                          <span className="text-xs text-ink-400 truncate max-w-[200px]">
                            {c.image}
                          </span>
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
                      {/* CPU: usage + limit two lines + bar; not running / no report shown as "—" */}
                      <Td className="hidden md:table-cell" align="right">
                        <div className="inline-flex flex-col items-end gap-1">
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
                          <UsageBar
                            pct={
                              c.state === "running" &&
                              typeof c.cpu_percent === "number"
                                ? c.cpu_percent
                                : null
                            }
                          />
                        </div>
                      </Td>
                      {/* Memory: usage / limit + bar; limit 0 = unlimited, only show usage with no bar */}
                      <Td className="hidden xl:table-cell" align="right">
                        <div className="inline-flex flex-col items-end gap-1">
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
                          <UsageBar pct={memPct} />
                        </div>
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
    </>
  );
}
