import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Globe,
  History,
  Network,
  Pencil,
  Plus,
  Power,
  RefreshCw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  api,
  servicesApi,
  trendApi,
  type ProbeResultView,
  type ProbeStateName,
  type ProbeView,
  type ServiceView,
} from "@/api";
import { LineChart } from "@/components/chart";
import { StatCards } from "@/components/stat-cards";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import {
  TimeRangePicker,
  presetRange,
  type TimeRange,
} from "@/components/ui/date-range-picker";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog } from "@/components/ui/dialog";
import { Input, SearchInput } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import {
  EmptyState,
  ErrorState,
  SearchEmptyState,
  Skeleton,
} from "@/components/ui/feedback";
import {
  Table,
  TBody,
  Td,
  Th,
  THead,
  Tr,
} from "@/components/ui/table";
import { useToast } from "@/components/ui/toast";
import { friendlyError, formatTime, nodeLabel, relativeTime } from "@/lib/utils";
import { usePrefs } from "@/components/prefs-provider";

/** 探针类型图标（规范：图标一律 Lucide SVG，禁止 emoji） */
const KIND_ICON: Record<string, React.ElementType> = {
  http: Globe,
  tcp: Network,
  tls: ShieldCheck,
};

/** 状态 → DotBadge tone */
function stateTone(state: ProbeStateName): "success" | "warn" | "danger" | "neutral" {
  if (state === "ok") return "success";
  if (state === "degraded") return "warn";
  if (state === "down") return "danger";
  return "neutral";
}

/** 状态 → i18n key（未知取值统一落到 unknown，避免露出原始字符串） */
function stateKey(state: ProbeStateName): string {
  return ["ok", "degraded", "down"].includes(state) ? state : "unknown";
}

/** 延迟展示：亚 10ms 保留一位小数，避免出现「0 ms」这种失真读数 */
function formatLatency(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "—";
  return `${ms < 10 ? ms.toFixed(1) : ms.toFixed(0)} ms`;
}

interface TargetShape {
  url?: string;
  host?: string;
  port?: number;
  sni?: string;
  method?: string;
  headers?: Record<string, string>;
  body?: string;
}

interface ExpectShape {
  status?: number[];
  body_contains?: string[];
  banner_contains?: string;
  max_latency_ms?: number;
  min_days_valid?: number;
  tls_verify?: boolean;
  verify?: boolean;
}

function parseJson<T>(raw: string, fallback: T): T {
  try {
    const v = JSON.parse(raw);
    return v && typeof v === "object" ? (v as T) : fallback;
  } catch {
    return fallback;
  }
}

/** 目标的紧凑展示（列表列） */
function targetSummary(probe: ProbeView): string {
  const target = parseJson<TargetShape>(probe.target_json, {});
  if (probe.kind === "http") return target.url ?? "—";
  const host = target.host ?? "—";
  return `${host}:${target.port ?? ""}`.replace(/:$/, "");
}

/**
 * 服务健康时间线：**一条线一个服务**，画的是每个时间桶里「有多少比例的探测是
 * ok 的」（0–100%）。
 *
 * 为什么用比例而不是单次结果：不同服务的探针间隔不同，单次结果画在一起会因为
 * 采样密度不同而忽高忽低；比例能在同一尺度上比较。
 */
function ServicesTimeline() {
  const { t } = useTranslation();
  const [range, setRange] = React.useState<TimeRange>(() => presetRange("1d"));
  const q = useQuery({
    queryKey: ["services-timeline", range.from, range.to],
    queryFn: () => trendApi.servicesTimeline(range.from, range.to, 60),
    refetchInterval: 60000,
  });

  const series = (q.data?.services ?? []).map((s) => ({
    name: s.name,
    data: s.points.map((p) => [p.t, p.v] as [number, number]),
  }));

  return (
    <Card>
      <CardHeader
        title={t("services.timelineTitle")}
        description={t("services.timelineSubtitle")}
        action={<TimeRangePicker value={range} onChange={setRange} />}
      />
      <CardBody compact>
        {q.isPending ? (
          <Skeleton className="h-64 w-full" />
        ) : q.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : series.length === 0 ? (
          <EmptyState
            title={t("detail.chartEmpty")}
            description={t("detail.chartEmptyHint")}
          />
        ) : (
          <LineChart
            series={series}
            height="md"
            unit="%"
            yMax={100}
            xMin={range.from}
            xMax={range.to}
          />
        )}
      </CardBody>
    </Card>
  );
}

export function Services() {
  const { t } = useTranslation();
  const { timezone: tz } = usePrefs();
  const qc = useQueryClient();
  const toast = useToast();

  const [q, setQ] = React.useState("");
  const [serviceDialog, setServiceDialog] = React.useState<{
    open: boolean;
    service?: ServiceView;
  }>({ open: false });
  const [probeDialog, setProbeDialog] = React.useState<{
    open: boolean;
    serviceId?: string;
    probe?: ProbeView;
  }>({ open: false });
  const [resultsFor, setResultsFor] = React.useState<ProbeView | null>(null);
  const [pendingDelete, setPendingDelete] = React.useState<{
    kind: "service" | "probe";
    id: string;
    name: string;
  } | null>(null);

  const servicesQ = useQuery({
    queryKey: ["services"],
    queryFn: servicesApi.list,
    refetchInterval: 15000,
  });
  const services: ServiceView[] = servicesQ.data ?? [];

  const toggleProbe = useMutation({
    mutationFn: (p: ProbeView) =>
      servicesApi.updateProbe(p.id, { enabled: !p.enabled }),
    onSuccess: () => {
      toast.push("success", t("services.updated"));
      void qc.invalidateQueries({ queryKey: ["services"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const toggleService = useMutation({
    mutationFn: (s: ServiceView) =>
      servicesApi.update(s.id, { enabled: !s.enabled }),
    onSuccess: () => {
      toast.push("success", t("services.updated"));
      void qc.invalidateQueries({ queryKey: ["services"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const removeEntity = useMutation({
    mutationFn: (target: { kind: "service" | "probe"; id: string }) =>
      target.kind === "service"
        ? servicesApi.remove(target.id)
        : servicesApi.removeProbe(target.id),
    onSuccess: () => {
      toast.push("success", t("services.deleted"));
      setPendingDelete(null);
      void qc.invalidateQueries({ queryKey: ["services"] });
    },
    onError: (e) => {
      toast.push("error", t(friendlyError(e)));
      setPendingDelete(null);
    },
  });

  const needle = q.trim().toLowerCase();
  const filtered = React.useMemo(() => {
    if (!needle) return services;
    return services.filter(
      (s) =>
        s.name.toLowerCase().includes(needle) ||
        s.group_name.toLowerCase().includes(needle) ||
        s.probes.some(
          (p) =>
            p.name.toLowerCase().includes(needle) ||
            targetSummary(p).toLowerCase().includes(needle),
        ),
    );
  }, [services, needle]);

  const healthy = services.filter((s) => s.health === "ok").length;
  const degradedCount = services.filter((s) => s.health === "degraded").length;
  const downCount = services.filter((s) => s.health === "down").length;

  return (
    <>
      <StatCards
        cards={[
          {
            key: "all",
            label: t("services.cardAll"),
            value: services.length,
            hint: t("services.cardAllHint"),
            tone: "neutral",
            icon: Activity,
          },
          {
            key: "ok",
            label: t("services.cardHealthy"),
            value: healthy,
            hint: t("services.cardHealthyHint"),
            tone: "success",
            icon: CheckCircle2,
          },
          {
            key: "degraded",
            label: t("state.degraded"),
            value: degradedCount,
            hint: t("services.cardDegradedHint"),
            tone: degradedCount > 0 ? "warn" : "neutral",
          },
          {
            key: "down",
            label: t("state.down"),
            value: downCount,
            hint: t("services.cardDownHint"),
            tone: downCount > 0 ? "danger" : "neutral",
            icon: AlertTriangle,
          },
        ]}
      />

      <ServicesTimeline />

      {/* 工具栏：左标题右操作（规范 7.3） */}
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h2 className="text-lg font-semibold text-ink-900 dark:text-surface-0">
            {t("services.title")}
          </h2>
          <p className="text-sm text-ink-500 mt-0.5">
            {t("services.subtitle")} · {t("services.cardHealthy", {
              healthy,
              total: services.length,
            })}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <SearchInput
            className="w-48 md:w-64"
            placeholder={t("services.searchPlaceholder")}
            aria-label={t("action.search")}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
          <Button
            variant="ghost"
            size="icon"
            aria-label={t("action.refresh")}
            onClick={() => void servicesQ.refetch()}
          >
            <RefreshCw className="w-5 h-5" aria-hidden="true" />
          </Button>
          <Button onClick={() => setServiceDialog({ open: true })}>
            <Plus className="w-4 h-4" aria-hidden="true" />
            {t("services.newService")}
          </Button>
        </div>
      </div>

      {servicesQ.isLoading ? (
        <Card>
          <CardBody>
            <Skeleton className="h-24 w-full" />
          </CardBody>
        </Card>
      ) : servicesQ.isError ? (
        <Card>
          <ErrorState
            message={t(friendlyError(servicesQ.error))}
            onRetry={() => void servicesQ.refetch()}
            retrying={servicesQ.isFetching}
          />
        </Card>
      ) : services.length === 0 ? (
        <Card>
          <EmptyState
            title={t("services.empty")}
            description={t("services.emptyHint")}
          />
        </Card>
      ) : filtered.length === 0 ? (
        <Card>
          <SearchEmptyState
            title={t("services.searchEmpty")}
            description={t("services.searchEmptyHint")}
          />
        </Card>
      ) : (
        <div className="space-y-4 md:space-y-6">
          {filtered.map((svc) => (
            <ServiceCard
              key={svc.id}
              service={svc}
              tz={tz}
              onNewProbe={() =>
                setProbeDialog({ open: true, serviceId: svc.id })
              }
              onEdit={() => setServiceDialog({ open: true, service: svc })}
              onToggle={() => toggleService.mutate(svc)}
              onDelete={() =>
                setPendingDelete({
                  kind: "service",
                  id: svc.id,
                  name: svc.name,
                })
              }
              onEditProbe={(p) =>
                setProbeDialog({ open: true, serviceId: svc.id, probe: p })
              }
              onToggleProbe={(p) => toggleProbe.mutate(p)}
              onResults={(p) => setResultsFor(p)}
              onDeleteProbe={(p) =>
                setPendingDelete({ kind: "probe", id: p.id, name: p.name })
              }
              t={t}
            />
          ))}
        </div>
      )}

      {serviceDialog.open && (
        <ServiceDialog
          service={serviceDialog.service}
          onClose={() => setServiceDialog({ open: false })}
        />
      )}

      {probeDialog.open && probeDialog.serviceId && (
        <ProbeDialog
          serviceId={probeDialog.serviceId}
          probe={probeDialog.probe}
          onClose={() => setProbeDialog({ open: false })}
        />
      )}

      {resultsFor && (
        <ResultsDialog
          probe={resultsFor}
          tz={tz}
          onClose={() => setResultsFor(null)}
        />
      )}

      <ConfirmDialog
        open={!!pendingDelete}
        danger
        title={
          pendingDelete?.kind === "service"
            ? t("services.deleteServiceTitle")
            : t("services.deleteProbeTitle")
        }
        message={
          pendingDelete?.kind === "service"
            ? t("services.deleteServiceMessage", { name: pendingDelete?.name })
            : t("services.deleteProbeMessage", { name: pendingDelete?.name })
        }
        confirmLabel={
          pendingDelete?.kind === "service"
            ? t("services.deleteServiceConfirm")
            : t("services.deleteProbeConfirm")
        }
        cancelLabel={t("action.cancel")}
        loading={removeEntity.isPending}
        onCancel={() => setPendingDelete(null)}
        onConfirm={() =>
          pendingDelete &&
          removeEntity.mutate({ kind: pendingDelete.kind, id: pendingDelete.id })
        }
      />
    </>
  );
}

function ServiceCard({
  service,
  tz,
  t,
  onNewProbe,
  onEdit,
  onToggle,
  onDelete,
  onEditProbe,
  onToggleProbe,
  onResults,
  onDeleteProbe,
}: {
  service: ServiceView;
  tz: string;
  t: (k: string, o?: Record<string, unknown>) => string;
  onNewProbe: () => void;
  onEdit: () => void;
  onToggle: () => void;
  onDelete: () => void;
  onEditProbe: (p: ProbeView) => void;
  onToggleProbe: (p: ProbeView) => void;
  onResults: (p: ProbeView) => void;
  onDeleteProbe: (p: ProbeView) => void;
}) {
  return (
    <Card>
      <CardHeader
        icon={
          <DotBadge
            tone={stateTone(service.health)}
            pulse={service.health === "ok"}
          >
            {t(`state.${stateKey(service.health)}`)}
          </DotBadge>
        }
        title={service.name}
        description={[
          service.group_name,
          t("services.probeCount", { n: service.probes.length }),
          service.enabled ? "" : t("services.disabled"),
          service.description,
        ]
          .filter(Boolean)
          .join(" · ")}
        action={
          <>
            <Button size="sm" variant="secondary" onClick={onNewProbe}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("services.newProbe")}
            </Button>
            <Button
              size="icon"
              variant="ghost"
              aria-label={t("action.edit")}
              onClick={onEdit}
            >
              <Pencil className="w-4 h-4" aria-hidden="true" />
            </Button>
            <Button
              size="icon"
              variant="ghost"
              aria-label={service.enabled ? t("action.disable") : t("action.enable")}
              onClick={onToggle}
            >
              <Power className="w-4 h-4" aria-hidden="true" />
            </Button>
            <Button
              size="icon"
              variant="ghost"
              aria-label={t("action.delete")}
              className="text-rose-600 dark:text-rose-400"
              onClick={onDelete}
            >
              <Trash2 className="w-4 h-4" aria-hidden="true" />
            </Button>
          </>
        }
      />

      {service.probes.length === 0 ? (
        <CardBody compact>
          <p className="text-sm text-ink-500">{t("services.noProbes")}</p>
        </CardBody>
      ) : (
        <Table>
          <THead>
            <tr>
              <Th>{t("services.colService")}</Th>
              <Th className="hidden sm:table-cell">{t("services.colKind")}</Th>
              <Th className="hidden md:table-cell">{t("services.colTarget")}</Th>
              <Th>{t("services.colState")}</Th>
              <Th align="right" className="hidden lg:table-cell">
                {t("services.colLatency")}
              </Th>
              <Th className="hidden xl:table-cell">
                {t("services.colLastCheck")}
              </Th>
              <Th align="right">{t("services.colActions")}</Th>
            </tr>
          </THead>
          <TBody>
            {service.probes.map((p) => {
              const Icon = KIND_ICON[p.kind] ?? Activity;
              const state = p.state.state;
              return (
                <Tr key={p.id}>
                  <Td>
                    <div className="text-sm font-medium text-ink-900 dark:text-surface-0 truncate max-w-[220px]">
                      {p.name}
                    </div>
                    <div className="text-xs text-ink-400 truncate max-w-[220px]">
                      {p.node_hostname ?? t("services.anyNode")}
                      {p.enabled ? "" : ` · ${t("services.disabled")}`}
                    </div>
                  </Td>
                  <Td className="hidden sm:table-cell">
                    <span className="inline-flex items-center gap-1.5 text-xs text-ink-500">
                      <Icon className="w-4 h-4" aria-hidden="true" />
                      {p.kind.toUpperCase()}
                    </span>
                  </Td>
                  <Td className="hidden md:table-cell">
                    <span className="text-xs text-ink-500 break-all">
                      {targetSummary(p)}
                    </span>
                  </Td>
                  <Td>
                    <DotBadge
                      tone={stateTone(state)}
                      pulse={state === "ok"}
                    >
                      {t(`state.${stateKey(state)}`)}
                    </DotBadge>
                    {state !== "ok" && p.state.last_error && (
                      <div className="mt-1 text-xs text-ink-400 max-w-[220px] truncate">
                        {p.state.last_error}
                      </div>
                    )}
                  </Td>
                  <Td align="right" className="hidden lg:table-cell">
                    <span className="text-sm tabular-nums text-ink-900 dark:text-surface-0">
                      {formatLatency(p.state.last_latency_ms)}
                    </span>
                  </Td>
                  <Td className="hidden xl:table-cell">
                    <span className="text-xs text-ink-500">
                      {p.state.last_check_at_unix_nano > 0
                        ? relativeTime(
                            p.state.last_check_at_unix_nano / 1e6,
                            t,
                          )
                        : t("services.never")}
                    </span>
                    {p.state.last_check_at_unix_nano > 0 && (
                      <div className="text-xs text-ink-400">
                        {formatTime(p.state.last_check_at_unix_nano / 1e6, tz)}
                      </div>
                    )}
                  </Td>
                  <Td align="right">
                    <div className="flex items-center justify-end gap-1">
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label={t("action.results")}
                        onClick={() => onResults(p)}
                      >
                        <History className="w-4 h-4" aria-hidden="true" />
                      </Button>
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label={t("action.edit")}
                        onClick={() => onEditProbe(p)}
                      >
                        <Pencil className="w-4 h-4" aria-hidden="true" />
                      </Button>
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label={
                          p.enabled ? t("action.disable") : t("action.enable")
                        }
                        onClick={() => onToggleProbe(p)}
                      >
                        <Power className="w-4 h-4" aria-hidden="true" />
                      </Button>
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label={t("action.delete")}
                        className="text-rose-600 dark:text-rose-400"
                        onClick={() => onDeleteProbe(p)}
                      >
                        <Trash2 className="w-4 h-4" aria-hidden="true" />
                      </Button>
                    </div>
                  </Td>
                </Tr>
              );
            })}
          </TBody>
        </Table>
      )}
    </Card>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span className="text-sm font-medium text-ink-700 dark:text-surface-4">
        {label}
      </span>
      <div className="mt-1">{children}</div>
      {hint && <p className="mt-1 text-xs text-ink-400">{hint}</p>}
    </label>
  );
}

// ---------- 服务表单 ----------

function ServiceDialog({
  service,
  onClose,
}: {
  service?: ServiceView;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [name, setName] = React.useState(service?.name ?? "");
  const [group, setGroup] = React.useState(service?.group_name ?? "");
  const [description, setDescription] = React.useState(
    service?.description ?? "",
  );
  const [tier, setTier] = React.useState(String(service?.tier ?? 2));

  const save = useMutation({
    mutationFn: () =>
      service
        ? servicesApi.update(service.id, {
            name,
            group_name: group,
            description,
            tier: Number(tier),
          })
        : servicesApi.create({
            name,
            group_name: group,
            description,
            tier: Number(tier),
          }),
    onSuccess: () => {
      toast.push("success", service ? t("services.updated") : t("services.created"));
      void qc.invalidateQueries({ queryKey: ["services"] });
      void qc.invalidateQueries({ queryKey: ["todo"] });
      onClose();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  return (
    <Dialog
      open
      bodyClassName="space-y-4"
      title={
        service ? t("services.dialogEditService") : t("services.dialogCreateService")
      }
      onClose={onClose}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("action.cancel")}
          </Button>
          <Button
            loading={save.isPending}
            disabled={!name.trim()}
            onClick={() => save.mutate()}
          >
            {t("action.save")}
          </Button>
        </>
      }
    >
      <Field label={t("services.formName")}>
        <Input
          value={name}
          placeholder={t("services.formNamePlaceholder")}
          onChange={(e) => setName(e.target.value)}
        />
      </Field>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
        <Field label={t("services.group")}>
          <Input
            value={group}
            placeholder={t("services.formGroupPlaceholder")}
            onChange={(e) => setGroup(e.target.value)}
          />
        </Field>
        <Field label={t("services.tier")}>
          <Select
            value={tier}
            onChange={(e) => setTier(e.target.value)}
          >
            <option value="1">{t("services.tier1")}</option>
            <option value="2">{t("services.tier2")}</option>
            <option value="3">{t("services.tier3")}</option>
          </Select>
        </Field>
      </div>
      <Field label={t("services.description")}>
        <Input
          value={description}
          placeholder={t("services.formDescriptionPlaceholder")}
          onChange={(e) => setDescription(e.target.value)}
        />
      </Field>
    </Dialog>
  );
}

// ---------- 探针表单 ----------

function ProbeDialog({
  serviceId,
  probe,
  onClose,
}: {
  serviceId: string;
  probe?: ProbeView;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();

  const target = React.useMemo(
    () => parseJson<TargetShape>(probe?.target_json ?? "{}", {}),
    [probe],
  );
  const expect = React.useMemo(
    () => parseJson<ExpectShape>(probe?.expect_json ?? "{}", {}),
    [probe],
  );

  const [name, setName] = React.useState(probe?.name ?? "");
  const [kind, setKind] = React.useState(probe?.kind ?? "http");
  const [url, setUrl] = React.useState(target.url ?? "");
  const [method, setMethod] = React.useState(target.method ?? "GET");
  const [host, setHost] = React.useState(target.host ?? "");
  const [port, setPort] = React.useState(String(target.port ?? 0));
  const [sni, setSni] = React.useState(target.sni ?? "");
  const [statusCodes, setStatusCodes] = React.useState(
    (expect.status ?? []).join(", "),
  );
  const [bodyContains, setBodyContains] = React.useState(
    (expect.body_contains ?? []).join(", "),
  );
  const [banner, setBanner] = React.useState(expect.banner_contains ?? "");
  const [maxLatency, setMaxLatency] = React.useState(
    expect.max_latency_ms ? String(expect.max_latency_ms) : "",
  );
  const [minDays, setMinDays] = React.useState(String(expect.min_days_valid ?? 30));
  const [tlsVerify, setTlsVerify] = React.useState(
    kind === "tls"
      ? (expect.verify ?? true)
      : (expect.tls_verify ?? true),
  );
  const [interval, setInterval] = React.useState(
    String(probe?.interval_seconds ?? 60),
  );
  const [timeoutMs, setTimeoutMs] = React.useState(
    String(probe?.timeout_ms ?? 5000),
  );
  const [threshold, setThreshold] = React.useState(
    String(probe?.failure_threshold ?? 3),
  );
  const [nodeId, setNodeId] = React.useState(probe?.node_id ?? "");

  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes });

  const buildPayload = () => {
    const splitList = (raw: string) =>
      raw
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);

    const targetJson: TargetShape = {};
    const expectJson: ExpectShape = {};

    if (kind === "http") {
      targetJson.url = url.trim();
      targetJson.method = method;
    } else {
      targetJson.host = host.trim();
      targetJson.port = Number(port);
      if (kind === "tls" && sni.trim()) targetJson.sni = sni.trim();
    }

    if (kind === "http") {
      const codes = splitList(statusCodes).map(Number).filter((n) => n > 0);
      if (codes.length) expectJson.status = codes;
      const body = splitList(bodyContains);
      if (body.length) expectJson.body_contains = body;
      expectJson.tls_verify = tlsVerify;
    }
    if (kind === "tcp" && banner.trim()) {
      expectJson.banner_contains = banner.trim();
    }
    if (kind === "tls") {
      expectJson.min_days_valid = Number(minDays);
      expectJson.verify = tlsVerify;
    }
    if (maxLatency.trim()) expectJson.max_latency_ms = Number(maxLatency);

    return {
      service_id: serviceId,
      name: name.trim(),
      kind,
      target_json: JSON.stringify(targetJson),
      expect_json: JSON.stringify(expectJson),
      interval_seconds: Number(interval),
      timeout_ms: Number(timeoutMs),
      failure_threshold: Number(threshold),
      node_id: nodeId || null,
      enabled: probe?.enabled ?? true,
    };
  };

  const save = useMutation({
    mutationFn: () => {
      const payload = buildPayload();
      return probe
        ? servicesApi.updateProbe(probe.id, {
            name: payload.name,
            kind: payload.kind,
            target_json: payload.target_json,
            expect_json: payload.expect_json,
            interval_seconds: payload.interval_seconds,
            timeout_ms: payload.timeout_ms,
            failure_threshold: payload.failure_threshold,
            node_id: payload.node_id,
          })
        : servicesApi.createProbe(payload);
    },
    onSuccess: () => {
      toast.push(
        "success",
        probe ? t("services.updated") : t("services.probeCreated"),
      );
      void qc.invalidateQueries({ queryKey: ["services"] });
      void qc.invalidateQueries({ queryKey: ["todo"] });
      onClose();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const targetValid =
    name.trim().length > 0 &&
    (kind === "http" ? url.trim().startsWith("http") : host.trim().length > 0 && Number(port) > 0);

  return (
    <Dialog
      open
      bodyClassName="space-y-4"
      title={probe ? t("services.dialogEditProbe") : t("services.dialogCreateProbe")}
      onClose={onClose}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("action.cancel")}
          </Button>
          <Button
            loading={save.isPending}
            disabled={!targetValid}
            onClick={() => save.mutate()}
          >
            {t("action.save")}
          </Button>
        </>
      }
    >
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
        <Field label={t("services.formProbeName")}>
          <Input
            value={name}
            placeholder={t("services.formProbeNamePlaceholder")}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
        <Field label={t("services.formKind")}>
          <Select
            value={kind}
            onChange={(e) => setKind(e.target.value)}
          >
            <option value="http">HTTP / HTTPS</option>
            <option value="tcp">TCP</option>
            <option value="tls">TLS</option>
          </Select>
        </Field>
      </div>

      {kind === "http" ? (
        <>
          <Field label={t("services.formUrl")}>
            <Input
              value={url}
              placeholder="https://example.com/healthz"
              onChange={(e) => setUrl(e.target.value)}
            />
          </Field>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
            <Field label={t("services.formMethod")}>
              <Select
                value={method}
                onChange={(e) => setMethod(e.target.value)}
              >
                {["GET", "HEAD", "POST"].map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </Select>
            </Field>
            <Field
              label={t("services.formExpectStatus")}
              hint={t("services.formExpectStatusHint")}
            >
              <Input
                value={statusCodes}
                placeholder="200, 204"
                onChange={(e) => setStatusCodes(e.target.value)}
              />
            </Field>
            <Field
              label={t("services.formExpectBody")}
              hint={t("services.formExpectBodyHint")}
            >
              <Input
                value={bodyContains}
                placeholder='"status":"ok"'
                onChange={(e) => setBodyContains(e.target.value)}
              />
            </Field>
          </div>
        </>
      ) : (
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
          <Field label={t("services.formHost")}>
            <Input
              value={host}
              placeholder="127.0.0.1"
              onChange={(e) => setHost(e.target.value)}
            />
          </Field>
          <Field label={t("services.formPort")}>
            <Input
              value={port}
              inputMode="numeric"
              placeholder={kind === "tls" ? "443" : "8080"}
              onChange={(e) => setPort(e.target.value)}
            />
          </Field>
          {kind === "tls" ? (
            <Field label={t("services.formMinDays")}>
              <Input
                value={minDays}
                inputMode="numeric"
                onChange={(e) => setMinDays(e.target.value)}
              />
            </Field>
          ) : (
            <Field label={t("services.formBanner")}>
              <Input
                value={banner}
                placeholder="PostgreSQL"
                onChange={(e) => setBanner(e.target.value)}
              />
            </Field>
          )}
        </div>
      )}

      {kind === "tls" && (
        <Field label={t("services.formSni")}>
          <Input
            value={sni}
            placeholder={host}
            onChange={(e) => setSni(e.target.value)}
          />
        </Field>
      )}

      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
        <Field label={t("services.formInterval")}>
          <Input
            value={interval}
            inputMode="numeric"
            onChange={(e) => setInterval(e.target.value)}
          />
        </Field>
        <Field label={t("services.formTimeout")}>
          <Input
            value={timeoutMs}
            inputMode="numeric"
            onChange={(e) => setTimeoutMs(e.target.value)}
          />
        </Field>
        <Field label={t("services.formThreshold")}>
          <Input
            value={threshold}
            inputMode="numeric"
            onChange={(e) => setThreshold(e.target.value)}
          />
        </Field>
      </div>

      <Field label={t("services.formMaxLatency")}>
        <Input
          value={maxLatency}
          inputMode="numeric"
          placeholder="500"
          onChange={(e) => setMaxLatency(e.target.value)}
        />
      </Field>

      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
        <Field label={t("services.formNode")}>
          <Select
            value={nodeId}
            onChange={(e) => setNodeId(e.target.value)}
          >
            <option value="">{t("services.anyNode")}</option>
            {(nodesQ.data ?? []).map((n) => (
              <option key={n.id} value={n.id}>
                {nodeLabel(n)}
              </option>
            ))}
          </Select>
        </Field>
        {/* 不用 Field 包裹：避免外层 label 与内层 checkbox 的 label 重复朗读 */}
        <div className="block">
          <span className="text-sm font-medium text-ink-700 dark:text-surface-4">
            {t("services.formTlsVerify")}
          </span>
          <label className="mt-1 flex items-center gap-2 h-9 text-sm text-ink-700 dark:text-surface-4">
            <input
              type="checkbox"
              checked={tlsVerify}
              onChange={(e) => setTlsVerify(e.target.checked)}
              className="w-4 h-4 rounded border-surface-3"
            />
            {t("services.formTlsVerify")}
          </label>
        </div>
      </div>
    </Dialog>
  );
}

// ---------- 结果明细 ----------

function ResultsDialog({
  probe,
  tz,
  onClose,
}: {
  probe: ProbeView;
  tz: string;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const resultsQ = useQuery({
    queryKey: ["probe-results", probe.id],
    queryFn: () => servicesApi.results(probe.id, 50),
    refetchInterval: 10000,
  });

  return (
    <Dialog
      open
      bodyClassName="space-y-4"
      title={t("services.resultsTitle", { name: probe.name })}
      onClose={onClose}
      footer={
        <Button variant="secondary" onClick={onClose}>
          {t("action.close")}
        </Button>
      }
    >
      {resultsQ.isLoading ? (
        <Skeleton className="h-24 w-full" />
      ) : (resultsQ.data ?? []).length === 0 ? (
        <EmptyState
          title={t("services.resultsEmpty")}
          description={t("services.resultsEmptyHint")}
        />
      ) : (
        <Table>
          <THead>
            <tr>
              <Th>{t("services.colState")}</Th>
              <Th align="right">{t("services.colLatency")}</Th>
              <Th>HTTP</Th>
              <Th>{t("services.colLastCheck")}</Th>
            </tr>
          </THead>
          <TBody>
            {(resultsQ.data ?? []).map((r: ProbeResultView) => (
              <Tr key={r.ts_unix_nano}>
                <Td>
                  <DotBadge tone={stateTone(r.state)}>
                    {t(`state.${stateKey(r.state)}`)}
                  </DotBadge>
                  {r.error && (
                    <div className="mt-1 text-xs text-ink-400">{r.error}</div>
                  )}
                </Td>
                <Td align="right" className="tabular-nums">
                  {formatLatency(r.latency_ms)}
                </Td>
                <Td>{r.status_code ?? "—"}</Td>
                <Td className="text-xs text-ink-500">
                  {formatTime(r.ts_unix_nano / 1e6, tz)}
                </Td>
              </Tr>
            ))}
          </TBody>
        </Table>
      )}
    </Dialog>
  );
}
