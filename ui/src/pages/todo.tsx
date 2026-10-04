/**
 * Todo (default page).
 *
 * Design: docs/superpowers/specs/2026-09-19-product-structure-design.md §4.
 */
import * as React from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  ChevronRight,
  PlugZap,
  Server,
  ShieldCheck,
  X,
} from "lucide-react";
import { alertsApi, todoApi, type TodoItem, type TodoSummary } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Card, CardBody } from "@/components/ui/card";
import { EmptyState, ErrorState, TableSkeleton } from "@/components/ui/feedback";
import { SilenceDialog } from "@/components/silence-dialog";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Select } from "@/components/ui/select";
import { StatCards, type StatCard } from "@/components/stat-cards";
import { useToast } from "@/components/ui/toast";
import { cn, friendlyError, relativeTime } from "@/lib/utils";

/** Source icon for each todo item */
function SourceIcon({ source, className }: { source: string; className?: string }) {
  switch (source) {
    case "probe":
      return <Activity className={className} aria-hidden="true" />;
    case "cert":
      return <ShieldCheck className={className} aria-hidden="true" />;
    case "node_offline":
      return <Server className={className} aria-hidden="true" />;
    case "command_channel":
      return <PlugZap className={className} aria-hidden="true" />;
    default:
      return <AlertTriangle className={className} aria-hidden="true" />;
  }
}

function TodoRow({ item, muted = false }: { item: TodoItem; muted?: boolean }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const alertId = item.id.startsWith("alert-") ? Number(item.id.slice(6)) : null;
  const [silenceOpen, setSilenceOpen] = React.useState(false);
  const [resolveOpen, setResolveOpen] = React.useState(false);
  const silence = useMutation({
    mutationFn: (minutes: number) =>
      alertsApi.silence(alertId as number, minutes),
    onSuccess: () => {
      toast.push("success", t("alerts.silencedToast"));
      void qc.invalidateQueries({ queryKey: ["todo"] });
      void qc.invalidateQueries({ queryKey: ["alerts"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });
  const resolve = useMutation({
    mutationFn: () => alertsApi.resolve(alertId as number),
    onSuccess: () => {
      toast.push("success", t("alerts.resolvedToast"));
      void qc.invalidateQueries({ queryKey: ["todo"] });
      void qc.invalidateQueries({ queryKey: ["alerts"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });
  const detail = item.detail
    ? item.detail
    : item.source === "node_offline"
      ? t("todo.nodeOfflineDetail", {
          ago: relativeTime(item.since_unix_nano / 1_000_000, t),
        })
      : item.source === "command_channel"
        ? t("todo.commandChannelDetail", {
            ago: relativeTime(item.since_unix_nano / 1_000_000, t),
          })
        : "";
  const title = item.title || t(`todo.source.${item.source}`);
  const at = item.resolved_at_unix_nano ?? item.since_unix_nano;

  return (
    <li className="flex items-start gap-3 px-6 py-4 border-b border-surface-3 last:border-b-0 dark:border-ink-700">
      <SourceIcon
        source={item.source}
        className={cn(
          "w-4 h-4 mt-0.5 shrink-0",
          muted
            ? "text-ink-400"
            : item.severity === "critical"
              ? "text-rose-600 dark:text-rose-400"
              : "text-amber-600 dark:text-amber-400",
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          <span
            className={cn(
              "text-sm font-medium",
              muted
                ? "text-ink-500"
                : "text-ink-900 dark:text-surface-0",
            )}
          >
            {title}
          </span>
          <Badge tone="neutral">{item.hostname}</Badge>
        </div>
        {detail && (
          <p className="mt-1 text-sm text-ink-500 break-words">{detail}</p>
        )}
        {!muted && (
          <p className="mt-1 text-xs text-ink-400">
            {t(`todo.hint.${item.hint_key}`)}
          </p>
        )}
      </div>
      <div className="shrink-0 text-right">
        <div className="text-xs text-ink-400 tabular-nums">
          {relativeTime(at / 1_000_000, t)}
        </div>
        {!muted && (
          <div className="mt-1 flex items-center justify-end gap-3">
            <Link
              to={item.link}
              className="text-xs text-brand-700 hover:underline dark:text-brand-100"
            >
              {t("todo.goLook")}
            </Link>
            {alertId !== null && (
              <>
                <button
                  type="button"
                  onClick={() => setResolveOpen(true)}
                  disabled={resolve.isPending}
                  className="text-xs text-ink-500 hover:text-ink-700 dark:text-ink-400 dark:hover:text-ink-200 disabled:opacity-50"
                >
                  {t("alerts.resolve")}
                </button>
                <button
                  type="button"
                  onClick={() => setSilenceOpen(true)}
                  className="text-xs text-ink-500 hover:text-ink-700 dark:text-ink-400 dark:hover:text-ink-200"
                >
                  {t("alerts.silence")}
                </button>
              </>
            )}
          </div>
        )}
      </div>
      {alertId !== null && (
        <>
          <SilenceDialog
            open={silenceOpen}
            loading={silence.isPending}
            onCancel={() => setSilenceOpen(false)}
            onConfirm={(minutes) => {
              silence.mutate(minutes, {
                onSettled: () => setSilenceOpen(false),
              });
            }}
          />
          <ConfirmDialog
            open={resolveOpen}
            title={t("alerts.resolveConfirmTitle")}
            message={t("alerts.resolveConfirmMessage")}
            confirmLabel={t("alerts.resolve")}
            cancelLabel={t("action.cancel")}
            danger
            loading={resolve.isPending}
            onCancel={() => setResolveOpen(false)}
            onConfirm={() => {
              resolve.mutate();
              setResolveOpen(false);
            }}
          />
        </>
      )}
    </li>
  );
}

function LoadMoreButton({
  onClick,
  loading,
}: {
  onClick: () => void;
  loading: boolean;
}) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={loading}
      className="w-full py-2 text-sm text-ink-500 hover:text-ink-700 dark:text-ink-400 dark:hover:text-ink-200 border-t border-surface-3 dark:border-ink-700 disabled:opacity-50"
    >
      {loading ? t("action.refreshing") : t("todo.loadMore")}
    </button>
  );
}

function Bucket({
  title,
  items,
  emptyText,
  hasMore,
  onLoadMore,
  loadingMore,
}: {
  title: string;
  items: TodoItem[];
  emptyText: string;
  hasMore?: boolean;
  onLoadMore?: () => void;
  loadingMore?: boolean;
}) {
  return (
    <Card>
      <div className="px-6 py-3 border-b border-surface-3 dark:border-ink-700 flex items-center gap-2">
        <h2 className="text-sm font-semibold text-ink-900 dark:text-surface-0">
          {title}
        </h2>
        {items.length > 0 && <Badge tone="neutral">{items.length}</Badge>}
      </div>
      {items.length === 0 ? (
        <CardBody compact>
          <p className="text-sm text-ink-400">{emptyText}</p>
        </CardBody>
      ) : (
        <>
          <ul>
            {items.map((i) => (
              <TodoRow key={i.id} item={i} />
            ))}
          </ul>
          {hasMore && onLoadMore && (
            <LoadMoreButton onClick={onLoadMore} loading={loadingMore ?? false} />
          )}
        </>
      )}
    </Card>
  );
}

/** Top four large cards */
function StatusCards({ summary }: { summary: TodoSummary }) {
  const { t } = useTranslation();
  const cards: StatCard[] = [
    {
      key: "nodes",
      to: "/nodes",
      label: t("todo.summaryNodes"),
      value: `${summary.nodes_online}/${summary.nodes_total}`,
      hint: t("todo.summaryNodesHint"),
      tone: summary.nodes_online < summary.nodes_total ? "danger" : "neutral",
      icon: Server,
    },
    {
      key: "probes",
      to: "/services",
      label: t("todo.summaryProbes"),
      value: `${summary.probes_healthy}/${summary.probes_total}`,
      hint: t("todo.summaryProbesHint"),
      tone:
        summary.probes_healthy < summary.probes_total ? "danger" : "success",
      icon: Activity,
    },
    {
      key: "certs",
      to: "/certificates",
      label: t("todo.summaryCerts"),
      value: `${summary.certs_total}`,
      hint: t("todo.summaryCertsHint"),
      tone: "neutral",
      icon: ShieldCheck,
    },
    {
      key: "containers",
      to: "/containers",
      label: t("todo.summaryContainers"),
      value: `${summary.containers_total}`,
      hint:
        summary.containers_failed > 0
          ? t("todo.summaryContainersFailed", { n: summary.containers_failed })
          : t("todo.summaryContainersHint"),
      tone: summary.containers_failed > 0 ? "danger" : "neutral",
      icon: AlertTriangle,
    },
  ];

  return <StatCards cards={cards} />;
}

/** Time range presets in milliseconds */
const TIME_PRESETS = [
  { label: "24h", ms: 24 * 60 * 60 * 1000 },
  { label: "7d", ms: 7 * 24 * 60 * 60 * 1000 },
  { label: "30d", ms: 30 * 24 * 60 * 60 * 1000 },
  { label: "1y", ms: 365 * 24 * 60 * 60 * 1000 },
];

interface FilterState {
  preset: string | null;
  since: number | null;
  until: number | null;
  status: "all" | "open" | "resolved";
  source: string;
}

function TodoFilters({
  filters,
  onChange,
}: {
  filters: FilterState;
  onChange: (f: FilterState) => void;
}) {
  const { t } = useTranslation();

  const applyPreset = (preset: (typeof TIME_PRESETS)[number] | null) => {
    if (preset) {
      const now = Date.now();
      onChange({
        ...filters,
        preset: preset.label,
        since: now - preset.ms,
        until: now,
      });
    } else {
      onChange({
        ...filters,
        preset: null,
        since: null,
        until: null,
      });
    }
  };

  const clearFilters = () => {
    onChange({
      preset: null,
      since: null,
      until: null,
      status: "all",
      source: "",
    });
  };

  const hasFilters =
    filters.preset !== null ||
    filters.status !== "all" ||
    filters.source !== "";

  return (
    <div className="flex flex-wrap items-center gap-2 px-6 py-3 bg-surface-2 dark:bg-ink-700/30 rounded-lg border border-surface-3 dark:border-ink-600">
      {/* Time presets */}
      <div className="flex items-center gap-1">
        {TIME_PRESETS.map((p) => (
          <button
            key={p.label}
            type="button"
            onClick={() => applyPreset(p)}
            className={cn(
              "px-2 py-1 text-xs rounded border transition-colors",
              filters.preset === p.label
                ? "bg-brand-100 text-brand-700 border-brand-300 dark:bg-brand-900 dark:text-brand-200 dark:border-brand-700"
                : "bg-surface-1 text-ink-600 border-surface-3 hover:border-ink-400 dark:bg-ink-700 dark:text-surface-4 dark:border-ink-600"
            )}
          >
            {p.label}
          </button>
        ))}
      </div>

      {/* Status filter */}
      <Select
        value={filters.status}
        onChange={(e) =>
          onChange({ ...filters, status: e.target.value as FilterState["status"] })
        }
        className="text-xs h-8"
      >
        <option value="all">{t("filter.all")}</option>
        <option value="open">{t("filter.open")}</option>
        <option value="resolved">{t("filter.resolved")}</option>
      </Select>

      {/* Source filter */}
      <Select
        value={filters.source}
        onChange={(e) => onChange({ ...filters, source: e.target.value })}
        className="text-xs h-8"
      >
        <option value="">{t("filter.allSources")}</option>
        <option value="rule">{t("filter.sourceRule")}</option>
        <option value="probe">{t("filter.sourceProbe")}</option>
        <option value="cert">{t("filter.sourceCert")}</option>
        <option value="node_offline">{t("filter.sourceNodeOffline")}</option>
        <option value="container">{t("filter.sourceContainer")}</option>
      </Select>

      {/* Clear filters */}
      {hasFilters && (
        <button
          type="button"
          onClick={clearFilters}
          className="ml-auto flex items-center gap-1 px-2 py-1 text-xs text-ink-500 hover:text-ink-700 dark:text-ink-400 dark:hover:text-ink-200"
        >
          <X className="w-3 h-3" />
          {t("filter.clear")}
        </button>
      )}
    </div>
  );
}

export function Todo() {
  const { t } = useTranslation();
  const [showRecovered, setShowRecovered] = React.useState(false);
  const [page, setPage] = React.useState(0);
  const [accumulated, setAccumulated] = React.useState<{
    now: TodoItem[];
    watch: TodoItem[];
    recovered: TodoItem[];
  }>({ now: [], watch: [], recovered: [] });
  const [filters, setFilters] = React.useState<FilterState>({
    preset: null,
    since: null,
    until: null,
    status: "all",
    source: "",
  });

  const queryParams = {
    page,
    page_size: 5,
    ...(filters.since ? { since: filters.since } : {}),
    ...(filters.until ? { until: filters.until } : {}),
    ...(filters.status !== "all" ? { status: filters.status } : {}),
    ...(filters.source ? { sources: filters.source } : {}),
  };

  const q = useQuery({
    queryKey: ["todo", queryParams],
    queryFn: () => todoApi.get(queryParams),
  });

  // Reset page when filters change
  const handleFilterChange = (newFilters: FilterState) => {
    setFilters(newFilters);
    setPage(0);
  };

  React.useEffect(() => {
    if (q.data) {
      if (page === 0) {
        setAccumulated({
          now: q.data.now,
          watch: q.data.watch,
          recovered: q.data.recovered,
        });
      } else {
        setAccumulated(prev => ({
          now: [...prev.now, ...q.data.now],
          watch: [...prev.watch, ...q.data.watch],
          recovered: [...prev.recovered, ...q.data.recovered],
        }));
      }
    }
  }, [q.data, page]);

  const handleLoadMore = () => {
    setPage(p => p + 1);
  };

  if (q.isPending) return <TableSkeleton rows={4} />;
  if (q.isError) {
    return (
      <ErrorState
        message={friendlyError(q.error)}
        onRetry={() => void q.refetch()}
      />
    );
  }

  const data = q.data;
  const { summary } = data;
  const counts = data.counts;
  const pagination = data.pagination;
  const allClear = counts.now === 0 && counts.watch === 0;

  // Check if there are more items
  const hasMoreNow = data.now.length === (pagination?.page_size ?? 5);
  const hasMoreWatch = data.watch.length === (pagination?.page_size ?? 5);
  const hasMoreRecovered = data.recovered.length === (pagination?.page_size ?? 5);

  return (
    <div className="space-y-4">
      <StatusCards summary={summary} />
      <TodoFilters filters={filters} onChange={handleFilterChange} />

      {allClear && (
        <Card>
          <EmptyState
            icon={<CheckCircle2 className="w-12 h-12" aria-hidden="true" />}
            title={t("todo.allClear")}
            description={t("todo.allClearHint")}
          />
        </Card>
      )}

      <Bucket
        title={t("todo.bucketNow")}
        items={accumulated.now}
        emptyText={t("todo.noneNow")}
        hasMore={hasMoreNow}
        onLoadMore={handleLoadMore}
        loadingMore={q.isFetching}
      />
      <Bucket
        title={t("todo.bucketWatch")}
        items={accumulated.watch}
        emptyText={t("todo.noneWatch")}
        hasMore={hasMoreWatch}
        onLoadMore={handleLoadMore}
        loadingMore={q.isFetching}
      />

      {counts.silenced > 0 && (
        <p className="text-xs text-ink-400">
          {t("todo.silencedNote", { n: counts.silenced })}
        </p>
      )}

      {counts.recovered > 0 && (
        <Card>
          <button
            type="button"
            onClick={() => setShowRecovered((v) => !v)}
            aria-expanded={showRecovered}
            className="w-full px-6 py-3 flex items-center gap-2 text-left"
          >
            <ChevronRight
              className={cn(
                "w-4 h-4 text-ink-400 transition-transform",
                showRecovered && "rotate-90",
              )}
              aria-hidden="true"
            />
            <h2 className="text-sm font-semibold text-ink-900 dark:text-surface-0">
              {t("todo.bucketRecovered")}
            </h2>
            <Badge tone="neutral">{counts.recovered}</Badge>
            <span className="ml-auto text-xs text-ink-400">
              {showRecovered ? t("todo.collapse") : t("todo.expand")}
            </span>
          </button>
          {showRecovered && (
            <>
              <ul className="border-t border-surface-3 dark:border-ink-700">
                {accumulated.recovered.map((i) => (
                  <TodoRow key={i.id} item={i} muted />
                ))}
              </ul>
              {hasMoreRecovered && (
                <LoadMoreButton
                  onClick={handleLoadMore}
                  loading={q.isFetching}
                />
              )}
            </>
          )}
        </Card>
      )}
    </div>
  );
}
