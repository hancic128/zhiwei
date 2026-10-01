/**
 * 待办（默认页）。
 *
 * 定位（docs/POSITIONING.md）：系统给的不是曲线，是「今天要我处理的几件事」。
 * 四条约束见 docs/superpowers/specs/2026-09-19-product-structure-design.md §4：
 * 每条带下一步 / 按「要不要现在动手」分档 / 空得有底气 / 已恢复留痕。
 */
import * as React from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Activity,
  AlertTriangle,
  BellOff,
  CheckCircle2,
  ChevronRight,
  PlugZap,
  RotateCcw,
  Server,
  ShieldCheck,
} from "lucide-react";
import { alertsApi, todoApi, type TodoItem, type TodoSummary } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardBody } from "@/components/ui/card";
import { EmptyState, ErrorState, TableSkeleton } from "@/components/ui/feedback";
import { SilenceDialog } from "@/components/silence-dialog";
import { StatCards, type StatCard } from "@/components/stat-cards";
import { Tooltip } from "@/components/ui/tooltip";
import { useToast } from "@/components/ui/toast";
import { cn, friendlyError, relativeTime } from "@/lib/utils";

/** 每条待办的来源图标——一眼看出「这是哪一类事」 */
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
  // 告警项的 id 形如 alert-12；节点离线 / 命令通道这类由系统算出来的项
  // 没有对应的告警，不能静默（原本只放过 node_offline，其余一律 Number()，
  // 别的来源一旦进来就会静默一个 NaN）
  const alertId = item.id.startsWith("alert-") ? Number(item.id.slice(6)) : null;
  const [silenceOpen, setSilenceOpen] = React.useState(false);
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
  // 手动关闭告警
  const resolve = useMutation({
    mutationFn: () => alertsApi.resolve(alertId as number),
    onSuccess: () => {
      toast.push("success", t("alerts.resolvedToast"));
      void qc.invalidateQueries({ queryKey: ["todo"] });
      void qc.invalidateQueries({ queryKey: ["alerts"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });
  // 节点离线的文案由前端拼：保证中英双语（后端只给时间）
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
          <div className="mt-1 flex items-center justify-end gap-1">
            <Link
              to={item.link}
              className="text-xs text-brand-700 hover:underline dark:text-brand-100"
            >
              {t("todo.goLook")}
            </Link>
            {alertId !== null && (
              <>
                <Tooltip content={t("alerts.resolve")}>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={t("alerts.resolve")}
                    onClick={() => resolve.mutate()}
                    disabled={resolve.isPending}
                  >
                    <RotateCcw className="w-4 h-4" aria-hidden="true" />
                  </Button>
                </Tooltip>
                <Tooltip content={t("alerts.silence")}>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={t("alerts.silence")}
                    onClick={() => setSilenceOpen(true)}
                  >
                    <BellOff className="w-4 h-4" aria-hidden="true" />
                  </Button>
                </Tooltip>
              </>
            )}
          </div>
        )}
      </div>
      {alertId !== null && (
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
      )}
    </li>
  );
}

function Bucket({
  title,
  items,
  emptyText,
}: {
  title: string;
  items: TodoItem[];
  emptyText: string;
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
        <ul>
          {items.map((i) => (
            <TodoRow key={i.id} item={i} />
          ))}
        </ul>
      )}
    </Card>
  );
}

/**
 * 顶部四张大字卡片：整卡可点，跳对应页面。
 *
 * 空待办不是空白页——这几张卡让「没事」这件事本身是可见的，
 * 也是从仪表盘切到待办时唯一需要的过渡（设计 §4 第 3 条）。
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
  const q = useQuery({ queryKey: ["todo"], queryFn: todoApi.get });

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
        items={now}
        emptyText={t("todo.noneNow")}
      />
      <Bucket
        title={t("todo.bucketWatch")}
        items={watch}
        emptyText={t("todo.noneWatch")}
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
            <ul className="border-t border-surface-3 dark:border-ink-700">
              {recovered.map((i) => (
                <TodoRow key={i.id} item={i} muted />
              ))}
            </ul>
          )}
        </Card>
      )}
    </div>
  );
}
