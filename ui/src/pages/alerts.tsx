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
import { ChevronRight, Pencil } from "lucide-react";
import {
  alertsApi,
  builtinAlertsApi,
  type AlertQuery,
  type BuiltinAlertRule,
} from "@/api";
import { TimeRangePicker, type TimeRange } from "@/components/ui/date-range-picker";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
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
import { TablePager, PAGE_SIZES } from "@/components/ui/pager";
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

const SOURCE_OPTIONS = [
  { value: "", label: "allSources" },
  { value: "rule", label: "sourceRule" },
  { value: "probe", label: "sourceProbe" },
  { value: "cert", label: "sourceCert" },
  { value: "node_offline", label: "sourceNodeOffline" },
  { value: "container", label: "sourceContainer" },
] as const;

function AlertHistorySection() {
  const { t } = useTranslation();
  const [showHistory, setShowHistory] = React.useState(false);
  const [timeRange, setTimeRange] = React.useState<TimeRange>({
    from: Date.now() - 7 * 24 * 60 * 60 * 1000,
    to: Date.now(),
  });
  const [statusFilter, setStatusFilter] = React.useState<"all" | "open" | "resolved">("all");
  const [sourceFilter, setSourceFilter] = React.useState("");
  const [page, setPage] = React.useState(1);
  const [pageSize, setPageSize] = React.useState(PAGE_SIZES[1]);

  // Reset page when filters change
  React.useEffect(() => {
    setPage(1);
  }, [timeRange, statusFilter, sourceFilter]);

  const queryParams: AlertQuery = {
    since: timeRange.from,
    until: timeRange.to,
    ...(statusFilter !== "all" ? { status: statusFilter } : {}),
    ...(sourceFilter ? { sources: sourceFilter } : {}),
    limit: pageSize,
    offset: (page - 1) * pageSize,
  };

  const q = useQuery({
    queryKey: ["alert-history", queryParams],
    queryFn: () => alertsApi.list(queryParams),
    enabled: showHistory,
  });

  const allAlerts = [...(q.data?.open ?? []), ...(q.data?.resolved ?? [])];
  const filteredAlerts = allAlerts.filter((a) => {
    if (statusFilter === "open" && a.resolved_at_unix_nano !== null) return false;
    if (statusFilter === "resolved" && a.resolved_at_unix_nano === null) return false;
    if (sourceFilter && a.source !== sourceFilter) return false;
    return true;
  });

  // Total from resolved query (open alerts don't paginate)
  const total = q.data?.total ?? filteredAlerts.length;
  const pageCount = Math.max(1, Math.ceil(total / pageSize));

  const sourceLabel = (source: string) => {
    const opt = SOURCE_OPTIONS.find((o) => o.value === source);
    return opt ? t(`filter.${opt.label}`) : source;
  };

  // Truncate message for display
  const truncateMessage = (msg: string, maxLen = 80) => {
    if (msg.length <= maxLen) return msg;
    return msg.slice(0, maxLen) + "…";
  };

  return (
    <div className="space-y-4">
      <button
        type="button"
        onClick={() => setShowHistory((v) => !v)}
        className="flex items-center gap-2 text-sm font-medium text-ink-700 hover:text-ink-900 dark:text-ink-300 dark:hover:text-ink-100"
      >
        <ChevronRight className={cn("w-4 h-4 transition-transform", showHistory && "rotate-90")} />
        {t("alerts.alertHistory")}
      </button>

      {showHistory && (
        <TableShell>
          <TableToolbar>
            <div className="flex flex-wrap items-center gap-3">
              <TimeRangePicker value={timeRange} onChange={setTimeRange} />
              <select
                value={statusFilter}
                onChange={(e) => setStatusFilter(e.target.value as typeof statusFilter)}
                className="h-8 px-2 text-sm border rounded bg-surface-1 dark:bg-ink-700 border-surface-3 dark:border-ink-600"
              >
                <option value="all">{t("filter.all")}</option>
                <option value="open">{t("filter.open")}</option>
                <option value="resolved">{t("filter.resolved")}</option>
              </select>
              <select
                value={sourceFilter}
                onChange={(e) => setSourceFilter(e.target.value)}
                className="h-8 px-2 text-sm border rounded bg-surface-1 dark:bg-ink-700 border-surface-3 dark:border-ink-600"
              >
                {SOURCE_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {t(`filter.${opt.label}`)}
                  </option>
                ))}
              </select>
            </div>
            <DotBadge tone="neutral">{total}</DotBadge>
          </TableToolbar>

          {q.isPending ? (
            <Skeleton className="h-48 w-full" />
          ) : q.isError ? (
            <ErrorState message={t("alerts.historyError")} onRetry={() => void q.refetch()} />
          ) : filteredAlerts.length === 0 ? (
            <EmptyState title={t("alerts.noAlerts")} />
          ) : (
            <>
              <Table>
                <THead>
                  <tr>
                    <Th>{t("alerts.colTime")}</Th>
                    <Th>{t("alerts.colSource")}</Th>
                    <Th>{t("alerts.colSeverity")}</Th>
                    <Th className="hidden md:table-cell">{t("alerts.colMessage")}</Th>
                  </tr>
                </THead>
                <TBody>
                  {filteredAlerts.map((alert) => (
                    <Tr key={alert.id}>
                      <Td className="whitespace-nowrap text-xs text-ink-500">
                        {new Date(alert.started_at_unix_nano / 1_000_000).toLocaleString()}
                      </Td>
                      <Td>
                        <span className="text-xs px-2 py-0.5 rounded bg-surface-3 dark:bg-ink-700 text-ink-600 dark:text-surface-4">
                          {sourceLabel(alert.source)}
                        </span>
                      </Td>
                      <Td>
                        <span
                          className={cn(
                            "text-xs px-2 py-0.5 rounded font-medium",
                            alert.severity === "critical"
                              ? "bg-rose-100 text-rose-700 dark:bg-rose-900/50 dark:text-rose-400"
                              : "bg-amber-100 text-amber-700 dark:bg-amber-900/50 dark:text-amber-400",
                          )}
                        >
                          {t(`alerts.${alert.severity}`)}
                        </span>
                      </Td>
                      <Td className="hidden md:table-cell min-w-[200px] max-w-[300px]">
                        <span
                          className="text-xs text-ink-500"
                          title={alert.message || alert.rule_name}
                        >
                          {truncateMessage(alert.message || alert.rule_name)}
                        </span>
                      </Td>
                    </Tr>
                  ))}
                </TBody>
              </Table>
              <TablePager
                page={page}
                pageCount={pageCount}
                pageSize={pageSize}
                onPage={setPage}
                onPageSize={(size) => {
                  setPageSize(size);
                  setPage(1);
                }}
                left={
                  <span className="text-xs text-ink-500">
                    {t("pager.total", { n: total })}
                  </span>
                }
              />
            </>
          )}
        </TableShell>
      )}
    </div>
  );
}
