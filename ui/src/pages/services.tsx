import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Globe,
  Network,
  Pencil,
  Plus,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  api,
  servicesApi,
  trendApi,
  type ProbeTestResult,
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
import { Switch } from "@/components/ui/switch";
import {
  cn,
  friendlyError,
  formatTime,
  nodeLabel,
  relativeTime,
} from "@/lib/utils";
import { usePrefs } from "@/components/prefs-provider";

/** 探针类型图标（规范：图标一律 Lucide SVG，禁止 emoji） */
const KIND_ICON: Record<string, React.ElementType> = {
  http: Globe,
  tcp: Network,
  tls: ShieldCheck,
};

/** 状态 → DotBadge tone */
function stateTone(state: string): "success" | "warn" | "danger" | "neutral" {
  if (state === "ok") return "success";
  if (state === "degraded") return "warn";
  if (state === "down") return "danger";
  return "neutral";
}

/** 状态 → i18n key（未知取值统一落到 unknown，避免露出原始字符串） */
function stateKey(state: string): string {
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
 * 服务健康时间线：一条线一个探针，画的是每个时间桶里
 * 「有多少比例的探测是 ok 的」（0–100%）。
 */
function ServicesTimeline() {
  const { t } = useTranslation();
  const [range, setRange] = React.useState<TimeRange>(() => presetRange("3h"));
  const q = useQuery({
    queryKey: ["services-timeline", range.from, range.to],
    queryFn: () => trendApi.servicesTimeline(range.from, range.to, 60, "probe"),
    refetchInterval: 60000,
  });

  const series = (q.data?.series ?? []).map((s) => ({
    name: s.name,
    data: s.points.map((p) => [p.t, p.v] as [number, number]),
  }));

  return (
    <Card>
      <CardHeader
        title={t("services.timelineTitle")}
        description={t("services.timelineSubtitleProbe")}
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
  const [stateFilter, setStateFilter] = React.useState("all");
  const [nodeFilter, setNodeFilter] = React.useState("all");
  const [showDisabled, setShowDisabled] = React.useState(false);
  const [serviceDialog, setServiceDialog] = React.useState<{
    open: boolean;
    service?: ServiceView;
  }>({ open: false });
  const [probeDialog, setProbeDialog] = React.useState<{
    open: boolean;
    serviceId?: string;
    probe?: ProbeView;
  }>({ open: false });
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

  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes });
  /** 节点下拉：按别名（没有则主机名）排序；节点列表变化时重算 */
  const nodeOptions = React.useMemo(
    () =>
      [...(nodesQ.data ?? [])].sort((a, b) =>
        nodeLabel(a).localeCompare(nodeLabel(b)),
      ),
    [nodesQ.data],
  );

  const toggleProbe = useMutation({
    mutationFn: (p: ProbeView) =>
      servicesApi.updateProbe(p.id, { enabled: !p.enabled }),
    onSuccess: (_data, p) => {
      toast.push("success", t(p.enabled ? "services.disabled" : "services.enabled"));
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

  // 扁平化为单层探针表：每个探针带 service_id / service_name / enabled。
  // service.enabled 不直接决定行是否可见——只要探针 enabled 就展示，
  // service 整组停用只是「一组都灰色」含义，探针表里仍要能看到（运维可能要核对）。
  const rows = React.useMemo(
    () =>
      services.flatMap((svc) =>
        svc.probes.map<FlatRow>((p) => ({
          probe: p,
          service_id: svc.id,
          service_name: svc.name,
          service_enabled: svc.enabled,
        })),
      ),
    [services],
  );

  // 顶部大字卡片按**探针**维度统计，且只算「启用中」的：探针停用或整组服务
  // 停用后，它的 down 不该再算进集群故障（列表里仍展示、置灰）。
  const active = rows.filter((r) => r.probe.enabled && r.service_enabled);
  const probesOk = active.filter((r) => r.probe.state.state === "ok").length;
  const degradedCount = active.filter((r) => r.probe.state.state === "degraded")
    .length;
  const downCount = active.filter((r) => r.probe.state.state === "down").length;

  const needle = q.trim().toLowerCase();
  const filtered = React.useMemo(() => {
    let list = rows.filter((r) => {
      if (stateFilter !== "all" && r.probe.state.state !== stateFilter) {
        return false;
      }
      if (nodeFilter !== "all") {
        if (nodeFilter === "__none__") {
          // 「不绑定节点」筛选项：探针的 node_ids 为空
          if (r.probe.node_ids.length > 0) return false;
        } else {
          if (!r.probe.node_ids.includes(nodeFilter)) return false;
        }
      }
      if (!needle) return true;
      return (
        r.service_name.toLowerCase().includes(needle) ||
        r.probe.name.toLowerCase().includes(needle) ||
        targetSummary(r.probe).toLowerCase().includes(needle)
      );
    });

    // 默认隐藏停用的服务和探针
    if (!showDisabled) {
      list = list.filter((r) => r.probe.enabled && r.service_enabled);
    }

    // 停用的服务和探针置底
    return list.sort((a, b) => {
      const aDisabled = !a.probe.enabled || !a.service_enabled;
      const bDisabled = !b.probe.enabled || !b.service_enabled;
      if (aDisabled !== bDisabled) {
        return aDisabled ? 1 : -1;
      }
      // 其他保持原有顺序
      return 0;
    });
  }, [rows, needle, stateFilter, nodeFilter, showDisabled]);

  return (
    <>
      <StatCards
        cards={[
          {
            key: "all",
            label: t("services.cardAll"),
            value: active.length,
            hint: t("services.cardAllHint"),
            tone: "neutral",
            icon: Activity,
          },
          {
            key: "ok",
            label: t("services.cardHealthy"),
            value: probesOk,
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
            {t("services.subtitle")} · {t("services.total", { n: services.length })} ·{" "}
            {t("services.probeCount", { n: rows.length })}
          </p>
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          <SearchInput
            className="w-48 md:w-64"
            placeholder={t("services.searchPlaceholder")}
            aria-label={t("action.search")}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
          <Select
            wrapperClassName="w-32"
            value={stateFilter}
            onChange={(e) => setStateFilter(e.target.value)}
            aria-label={t("services.filterState")}
          >
            <option value="all">{t("services.stateAll")}</option>
            <option value="ok">{t("state.ok")}</option>
            <option value="degraded">{t("state.degraded")}</option>
            <option value="down">{t("state.down")}</option>
          </Select>
          <Select
            wrapperClassName="w-40"
            value={nodeFilter}
            onChange={(e) => setNodeFilter(e.target.value)}
            aria-label={t("services.filterNode")}
          >
            <option value="all">{t("services.nodeAll")}</option>
            <option value="__none__">{t("services.anyNode")}</option>
            {nodeOptions.map((n) => (
              <option key={n.id} value={n.id}>
                {nodeLabel(n)}
              </option>
            ))}
          </Select>
          <div className="flex items-center gap-2">
            <Switch
              checked={showDisabled}
              onCheckedChange={setShowDisabled}
              aria-label={t("services.showDisabled")}
            />
            <span className="text-sm text-ink-600 dark:text-ink-300">
              {t("services.showDisabled")}
            </span>
          </div>
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
          {needle || stateFilter !== "all" || nodeFilter !== "all" ? (
            <SearchEmptyState
              title={t("services.searchEmpty")}
              description={t("services.searchEmptyHint")}
            />
          ) : (
            <EmptyState
              title={t("services.filterEmpty")}
              description={t("services.filterEmptyHint")}
            />
          )}
        </Card>
      ) : (
        <Card>
          <Table>
            <THead>
              <tr>
                <Th>{t("services.colProbeName")}</Th>
                <Th className="hidden sm:table-cell">{t("services.colKind")}</Th>
                <Th className="hidden md:table-cell">{t("services.colTarget")}</Th>
                <Th>{t("services.colNode")}</Th>
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
              {filtered.map((r) => {
                const p = r.probe;
                const Icon = KIND_ICON[p.kind] ?? Activity;
                const state = p.state.state;
                const dimmed = !p.enabled || !r.service_enabled;
                return (
                  <Tr key={p.id} className={cn(dimmed && "opacity-60")}>
                    <Td>
                      <div className="text-sm font-medium text-ink-900 dark:text-surface-0 truncate max-w-[220px]">
                        {p.name}
                      </div>
                      {/* 「服务」列已去掉：所属服务降到探针名下的一行小字，
                          既不再占一整列，又能让搜索服务名有可见的依据。 */}
                      <div className="text-xs text-ink-400 truncate max-w-[220px] flex items-center gap-1">
                        <span className="truncate">{r.service_name}</span>
                        <DotBadge tone="neutral">{t("services.enabled")}</DotBadge>
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
                      <div
                        className="text-xs text-ink-500 truncate max-w-[180px]"
                        title={
                          p.node_labels.length
                            ? p.node_labels.join(", ")
                            : t("services.anyNode")
                        }
                      >
                        {p.node_labels.length
                          ? p.node_labels.join(", ")
                          : t("services.anyNode")}
                      </div>
                    </Td>
                    <Td>
                      <DotBadge tone={stateTone(state)} pulse={state === "ok"}>
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
                          aria-label={t("action.edit")}
                          onClick={() =>
                            setProbeDialog({
                              open: true,
                              serviceId: r.service_id,
                              probe: p,
                            })
                          }
                        >
                          <Pencil className="w-4 h-4" aria-hidden="true" />
                        </Button>
                        <Switch
                          checked={p.enabled}
                          onCheckedChange={() => toggleProbe.mutate(p)}
                          disabled={toggleProbe.isPending}
                          aria-label={p.name}
                        />
                        <Button
                          size="icon"
                          variant="ghost"
                          aria-label={t("action.delete")}
                          className="text-rose-600 dark:text-rose-400"
                          onClick={() =>
                            setPendingDelete({
                              kind: "probe",
                              id: p.id,
                              name: p.name,
                            })
                          }
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
        </Card>
      )}

      {/* 服务整组的操作入口：表头放不下，放到列表上方一行——但目前扁平化之后
          服务作为分组不再有可见层级，「服务」列也已撤掉，所以这层只剩「新建服务」
          一个入口；改名 / 删服务暂时没有 UI 入口（ServiceDialog 与 removeEntity
          都支持，缺的是触发点）。考虑到改动控制面，本轮暂不补；如需要再加。 */}

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

/** 扁平表里的单行：探针本体 + 服务上下文（id/name/enabled） */
interface FlatRow {
  probe: ProbeView;
  service_id: string;
  service_name: string;
  service_enabled: boolean;
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
// tier 字段前端不再暴露：后端保留列与默认值（tier=2），新建时由后端兜底；
// 老数据 tier=1 / 2 / 3 也不会丢，只是不再有 UI 入口去改。

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
  const [description, setDescription] = React.useState(
    service?.description ?? "",
  );

  const save = useMutation({
    mutationFn: () =>
      service
        ? servicesApi.update(service.id, { name, description })
        : servicesApi.create({ name, description }),
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
  const [nodeIds, setNodeIds] = React.useState<string[]>(probe?.node_ids ?? []);
  /** 一次性测试结果；改任何会影响目标的字段就作废（避免显示过期结论） */
  const [testResult, setTestResult] = React.useState<ProbeTestResult | null>(null);

  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes });
  /** 按别名（没有则主机名）排序：勾选列表的顺序也要和「别名优先」一致 */
  const nodeOptions = React.useMemo(
    () =>
      [...(nodesQ.data ?? [])].sort((a, b) =>
        nodeLabel(a).localeCompare(nodeLabel(b)),
      ),
    [nodesQ.data],
  );
  const toggleNode = (id: string) =>
    setNodeIds((cur) =>
      cur.includes(id) ? cur.filter((v) => v !== id) : [...cur, id],
    );

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
      node_ids: nodeIds,
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
            node_ids: payload.node_ids,
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

  /**
   * 一次性测试：只发当前表单里的目标 / 期望，不落库。由 monitor 侧执行一次，
   * 用来在保存前确认「地址写对没、期望配置能不能通过」。
   */
  const testProbe = useMutation({
    mutationFn: () => {
      const p = buildPayload();
      return servicesApi.test({
        kind: p.kind,
        target_json: p.target_json,
        expect_json: p.expect_json,
        timeout_ms: p.timeout_ms,
      });
    },
    onSuccess: (r) => setTestResult(r),
    onError: (e) => {
      setTestResult(null);
      toast.push("error", t(friendlyError(e)));
    },
  });

  // 目标 / 期望项一改，上一次的测试结论就不算数了
  React.useEffect(() => {
    setTestResult(null);
  }, [
    kind,
    url,
    host,
    port,
    sni,
    statusCodes,
    bodyContains,
    banner,
    maxLatency,
    minDays,
    tlsVerify,
    timeoutMs,
  ]);

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
          {/* 测试放左边：它是「先验证再保存」的辅助动作，不该和主按钮争视觉位置 */}
          <Button
            variant="secondary"
            className="mr-auto"
            loading={testProbe.isPending}
            disabled={!targetValid}
            onClick={() => testProbe.mutate()}
          >
            {testProbe.isPending
              ? t("services.testing")
              : t("services.testProbe")}
          </Button>
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

      {/*
        执行节点：可多选、也可一个都不选。
        不选 = 不绑定节点（每台在线节点上都会跑）——这是探针的默认形态；
        选了就只在这些节点上跑，用来做多机对比或只在内网某台机器上探。

        不用 Field 包裹：它渲染成 <label>，把复选框列表套进去后「点标题文字」会
        误选第一个节点（内层 label 也会被重复朗读）。
      */}
      <div className="block">
        <span className="text-sm font-medium text-ink-700 dark:text-surface-4">
          {t("services.formNode")}
        </span>
        <div className="mt-1 rounded-lg border border-surface-3 dark:border-ink-700 max-h-44 overflow-y-auto scrollbar-thin divide-y divide-surface-2 dark:divide-ink-700">
          {nodesQ.isLoading ? (
            <p className="px-3 py-2 text-sm text-ink-400">
              {t("services.formNodeLoading")}
            </p>
          ) : nodeOptions.length === 0 ? (
            <p className="px-3 py-2 text-sm text-ink-400">
              {t("services.formNodeNone")}
            </p>
          ) : (
            nodeOptions.map((n) => {
              const label = nodeLabel(n);
              return (
                <label
                  key={n.id}
                  className="flex items-center gap-2 px-3 py-2 text-sm cursor-pointer hover:bg-surface-1 dark:hover:bg-ink-800"
                >
                  <input
                    type="checkbox"
                    checked={nodeIds.includes(n.id)}
                    onChange={() => toggleNode(n.id)}
                    className="w-4 h-4 rounded border-surface-3"
                  />
                  <span className="truncate text-ink-700 dark:text-surface-4">
                    {label}
                  </span>
                  {label !== n.hostname && (
                    <span className="truncate text-xs text-ink-400">
                      {n.hostname}
                    </span>
                  )}
                </label>
              );
            })
          )}
        </div>
        <p className="mt-1 text-xs text-ink-400">
          {nodeIds.length === 0
            ? t("services.formNodeAny")
            : t("services.formNodeBound", { n: nodeIds.length })}
        </p>
      </div>

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

      {/* 测试结论：reason 由后端给，文案在这里选，保证中英都通 */}
      {testResult && (
        <div
          className={cn(
            "rounded-lg border px-3 py-2",
            testResult.state === "ok"
              ? "border-emerald-200 dark:border-emerald-700/40"
              : testResult.state === "degraded"
                ? "border-amber-200 dark:border-amber-700/40"
                : "border-rose-200 dark:border-rose-700/40",
          )}
        >
          <div className="flex flex-wrap items-center gap-2">
            <DotBadge
              tone={
                testResult.state === "ok"
                  ? "success"
                  : testResult.state === "degraded"
                    ? "warn"
                    : "danger"
              }
            >
              {t(
                testResult.state === "ok"
                  ? "services.testPassed"
                  : testResult.state === "degraded"
                    ? "services.testDegraded"
                    : "services.testFailed",
              )}
            </DotBadge>
            {testResult.latency_ms !== null && (
              <span className="text-xs text-ink-400 tabular-nums">
                {formatLatency(testResult.latency_ms)}
              </span>
            )}
            {testResult.status_code !== null && (
              <span className="text-xs text-ink-400 tabular-nums">
                HTTP {testResult.status_code}
              </span>
            )}
          </div>
          <p className="mt-1 text-sm text-ink-700 dark:text-surface-4 break-all">
            {testReasonText(testResult, t)}
          </p>
        </div>
      )}
      <p className="text-xs text-ink-400">{t("services.testHint")}</p>
    </Dialog>
  );
}

/** 把后端的 reason/args 翻成人话；未知原因码原样带出来，便于排查 */
function testReasonText(r: ProbeTestResult, t: TFunction): string {
  const arg = (k: string) => (r.args?.[k] == null ? "" : String(r.args[k]));
  const expected = Array.isArray(r.args?.expected)
    ? (r.args.expected as unknown[]).join(", ")
    : "";
  switch (r.reason) {
    case "ok":
      return t("services.testReasonOk");
    case "timeout":
      return t("services.testReasonTimeout", { ms: arg("ms") });
    case "connect":
      return t("services.testReasonConnect", {
        target: arg("target"),
        detail: arg("detail"),
      });
    case "bad_url":
      return t("services.testReasonBadUrl");
    case "status":
      return t("services.testReasonStatus", {
        got: r.status_code ?? arg("got"),
        expected,
      });
    case "body":
      return t("services.testReasonBody", { needle: arg("needle") });
    case "latency":
      return t("services.testReasonLatency", {
        ms: arg("ms"),
        threshold: arg("threshold"),
      });
    case "banner":
      return t("services.testReasonBanner", { needle: arg("needle") });
    case "tls":
      return t("services.testReasonTls", { detail: arg("detail") });
    case "cert_expired":
      return t("services.testReasonCertExpired", { days: arg("days") });
    case "cert_days":
      return t("services.testReasonCertDays", {
        days: arg("days"),
        min: arg("min"),
      });
    case "unsupported":
      return t("services.testReasonUnsupported", { kind: arg("kind") });
    default:
      return t("services.testReasonUnknown", { reason: r.reason });
  }
}
