import * as React from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Box, RotateCw } from "lucide-react";
import {
  containerLastChangeMs,
  containerIsFailed,
  containerTone,
  containersApi,
  shortId,
  type ContainerInfo,
} from "@/api";
import { ContainerActions } from "@/components/container-actions";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import {
  EmptyState,
  ErrorState,
  SearchEmptyState,
  Skeleton,
} from "@/components/ui/feedback";
import { SearchInput } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { SortHeader, type SortDir } from "@/components/ui/sort-header";
import { TBody, Td, Th, THead, Tr } from "@/components/ui/table";
import { TablePager, paginate, sortRows } from "@/components/ui/pager";
import { Tooltip } from "@/components/ui/tooltip";
import { usePrefs } from "@/components/prefs-provider";
import {
  formatCpuLimit,
  formatPercent,
  formatTime,
  formatUptime,
  formatUsagePair,
  friendlyError,
  relativeTime,
} from "@/lib/utils";

type StatusFilter = "all" | "running" | "stopped" | "failed";
type SortKey = "name" | "state" | "uptime" | "updated";

/** 状态排序：异常最前，其次运行中，最后已停止 */
const STATE_RANK: Record<string, number> = {
  dead: 0,
  restarting: 1,
  exited: 3,
  created: 4,
  paused: 4,
};

/**
 * 节点详情页的「容器组」：列表 + 搜索/过滤/排序/分页 + 启停重启与看日志。
 *
 * 数据来自 inventory 快照（节点侧 5 分钟一次），所以每个动作完成后会额外
 * 让节点「立刻重采一次快照」，并在一段时间内重复拉取，避免用户干等 5 分钟。
 */
export function NodeContainers({
  nodeId,
  refreshMs,
  onError,
}: {
  nodeId: string;
  refreshMs: number;
  onError?: (message: string) => void;
}) {
  const { t } = useTranslation();
  const { timezone } = usePrefs();

  const [q, setQ] = React.useState("");
  const [status, setStatus] = React.useState<StatusFilter>("all");
  const [sortKey, setSortKey] = React.useState<SortKey>("state");
  const [sortDir, setSortDir] = React.useState<SortDir>("asc");
  const [page, setPage] = React.useState(1);
  const [pageSize, setPageSize] = React.useState(10);

  const qy = useQuery({
    queryKey: ["containers", nodeId],
    queryFn: () => containersApi.byNode(nodeId),
    enabled: !!nodeId,
    refetchInterval: refreshMs,
  });
  const all = React.useMemo(() => qy.data?.containers ?? [], [qy.data]);

  React.useEffect(() => {
    setPage(1);
  }, [q, status, pageSize]);

  const rows = React.useMemo(() => {
    const needle = q.trim().toLowerCase();
    const list = all.filter((c) => {
      if (status === "running" && c.state !== "running") return false;
      if (status === "stopped" && c.state === "running") return false;
      if (status === "failed" && !containerIsFailed(c)) return false;
      if (!needle) return true;
      return (
        c.name.toLowerCase().includes(needle) ||
        c.image.toLowerCase().includes(needle) ||
        c.id.toLowerCase().includes(needle)
        );
    });
    return sortRows(list, sortDir, (c: ContainerInfo) => {
      switch (sortKey) {
        case "name":
          return c.name.toLowerCase();
        case "uptime":
          return c.started_at_unix_nano ?? 0;
        case "updated":
          return containerLastChangeMs(c);
        case "state":
        default:
          return STATE_RANK[c.state] ?? 2;
      }
    });
  }, [all, q, status, sortKey, sortDir]);

  const { pageCount, current, visible } = paginate(rows, page, pageSize);
  const runningCount = all.filter((c) => c.state === "running").length;

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir(sortDir === "asc" ? "desc" : "asc");
      return;
    }
    setSortKey(key);
    setSortDir(key === "name" ? "asc" : "desc");
  };
  const now = Date.now();

  return (
    <Card>
      <CardHeader
        title={t("containers.title")}
        description={t("containers.nodeSubtitle", {
          n: all.length,
          running: runningCount,
        })}
        action={
          <Tooltip content={t("action.refresh")}>
            <Button
              variant="ghost"
              size="icon"
              aria-label={t("action.refresh")}
              disabled={qy.isFetching}
              onClick={() => void qy.refetch()}
            >
              <RotateCw className="w-4 h-4" aria-hidden="true" />
            </Button>
          </Tooltip>
        }
      />

      <div className="px-4 py-3 border-b border-surface-3 dark:border-ink-700 flex flex-wrap items-center gap-2">
        <Select
          wrapperClassName="w-32"
          className="h-8 text-xs"
          value={status}
          onChange={(e) => setStatus(e.target.value as StatusFilter)}
          aria-label={t("containers.filterState")}
        >
          <option value="all">{t("containers.filterAll")}</option>
          <option value="running">{t("containers.filterRunning")}</option>
          <option value="stopped">{t("containers.filterStopped")}</option>
          <option value="failed">{t("containers.filterFailed")}</option>
        </Select>
        <SearchInput
          className="w-full sm:w-56"
          placeholder={t("containers.searchPlaceholder")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          aria-label={t("action.search")}
        />
      </div>

      <CardBody compact>
        {qy.isLoading ? (
          <Skeleton className="h-32 w-full" />
        ) : qy.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(qy.error))}
            onRetry={() => void qy.refetch()}
            retrying={qy.isFetching}
          />
        ) : all.length === 0 ? (
          <EmptyState
            icon={<Box className="w-12 h-12" aria-hidden="true" />}
            title={t("containers.nodeEmpty")}
            description={t("containers.nodeEmptyHint")}
          />
        ) : rows.length === 0 ? (
          <SearchEmptyState
            title={t("containers.searchEmpty")}
            description={t("containers.searchEmptyHint")}
          />
        ) : (
          <div className="overflow-x-auto scrollbar-thin">
            <table className="w-full text-sm">
              <THead>
                <tr>
                  <Th>{t("containers.colId")}</Th>
                  <SortHeader
                    label={t("containers.colName")}
                    k="name"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
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
                  <Th align="right" className="hidden lg:table-cell">
                    {t("containers.colMem")}
                  </Th>
                  <SortHeader
                    label={t("containers.colUptime")}
                    k="uptime"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    align="right"
                    className="hidden md:table-cell"
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
                  const running = c.state === "running";
                  const change = containerLastChangeMs(c);
                  return (
                    <Tr key={c.id}>
                      <Td>
                        <span className="text-xs font-mono text-ink-400">
                          {shortId(c.id)}
                        </span>
                      </Td>
                      <Td>
                        <div className="text-sm text-ink-900 dark:text-surface-0 truncate max-w-[220px]">
                          {c.name}
                        </div>
                        <div className="text-xs text-ink-400 truncate max-w-[220px]">
                          {c.image}
                        </div>
                      </Td>
                      <Td>
                        <DotBadge tone={containerTone(c.state)}>{c.state}</DotBadge>
                      </Td>
                      {/* CPU：占用 + 限额两行；未运行 / 未上报按「—」处理 */}
                      <Td className="hidden md:table-cell" align="right">
                        <div className="text-sm tabular-nums text-ink-700 dark:text-ink-100">
                          {typeof c.cpu_percent === "number"
                            ? formatPercent(c.cpu_percent, 1)
                            : "—"}
                        </div>
                        <div className="text-xs text-ink-400 tabular-nums">
                          {running
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
                      <Td className="hidden lg:table-cell" align="right">
                        <span className="text-sm tabular-nums text-ink-700 dark:text-ink-100">
                          {running
                            ? formatUsagePair(c.mem_usage_bytes, c.mem_limit_bytes)
                            : "—"}
                        </span>
                      </Td>
                      <Td className="hidden md:table-cell" align="right">
                        <span className="text-sm tabular-nums text-ink-500">
                          {running && c.started_at_unix_nano
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
                        <ContainerActions
                          nodeId={nodeId}
                          container={c}
                          onError={onError}
                          onDone={() =>
                            void qy.refetch().catch(() => undefined)
                          }
                        />
                      </Td>
                    </Tr>
                  );
                })}
              </TBody>
            </table>
          </div>
        )}
      </CardBody>

      {rows.length > 0 && (
        <TablePager
          page={current}
          pageCount={pageCount}
          pageSize={pageSize}
          onPage={setPage}
          onPageSize={setPageSize}
          left={
            <p className="text-xs text-ink-500">
              {t("containers.total", { n: rows.length })}
              {qy.data?.ts_unix_nano ? (
                <span className="ml-1 text-ink-400">
                  ·{" "}
                  {t("containers.snapshotAt", {
                    ago: relativeTime(Math.floor(qy.data.ts_unix_nano / 1e6), t),
                  })}
                </span>
              ) : null}
            </p>
          }
        />
      )}

    </Card>
  );
}
