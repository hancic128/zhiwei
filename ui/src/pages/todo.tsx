/**
 * Todo (default page).
 *
 * Positioning (docs/POSITIONING.md): the system delivers not curves but
 * "a few things for me to handle today". The four constraints are in
 * docs/superpowers/specs/2026-09-19-product-structure-design.md §4: each item
 * has a next step, items are tiered by "do I need to act now", empty must
 * feel confident, and recovered items leave a trail.
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
} from "lucide-react";
import { alertsApi, todoApi, type TodoItem, type TodoSummary } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Card, CardBody } from "@/components/ui/card";
import { EmptyState, ErrorState, TableSkeleton } from "@/components/ui/feedback";
import { SilenceDialog } from "@/components/silence-dialog";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { StatCards, type StatCard } from "@/components/stat-cards";
import { useToast } from "@/components/ui/toast";
import { cn, friendlyError, relativeTime } from "@/lib/utils";

/** Source icon for each todo item — see at a glance "which kind of thing is this" */
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
  // Alert item ids look like alert-12; items computed by the system such as
  // node offline / command channel have no corresponding alert and must not
  // be silenced (the previous code only let node_offline through and ran
  // Number() on the rest, which would silence NaN for any other source)
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
  // Manually resolve an alert
  const resolve = useMutation({
    mutationFn: () => alertsApi.resolve(alertId as number),
    onSuccess: () => {
      toast.push("success", t("alerts.resolvedToast"));
      void qc.invalidateQueries({ queryKey: ["todo"] });
      void qc.invalidateQueries({ queryKey: ["alerts"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });
  // The copy for node offline is assembled in the UI to keep both Chinese and
  // English in sync (the backend only provides the timestamp)
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

/**
 * Top four large cards: each card is clickable and navigates to its page.
 *
 * An empty todo is not a blank page — these cards make "nothing to do" itself
 * visible, and they are the only transition needed when switching from the
 * dashboard to todo (design §4 item 3).
 */
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

export function Todo() {
  const { t } = useTranslation();
  const [showRecovered, setShowRecovered] = React.useState(false);
  const [nowCursor, setNowCursor] = React.useState<number | undefined>(undefined);
  const [watchCursor, setWatchCursor] = React.useState<number | undefined>(undefined);
  const [recoveredCursor, setRecoveredCursor] = React.useState<number | undefined>(undefined);
  const [loadedItems, setLoadedItems] = React.useState<{ now: TodoItem[]; watch: TodoItem[]; recovered: TodoItem[] }>({
    now: [],
    watch: [],
    recovered: [],
  });
  const [hasMore, setHasMore] = React.useState({ now: false, watch: false, recovered: false });
  const [loadingMore, setLoadingMore] = React.useState({ now: false, watch: false, recovered: false });

  const q = useQuery({
    queryKey: ["todo", nowCursor],
    queryFn: () => todoApi.get({ cursor: nowCursor, limit: 5 }),
    initialPageData: false,
  });

  // Initialize or append items
  React.useEffect(() => {
    if (q.data) {
      if (nowCursor === undefined) {
        // First load
        setLoadedItems({
          now: q.data.now,
          watch: q.data.watch,
          recovered: q.data.recovered,
        });
      } else {
        // Load more
        setLoadedItems(prev => ({
          now: [...prev.now, ...q.data.now],
          watch: [...prev.watch, ...q.data.watch],
          recovered: [...prev.recovered, ...q.data.recovered],
        }));
      }
      if (q.data.pagination) {
        setHasMore({
          now: q.data.pagination.has_more.now,
          watch: q.data.pagination.has_more.watch,
          recovered: q.data.pagination.has_more.recovered,
        });
      }
    }
  }, [q.data, nowCursor]);

  const handleLoadMore = (bucket: "now" | "watch" | "recovered") => {
    if (!q.data?.pagination) return;
    setLoadingMore(prev => ({ ...prev, [bucket]: true }));

    let nextCursor: number | undefined;
    if (bucket === "now") {
      nextCursor = q.data.pagination.next_cursor.now ?? undefined;
      setNowCursor(nextCursor);
    } else if (bucket === "watch") {
      nextCursor = q.data.pagination.next_cursor.watch ?? undefined;
      setWatchCursor(nextCursor);
    } else {
      nextCursor = q.data.pagination.next_cursor.recovered ?? undefined;
      setRecoveredCursor(nextCursor);
    }

    // Refetch with new cursor
    void q.refetch().finally(() => {
      setLoadingMore(prev => ({ ...prev, [bucket]: false }));
    });
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

  const { summary, counts, now, watch, recovered } = q.data;
  const displayNow = nowCursor === undefined ? now : loadedItems.now;
  const displayWatch = nowCursor === undefined ? watch : loadedItems.watch;
  const displayRecovered = nowCursor === undefined ? recovered : loadedItems.recovered;
  const allClear = counts.now === 0 && counts.watch === 0;

  return (
    <div className="space-y-4">
      <StatusCards summary={summary} />

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
        items={displayNow}
        emptyText={t("todo.noneNow")}
        hasMore={hasMore.now}
        onLoadMore={() => handleLoadMore("now")}
        loadingMore={loadingMore.now}
      />
      <Bucket
        title={t("todo.bucketWatch")}
        items={displayWatch}
        emptyText={t("todo.noneWatch")}
        hasMore={hasMore.watch}
        onLoadMore={() => handleLoadMore("watch")}
        loadingMore={loadingMore.watch}
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
                {displayRecovered.map((i) => (
                  <TodoRow key={i.id} item={i} muted />
                ))}
              </ul>
              {hasMore.recovered && (
                <LoadMoreButton
                  onClick={() => handleLoadMore("recovered")}
                  loading={loadingMore.recovered}
                />
              )}
            </>
          )}
        </Card>
      )}
    </div>
  );
}
