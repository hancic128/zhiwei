/**
 * Alerts page — manages builtin alert rules (node online/offline, service probe,
 * container, certificate events).
 *
 * All alerts are builtin, no custom threshold rules. Users can:
 * - Enable/disable each alert
 * - Edit threshold and duration for each alert
 * - Configure notification channels in settings
 */
import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Pencil } from "lucide-react";
import {
  builtinAlertsApi,
  type BuiltinAlertRule,
} from "@/api";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  EmptyState,
  ErrorState,
  Skeleton,
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
import { useToast } from "@/components/ui/toast";
import { cn, friendlyError } from "@/lib/utils";

/// Metadata for builtin alert rules: metric expression, severity badge
const BUILTIN_META: Record<
  string,
  { metric: string; severity: "info" | "warning" | "critical"; tone: "info" | "danger" | "warn" | "ok" }
> = {
  node_offline: { metric: "host.online = 0", severity: "critical", tone: "danger" },
  node_online: { metric: "host.online = 1", severity: "info", tone: "info" },
  service_offline: { metric: "probe.state = down", severity: "critical", tone: "danger" },
  service_online: { metric: "probe.state = ok", severity: "info", tone: "info" },
  container_stopped: { metric: "container.state = stopped", severity: "warning", tone: "warn" },
  container_started: { metric: "container.state = started", severity: "info", tone: "info" },
  cert_expired: { metric: "cert.days_left < 0", severity: "critical", tone: "danger" },
  cert_expiring: { metric: "cert.days_left < notify_days_before", severity: "warning", tone: "warn" },
};

const BUILTIN_TONE_CLASS: Record<string, string> = {
  danger: "bg-rose-50 text-rose-700 dark:bg-rose-700/20 dark:text-rose-400",
  warn: "bg-amber-50 text-amber-700 dark:bg-amber-700/20 dark:text-amber-400",
  ok: "bg-emerald-50 text-emerald-700 dark:bg-emerald-700/20 dark:text-emerald-400",
  info: "bg-sky-50 text-sky-700 dark:bg-sky-700/20 dark:text-sky-400",
};

function builtinMeta(id: string) {
  return (
    BUILTIN_META[id] ?? {
      metric: id,
      severity: "warning" as const,
      tone: "warn" as const,
    }
  );
}

export function Alerts() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const q = useQuery({
    queryKey: ["builtin-alerts"],
    queryFn: builtinAlertsApi.list,
  });

  const [editingRule, setEditingRule] = React.useState<BuiltinAlertRule | null>(null);

  const toggle = useMutation({
    mutationFn: (r: BuiltinAlertRule) =>
      builtinAlertsApi.update(r.id, { enabled: !r.enabled }),
    onSuccess: () => {
      toast.push("success", t("alerts.updated"));
      void qc.invalidateQueries({ queryKey: ["builtin-alerts"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const rules = q.data ?? [];

  return (
    <div className="space-y-4 md:space-y-6">
      <div>
        <h2 className="text-lg font-semibold text-ink-900 dark:text-surface-0">
          {t("alerts.title")}
        </h2>
        <p className="text-sm text-ink-500 mt-0.5">{t("alerts.subtitle")}</p>
      </div>

      <TableShell>
        <TableToolbar>
          <div>
            <h2 className="text-base font-semibold text-ink-900 dark:text-surface-0">
              {t("alerts.builtinTitle")}
            </h2>
            <p className="text-sm text-ink-500 mt-0.5">
              {t("alerts.builtinSubtitle")}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <DotBadge tone="neutral">{rules.length}</DotBadge>
          </div>
        </TableToolbar>

        {q.isPending ? (
          <Skeleton className="h-20 w-full" />
        ) : q.isError ? (
          <ErrorState
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : rules.length === 0 ? (
          <EmptyState
            title={t("alerts.emptyRules")}
            description={t("alerts.emptyHintRules")}
          />
        ) : (
          <Table>
            <THead>
              <tr>
                <Th>{t("alerts.ruleName")}</Th>
                <Th className="hidden md:table-cell">{t("alerts.metric")}</Th>
                <Th className="hidden lg:table-cell">{t("alerts.severity")}</Th>
                <Th className="hidden lg:table-cell">{t("alerts.duration")}</Th>
                <Th align="right">{t("alerts.colActions")}</Th>
              </tr>
            </THead>
            <TBody>
              {rules.map((r) => (
                <Tr key={r.id}>
                  <Td>
                    <div className="text-sm font-medium text-ink-900 dark:text-surface-0">
                      {r.name}
                    </div>
                    <div className="text-xs text-ink-400">{r.id}</div>
                  </Td>
                  <Td className="hidden md:table-cell">
                    <span className="text-xs tabular-nums text-ink-500">
                      {builtinMeta(r.id).metric}
                    </span>
                  </Td>
                  <Td className="hidden lg:table-cell">
                    <span
                      className={cn(
                        "inline-flex items-center px-2 py-0.5 rounded text-xs font-medium",
                        BUILTIN_TONE_CLASS[builtinMeta(r.id).tone],
                      )}
                    >
                      {t(
                        builtinMeta(r.id).severity === "critical"
                          ? "alerts.critical"
                          : builtinMeta(r.id).severity === "warning"
                            ? "alerts.warning"
                            : "alerts.info",
                      )}
                    </span>
                  </Td>
                  <Td className="hidden lg:table-cell">
                    <span className="text-xs tabular-nums text-ink-500">
                      {r.duration_seconds}s
                    </span>
                  </Td>
                  <Td align="right">
                    <div className="flex items-center justify-end gap-2">
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label={t("action.edit")}
                        onClick={() => setEditingRule(r)}
                      >
                        <Pencil className="w-4 h-4" aria-hidden="true" />
                      </Button>
                      <Switch
                        checked={r.enabled}
                        onCheckedChange={() => toggle.mutate(r)}
                        disabled={toggle.isPending}
                        aria-label={`${r.name} — ${t(
                          r.enabled ? "alerts.disable" : "alerts.enable",
                        )}`}
                      />
                    </div>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </Table>
        )}
      </TableShell>

      {editingRule && (
        <EditBuiltinAlertDialog
          rule={editingRule}
          onClose={() => setEditingRule(null)}
        />
      )}
    </div>
  );
}

interface EditBuiltinAlertDialogProps {
  rule: BuiltinAlertRule;
  onClose: () => void;
}

function EditBuiltinAlertDialog({ rule, onClose }: EditBuiltinAlertDialogProps) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const meta = builtinMeta(rule.id);

  // Determine which fields are applicable based on alert type
  const isOfflineAlert = rule.id === "node_offline" || rule.id === "service_offline" || rule.id === "container_stopped";
  const isCertAlert = rule.id === "cert_expiring" || rule.id === "cert_expired";

  const [form, setForm] = React.useState({
    duration_seconds: rule.duration_seconds || 300,
    // For node offline: threshold is seconds before marking offline (default 60)
    // For cert expiring: threshold is days before expiry (default 30)
    threshold: rule.threshold || (isOfflineAlert ? 60 : isCertAlert ? 30 : 0),
  });

  const save = useMutation({
    mutationFn: () =>
      builtinAlertsApi.update(rule.id, {
        duration_seconds: form.duration_seconds,
        threshold: form.threshold,
      }),
    onSuccess: () => {
      toast.push("success", t("alerts.updated"));
      void qc.invalidateQueries({ queryKey: ["builtin-alerts"] });
      onClose();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const thresholdLabel = isOfflineAlert
    ? t("alerts.offlineAfterSeconds")
    : isCertAlert
      ? t("alerts.daysBeforeExpiry")
      : t("alerts.threshold");

  return (
    <Dialog
      open
      onClose={onClose}
      size="md"
      title={t("alerts.editRule")}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("alerts.cancel")}
          </Button>
          <Button
            loading={save.isPending}
            onClick={() => save.mutate()}
          >
            {t("action.save")}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <div className="text-sm text-ink-500">
          {t("alerts.ruleName")}: <span className="font-medium text-ink-700 dark:text-surface-100">{rule.name}</span>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <Field label={thresholdLabel}>
            <Input
              type="number"
              value={form.threshold}
              onChange={(e) =>
                setForm({ ...form, threshold: Number(e.target.value) })
              }
              min={0}
            />
          </Field>
          <Field label={t("alerts.duration")}>
            <Input
              type="number"
              value={form.duration_seconds}
              onChange={(e) =>
                setForm({ ...form, duration_seconds: Number(e.target.value) })
              }
              min={0}
            />
          </Field>
        </div>

        <p className="text-xs text-ink-400">
          {isOfflineAlert && t("alerts.offlineAfterHint")}
          {isCertAlert && t("alerts.daysBeforeExpiryHint")}
        </p>
      </div>
    </Dialog>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="block text-xs text-ink-500 mb-1">{label}</span>
      {children}
    </label>
  );
}
