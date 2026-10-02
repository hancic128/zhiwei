import * as React from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Box, RefreshCw } from "lucide-react";
import {
  containerTone,
  containersApi,
  shortId,
  type ContainerInfo,
} from "@/api";
import { ContainerActions } from "@/components/container-actions";
import { Badge, DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardHeader } from "@/components/ui/card";
import {
  EmptyState,
  ErrorState,
  SearchEmptyState,
  Skeleton,
} from "@/components/ui/feedback";
import { SearchInput } from "@/components/ui/input";
import { SortHeader, type SortDir } from "@/components/ui/sort-header";
import {
  Table,
  TableShell,
  TBody,
  Td,
  Th,
  THead,
  Tr,
} from "@/components/ui/table";
import { TablePager, paginate, sortRows } from "@/components/ui/pager";
import { Tooltip } from "@/components/ui/tooltip";
import { UsageBar } from "@/components/usage-bar";
import {
  containerStateLabel,
  containerStatusLabel,
  formatCpuLimit,
  formatPercent,
  formatUsagePair,
  friendlyError,
  relativeTime,
} from "@/lib/utils";

type SortKey = "name" | "state" | "cpu" | "mem";

/** 状态排序：异常最前，其次运行中，最后已停止 */
const STATE_RANK: Record<string, number> = {
  dead: 0,
  restarting: 1,
  running: 2,
  exited: 3,
  created: 4,
  paused: 4,
};

/**
 * 节点详情页的「容器组」：列表 + 搜索/排序/分页 + 启停重启。
 *
 * 数据来自 inventory 快照（节点侧 5 分钟一次），所以每个动作完成后会额外
 * 让节点「立刻重采一次快照」，并在一段时间内重复拉取，避免用户干等 5 分钟。
 *
 * 视觉与「独立容器页 /containers」一致：同一列布局（名称/状态/CPU/内存/操作），
 * 进度条、徽章、限制提示全部沿用同一份样式。
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

  const [q, setQ] = React.useState("");
  const [sortKey, setSortKey] = React.useState<SortKey>("state");
  const [sortDir, setSortDir] = React.useState<SortDir>("asc");
  const [page, setPage] = React.useState(1);

  const PAGE_SIZE = 10;

  const qy = useQuery({
    queryKey: ["containers", nodeId],
    queryFn: () => containersApi.byNode(nodeId),
    enabled: !!nodeId,
    refetchInterval: refreshMs,
  });
  const all = React.useMemo(() => qy.data?.containers ?? [], [qy.data]);

  React.useEffect(() => {
    setPage(1);
  }, [q]);

  const rows = React.useMemo(() => {
    const needle = q.trim().toLowerCase();
    const list = all.filter((c) => {
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
        case "cpu":
          // 未运行 / 未上报 = 0，统一落到 -1 沉底（默认降序：占用高的在前）
          return c.cpu_percent ?? -1;
        case "mem":
          return c.mem_usage_bytes ?? -1;
        case "state":
        default:
          return STATE_RANK[c.state] ?? 2;
      }
    });
  }, [all, q, sortKey, sortDir]);

  const { pageCount, current, visible } = paginate(rows, page, PAGE_SIZE);
  const runningCount = all.filter((c) => c.state === "running").length;

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir(sortDir === "asc" ? "desc" : "asc");
      return;
    }
    setSortKey(key);
    setSortDir(key === "name" ? "asc" : "desc");
  };

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
              <RefreshCw className="w-4 h-4" aria-hidden="true" />
            </Button>
          </Tooltip>
        }
      />

      <div className="px-4 py-3 border-b border-surface-3 dark:border-ink-700 flex flex-wrap items-center gap-2">
        <SearchInput
          className="w-full sm:w-56"
          placeholder={t("containers.searchPlaceholder")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          aria-label={t("action.search")}
        />
      </div>

      <TableShell>
        {qy.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(qy.error))}
            onRetry={() => void qy.refetch()}
            retrying={qy.isFetching}
          />
        ) : qy.isLoading ? (
          <Skeleton className="h-32 w-full m-4" />
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
                  const running = c.state === "running";
                  const memPct =
                    running && c.mem_limit_bytes
                      ? ((c.mem_usage_bytes ?? 0) / c.mem_limit_bytes) * 100
                      : null;
                  return (
                    <Tr key={c.id}>
                      <Td>
                        {/* 名称 + ID 同一列：名是头、ID 用 mono 标记成身份号（与 /containers 一致） */}
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
                      {/* CPU：占用 + 限额两行 + 进度条；未运行 / 未上报按「—」处理 */}
                      <Td className="hidden md:table-cell" align="right">
                        <div className="inline-flex flex-col items-end gap-1">
                          <div className="text-sm tabular-nums text-ink-700 dark:text-ink-100">
                            {running && typeof c.cpu_percent === "number"
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
                          <UsageBar
                            pct={
                              running && typeof c.cpu_percent === "number"
                                ? c.cpu_percent
                                : null
                            }
                          />
                        </div>
                      </Td>
                      {/* 内存：用量 / 限额 + 进度条；限额 0 = 不限，只显示用量且无条 */}
                      <Td className="hidden xl:table-cell" align="right">
                        <div className="inline-flex flex-col items-end gap-1">
                          <span
                            className="text-sm tabular-nums text-ink-700 dark:text-ink-100"
                            title={
                              running && !c.mem_usage_bytes && !c.mem_limit_bytes
                                ? t("containers.usageUnknownHint")
                                : undefined
                            }
                          >
                            {running
                              ? formatUsagePair(c.mem_usage_bytes, c.mem_limit_bytes)
                              : "—"}
                          </span>
                          <UsageBar pct={memPct} />
                        </div>
                      </Td>
                      <Td align="right">
                        <ContainerActions
                          nodeId={nodeId}
                          container={c}
                          onError={onError}
                          onDone={() => void qy.refetch().catch(() => undefined)}
                        />
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
          </>
        )}
      </TableShell>
    </Card>
  );
}