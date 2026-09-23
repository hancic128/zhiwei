import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Activity } from "lucide-react";
import { servicesApi, type ProbeView } from "@/api";
import { DotBadge } from "@/components/ui/badge";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import { EmptyState, ErrorState, Skeleton } from "@/components/ui/feedback";
import { cn, friendlyError, relativeTime } from "@/lib/utils";

/**
 * 节点详情页的「服务探针」：这台机器上跑着哪些探针、现在什么状态。
 *
 * 服务页是按服务组织的（正向视图），这里是按节点组织（反向视图）——
 * 排一台机器的问题时，需要知道「它到底在替谁探什么」。
 * 只显示绑定了本节点的探针；`node_ids` 为空的探针会在任意节点上执行，
 * 放在某一台节点的详情里会误导，因此不列。
 */
export function NodeProbes({ nodeId, refreshMs }: { nodeId: string; refreshMs: number }) {
  const { t } = useTranslation();
  const q = useQuery({
    queryKey: ["services"],
    queryFn: servicesApi.list,
    refetchInterval: refreshMs,
  });

  const probes: ProbeView[] = React.useMemo(
    () =>
      (q.data ?? [])
        .flatMap((s) => s.probes)
        .filter((p) => p.node_ids.includes(nodeId))
        .sort((a, b) => a.service_name.localeCompare(b.service_name)),
    [q.data, nodeId],
  );

  const downCount = probes.filter((p) => p.state.state === "down").length;

  return (
    <Card>
      <CardHeader
        icon={<Activity className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("detail.probesTitle")}
        description={t("detail.probesSubtitle", { n: probes.length })}
        action={
          downCount > 0 ? (
            <DotBadge tone="danger">{t("detail.probesDown", { n: downCount })}</DotBadge>
          ) : probes.length > 0 ? (
            <DotBadge tone="success">{t("state.ok")}</DotBadge>
          ) : undefined
        }
      />
      <CardBody compact>
        {q.isLoading ? (
          <Skeleton className="h-20 w-full" />
        ) : q.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : probes.length === 0 ? (
          <EmptyState
            title={t("detail.probesEmpty")}
            description={t("detail.probesEmptyHint")}
          />
        ) : (
          <div className="overflow-x-auto scrollbar-thin">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-surface-3 dark:border-ink-700">
                  <th className="px-4 py-2 text-left text-xs font-semibold text-ink-500 uppercase tracking-wider">
                    {t("services.colService")}
                  </th>
                  <th className="px-4 py-2 text-left text-xs font-semibold text-ink-500 uppercase tracking-wider">
                    {t("services.colState")}
                  </th>
                  <th className="hidden md:table-cell px-4 py-2 text-right text-xs font-semibold text-ink-500 uppercase tracking-wider">
                    {t("services.colLatency")}
                  </th>
                  <th className="hidden lg:table-cell px-4 py-2 text-right text-xs font-semibold text-ink-500 uppercase tracking-wider">
                    {t("services.colLastCheck")}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-2 dark:divide-ink-700">
                {probes.map((p) => (
                  <tr key={p.id}>
                    <td className="px-4 py-3">
                      <div className="text-sm text-ink-900 dark:text-surface-0 truncate max-w-[260px]">
                        {p.name}
                      </div>
                      <div className="text-xs text-ink-400 truncate max-w-[260px]">
                        {p.service_name} · {p.kind}
                      </div>
                    </td>
                    <td className="px-4 py-3">
                      <DotBadge tone={probeTone(p.state.state)}>
                        {t(`state.${p.state.state}`)}
                      </DotBadge>
                      {p.state.last_error && (
                        <div className="mt-1 text-xs text-ink-400 truncate max-w-[220px]" title={p.state.last_error}>
                          {p.state.last_error}
                        </div>
                      )}
                    </td>
                    <td className="hidden md:table-cell px-4 py-3 text-right">
                      <span
                        className={cn(
                          "text-sm tabular-nums",
                          p.state.last_latency_ms === null
                            ? "text-ink-400"
                            : p.state.last_latency_ms > 1000
                              ? "text-amber-600 dark:text-amber-400"
                              : "text-ink-900 dark:text-surface-0",
                        )}
                      >
                        {p.state.last_latency_ms === null
                          ? "—"
                          : `${p.state.last_latency_ms < 10 ? p.state.last_latency_ms.toFixed(1) : Math.round(p.state.last_latency_ms)} ms`}
                      </span>
                    </td>
                    <td className="hidden lg:table-cell px-4 py-3 text-right">
                      <span className="text-xs tabular-nums text-ink-500">
                        {p.state.last_check_at_unix_nano
                          ? relativeTime(
                              Math.floor(p.state.last_check_at_unix_nano / 1e6),
                              t,
                            )
                          : "—"}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </CardBody>
    </Card>
  );
}

/** 探针状态 → Badge 语义色（与状态常量 ok / degraded / down / unknown 对齐） */
function probeTone(state: string): "success" | "warn" | "danger" | "neutral" {
  if (state === "ok") return "success";
  if (state === "degraded") return "warn";
  if (state === "down") return "danger";
  return "neutral";
}
