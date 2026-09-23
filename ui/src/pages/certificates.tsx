import * as React from "react";
import { useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  FlaskConical,
  Info,
  Pencil,
  Plus,
  RefreshCw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  api,
  certSourcesApi,
  certsApi,
  daysLeft,
  expiryTone,
  type CertSourceView,
  type CertInfo,
  type NodeCertsGroup,
} from "@/api";
import { CertScanDialog, CertSourceDialog } from "@/components/cert-source-dialog";
import {
  CertDetailDialog,
  type CertDetailTarget,
} from "@/components/cert-detail-dialog";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { StatCards } from "@/components/stat-cards";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { SearchInput } from "@/components/ui/input";
import { SortHeader, type SortDir } from "@/components/ui/sort-header";
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
import { Tooltip } from "@/components/ui/tooltip";
import { useToast } from "@/components/ui/toast";
import { usePrefs } from "@/components/prefs-provider";
import {
  cn,
  formatTime,
  friendlyError,
  nodeLabel,
  relativeTime,
} from "@/lib/utils";

type Filter = "all" | "expiring" | "expired";
type SortKey = "domain" | "node" | "expiry" | "days";

interface FlatCert extends CertInfo {
  nodeId: string;
  hostname: string;
  /** 显示用节点名（别名优先）。hostname 是节点自报的，不随别名变。 */
  label: string;
  days: number;
}

export function Certificates() {
  const { t } = useTranslation();
  const { timezone } = usePrefs();
  const qc = useQueryClient();
  const toast = useToast();
  const [searchParams, setSearchParams] = useSearchParams();
  const [q, setQ] = React.useState("");
  const [formOpen, setFormOpen] = React.useState(false);
  const [editing, setEditing] = React.useState<CertSourceView | null>(null);
  const [testing, setTesting] = React.useState<CertSourceView | null>(null);
  const [removing, setRemoving] = React.useState<CertSourceView | null>(null);
  const [detail, setDetail] = React.useState<CertDetailTarget | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [sortKey, setSortKey] = React.useState<SortKey>("days");
  const [sortDir, setSortDir] = React.useState<SortDir>("asc");
  const [page, setPage] = React.useState(1);
  const [pageSize, setPageSize] = React.useState(10);

  // 概览卡片会带 ?filter=expiring 进来，这里按 query 初始化筛选
  const [filter, setFilter] = React.useState<Filter>(() => {
    const raw = (searchParams.get("filter") ?? "").toLowerCase();
    return raw === "expiring" || raw === "expired" ? (raw as Filter) : "all";
  });

  const selectFilter = React.useCallback(
    (next: Filter) => {
      setFilter(next);
      const params = new URLSearchParams();
      if (next !== "all") params.set("filter", next);
      setSearchParams(params, { replace: true });
    },
    [setSearchParams],
  );

  const groupsQ = useQuery({ queryKey: ["certs"], queryFn: certsApi.all });
  const sourcesQ = useQuery({
    queryKey: ["cert-sources"],
    queryFn: certSourcesApi.list,
  });
  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes });

  const groups: NodeCertsGroup[] = groupsQ.data ?? [];
  const sources: CertSourceView[] = sourcesQ.data ?? [];
  // 「测试」要真发命令，默认挑最近还在线的节点（离线机器只会等回执超时）
  const nodes = React.useMemo(
    () =>
      [...(nodesQ.data ?? [])].sort(
        (a, b) => (b.last_seen_ms ?? 0) - (a.last_seen_ms ?? 0),
      ),
    [nodesQ.data],
  );

  // 证书接口只带 hostname，别名要回节点列表里取一次：
  // 证书页的「节点」列与别名显示保持一致，不然同一个节点会有两个名字。
  const labelByNodeId = React.useMemo(() => {
    const m = new Map<string, string>();
    for (const n of nodesQ.data ?? []) m.set(n.id, nodeLabel(n));
    return m;
  }, [nodesQ.data]);
  const labelOf = (nodeId: string, fallback?: string | null) =>
    labelByNodeId.get(nodeId) || fallback || "";

  const flat: FlatCert[] = React.useMemo(
    () =>
      groups
        .flatMap((g) =>
          g.certificates.map((c) => ({
            ...c,
            nodeId: g.node_id,
            hostname: g.hostname,
            label: labelByNodeId.get(g.node_id) || g.hostname,
            days: daysLeft(c.not_after_unix_nano),
          })),
        )
        .sort((a, b) => a.days - b.days), // 最紧急的在前
    [groups, labelByNodeId],
  );

  const counts = React.useMemo(() => {
    const valid = flat.filter((c) => !c.parse_error);
    return {
      total: valid.length,
      expired: valid.filter((c) => c.days < 0).length,
      expiring: valid.filter((c) => c.days >= 0 && c.days < 30).length,
    };
  }, [flat]);

  /** 证书条目 → 来源路径（老节点不带 source_id 时按路径规则兜底匹配） */
  const sourcePathOf = React.useCallback(
    (c: FlatCert): string | null => {
      const byId = sources.find(
        (s) =>
          s.id === c.source_id && (s.all_nodes || s.node_id === c.nodeId),
      );
      if (byId) return byId.path;
      if (c.source_id) return null;
      const byPath = sources.find(
        (s) =>
          (s.all_nodes || s.node_id === c.nodeId) && globMatch(s.path, c.path),
      );
      return byPath?.path ?? null;
    },
    [sources],
  );

  const needle = q.trim().toLowerCase();
  const filtered = flat.filter((c) => {
    if (filter === "expiring" && !(c.days >= 0 && c.days < 30)) return false;
    if (filter === "expired" && c.days >= 0) return false;
    if (!needle) return true;
    return (
      c.subject.toLowerCase().includes(needle) ||
      c.path.toLowerCase().includes(needle) ||
      c.issuer.toLowerCase().includes(needle) ||
      c.hostname.toLowerCase().includes(needle) ||
      c.label.toLowerCase().includes(needle) ||
      c.domains.some((d) => d.toLowerCase().includes(needle))
    );
  });

  const sorted = React.useMemo(
    () =>
      sortRows(filtered, sortDir, (c: FlatCert) => {
        switch (sortKey) {
          case "domain":
            return (c.domains[0] || c.subject || c.path).toLowerCase();
          case "node":
            return c.label.toLowerCase();
          case "expiry":
            return c.not_after_unix_nano;
          case "days":
          default:
            return c.days;
        }
      }),
    [filtered, sortDir, sortKey],
  );

  // 过滤 / 搜索 / 排序变化后回到第 1 页，否则会停在空页
  React.useEffect(() => {
    setPage(1);
  }, [q, filter, pageSize, sortKey, sortDir]);

  const { pageCount, current, visible } = paginate(sorted, page, pageSize);

  /** 来源列表里按 id 取一条（详情里显示这条证书属于哪个来源） */
  const sourceOf = (c: FlatCert): CertSourceView | undefined =>
    sources.find(
      (s) => s.id === c.source_id && (s.all_nodes || s.node_id === c.nodeId),
    );

  const onSort = (key: SortKey) => {
    if (key === sortKey) {
      setSortDir(sortDir === "asc" ? "desc" : "asc");
      return;
    }
    setSortKey(key);
    setSortDir(key === "domain" || key === "node" ? "asc" : "desc");
  };

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["cert-sources"] });
    void qc.invalidateQueries({ queryKey: ["certs"] });
  };

  const toggleNotify = async (s: CertSourceView, value: boolean) => {
    try {
      await certSourcesApi.patch(s.id, { notify_enabled: value });
      invalidate();
    } catch (e) {
      toast.push("error", t(friendlyError(e)));
    }
  };

  const remove = async (s: CertSourceView) => {
    setBusy(true);
    try {
      await certSourcesApi.remove(s.id);
      toast.push("success", t("toast.deleted"));
      invalidate();
    } catch (e) {
      toast.push("error", t(friendlyError(e)));
    } finally {
      setBusy(false);
      setRemoving(null);
    }
  };

  return (
    <div className="space-y-4 md:space-y-6">
      {/* 顶部大字卡片：按到期紧急度分桶，点一下就地筛选下面的列表 */}
      <StatCards
        cards={[
          {
            key: "all",
            label: t("certs.cardAll"),
            value: counts.total,
            hint: t("certs.cardAllHint"),
            tone: "neutral",
            icon: ShieldCheck,
            active: filter === "all",
            onClick: () => selectFilter("all"),
          },
          {
            key: "expired",
            label: t("certs.cardExpired"),
            value: counts.expired,
            hint: t("certs.cardExpiredHint"),
            tone: counts.expired > 0 ? "danger" : "neutral",
            active: filter === "expired",
            onClick: () => selectFilter("expired"),
          },
          {
            key: "expiring",
            label: t("certs.cardExpiring"),
            value: counts.expiring,
            hint: t("certs.cardExpiringHint"),
            tone: counts.expiring > 0 ? "warn" : "neutral",
            active: filter === "expiring",
            onClick: () => selectFilter("expiring"),
          },
          {
            key: "healthy",
            label: t("certs.cardHealthy"),
            value: counts.total - counts.expired - counts.expiring,
            hint: t("certs.cardHealthyHint"),
            tone: "success",
          },
        ]}
      />

      {/* 证书路径（来源）配置 */}
      <Card>
        <CardHeader
          icon={<ShieldCheck className="w-5 h-5 text-brand-600" aria-hidden="true" />}
          title={t("certs.sources.title")}
          description={t("certs.sources.subtitle")}
          action={
            <Button
              onClick={() => {
                setEditing(null);
                setFormOpen(true);
              }}
            >
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("certs.sources.add")}
            </Button>
          }
        />
        <CardBody compact>
          {sourcesQ.isLoading ? (
            <Skeleton className="h-24 w-full" />
          ) : sourcesQ.isError ? (
            <ErrorState
              compact
              message={t(friendlyError(sourcesQ.error))}
              onRetry={() => void sourcesQ.refetch()}
              retrying={sourcesQ.isFetching}
            />
          ) : sources.length === 0 ? (
            <EmptyState
              title={t("certs.sources.empty")}
              description={t("certs.sources.emptyHint")}
            />
          ) : (
            <div className="overflow-x-auto scrollbar-thin">
              <table className="w-full text-sm">
                <THead>
                  <tr>
                    <Th>{t("certs.sources.colPath")}</Th>
                    <Th className="hidden md:table-cell">
                      {t("certs.sources.colNode")}
                    </Th>
                    <Th>{t("certs.sources.colNotify")}</Th>
                    <Th align="right">{t("certs.sources.colMatched")}</Th>
                    <Th className="hidden lg:table-cell" align="right">
                      {t("certs.sources.colScanned")}
                    </Th>
                    <Th align="right">{t("containers.colActions")}</Th>
                  </tr>
                </THead>
                <TBody>
                  {sources.map((s) => (
                    <Tr key={s.id}>
                      <Td>
                        <div className="text-sm text-ink-900 dark:text-surface-0 truncate max-w-[280px]">
                          {s.path}
                        </div>
                        {!s.enabled && (
                          <DotBadge tone="neutral">
                            {t("certs.sources.disabled")}
                          </DotBadge>
                        )}
                      </Td>
                      <Td className="hidden md:table-cell">
                        {s.all_nodes ? (
                          <DotBadge tone="neutral">
                            {t("certs.sources.allNodes")}
                          </DotBadge>
                        ) : (
                          <span className="text-xs text-ink-500">
                            {s.node_hostname ? (
                              labelOf(s.node_id, s.node_hostname)
                            ) : (
                              <span className="text-rose-600 dark:text-rose-400">
                                {t("certs.sources.nodeGone")}
                              </span>
                            )}
                          </span>
                        )}
                      </Td>
                      <Td>
                        <button
                          type="button"
                          onClick={() => void toggleNotify(s, !s.notify_enabled)}
                          aria-pressed={s.notify_enabled}
                          className={cn(
                            "rounded-md px-2 py-1 text-xs font-medium transition-colors",
                            s.notify_enabled
                              ? "bg-emerald-50 text-emerald-700 dark:bg-emerald-700/20 dark:text-emerald-300"
                              : "bg-surface-2 text-ink-500 dark:bg-ink-700/60",
                          )}
                        >
                          {s.notify_enabled ? t("certs.sources.notifyOn") : t("certs.sources.notifyOff")}
                        </button>
                        {s.notify_enabled && (
                          <div className="mt-1 text-xs text-ink-400">
                            {t("certs.sources.notifyBeforeShort", {
                              n: s.notify_days_before,
                            })}
                          </div>
                        )}
                      </Td>
                      <Td align="right">
                        <span className="text-sm tabular-nums text-ink-900 dark:text-surface-0">
                          {s.matched}
                        </span>
                        {s.nearest_days_left !== null && (
                          <div className="mt-1">
                            <DotBadge tone={expiryTone(s.nearest_days_left)}>
                              {s.nearest_days_left < 0
                                ? t("certs.expired", {
                                    n: Math.abs(Math.floor(s.nearest_days_left)),
                                  })
                                : t("certs.daysLeft", {
                                    n: Math.floor(s.nearest_days_left),
                                  })}
                            </DotBadge>
                          </div>
                        )}
                      </Td>
                      <Td className="hidden lg:table-cell" align="right">
                        <span className="text-xs text-ink-500 tabular-nums">
                          {s.snapshot_at_unix_nano
                            ? relativeTime(
                                Math.floor(s.snapshot_at_unix_nano / 1e6),
                                t,
                              )
                            : "—"}
                        </span>
                      </Td>
                      <Td align="right">
                        <div className="flex items-center justify-end gap-1">
                          <Tooltip content={t("certs.sources.test")}>
                            <Button
                              variant="ghost"
                              size="icon"
                              aria-label={t("certs.sources.test")}
                              onClick={() => setTesting(s)}
                            >
                              <FlaskConical className="w-4 h-4" aria-hidden="true" />
                            </Button>
                          </Tooltip>
                          <Tooltip content={t("action.edit")}>
                            <Button
                              variant="ghost"
                              size="icon"
                              aria-label={t("action.edit")}
                              onClick={() => {
                                setEditing(s);
                                setFormOpen(true);
                              }}
                            >
                              <Pencil className="w-4 h-4" aria-hidden="true" />
                            </Button>
                          </Tooltip>
                          <Tooltip content={t("action.delete")}>
                            <Button
                              variant="ghost"
                              size="icon"
                              aria-label={t("action.delete")}
                              onClick={() => setRemoving(s)}
                              className="text-rose-600 dark:text-rose-400"
                            >
                              <Trash2 className="w-4 h-4" aria-hidden="true" />
                            </Button>
                          </Tooltip>
                        </div>
                      </Td>
                    </Tr>
                  ))}
                </TBody>
              </table>
            </div>
          )}
        </CardBody>
      </Card>

      {/* 证书列表 */}
      <TableShell>
        <TableToolbar>
          <div>
            <h2 className="text-base font-semibold text-ink-900 dark:text-surface-0">
              {t("certs.title")}
            </h2>
            <p className="text-sm text-ink-500 mt-0.5">
              {t("certs.subtitle")}
              {counts.total > 0 && (
                <span className="ml-2">
                  · {t("certs.total", { n: counts.total })}
                  {counts.expiring > 0 && (
                    <span className="text-amber-600 dark:text-amber-400">
                      {" "}
                      · {t("certs.expiringCount", { n: counts.expiring })}
                    </span>
                  )}
                  {counts.expired > 0 && (
                    <span className="text-rose-600 dark:text-rose-400">
                      {" "}
                      · {t("certs.expiredCount", { n: counts.expired })}
                    </span>
                  )}
                </span>
              )}
            </p>
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            <div className="flex items-center gap-1 rounded-lg border border-surface-3 dark:border-ink-700 p-1">
              {(["all", "expiring", "expired"] as Filter[]).map((f) => (
                <button
                  key={f}
                  type="button"
                  onClick={() => selectFilter(f)}
                  className={cn(
                    "px-3 py-1.5 rounded-md text-xs font-medium transition-colors",
                    filter === f
                      ? "bg-brand-50 text-brand-700 dark:bg-brand-900 dark:text-brand-100"
                      : "text-ink-500 hover:bg-surface-2 dark:hover:bg-ink-700/60",
                  )}
                >
                  {t(
                    f === "all"
                      ? "certs.filterAll"
                      : f === "expiring"
                        ? "certs.filterExpiring"
                        : "certs.filterExpired",
                  )}
                </button>
              ))}
            </div>
            <SearchInput
              className="w-full sm:w-64"
              placeholder={t("certs.searchPlaceholder")}
              value={q}
              onChange={(e) => setQ(e.target.value)}
              aria-label={t("action.search")}
            />
            <Tooltip content={t("action.refresh")}>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("action.refresh")}
                onClick={invalidate}
              >
                <RefreshCw
                  className={cn(
                    "w-4 h-4",
                    (groupsQ.isFetching || sourcesQ.isFetching) && "animate-spin",
                  )}
                  aria-hidden="true"
                />
              </Button>
            </Tooltip>
          </div>
        </TableToolbar>

        {groupsQ.isLoading ? (
          <TableSkeleton rows={4} />
        ) : groupsQ.isError ? (
          <ErrorState
            message={t(friendlyError(groupsQ.error))}
            onRetry={() => void groupsQ.refetch()}
            retrying={groupsQ.isFetching}
          />
        ) : flat.length === 0 ? (
          <EmptyState
            icon={<ShieldCheck className="w-12 h-12" aria-hidden="true" />}
            title={t("certs.empty")}
            description={t("certs.emptyHint")}
          />
        ) : visible.length === 0 ? (
          <SearchEmptyState
            title={t("certs.searchEmpty")}
            description={t("certs.searchEmptyHint")}
          />
        ) : (
          <>
            <Table>
              <THead>
                <tr>
                  <SortHeader
                    label={t("certs.colDomain")}
                    k="domain"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                  />
                  <SortHeader
                    label={t("certs.colNode")}
                    k="node"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    className="hidden md:table-cell"
                  />
                  <Th className="hidden xl:table-cell">
                    {t("certs.sources.colPath")}
                  </Th>
                  <SortHeader
                    label={t("certs.colExpiry")}
                    k="expiry"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    className="hidden lg:table-cell"
                  />
                  <SortHeader
                    label={t("certs.colDays")}
                    k="days"
                    sortKey={sortKey}
                    sortDir={sortDir}
                    onSort={onSort}
                    align="right"
                  />
                  <Th align="right">{t("containers.colActions")}</Th>
                </tr>
              </THead>
              <TBody>
                {visible.map((c) => {
                  const src = sourcePathOf(c);
                  return (
                    <Tr
                      key={`${c.nodeId}-${c.path}`}
                      className="cursor-pointer"
                      onClick={() =>
                        setDetail({
                          ...c,
                          hostname: c.hostname,
                          nodeId: c.nodeId,
                          sourcePath: sourceOf(c)?.path ?? src,
                        })
                      }
                    >
                      <Td>
                        <div className="text-sm font-medium text-ink-900 dark:text-surface-0 truncate max-w-[260px]">
                          {c.parse_error
                            ? c.path
                            : c.domains[0] || c.subject.replace(/^CN=/, "").split(",")[0]}
                        </div>
                        <div className="text-xs text-ink-400 truncate max-w-[260px]" title={c.path}>
                          {c.path}
                        </div>
                        {!c.parse_error && c.domains.length > 1 && (
                          <div className="mt-1 flex flex-wrap gap-1">
                            {c.domains.slice(1, 3).map((d) => (
                              <DotBadge key={d} tone="neutral">
                                {d}
                              </DotBadge>
                            ))}
                            {c.domains.length > 3 && (
                              <DotBadge tone="neutral">
                                +{c.domains.length - 3}
                              </DotBadge>
                            )}
                          </div>
                        )}
                      </Td>
                      <Td className="hidden md:table-cell">
                        <span className="text-xs text-ink-500">{c.label}</span>
                      </Td>
                      <Td className="hidden xl:table-cell">
                        <span className="text-xs text-ink-500">
                          {src ?? t("certs.sources.builtin")}
                        </span>
                      </Td>
                      <Td className="hidden lg:table-cell">
                        <span className="text-xs tabular-nums text-ink-500">
                          {c.parse_error
                            ? t("certs.parseError")
                            : formatTime(c.not_after_unix_nano / 1e6, timezone)}
                        </span>
                      </Td>
                      <Td align="right">
                        {c.parse_error ? (
                          <DotBadge tone="danger">{t("certs.parseError")}</DotBadge>
                        ) : (
                          <DotBadge tone={expiryTone(c.days)}>
                            {c.days < 0
                              ? t("certs.expired", { n: Math.abs(Math.floor(c.days)) })
                              : c.days < 1
                                ? t("certs.today")
                                : t("certs.daysLeft", { n: Math.floor(c.days) })}
                          </DotBadge>
                        )}
                      </Td>
                      <Td align="right">
                        <Tooltip content={t("certs.detail.title")}>
                          <Button
                            variant="ghost"
                            size="icon"
                            aria-label={t("certs.detail.title")}
                            onClick={(e) => {
                              e.stopPropagation();
                              setDetail({
                                ...c,
                                hostname: c.label,
                                nodeId: c.nodeId,
                                sourcePath: sourceOf(c)?.path ?? src,
                              });
                            }}
                          >
                            <Info className="w-4 h-4" aria-hidden="true" />
                          </Button>
                        </Tooltip>
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
                  {t("certs.total", { n: sorted.length })}
                  {counts.expired === 0 && counts.expiring === 0 && (
                    <span className="ml-1 text-emerald-600 dark:text-emerald-400">
                      {t("certs.allGood")}
                    </span>
                  )}
                </p>
              }
            />
          </>
        )}
      </TableShell>

      {formOpen && (
        <CertSourceDialog
          open
          onClose={() => setFormOpen(false)}
          nodes={nodes}
          initial={editing}
          onSaved={invalidate}
        />
      )}

      {testing && (
        <CertScanDialog
          source={testing}
          nodes={nodes}
          onClose={() => setTesting(null)}
        />
      )}

      {removing && (
        <ConfirmDialog
          open
          danger
          loading={busy}
          title={t("certs.sources.removeTitle")}
          message={t("certs.sources.removeMessage", { path: removing.path })}
          confirmLabel={t("action.delete")}
          cancelLabel={t("action.cancel")}
          onCancel={() => setRemoving(null)}
          onConfirm={() => void remove(removing)}
        />
      )}

      {detail && (
        <CertDetailDialog cert={detail} onClose={() => setDetail(null)} />
      )}
    </div>
  );
}

/**
 * 与后端 `zhiwei_common::certpath` 同一套规则的路径匹配。
 * 只用于**老版本节点**（证书条目不带 source_id）的来源反查：
 * 目录展开为证书后缀 glob，文件与 glob 原样匹配。
 */
const CERT_EXTS = ["pem", "crt", "cer", "cert"];

function expandPatterns(raw: string): string[] {
  const p = raw.trim();
  if (!p) return [];
  if (p.includes("*") || p.includes("?") || p.includes("[")) return [p];
  const ext = p.split(".").pop() ?? "";
  if (CERT_EXTS.some((e) => ext.toLowerCase() === e)) return [p];
  return CERT_EXTS.map((e) => `${p.replace(/\/+$/, "")}/*.${e}`);
}

function globMatch(raw: string, path: string): boolean {
  return expandPatterns(raw).some((pat) => globToRegExp(pat).test(path));
}

function globToRegExp(pattern: string): RegExp {
  let out = "^";
  for (const ch of pattern) {
    if (ch === "*") out += "[^/]*";
    else if (ch === "?") out += "[^/]";
    else out += ch.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(out + "$");
}
