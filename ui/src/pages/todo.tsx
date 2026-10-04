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
import { alertsApi, todoApi, type TodoItem, type TodoQuery, type TodoSummary } from "@/api";
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

function Bucket({
  title,
  page,
  emptyText,
  onLoadMore,
  hasMore,
  loadingMore,
}: {
  title: string;
  page: { items: TodoItem[]; next_cursor: number | null; total: number };
  emptyText: string;
  onLoadMore: () => void;
  hasMore: boolean;
  loadingMore: boolean;
}) {
  const { t } = useTranslation();
  return (
    <Card>
      <div className="px-6 py-3 border-b border-surface-3 dark:border-ink-700 flex items-center gap-2">
        <h2 className="text-sm font-semibold text-ink-900 dark:text-surface-0">
          {title}
        </h2>
        {page.total > 0 && <Badge tone="neutral">{page.total}</Badge>}
      </div>
      {page.items.length === 0 ? (
        <CardBody compact>
          <p className="text-sm text-ink-400">{emptyText}</p>
        </CardBody>
      ) : (
        <>
          <ul>
            {page.items.map((i) => (
              <TodoRow key={i.id} item={i} />
            ))}
          </ul>
          {hasMore && (
            <div className="px-6 py-3 border-t border-surface-3 dark:border-ink-700">
              <button
                type="button"
                onClick={onLoadMore}
                disabled={loadingMore}
                className="text-sm text-brand-700 hover:underline dark:text-brand-100 disabled:opacity-50"
              >
                {loadingMore ? t("todo.loadingMore") : t("todo.loadMore")}
              </button>
            </div>
          )}
        </>
      )}
    </Card>
  );
}

function RecoveredBucket({
  page,
  onLoadMore,
  hasMore,
  loadingMore,
  expanded,
  onToggle,
}: {
  page: { items: TodoItem[]; next_cursor: number | null; total: number };
  onLoadMore: () => void;
  hasMore: boolean;
  loadingMore: boolean;
  expanded: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Card>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        className="w-full px-6 py-3 flex items-center gap-2 text-left"
      >
        <ChevronRight
          className={cn(
            "w-4 h-4 text-ink-400 transition-transform",
            expanded && "rotate-90",
          )}
          aria-hidden="true"
        />
        <h2 className="text-sm font-semibold text-ink-900 dark:text-surface-0">
          {t("todo.bucketRecovered")}
        </h2>
        <Badge tone="neutral">{page.total}</Badge>
        <span className="ml-auto text-xs text-ink-400">
          {expanded ? t("todo.collapse") : t("todo.expand")}
        </span>
      </button>
      {expanded && (
        <>
          <ul className="border-t border-surface-3 dark:border-ink-700">
            {page.items.map((i) => (
              <TodoRow key={i.id} item={i} muted />
            ))}
          </ul>
          {hasMore && (
            <div className="px-6 py-3 border-t border-surface-3 dark:border-ink-700">
              <button
                type="button"
                onClick={onLoadMore}
                disabled={loadingMore}
                className="text-sm text-ink-500 hover:text-ink-700 dark:text-ink-400 dark:hover:text-ink-200 disabled:opacity-50"
              >
                {loadingMore ? t("todo.loadingMore") : t("todo.loadMore")}
              </button>
            </div>
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

  // Query for initial data
  const q = useQuery({
    queryKey: ["todo"],
    queryFn: () => todoApi.get({ page_size: 20 }),
  });

  // Mutation for loading more items
  const loadMore = useMutation({
    mutationFn: async ({
      type,
      cursor,
    }: {
      type: "now" | "watch" | "recovered";
      cursor: number;
    }) => {
      const params: TodoQuery = { page_size: 20 };
      if (type === "now") params.now_cursor = cursor;
      if (type === "watch") params.watch_cursor = cursor;
      if (type === "recovered") params.recovered_cursor = cursor;
      return todoApi.get(params);
    },
  });

  // Accumulated data state
  const [accumulated, setAccumulated] = React.useState<{
    now: TodoItem[];
    watch: TodoItem[];
    recovered: TodoItem[];
  }>({ now: [], watch: [], recovered: [] });

  // Sync initial data into accumulated state
  React.useEffect(() => {
    if (q.data && accumulated.now.length === 0 && accumulated.watch.length === 0) {
      setAccumulated({
        now: q.data.now.items,
        watch: q.data.watch.items,
        recovered: q.data.recovered.items,
      });
    }
  }, [q.data]);

  const handleLoadMore = (type: "now" | "watch" | "recovered") => {
    const page = q.data?.[type];
    if (!page?.next_cursor) return;

    loadMore.mutate(
      { type, cursor: page.next_cursor },
      {
        onSuccess: (data) => {
          setAccumulated((prev) => ({
            ...prev,
            [type]: [...prev[type], ...data[type].items],
          }));
        },
      }
    );
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
  const allClear = counts.now === 0 && counts.watch === 0;

  // Use accumulated data if available, otherwise use initial data
  const nowPage = {
    items: accumulated.now.length > 0 ? accumulated.now : now.items,
    next_cursor: accumulated.now.length > 0 ? now.next_cursor : now.next_cursor,
    total: counts.now,
  };

  const watchPage = {
    items: accumulated.watch.length > 0 ? accumulated.watch : watch.items,
    next_cursor: accumulated.watch.length > 0 ? watch.next_cursor : watch.next_cursor,
    total: counts.watch,
  };

  const recoveredPage = {
    items: accumulated.recovered.length > 0 ? accumulated.recovered : recovered.items,
    next_cursor:
      accumulated.recovered.length > 0 ? recovered.next_cursor : recovered.next_cursor,
    total: counts.recovered,
  };

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
        page={nowPage}
        emptyText={t("todo.noneNow")}
        onLoadMore={() => handleLoadMore("now")}
        hasMore={!!nowPage.next_cursor}
        loadingMore={loadMore.isPending && loadMore.variables?.type === "now"}
      />
      <Bucket
        title={t("todo.bucketWatch")}
        page={watchPage}
        emptyText={t("todo.noneWatch")}
        onLoadMore={() => handleLoadMore("watch")}
        hasMore={!!watchPage.next_cursor}
        loadingMore={loadMore.isPending && loadMore.variables?.type === "watch"}
      />

      {counts.silenced > 0 && (
        <p className="text-xs text-ink-400">
          {t("todo.silencedNote", { n: counts.silenced })}
        </p>
      )}

      {counts.recovered > 0 && (
        <RecoveredBucket
          page={recoveredPage}
          onLoadMore={() => handleLoadMore("recovered")}
          hasMore={!!recoveredPage.next_cursor}
          loadingMore={loadMore.isPending && loadMore.variables?.type === "recovered"}
          expanded={showRecovered}
          onToggle={() => setShowRecovered((v) => !v)}
        />
      )}
    </div>
  );
}
