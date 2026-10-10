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
 * Node detail page's "Certificates": TLS certificates discovered on this machine.
 * The certificates page is a forward view organized by all nodes; here is the reverse view
 * organized by node — when troubleshooting certificates on one machine, no need to jump to the
 * global page and filter.
 *
 * Data is reused from /v1/certificates (fetched once for all, filtered by node_id, snapshots change every 5 min,
 * low refetch cost), column layout / badges / detail dialog all reuse the same styles as the certificates page.
 */
export function NodeCerts({
  nodeId,
  refreshMs,
  hostLabel,
}: {
  nodeId: string;
  refreshMs: number;
  /** This node's display name (alias preferred), used by the certificate detail dialog's "Node" row */
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
    // Most urgent first (consistent with the certificates page's default sort)
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
                <tr className="border-b border-surface-3 dark:border-ink-500">
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
              <tbody className="divide-y divide-surface-2 dark:divide-ink-500">
                {certs.map((c) => {
                  const days = daysLeft(c.not_after_unix_nano);
                  // Primary display = first SAN domain; fall back to path on parse failure (consistent with certificates page)
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