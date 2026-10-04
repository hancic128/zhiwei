/**
 * Alerts page — manages builtin alert rules (node online/offline, service probe,
 * container, certificate events) and alert history.
 *
 * All alerts are builtin, no custom threshold rules. Users can:
 * - Enable/disable each alert
 * - Edit threshold and duration for each alert
 * - Configure notification channels in settings
 * - View and filter alert history
 */
import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Calendar, ChevronRight, Pencil, X } from "lucide-react";
import {
  alertsApi,
  builtinAlertsApi,
  type AlertQuery,
  type BuiltinAlertRule,
} from "@/api";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
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
  cpu_high: { metric: "host.cpu.usage > threshold", severity: "warning", tone: "warn" },
  mem_high: { metric: "host.mem.usage > threshold", severity: "warning", tone: "warn" },
  disk_high: { metric: "host.disk.usage > threshold", severity: "warning", tone: "warn" },
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

      <AlertHistorySection />
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

  // Determine which fields are applicable based on alert type
  const isOfflineAlert = rule.id === "node_offline" || rule.id === "service_offline" || rule.id === "container_stopped";
  const isCertAlert = rule.id === "cert_expiring" || rule.id === "cert_expired";
  const isMetricAlert = rule.id === "cpu_high" || rule.id === "mem_high" || rule.id === "disk_high";

  const [form, setForm] = React.useState({
    duration_seconds: rule.duration_seconds || 300,
    threshold: rule.threshold || (isOfflineAlert ? 60 : isCertAlert ? 30 : isMetricAlert ? 80 : 0),
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
      : isMetricAlert
        ? t("alerts.thresholdPercent")
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

// ---------- Alert History ----------

/** Time range presets in milliseconds */
const TIME_PRESETS = [
  { label: "24h", ms: 24 * 60 * 60 * 1000 },
  { label: "7d", ms: 7 * 24 * 60 * 60 * 1000 },
  { label: "30d", ms: 30 * 24 * 60 * 60 * 1000 },
  { label: "1y", ms: 365 * 24 * 60 * 60 * 1000 },
];

interface AlertFilterState {
  preset: string | null;
  since: number | null;
  until: number | null;
  status: "all" | "open" | "resolved";
  source: string;
}

function parseDateInput(val: string): number | null {
  if (!val) return null;
  return new Date(val).getTime();
}

function AlertHistorySection() {
  const { t } = useTranslation();
  const [showHistory, setShowHistory] = React.useState(false);
  const [filters, setFilters] = React.useState<AlertFilterState>({
    preset: null,
    since: null,
    until: null,
    status: "all",
    source: "",
  });
  const [showDatePicker, setShowDatePicker] = React.useState(false);
  const [customSince, setCustomSince] = React.useState("");
  const [customUntil, setCustomUntil] = React.useState("");

  const queryParams: AlertQuery = {
    ...(filters.since ? { since: filters.since } : {}),
    ...(filters.until ? { until: filters.until } : {}),
    ...(filters.status !== "all" ? { status: filters.status } : {}),
    ...(filters.source ? { sources: filters.source } : {}),
    limit: 100,
  };

  const q = useQuery({
    queryKey: ["alert-history", queryParams],
    queryFn: () => alertsApi.list(queryParams),
    enabled: showHistory,
  });

  const applyPreset = (preset: (typeof TIME_PRESETS)[number] | null) => {
    if (preset) {
      const now = Date.now();
      setFilters({
        ...filters,
        preset: preset.label,
        since: now - preset.ms,
        until: now,
      });
      setShowDatePicker(false);
    } else {
      setFilters({
        preset: null,
        since: null,
        until: null,
        status: "all",
        source: "",
      });
      setCustomSince("");
      setCustomUntil("");
      setShowDatePicker(false);
    }
  };

  const applyCustomRange = () => {
    setFilters({
      preset: null,
      since: parseDateInput(customSince),
      until: parseDateInput(customUntil) || Date.now(),
      status: filters.status,
      source: filters.source,
    });
    setShowDatePicker(false);
  };

  const clearFilters = () => {
    setFilters({
      preset: null,
      since: null,
      until: null,
      status: "all",
      source: "",
    });
    setCustomSince("");
    setCustomUntil("");
    setShowDatePicker(false);
  };

  const hasFilters =
    filters.preset !== null ||
    filters.status !== "all" ||
    filters.source !== "" ||
    filters.since !== null;

  const allAlerts = [...(q.data?.open ?? []), ...(q.data?.resolved ?? [])];
  const filteredAlerts = allAlerts.filter((a) => {
    if (filters.status === "open" && a.resolved_at_unix_nano !== null) return false;
    if (filters.status === "resolved" && a.resolved_at_unix_nano === null) return false;
    if (filters.source && a.source !== filters.source) return false;
    return true;
  });

  return (
    <div className="space-y-4">
      <button
        type="button"
        onClick={() => setShowHistory((v) => !v)}
        className="flex items-center gap-2 text-sm font-medium text-ink-700 hover:text-ink-900 dark:text-ink-300 dark:hover:text-ink-100"
      >
        <ChevronRight
          className={cn("w-4 h-4 transition-transform", showHistory && "rotate-90")}
        />
        {t("alerts.alertHistory")}
      </button>

      {showHistory && (
        <Card>
          {/* Filters */}
          <div className="flex flex-wrap items-center gap-2 px-4 py-3 border-b border-surface-3 dark:border-ink-700">
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
                      : "bg-surface-1 text-ink-600 border-surface-3 hover:border-ink-400 dark:bg-ink-700 dark:text-surface-4 dark:border-ink-600",
                  )}
                >
                  {p.label}
                </button>
              ))}
              <button
                type="button"
                onClick={() => setShowDatePicker((v) => !v)}
                className={cn(
                  "px-2 py-1 text-xs rounded border transition-colors",
                  showDatePicker
                    ? "bg-brand-100 text-brand-700 border-brand-300 dark:bg-brand-900 dark:text-brand-200 dark:border-brand-700"
                    : "bg-surface-1 text-ink-600 border-surface-3 hover:border-ink-400 dark:bg-ink-700 dark:text-surface-4 dark:border-ink-600",
                )}
              >
                <Calendar className="w-3 h-3 inline mr-1" />
                {t("alerts.customRange")}
              </button>
            </div>

            {/* Status filter */}
            <Select
              value={filters.status}
              onChange={(e) =>
                setFilters({ ...filters, status: e.target.value as AlertFilterState["status"] })
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
              onChange={(e) => setFilters({ ...filters, source: e.target.value })}
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

          {/* Date range picker */}
          {showDatePicker && (
            <div className="flex flex-wrap items-end gap-2 px-4 py-3 bg-surface-2 dark:bg-ink-700/30 rounded-lg border border-surface-3 dark:border-ink-600">
              <div className="flex items-center gap-2">
                <label className="text-xs text-ink-500">
                  {t("alerts.from")}
                  <input
                    type="datetime-local"
                    value={customSince}
                    onChange={(e) => setCustomSince(e.target.value)}
                    className="block mt-1 px-2 py-1 text-sm border rounded bg-surface-1 dark:bg-ink-700 border-surface-3 dark:border-ink-600"
                  />
                </label>
                <label className="text-xs text-ink-500">
                  {t("alerts.to")}
                  <input
                    type="datetime-local"
                    value={customUntil}
                    onChange={(e) => setCustomUntil(e.target.value)}
                    className="block mt-1 px-2 py-1 text-sm border rounded bg-surface-1 dark:bg-ink-700 border-surface-3 dark:border-ink-600"
                  />
                </label>
              </div>
              <Button size="sm" onClick={applyCustomRange}>
                {t("action.apply")}
              </Button>
            </div>
          )}

          {/* Alert list */}
          {q.isPending ? (
            <div className="p-4"><Skeleton className="h-48 w-full" /></div>
          ) : q.isError ? (
            <div className="p-4"><ErrorState
              message={t("alerts.historyError")}
              onRetry={() => void q.refetch()}
            /></div>
          ) : filteredAlerts.length === 0 ? (
            <div className="p-4"><EmptyState title={t("alerts.noAlerts")} /></div>
          ) : (
            <div className="border-t border-surface-3 dark:border-ink-700">
              <table className="w-full text-sm">
                <thead className="bg-surface-2 dark:bg-ink-700/50">
                  <tr>
                    <th className="px-4 py-2 text-left text-xs text-ink-500 font-medium">{t("alerts.colTime")}</th>
                    <th className="px-4 py-2 text-left text-xs text-ink-500 font-medium">{t("alerts.colSource")}</th>
                    <th className="px-4 py-2 text-left text-xs text-ink-500 font-medium">{t("alerts.colSeverity")}</th>
                    <th className="px-4 py-2 text-left text-xs text-ink-500 font-medium hidden md:table-cell">{t("alerts.colMessage")}</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-surface-3 dark:divide-ink-700">
                  {filteredAlerts.map((alert) => (
                    <tr key={alert.id} className="hover:bg-surface-2 dark:hover:bg-ink-700/30">
                      <td className="px-4 py-2 text-xs text-ink-500">
                        {new Date(alert.started_at_unix_nano / 1_000_000).toLocaleString()}
                      </td>
                      <td className="px-4 py-2">
                        <span className="text-xs px-2 py-0.5 rounded bg-surface-3 dark:bg-ink-700 text-ink-600 dark:text-surface-4">
                          {t(`filter.source${alert.source.charAt(0).toUpperCase() + alert.source.slice(1).replace("_", "")}`)}
                        </span>
                      </td>
                      <td className="px-4 py-2">
                        <span
                          className={cn(
                            "text-xs px-2 py-0.5 rounded font-medium",
                            alert.severity === "critical"
                              ? "bg-rose-100 text-rose-700 dark:bg-rose-900/50 dark:text-rose-400"
                              : "bg-amber-100 text-amber-700 dark:bg-amber-900/50 dark:text-amber-400",
                          )}
                        >
                          {alert.severity}
                        </span>
                      </td>
                      <td className="px-4 py-2 text-xs text-ink-500 hidden md:table-cell max-w-md truncate">
                        {alert.message || alert.rule_name}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Card>
      )}
    </div>
  );
}
