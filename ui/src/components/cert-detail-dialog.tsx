import * as React from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, ShieldAlert, ShieldCheck } from "lucide-react";
import { daysLeft, expiryTone, type CertInfo } from "@/api";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { usePrefs } from "@/components/prefs-provider";
import { copyText, formatTime } from "@/lib/utils";

export interface CertDetailTarget extends CertInfo {
  hostname: string;
  nodeId: string;
  /** Matched certificate path source (empty = built-in scan) */
  sourcePath: string | null;
}

/**
 * Certificate detail: all readable info for a certificate (the list column has limited width,
 * so SAN / serial / full subject don't fit in the table, but are essential when troubleshooting "chain mismatch / domain not covered").
 */
export function CertDetailDialog({
  cert,
  onClose,
}: {
  cert: CertDetailTarget;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const { timezone } = usePrefs();
  const [copied, setCopied] = React.useState<string | null>(null);
  const days = daysLeft(cert.not_after_unix_nano);

  // Detect wildcard certificate
  const isWildcard = cert.domains.some((d) => d.startsWith("*."));

  const copy = async (label: string, value: string) => {
    if (!(await copyText(value))) return;
    setCopied(label);
    window.setTimeout(() => setCopied(null), 1500);
  };

  const rows: Array<{ label: string; value: React.ReactNode; copyValue?: string } | null> = [
    { label: t("certs.detail.primary"), value: cert.domains[0] || cert.subject || "—" },
    { label: t("certs.detail.subject"), value: cert.subject || "—" },
    { label: t("certs.detail.issuer"), value: cert.issuer || "—" },
    {
      label: t("certs.detail.domains"),
      value:
        cert.domains.length > 0 ? (
          <div className="flex flex-wrap gap-1">
            {cert.domains.map((d) => (
              <DotBadge key={d} tone="neutral">
                {d}
              </DotBadge>
            ))}
          </div>
        ) : (
          "—"
        ),
      copyValue: cert.domains.join("\n"),
    },
    // Show wildcard indicator if applicable
    isWildcard ? {
      label: t("certs.detail.type"),
      value: <DotBadge tone="warn">{t("certs.wildcard")}</DotBadge>,
    } : null,
    {
      label: t("certs.detail.notBefore"),
      value: cert.not_before_unix_nano
        ? formatTime(cert.not_before_unix_nano / 1e6, timezone)
        : "—",
    },
    {
      label: t("certs.detail.notAfter"),
      value: cert.not_after_unix_nano
        ? formatTime(cert.not_after_unix_nano / 1e6, timezone)
        : "—",
    },
    { label: t("certs.detail.serial"), value: cert.serial || "—", copyValue: cert.serial },
    { label: t("certs.colNode"), value: cert.hostname },
    {
      label: t("certs.detail.path"),
      value: <span className="font-mono text-xs">{cert.path}</span>,
      copyValue: cert.path,
    },
    {
      label: t("certs.sources.colPath"),
      value: cert.sourcePath ?? t("certs.sources.builtin"),
    },
  ];

  return (
    <Dialog
      open
      onClose={onClose}
      size="lg"
      title={cert.domains[0] || cert.subject || cert.path}
      description={t("certs.detail.subtitle")}
      footer={
        <Button variant="ghost" onClick={onClose}>
          {t("action.close")}
        </Button>
      }
    >
      <div className="space-y-4">
        <div className="flex items-center gap-2">
          {cert.parse_error ? (
            <>
              <ShieldAlert className="w-5 h-5 text-rose-600 dark:text-rose-400" aria-hidden="true" />
              <DotBadge tone="danger">{t("certs.parseError")}</DotBadge>
            </>
          ) : (
            <>
              <ShieldCheck
                className="w-5 h-5 text-emerald-600 dark:text-emerald-400"
                aria-hidden="true"
              />
              <DotBadge tone={expiryTone(days)}>
                {days < 0
                  ? t("certs.expired", { n: Math.abs(Math.floor(days)) })
                  : days < 1
                    ? t("certs.today")
                    : t("certs.daysLeft", { n: Math.floor(days) })}
              </DotBadge>
            </>
          )}
        </div>

        <dl className="divide-y divide-surface-2 dark:divide-ink-700 border-y border-surface-2 dark:border-ink-700">
          {rows.filter((r): r is NonNullable<typeof r> => r !== null).map((row) => (
            <div key={row.label} className="flex items-start justify-between gap-4 py-3">
              <dt className="text-xs text-ink-500 shrink-0 pt-0.5">{row.label}</dt>
              <dd className="text-sm text-ink-900 dark:text-surface-0 text-right min-w-0 break-all">
                {row.value}
                {row.copyValue && (
                  <Button
                    variant="ghost"
                    size="icon"
                    className="ml-1 align-middle"
                    aria-label={t("action.copy")}
                    onClick={() => void copy(row.label, row.copyValue as string)}
                  >
                    {copied === row.label ? (
                      <Check className="w-4 h-4 text-emerald-600" aria-hidden="true" />
                    ) : (
                      <Copy className="w-4 h-4" aria-hidden="true" />
                    )}
                  </Button>
                )}
              </dd>
            </div>
          ))}
        </dl>
      </div>
    </Dialog>
  );
}
