import * as React from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { RefreshCw, ShieldCheck } from "lucide-react";
import { certsApi, daysLeft, expiryTone, type CertInfo } from "@/api";
import {
  CertDetailDialog,
  type CertDetailTarget,
} from "@/components/cert-detail-dialog";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import { EmptyState, ErrorState, Skeleton } from "@/components/ui/feedback";
import { Tooltip } from "@/components/ui/tooltip";
import { usePrefs } from "@/components/prefs-provider";
import { formatTime, friendlyError } from "@/lib/utils";

/**
 * 节点详情页的「证书」：这台机器上发现的 TLS 证书。
 * 证书页是按全部节点组织的正向视图，这里是按节点组织的反向视图——
 * 排一台机器的证书问题时，不用跳到全局页面再过滤。
 *
 * 数据复用 /v1/certificates（一次拉全量、按 node_id 过滤，快照 5 分钟才变，
 * 重取成本低），列布局 / 徽章 / 详情对话框全部沿用证书页同一套样式。
 */
export function NodeCerts({
  nodeId,
  refreshMs,
  hostLabel,
}: {
  nodeId: string;
  refreshMs: number;
  /** 本节点展示名（别名优先），证书详情对话框的「节点」行用它 */
  hostLabel: string;
}) {
  const { t } = useTranslation();
  const { timezone } = usePrefs();
  const [detail, setDetail] = React.useState<CertDetailTarget | null>(null);

  const q = useQuery({
    queryKey: ["certs"],
    queryFn: certsApi.all,
    refetchInterval: refreshMs,
  });

  const certs: CertInfo[] = React.useMemo(() => {
    const group = (q.data ?? []).find((g) => g.node_id === nodeId);
    // 最紧急的在前（与证书页默认排序一致）
    return [...(group?.certificates ?? [])].sort(
      (a, b) => daysLeft(a.not_after_unix_nano) - daysLeft(b.not_after_unix_nano),
    );
  }, [q.data, nodeId]);

  return (
    <Card>
      <CardHeader
        icon={<ShieldCheck className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("certs.title")}
        description={t("certs.nodeSubtitle", { n: certs.length })}
        action={
          <Tooltip content={t("action.refresh")}>
            <Button
              variant="ghost"
              size="icon"
              aria-label={t("action.refresh")}
              disabled={q.isFetching}
              onClick={() => void q.refetch()}
            >
              <RefreshCw className="w-4 h-4" aria-hidden="true" />
            </Button>
          </Tooltip>
        }
      />
      <CardBody compact>
        {q.isLoading ? (
          <Skeleton className="h-32 w-full" />
        ) : q.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : certs.length === 0 ? (
          <EmptyState
            title={t("certs.nodeEmpty")}
            description={t("certs.emptyHint")}
          />
        ) : (
          <div className="overflow-x-auto scrollbar-thin">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-surface-3 dark:border-ink-700">
                  <th className="px-4 py-2 text-left text-xs font-semibold text-ink-500 uppercase tracking-wider">
                    {t("certs.colDomain")}
                  </th>
                  <th className="hidden xl:table-cell px-4 py-2 text-left text-xs font-semibold text-ink-500 uppercase tracking-wider">
                    {t("certs.sources.colPath")}
                  </th>
                  <th className="hidden lg:table-cell px-4 py-2 text-left text-xs font-semibold text-ink-500 uppercase tracking-wider">
                    {t("certs.colExpiry")}
                  </th>
                  <th className="px-4 py-2 text-right text-xs font-semibold text-ink-500 uppercase tracking-wider">
                    {t("certs.colDays")}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-surface-2 dark:divide-ink-700">
                {certs.map((c) => {
                  const days = daysLeft(c.not_after_unix_nano);
                  // 主显示 = 第一个 SAN 域名，解析失败时退回路径（与证书页一致）
                  const primary = c.parse_error
                    ? c.path
                    : c.domains[0] || c.subject.replace(/^CN=/, "").split(",")[0];
                  return (
                    <tr
                      key={`${c.path}-${c.serial}`}
                      className="cursor-pointer hover:bg-surface-1 dark:hover:bg-ink-700/40 transition-colors"
                      onClick={() =>
                        setDetail({
                          ...c,
                          hostname: hostLabel,
                          nodeId,
                          sourcePath: null,
                        })
                      }
                    >
                      <td className="px-4 py-2">
                        <div className="text-sm font-medium text-ink-900 dark:text-surface-0 truncate max-w-[260px]">
                          {primary}
                        </div>
                        <div
                          className="text-xs text-ink-400 truncate max-w-[260px]"
                          title={c.path}
                        >
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
                      </td>
                      <td className="hidden xl:table-cell px-4 py-2">
                        <span className="text-xs text-ink-500">{c.path}</span>
                      </td>
                      <td className="hidden lg:table-cell px-4 py-2">
                        <span className="text-xs tabular-nums text-ink-500">
                          {c.parse_error
                            ? t("certs.parseError")
                            : formatTime(c.not_after_unix_nano / 1e6, timezone)}
                        </span>
                      </td>
                      <td className="px-4 py-2 text-right">
                        {c.parse_error ? (
                          <DotBadge tone="danger">{t("certs.parseError")}</DotBadge>
                        ) : (
                          <DotBadge tone={expiryTone(days)}>
                            {days < 0
                              ? t("certs.expired", { n: Math.abs(Math.floor(days)) })
                              : days < 1
                                ? t("certs.today")
                                : t("certs.daysLeft", { n: Math.floor(days) })}
                          </DotBadge>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </CardBody>

      {detail && <CertDetailDialog cert={detail} onClose={() => setDetail(null)} />}
    </Card>
  );
}