/**
 * Probes page — manage HTTP/TCP/TLS health checks.
 */
import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Activity,
  Globe,
  Network,
  Pencil,
  Plus,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  api,
  probesApi,
  trendApi,
  type ProbeTestResult,
  type ProbeView,
} from "@/api";
import { LineChart } from "@/components/chart";
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
import { Switch } from "@/components/ui/switch";
import { useToast } from "@/components/ui/toast";
import { cn, friendlyError } from "@/lib/utils";

interface TargetShape {
  url?: string;
  method?: string;
  host?: string;
  port?: number;
  sni?: string;
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

/** Compact target display */
function targetSummary(probe: ProbeView): string {
  const target = parseJson<TargetShape>(probe.target_json, {});
  if (probe.kind === "http") return target.url ?? "—";
  const host = target.host ?? "—";
  return `${host}:${target.port ?? ""}`.replace(/:$/, "");
}

/**
 * Probe health timeline: one line per probe.
 */
function ProbesTimeline() {
  const { t } = useTranslation();
  const [range, setRange] = React.useState<TimeRange>(() => presetRange("3h"));
  const q = useQuery({
    queryKey: ["probes-timeline", range.from, range.to],
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
        title={t("probes.timelineTitle")}
        description={t("probes.timelineSubtitleProbe")}
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
  const qc = useQueryClient();
  const toast = useToast();

  const [q, setQ] = React.useState("");
  const [stateFilter, setStateFilter] = React.useState("all");
  const [nodeFilter, setNodeFilter] = React.useState("all");
  const [showDisabled, setShowDisabled] = React.useState(false);
  const [probeDialog, setProbeDialog] = React.useState<{
    open: boolean;
    probe?: ProbeView;
  }>({ open: false });
  const [pendingDelete, setPendingDelete] = React.useState<{
    kind: "probe";
    id: string;
    name: string;
  } | null>(null);

  const probesQ = useQuery({
    queryKey: ["probes"],
    queryFn: probesApi.list,
  });

  const nodesQ = useQuery({
    queryKey: ["nodes"],
    queryFn: api.nodes,
  });

  const nodeOptions = React.useMemo(
    () => [...(nodesQ.data ?? [])],
    [nodesQ.data],
  );

  const nodeLabel = (n: { alias?: string; hostname: string }) =>
    n.alias || n.hostname;

  // Filter probes
  const filtered = React.useMemo(() => {
    let list = probesQ.data ?? [];
    const needle = q.trim().toLowerCase();
    if (needle) {
      list = list.filter(
        (p) =>
          p.name.toLowerCase().includes(needle) ||
          p.description.toLowerCase().includes(needle) ||
          p.kind.toLowerCase().includes(needle),
      );
    }
    if (stateFilter !== "all") {
      list = list.filter((p) => p.state.state === stateFilter);
    }
    if (nodeFilter !== "all") {
      if (nodeFilter === "__none__") {
        list = list.filter((p) => p.node_ids.length === 0);
      } else {
        list = list.filter((p) => p.node_ids.includes(nodeFilter));
      }
    }
    if (!showDisabled) {
      list = list.filter((p) => p.enabled);
    }
    return list;
  }, [probesQ.data, q, stateFilter, nodeFilter, showDisabled]);

  const toggleProbe = useMutation({
    mutationFn: (p: ProbeView) =>
      probesApi.update(p.id, { enabled: !p.enabled }),
    onSuccess: () => {
      toast.push("success", t("probes.updated"));
      void qc.invalidateQueries({ queryKey: ["probes"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const removeEntity = useMutation({
    mutationFn: ({
      id,
    }: {
      id: string;
    }) => probesApi.remove(id),
    onSuccess: () => {
      toast.push("success", t("probes.deleted"));
      void qc.invalidateQueries({ queryKey: ["probes"] });
      void qc.invalidateQueries({ queryKey: ["todo"] });
      setPendingDelete(null);
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const KIND_ICON: Record<string, React.ElementType> = {
    http: Globe,
    tcp: Network,
    tls: ShieldCheck,
  };

  return (
    <>
      <ProbesTimeline />

      {/* Toolbar */}
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h2 className="text-lg font-semibold text-ink-900 dark:text-surface-0">
            {t("probes.title")}
          </h2>
          <p className="text-sm text-ink-500 mt-0.5">
            {t("probes.total", { n: filtered.length })}
          </p>
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          <SearchInput
            className="w-48 md:w-64"
            placeholder={t("probes.searchPlaceholder")}
            aria-label={t("action.search")}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
          <Select
            wrapperClassName="w-32"
            value={stateFilter}
            onChange={(e) => setStateFilter(e.target.value)}
            aria-label={t("probes.filterState")}
          >
            <option value="all">{t("probes.stateAll")}</option>
            <option value="ok">{t("state.ok")}</option>
            <option value="degraded">{t("state.degraded")}</option>
            <option value="down">{t("state.down")}</option>
          </Select>
          <Select
            wrapperClassName="w-40"
            value={nodeFilter}
            onChange={(e) => setNodeFilter(e.target.value)}
            aria-label={t("probes.filterNode")}
          >
            <option value="all">{t("probes.nodeAll")}</option>
            <option value="__none__">{t("probes.anyNode")}</option>
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
              aria-label={t("probes.showDisabled")}
            />
            <span className="text-sm text-ink-600 dark:text-ink-300">
              {t("probes.showDisabled")}
            </span>
          </div>
          <Button onClick={() => setProbeDialog({ open: true })}>
            <Plus className="w-4 h-4" aria-hidden="true" />
            {t("probes.newProbe")}
          </Button>
        </div>
      </div>

      {probesQ.isLoading ? (
        <Card>
          <CardBody>
            <Skeleton className="h-24 w-full" />
          </CardBody>
        </Card>
      ) : probesQ.isError ? (
        <Card>
          <ErrorState
            message={t(friendlyError(probesQ.error))}
            onRetry={() => void probesQ.refetch()}
            retrying={probesQ.isFetching}
          />
        </Card>
      ) : filtered.length === 0 ? (
        <Card>
          {q || stateFilter !== "all" || nodeFilter !== "all" ? (
            <SearchEmptyState
              title={t("probes.searchEmpty")}
              description={t("probes.searchEmptyHint")}
            />
          ) : (
            <EmptyState
              title={t("probes.empty")}
              description={t("probes.emptyHint")}
            />
          )}
        </Card>
      ) : (
        <Card>
          <Table>
            <THead>
              <tr>
                <Th>{t("probes.colProbeName")}</Th>
                <Th className="hidden sm:table-cell">{t("probes.colKind")}</Th>
                <Th className="hidden md:table-cell">{t("probes.colTarget")}</Th>
                <Th className="hidden lg:table-cell">{t("probes.colNode")}</Th>
                <Th>{t("probes.colState")}</Th>
                <Th align="right" className="hidden xl:table-cell">
                  {t("probes.colLatency")}
                </Th>
                <Th className="hidden 2xl:table-cell">
                  {t("probes.colLastCheck")}
                </Th>
                <Th align="right">{t("probes.colActions")}</Th>
              </tr>
            </THead>
            <TBody>
              {filtered.map((p) => {
                const Icon = KIND_ICON[p.kind] ?? Activity;
                const state = p.state.state;
                const dimmed = !p.enabled;
                return (
                  <Tr key={p.id} className={cn(dimmed && "opacity-60")}>
                    <Td>
                      <div className="text-sm font-medium text-ink-900 dark:text-surface-0 truncate max-w-[220px]">
                        {p.name}
                      </div>
                      {p.description && (
                        <div className="text-xs text-ink-400 truncate max-w-[220px]">
                          {p.description}
                        </div>
                      )}
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
                    <Td className="hidden lg:table-cell">
                      <span className="text-xs text-ink-500 truncate max-w-[120px]" title={p.node_labels.join(", ") || "—"}>
                        {p.node_labels.join(", ") || "—"}
                      </span>
                    </Td>
                    <Td>
                      <DotBadge tone={state === "ok" ? "success" : state === "degraded" ? "warn" : "danger"}>
                        {t(`state.${state}`)}
                      </DotBadge>
                      {p.state.last_error && (
                        <div
                          className="mt-1 text-xs text-ink-400 truncate max-w-[200px]"
                          title={p.state.last_error}
                        >
                          {p.state.last_error}
                        </div>
                      )}
                    </Td>
                    <Td align="right" className="hidden xl:table-cell">
                      <span className="text-sm text-ink-500">
                        {p.state.last_latency_ms != null
                          ? `${p.state.last_latency_ms.toFixed(1)} ms`
                          : "—"}
                      </span>
                    </Td>
                    <Td className="hidden 2xl:table-cell">
                      <span className="text-xs text-ink-500">
                        {p.state.last_check_at_unix_nano
                          ? new Date(
                              p.state.last_check_at_unix_nano / 1_000_000,
                            ).toLocaleTimeString()
                          : t("probes.never")}
                      </span>
                    </Td>
                    <Td align="right">
                      <div className="flex items-center justify-end gap-1">
                        <Button
                          size="icon"
                          variant="ghost"
                          aria-label={t("action.edit")}
                          onClick={() =>
                            setProbeDialog({ open: true, probe: p })
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

      {probeDialog.open && (
        <ProbeDialog
          probe={probeDialog.probe}
          onClose={() => setProbeDialog({ open: false })}
        />
      )}

      <ConfirmDialog
        open={!!pendingDelete}
        danger
        title={t("probes.deleteProbeTitle")}
        message={t("probes.deleteProbeMessage", {
          name: pendingDelete?.name,
        })}
        confirmLabel={t("probes.deleteProbeConfirm")}
        cancelLabel={t("action.cancel")}
        loading={removeEntity.isPending}
        onCancel={() => setPendingDelete(null)}
        onConfirm={() =>
          pendingDelete &&
          removeEntity.mutate({
            id: pendingDelete.id,
          })
        }
      />
    </>
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

// ---------- Probe form ----------
function ProbeDialog({
  probe,
  onClose,
}: {
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
  const [description, setDescription] = React.useState(probe?.description ?? "");
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
  const [testResult, setTestResult] = React.useState<ProbeTestResult | null>(null);

  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes });
  const nodeOptions = React.useMemo(
    () =>
      [...(nodesQ.data ?? [])].sort((a, b) =>
        (a.alias || a.hostname).localeCompare(b.alias || b.hostname),
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

    return {
      name: name.trim(),
      description: description.trim(),
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
        ? probesApi.update(probe.id, {
            name: payload.name,
            description: payload.description,
            kind: payload.kind,
            target_json: payload.target_json,
            expect_json: payload.expect_json,
            interval_seconds: payload.interval_seconds,
            timeout_ms: payload.timeout_ms,
            failure_threshold: payload.failure_threshold,
            node_ids: payload.node_ids,
          })
        : probesApi.create(payload);
    },
    onSuccess: () => {
      toast.push(
        "success",
        probe ? t("probes.updated") : t("probes.probeCreated"),
      );
      void qc.invalidateQueries({ queryKey: ["probes"] });
      void qc.invalidateQueries({ queryKey: ["todo"] });
      onClose();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const testProbe = useMutation({
    mutationFn: () => {
      const p = buildPayload();
      return probesApi.test({
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
      title={probe ? t("probes.dialogEditProbe") : t("probes.dialogCreateProbe")}
      onClose={onClose}
      footer={
        <>
          <Button
            variant="secondary"
            className="mr-auto"
            onClick={() => testProbe.mutate()}
            disabled={!targetValid || testProbe.isPending}
          >
            {testProbe.isPending ? t("probes.testing") : t("probes.testProbe")}
          </Button>
          <Button variant="ghost" onClick={onClose}>
            {t("action.cancel")}
          </Button>
          <Button
            onClick={() => save.mutate()}
            disabled={!targetValid || save.isPending}
          >
            {save.isPending ? t("state.loading") : t("action.save")}
          </Button>
        </>
      }
    >
      <Field label={t("probes.formName")} hint={t("probes.formNamePlaceholder")}>
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("probes.formNamePlaceholder")}
        />
      </Field>

      <Field label={t("probes.description")} hint={t("probes.formDescriptionPlaceholder")}>
        <Input
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          placeholder={t("probes.formDescriptionPlaceholder")}
        />
      </Field>

      <Field label={t("probes.formKind")}>
        <Select value={kind} onChange={(e) => setKind(e.target.value)}>
          <option value="http">HTTP</option>
          <option value="tcp">TCP</option>
          <option value="tls">TLS</option>
        </Select>
      </Field>

      {kind === "http" && (
        <>
          <Field label={t("probes.formUrl")}>
            <Input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://example.com/health"
            />
          </Field>
          <Field label={t("probes.formMethod")}>
            <Select value={method} onChange={(e) => setMethod(e.target.value)}>
              <option value="GET">GET</option>
              <option value="POST">POST</option>
              <option value="PUT">PUT</option>
              <option value="DELETE">DELETE</option>
            </Select>
          </Field>
          <Field label={t("probes.formExpectStatus")} hint={t("probes.formExpectStatusHint")}>
            <Input
              value={statusCodes}
              onChange={(e) => setStatusCodes(e.target.value)}
              placeholder="200, 201"
            />
          </Field>
          <Field label={t("probes.formExpectBody")} hint={t("probes.formExpectBodyHint")}>
            <Input
              value={bodyContains}
              onChange={(e) => setBodyContains(e.target.value)}
              placeholder='"ok", "healthy"'
            />
          </Field>
          <Field label={t("probes.formTlsVerify")}>
            <Switch checked={tlsVerify} onCheckedChange={setTlsVerify} aria-label={t("probes.formTlsVerify")} />
          </Field>
        </>
      )}

      {kind === "tcp" && (
        <>
          <div className="grid grid-cols-2 gap-4">
            <Field label={t("probes.formHost")}>
              <Input
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="example.com"
              />
            </Field>
            <Field label={t("probes.formPort")}>
              <Input
                type="number"
                value={port}
                onChange={(e) => setPort(e.target.value)}
                placeholder="443"
              />
            </Field>
          </div>
          <Field label={t("probes.formBanner")} hint={t("probes.formExpectBodyHint")}>
            <Input
              value={banner}
              onChange={(e) => setBanner(e.target.value)}
              placeholder={t("probes.formBanner")}
            />
          </Field>
        </>
      )}

      {kind === "tls" && (
        <>
          <div className="grid grid-cols-2 gap-4">
            <Field label={t("probes.formHost")}>
              <Input
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="example.com"
              />
            </Field>
            <Field label={t("probes.formPort")}>
              <Input
                type="number"
                value={port}
                onChange={(e) => setPort(e.target.value)}
                placeholder="443"
              />
            </Field>
          </div>
          <Field label={t("probes.formSni")}>
            <Input
              value={sni}
              onChange={(e) => setSni(e.target.value)}
              placeholder={t("probes.formSni")}
            />
          </Field>
          <Field label={t("probes.formMinDays")}>
            <Input
              type="number"
              value={minDays}
              onChange={(e) => setMinDays(e.target.value)}
            />
          </Field>
          <Field label={t("probes.formTlsVerify")}>
            <Switch checked={tlsVerify} onCheckedChange={setTlsVerify} aria-label={t("probes.formTlsVerify")} />
          </Field>
        </>
      )}

      <div className="grid grid-cols-3 gap-4">
        <Field label={t("probes.formInterval")}>
          <Input
            type="number"
            value={interval}
            onChange={(e) => setInterval(e.target.value)}
          />
        </Field>
        <Field label={t("probes.formTimeout")}>
          <Input
            type="number"
            value={timeoutMs}
            onChange={(e) => setTimeoutMs(e.target.value)}
          />
        </Field>
        <Field label={t("probes.formThreshold")}>
          <Input
            type="number"
            value={threshold}
            onChange={(e) => setThreshold(e.target.value)}
          />
        </Field>
      </div>

      <Field label={t("probes.formNode")}>
        {nodesQ.isLoading ? (
          <p className="text-sm text-ink-400">{t("probes.formNodeLoading")}</p>
        ) : nodeOptions.length === 0 ? (
          <p className="text-sm text-ink-400">{t("probes.formNodeNone")}</p>
        ) : (
          <div className="space-y-1">
            <p className="text-xs text-ink-400">{t("probes.formNodeAny")}</p>
            {nodeOptions.map((n) => (
              <label key={n.id} className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={nodeIds.includes(n.id)}
                  onChange={() => toggleNode(n.id)}
                />
                <span className="text-sm">{n.alias || n.hostname}</span>
              </label>
            ))}
          </div>
        )}
      </Field>

      {testResult && (
        <div
          className={cn(
            "p-3 rounded text-sm",
            testResult.state === "ok"
              ? "bg-emerald-50 dark:bg-emerald-900/20 text-emerald-700 dark:text-emerald-400"
              : testResult.state === "degraded"
                ? "bg-amber-50 dark:bg-amber-900/20 text-amber-700 dark:text-amber-400"
                : "bg-rose-50 dark:bg-rose-900/20 text-rose-700 dark:text-rose-400",
          )}
        >
          <p className="font-medium">
            {testResult.state === "ok"
              ? t("probes.testPassed")
              : testResult.state === "degraded"
                ? t("probes.testDegraded")
                : t("probes.testFailed")}
          </p>
          {testResult.latency_ms != null && (
            <p className="text-xs mt-1">
              Latency: {testResult.latency_ms.toFixed(1)} ms
            </p>
          )}
        </div>
      )}
    </Dialog>
  );
}
