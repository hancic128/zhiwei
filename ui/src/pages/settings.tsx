import * as React from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Clock,
  Copy,
  Eye,
  EyeOff,
  HelpCircle,
  Info,
  KeyRound,
  Network,
  Pencil,
  Plus,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  aiTokens,
  alertsApi,
  enrollTokens,
  retentionApi,
  setToken,
  settingsApi,
  type AiTokenMeta,
  type EnrollTokenMeta,
  type NotifyChannel,
} from "@/api";
import { AiTokenDialog } from "@/components/ai-token-dialog";
import { EnrollTokenDialog } from "@/components/enroll-token-dialog";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { EmptyState, ErrorState, Skeleton } from "@/components/ui/feedback";
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
import { usePrefs } from "@/components/prefs-provider";
import { cn, copyText, formatTime, friendlyError, TIMEZONES } from "@/lib/utils";
import type { Locale } from "@/lib/prefs";

export function Settings() {
  const { t } = useTranslation();
  return (
    <div className="space-y-4 md:space-y-6">
      <UiSection />
      <RetentionSection />
      <ChannelsSection />
      <CredentialSection />
      <EnrollTokensSection />
      <McpSection />
      <CaSection />
      <p className="text-xs text-ink-400">{t("settings.caWarn")}</p>
    </div>
  );
}

function RetentionSection() {
  const { t } = useTranslation();
  const toast = useToast();
  const q = useQuery({ queryKey: ["retention"], queryFn: retentionApi.get });
  const update = useMutation({
    mutationFn: (params: { raw?: number; hourly?: number; alerts?: number }) =>
      retentionApi.update(params),
    onSuccess: () => {
      toast.push("success", t("settings.retentionSaved"));
      void q.refetch();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const [editRaw, setEditRaw] = React.useState<number | null>(null);
  const [editHourly, setEditHourly] = React.useState<number | null>(null);
  const [editAlert, setEditAlert] = React.useState<number | null>(null);

  React.useEffect(() => {
    if (q.data) {
      setEditRaw(q.data.raw_retention_days.value);
      setEditHourly(q.data.hourly_retention_days.value);
      setEditAlert(q.data.alert_retention_days.value);
    }
  }, [q.data]);

  const handleSave = () => {
    update.mutate({
      raw: editRaw ?? undefined,
      hourly: editHourly ?? undefined,
      alerts: editAlert ?? undefined,
    });
  };

  const isDirty = editRaw !== q.data?.raw_retention_days.value ||
    editHourly !== q.data?.hourly_retention_days.value ||
    editAlert !== q.data?.alert_retention_days.value;

  return (
    <Card>
      <CardHeader
        icon={<Clock className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("retention.title")}
        description={t("retention.subtitle")}
      />
      <CardBody>
        {q.isLoading ? (
          <Skeleton className="h-24 w-full" />
        ) : q.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : q.data ? (
          <div className="space-y-4">
            <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 text-sm">
              <div>
                <p className="text-xs text-ink-400">{t("retention.rawTitle")}</p>
                <div className="flex items-center gap-2 mt-1">
                  <input
                    type="number"
                    min="1"
                    max="3650"
                    value={editRaw ?? ""}
                    onChange={(e) => setEditRaw(parseInt(e.target.value) || null)}
                    className="w-20 px-2 py-1 text-sm border rounded bg-surface-1 dark:bg-ink-700 border-surface-3 dark:border-ink-600"
                  />
                  <span className="text-ink-500">{t("retention.daysUnit")}</span>
                </div>
                <p className="text-xs text-ink-400 mt-1">{t("retention.rawHint")}</p>
                <p className="text-xs text-ink-500 mt-1">{t(q.data.raw_retention_days.description)}</p>
                <p className="text-xs text-brand-600 dark:text-brand-400 mt-1">{t(q.data.raw_retention_days.when_effective)}</p>
              </div>
              <div>
                <p className="text-xs text-ink-400">{t("retention.hourlyTitle")}</p>
                <div className="flex items-center gap-2 mt-1">
                  <input
                    type="number"
                    min="1"
                    max="3650"
                    value={editHourly ?? ""}
                    onChange={(e) => setEditHourly(parseInt(e.target.value) || null)}
                    className="w-20 px-2 py-1 text-sm border rounded bg-surface-1 dark:bg-ink-700 border-surface-3 dark:border-ink-600"
                  />
                  <span className="text-ink-500">{t("retention.daysUnit")}</span>
                </div>
                <p className="text-xs text-ink-400 mt-1">{t("retention.hourlyHint")}</p>
                <p className="text-xs text-ink-500 mt-1">{t(q.data.hourly_retention_days.description)}</p>
                <p className="text-xs text-brand-600 dark:text-brand-400 mt-1">{t(q.data.hourly_retention_days.when_effective)}</p>
              </div>
              <div>
                <p className="text-xs text-ink-400">{t("retention.alertTitle")}</p>
                <div className="flex items-center gap-2 mt-1">
                  <input
                    type="number"
                    min="1"
                    max="3650"
                    value={editAlert ?? ""}
                    onChange={(e) => setEditAlert(parseInt(e.target.value) || null)}
                    className="w-20 px-2 py-1 text-sm border rounded bg-surface-1 dark:bg-ink-700 border-surface-3 dark:border-ink-600"
                  />
                  <span className="text-ink-500">{t("retention.daysUnit")}</span>
                </div>
                <p className="text-xs text-ink-400 mt-1">{t("retention.alertDaysHint")}</p>
                <p className="text-xs text-ink-500 mt-1">{t(q.data.alert_retention_days.description)}</p>
                <p className="text-xs text-brand-600 dark:text-brand-400 mt-1">{t(q.data.alert_retention_days.when_effective)}</p>
              </div>
            </div>
            {isDirty && (
              <div className="flex justify-end">
                <Button
                  onClick={handleSave}
                  loading={update.isPending}
                  size="sm"
                >
                  {t("action.save")}
                </Button>
              </div>
            )}
          </div>
        ) : null}
      </CardBody>
    </Card>
  );
}

function CaSection() {
  const { t } = useTranslation();
  const { timezone } = usePrefs();
  const toast = useToast();
  const caQ = useQuery({ queryKey: ["ca"], queryFn: settingsApi.ca });

  return (
    <Card>
      <CardHeader
        icon={<ShieldCheck className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("settings.caTitle")}
        description={t("settings.caSubtitle")}
      />
      <CardBody>
        {caQ.isLoading ? (
          <Skeleton className="h-24 w-full" />
        ) : caQ.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(caQ.error))}
            onRetry={() => void caQ.refetch()}
            retrying={caQ.isFetching}
          />
        ) : caQ.data ? (
          <div className="space-y-4">
            {/* When TLS is terminated at the edge, this CA plays no role — don't make it look like the "cluster identity root" */}
            {!caQ.data.tls_terminated_locally && (
              <div className="flex items-start gap-2 rounded-lg bg-surface-2 dark:bg-ink-700/60 px-3 py-2">
                <Info
                  className="w-4 h-4 mt-0.5 shrink-0 text-ink-400"
                  aria-hidden="true"
                />
                <p className="text-xs text-ink-500">{t("settings.caEdgeNote")}</p>
              </div>
            )}
          <dl className="grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-4 text-sm">
            <Row label={t("settings.caSubject")} value={caQ.data.subject} />
            <Row
              label={t("settings.caSerial")}
              value={caQ.data.serial}
              mono
            />
            <Row
              label={t("settings.caValid")}
              value={`${formatTime(caQ.data.not_before_unix_nano / 1e6, timezone)} → ${formatTime(
                caQ.data.not_after_unix_nano / 1e6,
                timezone,
              )}`}
            />
            <Row
              label={t("settings.caNodes")}
              value={String(caQ.data.nodes_enrolled)}
              mono
            />
            <div className="sm:col-span-2 min-w-0">
              <dt className="text-xs text-ink-400">
                {t("settings.caFingerprint")}
              </dt>
              <dd className="mt-1 flex items-start gap-2">
                <code className="flex-1 min-w-0 break-all rounded bg-surface-2 dark:bg-ink-700/60 px-2 py-1 text-xs text-ink-700 dark:text-surface-4">
                  {caQ.data.fingerprint_sha256}
                </code>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={t("settings.caCopy")}
                  onClick={() => {
                    void copyText(caQ.data!.fingerprint_sha256).then((ok) =>
                      toast.push(
                        ok ? "success" : "error",
                        t(ok ? "settings.caCopied" : "toast.copyFailed"),
                      ),
                    );
                  }}
                >
                  <Copy className="w-4 h-4" aria-hidden="true" />
                </Button>
              </dd>
            </div>
          </dl>
          </div>
        ) : null}
      </CardBody>
    </Card>
  );
}

/** Feishu receive ID types (synced with backend FEISHU_RECEIVE_ID_TYPES) */
const RECEIVE_ID_TYPES = [
  "chat_id",
  "open_id",
  "user_id",
  "union_id",
  "email",
] as const;

/** Form row: label + input + optional hint (channel form assembles fields by type, many fields but no extra behavior) */
function Field({
  label,
  hint,
  ...input
}: { label: string; hint?: string } & React.ComponentProps<typeof Input>) {
  return (
    <label className="block">
      <span className="block text-xs text-ink-500 mb-1">{label}</span>
      <Input {...input} />
      {hint ? (
        <span className="mt-1 block text-xs text-ink-400">{hint}</span>
      ) : null}
    </label>
  );
}

function ChannelsSection() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const chQ = useQuery({ queryKey: ["channels"], queryFn: alertsApi.channels });

  const EMPTY = {
    name: "",
    kind: "feishu",
    url: "",
    /** Generic webhook token */
    secret: "",
    /** Feishu App Secret (stored separately from the Token above so switching type doesn't send credentials to the wrong field) */
    app_secret: "",
    app_id: "",
    receive_id: "",
    receive_id_type: "chat_id",
    min_severity: "info",
  };
  const [dialogOpen, setDialogOpen] = React.useState(false);
  const [form, setForm] = React.useState(EMPTY);
  /** Non-null = editing this channel (save uses PATCH instead of POST) */
  const [editing, setEditing] = React.useState<NotifyChannel | null>(null);
  const [confirmDelete, setConfirmDelete] = React.useState<NotifyChannel | null>(
    null,
  );
  const [testing, setTesting] = React.useState(false);
  /** App Secret shown in plaintext */
  const [secretRevealed, setSecretRevealed] = React.useState(false);

  const invalidate = () => void qc.invalidateQueries({ queryKey: ["channels"] });

  /** When editing, prefill form with existing channel values: credentials included (the list endpoint returns them in plaintext),
   *  and saved back as-is, no need for the user to re-enter. */
  const formOf = (c: NotifyChannel) => ({
    name: c.name,
    kind: c.kind,
    url: c.url,
    secret: c.kind === "feishu" ? "" : c.secret,
    app_secret: c.kind === "feishu" ? c.secret : "",
    app_id: c.app_id,
    receive_id: c.receive_id,
    receive_id_type: c.receive_id_type || "chat_id",
    min_severity: c.min_severity || "info",
  });
  const openCreate = () => {
    setEditing(null);
    setForm(EMPTY);
    setSecretRevealed(false);
    setDialogOpen(true);
  };
  const openEdit = (c: NotifyChannel) => {
    setEditing(c);
    setForm(formOf(c));
    setSecretRevealed(false);
    setDialogOpen(true);
  };
  const closeDialog = () => {
    setDialogOpen(false);
    setEditing(null);
  };

  const isFeishu = form.kind === "feishu";
  const isSlack = form.kind === "slack";
  const isBluebird = form.kind === "bluebird";
  /** Form → API fields. Backend's `secret` column is reused by type: feishu = App Secret, bluebird / generic webhook = Token */
  const payload = {
    name: form.name.trim(),
    kind: form.kind,
    url: isFeishu ? "" : form.url.trim(),
    secret: isFeishu ? form.app_secret.trim() : isSlack ? "" : form.secret.trim(),
    app_id: isFeishu ? form.app_id.trim() : "",
    receive_id: isFeishu ? form.receive_id.trim() : "",
    receive_id_type: form.receive_id_type,
    min_severity: form.min_severity,
  };

  const create = useMutation({
    mutationFn: () =>
      editing
        ? settingsApi.updateChannel(editing.id, payload)
        : settingsApi.createChannel(payload),
    onSuccess: () => {
      toast.push(
        "success",
        t(editing ? "settings.chUpdated" : "settings.chCreated"),
      );
      setForm(EMPTY);
      closeDialog();
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  /** Send with current params to actually test — know if the address is right before saving */
  const test = async () => {
    setTesting(true);
    try {
      const r = await settingsApi.testChannel(payload);
      if (r.ok) toast.push("success", t("settings.chTestOk"));
      else toast.push("error", t("settings.chTestFail", { detail: r.detail }));
    } catch (e) {
      toast.push("error", t(friendlyError(e)));
    } finally {
      setTesting(false);
    }
  };

  const toggle = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) =>
      settingsApi.toggleChannel(id, enabled),
    onSuccess: invalidate,
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });
  const remove = useMutation({
    mutationFn: (id: number) => settingsApi.deleteChannel(id),
    onSuccess: () => {
      toast.push("success", t("settings.chDeleted"));
      setConfirmDelete(null);
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const channels = chQ.data ?? [];
  // Each channel type needs different fields: feishu uses app credentials + receive ID,
  // bluebird needs URL + Token, Slack / generic webhook just needs URL
  const canSubmit =
    !!form.name.trim() &&
    (isFeishu
      ? !!form.app_id.trim() && !!form.app_secret.trim() && !!form.receive_id.trim()
      : isBluebird
        ? !!form.url.trim() && !!form.secret.trim()
        : !!form.url.trim());

  /** List "target" column: feishu has no URL, show where it sends to */
  const targetOf = (c: NotifyChannel) =>
    c.kind === "feishu"
      ? `${t(`settings.chRid_${c.receive_id_type}`)} · ${c.receive_id}`
      : c.url;

  /** Credential status: Slack's URL is itself the credential, nothing to label */
  const secretHint = (c: NotifyChannel) =>
    c.kind === "feishu"
      ? // Legacy custom bot channels only have url + signing secret; don't call that secret "App Secret"
        t(
          c.app_id && c.secret
            ? "settings.chAppSecretSet"
            : "settings.chAppSecretUnset",
        )
      : c.kind === "webhook" || c.kind === "bluebird"
        ? t(c.secret ? "settings.chTokenSet" : "settings.chTokenUnset")
        : "";

  return (
    <>
      <TableShell>
        <TableToolbar>
          <div>
            <h2 className="text-base font-semibold text-ink-900 dark:text-surface-0">
              {t("settings.chTitle")}
            </h2>
            <p className="text-sm text-ink-500 mt-0.5">
              {t("settings.chSubtitle")}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <DotBadge tone="neutral">{channels.length}</DotBadge>
            <Button size="sm" onClick={openCreate}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("settings.chAdd")}
            </Button>
          </div>
        </TableToolbar>

        {chQ.isError ? (
          <ErrorState
            message={t(friendlyError(chQ.error))}
            onRetry={() => void chQ.refetch()}
            retrying={chQ.isFetching}
          />
        ) : chQ.isLoading ? (
          <Skeleton className="h-20 w-full" />
        ) : channels.length === 0 ? (
          <EmptyState
            title={t("settings.chEmpty")}
            description={t("settings.chEmptyHint")}
          />
        ) : (
          <Table>
            <THead>
              <tr>
                <Th>{t("settings.chName")}</Th>
                <Th className="hidden md:table-cell">{t("settings.chKind")}</Th>
                <Th className="hidden lg:table-cell">{t("settings.chTarget")}</Th>
                <Th>{t("settings.chMinSeverity")}</Th>
                <Th align="right">{t("alerts.colActions")}</Th>
              </tr>
            </THead>
            <TBody>
              {channels.map((c) => (
                <Tr key={c.id}>
                  <Td>
                    <div className="text-sm font-medium text-ink-900 dark:text-surface-0">
                      {c.name}
                    </div>
                    {secretHint(c) ? (
                      <div className="text-xs text-ink-400">{secretHint(c)}</div>
                    ) : null}
                  </Td>
                  <Td className="hidden md:table-cell">
                    <span className="text-xs text-ink-500">
                      {t(`settings.chKind_${c.kind}`)}
                    </span>
                  </Td>
                  <Td className="hidden lg:table-cell">
                    <span className="text-xs text-ink-400 truncate max-w-[240px] block">
                      {targetOf(c)}
                    </span>
                  </Td>
                  <Td>
                    <DotBadge
                      tone={
                        c.min_severity === "critical"
                          ? "danger"
                          : c.min_severity === "warning"
                            ? "warn"
                            : "neutral"
                      }
                    >
                      {t(
                        c.min_severity === "critical"
                          ? "alerts.critical"
                          : c.min_severity === "warning"
                            ? "alerts.warning"
                            : "alerts.info",
                      )}
                    </DotBadge>
                  </Td>
                  <Td align="right">
                    <div className="flex items-center justify-end gap-2">
                      <Switch
                        checked={c.enabled}
                        onCheckedChange={(next) =>
                          toggle.mutate({ id: c.id, enabled: next })
                        }
                        disabled={toggle.isPending}
                        aria-label={`${c.name} — ${t(
                          c.enabled ? "alerts.disable" : "alerts.enable",
                        )}`}
                      />
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label={t("settings.chEdit")}
                        onClick={() => openEdit(c)}
                      >
                        <Pencil className="w-4 h-4" aria-hidden="true" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label={t("alerts.delete")}
                        onClick={() => setConfirmDelete(c)}
                        className="text-rose-600 dark:text-rose-400"
                      >
                        <Trash2 className="w-4 h-4" aria-hidden="true"
                        />
                      </Button>
                    </div>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </Table>
        )}
      </TableShell>

      <Dialog
        open={dialogOpen}
        onClose={closeDialog}
        title={editing ? t("settings.chEdit") : t("settings.chAdd")}
        description={
          editing ? t("settings.chEditHint") : t("settings.chHttpsNote")
        }
        footer={
          <>
            <div className="flex items-center gap-2 mr-auto">
              <Button
                variant="secondary"
                loading={testing}
                disabled={!canSubmit}
                onClick={() => void test()}
              >
                {testing ? t("settings.chTesting") : t("settings.chTest")}
              </Button>
              {testing && (
                <span className="text-xs text-ink-500 animate-pulse">
                  {t("settings.chTestingHint")}
                </span>
              )}
            </div>
            <Button variant="secondary" onClick={closeDialog}>
              {t("alerts.cancel")}
            </Button>
            <Button
              loading={create.isPending}
              disabled={!canSubmit}
              onClick={() => create.mutate()}
            >
              {editing ? t("settings.chSave") : t("settings.chAdd")}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <div className="grid grid-cols-2 gap-4">
            <label className="block">
              <span className="block text-xs text-ink-500 mb-1">
                {t("settings.chKind")}
              </span>
              <Select
                value={form.kind}
                onChange={(e) => setForm({ ...form, kind: e.target.value })}
                aria-label={t("settings.chKind")}
                disabled={!!editing}
              >
                <option value="feishu">{t("settings.chKind_feishu")}</option>
                <option value="slack">{t("settings.chKind_slack")}</option>
                <option value="bluebird">{t("settings.chKind_bluebird")}</option>
                <option value="webhook">{t("settings.chKind_webhook")}</option>
              </Select>
            </label>
            <label className="block">
              <span className="block text-xs text-ink-500 mb-1">
                {t("settings.chMinSeverity")}
              </span>
              <Select
                value={form.min_severity}
                onChange={(e) =>
                  setForm({ ...form, min_severity: e.target.value })
                }
                aria-label={t("settings.chMinSeverity")}
              >
                <option value="info">{t("alerts.info")}</option>
                <option value="warning">{t("alerts.warning")}</option>
                <option value="critical">{t("alerts.critical")}</option>
              </Select>
            </label>
          </div>
          <Field
            label={t("settings.chName")}
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder="ops-feishu"
          />
          {/* Feishu: app credentials + receiver (mirrors bluebird's distribution channel) */}
          {isFeishu ? (
            <>
              <Field
                label={t("settings.chAppId")}
                value={form.app_id}
                onChange={(e) => setForm({ ...form, app_id: e.target.value })}
                placeholder="cli_xxxxxxxxxxxxxxxx"
                autoComplete="off"
              />
              <div>
                <label className="block text-xs text-ink-500 mb-1">
                  {t("settings.chAppSecret")}
                </label>
                <div className="relative">
                  <Input
                    type={secretRevealed ? "text" : "password"}
                    value={form.app_secret}
                    onChange={(e) => setForm({ ...form, app_secret: e.target.value })}
                    autoComplete="off"
                    className="pr-10"
                  />
                  <button
                    type="button"
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-ink-400 hover:text-ink-600"
                    onClick={() => setSecretRevealed(!secretRevealed)}
                    aria-label={secretRevealed ? t("settings.aiHide") : t("settings.aiReveal")}
                  >
                    {secretRevealed ? (
                      <EyeOff className="w-4 h-4" />
                    ) : (
                      <Eye className="w-4 h-4" />
                    )}
                  </button>
                </div>
              </div>
              <div>
                <label className="block text-xs text-ink-500 mb-1">
                  {t("settings.chReceiveIdType")}
                </label>
                <Select
                  value={form.receive_id_type}
                  onChange={(e) =>
                    setForm({ ...form, receive_id_type: e.target.value })
                  }
                  aria-label={t("settings.chReceiveIdType")}
                >
                  {RECEIVE_ID_TYPES.map((v) => (
                    <option key={v} value={v}>
                      {t(`settings.chRid_${v}`)}
                    </option>
                  ))}
                </Select>
              </div>
              <Field
                label={t("settings.chReceiveId")}
                value={form.receive_id}
                onChange={(e) => setForm({ ...form, receive_id: e.target.value })}
                placeholder={t("settings.chReceiveIdPlaceholder")}
              />
              <p className="text-xs text-ink-400">{t("settings.chFeishuNote")}</p>
            </>
          ) : (
            <>
              <div>
                <label className="block text-xs text-ink-500 mb-1">
                  {isSlack
                    ? t("settings.chSlackUrl")
                    : isBluebird
                      ? t("settings.chBluebirdUrl")
                      : t("settings.chWebhookUrl")}
                </label>
                <div className="relative">
                  <Input
                    type={secretRevealed ? "text" : "password"}
                    value={form.url}
                    onChange={(e) => setForm({ ...form, url: e.target.value })}
                    placeholder={
                      isSlack
                        ? t("settings.chSlackUrlPlaceholder")
                        : isBluebird
                          ? t("settings.chBluebirdUrlPlaceholder")
                          : t("settings.chWebhookUrlPlaceholder")
                    }
                    autoComplete="off"
                    className="pr-10"
                  />
                  <button
                    type="button"
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-ink-400 hover:text-ink-600"
                    onClick={() => setSecretRevealed(!secretRevealed)}
                    aria-label={secretRevealed ? t("settings.aiHide") : t("settings.aiReveal")}
                  >
                    {secretRevealed ? (
                      <EyeOff className="w-4 h-4" />
                    ) : (
                      <Eye className="w-4 h-4" />
                    )}
                  </button>
                </div>
              </div>
              {isSlack ? null : (
                <div>
                  <label className="block text-xs text-ink-500 mb-1">
                    {isBluebird ? t("settings.chBluebirdToken") : t("settings.chToken")}
                  </label>
                  <div className="relative">
                    <Input
                      type={secretRevealed ? "text" : "password"}
                      value={form.secret}
                      onChange={(e) => setForm({ ...form, secret: e.target.value })}
                      placeholder={
                        isBluebird
                          ? t("settings.chBluebirdTokenPlaceholder")
                          : t("settings.chTokenPlaceholder")
                      }
                      autoComplete="off"
                      className="pr-10"
                    />
                    <button
                      type="button"
                      className="absolute right-2 top-1/2 -translate-y-1/2 text-ink-400 hover:text-ink-600"
                      onClick={() => setSecretRevealed(!secretRevealed)}
                      aria-label={secretRevealed ? t("settings.aiHide") : t("settings.aiReveal")}
                    >
                      {secretRevealed ? (
                        <EyeOff className="w-4 h-4" />
                      ) : (
                        <Eye className="w-4 h-4" />
                      )}
                    </button>
                  </div>
                  <p className="mt-1 text-xs text-ink-400">
                    {isBluebird ? t("settings.chBluebirdTokenNote") : t("settings.chTokenNote")}
                  </p>
                </div>
              )}
              <p className="text-xs text-ink-400">
                {isSlack
                  ? t("settings.chSlackNote")
                  : isBluebird
                    ? t("settings.chBluebirdNote")
                    : t("settings.chWebhookNote")}
              </p>
            </>
          )}
        </div>
      </Dialog>

      <ConfirmDialog
        open={!!confirmDelete}
        title={t("settings.chDeleteConfirmTitle")}
        message={t("settings.chDeleteConfirmMessage")}
        confirmLabel={t("alerts.delete")}
        cancelLabel={t("alerts.cancel")}
        danger
        loading={remove.isPending}
        onCancel={() => setConfirmDelete(null)}
        onConfirm={() => confirmDelete && remove.mutate(confirmDelete.id)}
      />
    </>
  );
}

/**
 * UI preferences. Timezone was previously in the floating button group at bottom-right,
 * removed per requirement; the functionality can't go with it — time display everywhere needs it —
 * so it lives here on the settings page.
 */
/**
 * New credential strength assessment: three length tiers (8 / 12 / 16) + character classes.
 * Only for display — the backend's "at least 16 characters" is what actually enforces.
 * Here we give intuitive feedback on "is it long enough, is it varied enough".
 * A single character class (e.g. 16 lowercase letters) gets demoted one tier, to avoid
 * length alone creating a "strong" rating.
 */
function passwordStrength(
  pw: string,
): { score: number; labelKey: string; bar: string } | null {
  if (!pw) return null;
  let score = 0;
  if (pw.length >= 8) score += 1;
  if (pw.length >= 12) score += 1;
  if (pw.length >= 16) score += 1;
  const classes = [/[a-z]/, /[A-Z]/, /\d/, /[^a-zA-Z0-9]/].filter((re) =>
    re.test(pw),
  ).length;
  if (classes >= 3) score += 1;
  else if (classes <= 1) score -= 1;

  const level = Math.max(1, Math.min(4, score));
  const labelKey =
    level === 1
      ? "settings.pwStrWeak"
      : level === 2
        ? "settings.pwStrFair"
        : level === 3
          ? "settings.pwStrStrong"
          : "settings.pwStrVeryStrong";
  const bar =
    level === 1
      ? "bg-rose-500"
      : level === 2
        ? "bg-amber-500"
        : level === 3
          ? "bg-emerald-500"
          : "bg-brand-600";
  return { score: level, labelKey, bar };
}

/**
 * Change console credentials.
 *
 * The console has only this one key (nodes use signatures, don't trust it), so requirement:
 * provide current credential + new credential twice. Takes effect immediately, and also
 * swaps the locally-stored one — otherwise the next request returns 401.
 */
function CredentialSection() {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = React.useState({
    current: "",
    next: "",
    confirm: "",
  });

  const mismatch = form.confirm.length > 0 && form.next !== form.confirm;
  const tooShort = form.next.length > 0 && form.next.length < 16;
  const strength = passwordStrength(form.next);
  const canSubmit =
    !!form.current && form.next.length >= 16 && form.next === form.confirm;

  const save = useMutation({
    mutationFn: () => settingsApi.changeToken(form.current, form.next),
    onSuccess: () => {
      setToken(form.next);
      toast.push("success", t("settings.pwChanged"));
      setForm({ current: "", next: "", confirm: "" });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  return (
    <Card>
      <CardHeader
        icon={<KeyRound className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("settings.pwTitle")}
        description={t("settings.pwSubtitle")}
      />
      <CardBody>
        <div className="space-y-4">
          <div>
            <label className="block text-xs text-ink-500 mb-1">
              {t("settings.pwCurrent")}
            </label>
            <Input
              type="password"
              value={form.current}
              onChange={(e) => setForm({ ...form, current: e.target.value })}
              autoComplete="off"
            />
          </div>
          <div>
            <label className="block text-xs text-ink-500 mb-1">
              {t("settings.pwNew")}
            </label>
            <Input
              type="password"
              value={form.next}
              onChange={(e) => setForm({ ...form, next: e.target.value })}
              autoComplete="off"
            />
            {strength ? (
              <div className="mt-2 space-y-1">
                <div className="flex gap-1" aria-hidden="true">
                  {[0, 1, 2, 3].map((i) => (
                    <span
                      key={i}
                      className={cn(
                        "h-1 flex-1 rounded-full",
                        i < strength.score
                          ? strength.bar
                          : "bg-ink-100 dark:bg-ink-700",
                      )}
                    />
                  ))}
                </div>
                <span className="block text-xs text-ink-500">
                  {t("settings.pwStrength")}：{t(strength.labelKey)}
                </span>
              </div>
            ) : null}
            {(tooShort || mismatch) && (
              <span className="mt-1 block text-xs text-rose-600 dark:text-rose-400">
                {tooShort ? t("settings.pwTooShort") : t("settings.pwMismatch")}
              </span>
            )}
          </div>
          <div>
            <label className="block text-xs text-ink-500 mb-1">
              {t("settings.pwConfirm")}
            </label>
            <Input
              type="password"
              value={form.confirm}
              onChange={(e) => setForm({ ...form, confirm: e.target.value })}
              autoComplete="off"
            />
          </div>
          <div className="flex justify-end">
            <Button
              loading={save.isPending}
              disabled={!canSubmit}
              onClick={() => save.mutate()}
            >
              {t("settings.pwSave")}
            </Button>
          </div>
        </div>
        <p className="text-xs text-ink-400 mt-4">{t("settings.pwNote")}</p>
      </CardBody>
    </Card>
  );
}

/**
 * AI integration + AI tokens (combined into one section: "integration guide" for the agent to read,
 * "token list" for the admin to manage).
 */
/**
 * MCP Integration section — AI token management + MCP config display.
 */
function McpSection() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const toast = useToast();
  const baseUrl = window.location.origin;
  const [dialogOpen, setDialogOpen] = React.useState(false);
  const [pendingRevoke, setPendingRevoke] = React.useState<AiTokenMeta | null>(null);

  const tokensQ = useQuery({
    queryKey: ["ai-tokens"],
    queryFn: aiTokens.list,
    staleTime: 30_000,
  });

  const invalidate = () => void qc.invalidateQueries({ queryKey: ["ai-tokens"] });

  const revoke = useMutation({
    mutationFn: (id: string) => aiTokens.revoke(id),
    onSuccess: () => {
      toast.push("success", t("settings.mcpTokenRevoked"));
      setPendingRevoke(null);
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const tokens = tokensQ.data?.tokens ?? [];

  const copy = (text: string, okKey: string) =>
    void copyText(text).then((ok) =>
      toast.push(ok ? "success" : "error", t(ok ? okKey : "toast.copyFailed")),
    );

  const mcpConfig = `{
  "mcpServers": {
    "zhiwei-monitor": {
      "type": "http",
      "url": "${baseUrl}/mcp/sse",
      "headers": {
        "Authorization": "Bearer <YOUR_TOKEN>"
      }
    }
  }
}`;

  return (
    <>
      <Card>
        <CardHeader
          icon={<Network className="w-5 h-5 text-brand-600" aria-hidden="true" />}
          title={t("settings.mcpTitle")}
          description={t("settings.mcpSubtitle")}
          action={
            <Button variant="ghost" size="sm" onClick={() => navigate("/help#ai-integration")}>
              <HelpCircle className="w-4 h-4 mr-1" />
              {t("settings.mcpHelp")}
            </Button>
          }
        />
        <CardBody compact className="space-y-4">
          <div className="flex items-center justify-between">
            <span className="text-xs text-ink-400">{t("settings.mcpEndpoint")}</span>
            <code className="text-sm bg-surface-2 px-2 py-1 rounded">{baseUrl}/mcp/sse</code>
          </div>
          <div className="relative">
            <pre className="bg-surface-2 dark:bg-ink-700/60 rounded-lg px-4 py-3 text-xs overflow-x-auto">
              <code className="text-ink-700 dark:text-surface-4">{mcpConfig}</code>
            </pre>
            <Button
              variant="secondary"
              size="sm"
              className="absolute top-2 right-2"
              onClick={() => copy(mcpConfig, "settings.mcpConfigCopied")}
            >
              <Copy className="w-4 h-4 mr-1" />
              {t("action.copy")}
            </Button>
          </div>
        </CardBody>
      </Card>

      <TableShell>
        <TableToolbar>
          <div>
            <h2 className="text-base font-semibold text-ink-900 dark:text-surface-0">
              {t("settings.mcpTokensTitle")}
            </h2>
            <p className="text-sm text-ink-500 mt-0.5">
              {t("settings.mcpTokensSubtitle")}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <DotBadge tone="neutral">{tokens.length}</DotBadge>
            <Button size="sm" onClick={() => setDialogOpen(true)}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("settings.mcpTokensCreate")}
            </Button>
          </div>
        </TableToolbar>

        {tokensQ.isError ? (
          <ErrorState
            message={t(friendlyError(tokensQ.error))}
            onRetry={() => void tokensQ.refetch()}
            retrying={tokensQ.isFetching}
          />
        ) : tokensQ.isLoading ? (
          <Skeleton className="h-20 w-full" />
        ) : tokens.length === 0 ? (
          <EmptyState
            title={t("settings.mcpTokensEmpty")}
            description={t("settings.mcpTokensEmptyHint")}
          />
        ) : (
          <Table>
            <THead>
              <tr>
                <Th>{t("settings.mcpTokenColName")}</Th>
                <Th>{t("settings.mcpTokenColCreated")}</Th>
                <Th>{t("settings.mcpTokenColLastUsed")}</Th>
                <Th align="right">{t("alerts.colActions")}</Th>
              </tr>
            </THead>
            <TBody>
              {tokens.map((tok) => (
                <Tr key={tok.id}>
                  <Td>
                    <div className="text-sm font-medium text-ink-900 dark:text-surface-0">
                      {tok.name}
                    </div>
                    <div className="text-xs tabular-nums text-ink-400">
                      {tok.id}
                    </div>
                  </Td>
                  <Td>
                    <AiTokenTime tsUnixNano={tok.created_at_unix_nano} />
                  </Td>
                  <Td>
                    {tok.last_used_at_unix_nano == null ? (
                      <span className="text-sm text-ink-400">
                        {t("time.never")}
                      </span>
                    ) : (
                      <AiTokenTime tsUnixNano={tok.last_used_at_unix_nano} />
                    )}
                  </Td>
                  <Td align="right">
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={t("alerts.delete")}
                      onClick={() => setPendingRevoke(tok)}
                      className="text-rose-600 dark:text-rose-400"
                      disabled={tok.revoked_at_unix_nano != null}
                    >
                      <Trash2 className="w-4 h-4" aria-hidden="true" />
                    </Button>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </Table>
        )}
      </TableShell>

      <AiTokenDialog open={dialogOpen} onClose={() => setDialogOpen(false)} />

      <ConfirmDialog
        open={!!pendingRevoke}
        title={t("settings.mcpTokenRevokeTitle")}
        message={t("settings.mcpTokenRevokeMessage")}
        confirmLabel={t("settings.mcpTokenRevoke")}
        cancelLabel={t("alerts.cancel")}
        danger
        loading={revoke.isPending}
        onCancel={() => setPendingRevoke(null)}
        onConfirm={() => pendingRevoke && revoke.mutate(pendingRevoke.id)}
      />
    </>
  );
}

/**
 * Enroll tokens — one-shot `curl | bash` commands for target machines.
 *
 * Lists metadata for currently non-expired enroll tokens (**not** including plaintext token strings);
 * long-lived ones come from `ZHIWEI_BOOTSTRAP_TOKEN` env var and are labeled `permanent` separately.
 */
function EnrollTokensSection() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [dialogOpen, setDialogOpen] = React.useState(false);
  const [pendingRevoke, setPendingRevoke] = React.useState<EnrollTokenMeta | null>(
    null,
  );

  const q = useQuery({
    queryKey: ["enroll-tokens"],
    queryFn: enrollTokens.list,
    // Enroll operation is rare, just keep polling, no aggressive retry
    staleTime: 30_000,
  });

  const invalidate = () =>
    void qc.invalidateQueries({ queryKey: ["enroll-tokens"] });

  const revoke = useMutation({
    mutationFn: (id: string) => enrollTokens.revoke(id),
    onSuccess: () => {
      toast.push("success", t("settings.enrollTokenRevoked"));
      setPendingRevoke(null);
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const tokens = q.data?.tokens ?? [];

  return (
    <>
      <TableShell>
        <TableToolbar>
          <div>
            <h2 className="text-base font-semibold text-ink-900 dark:text-surface-0">
              {t("settings.enrollTokensTitle")}
            </h2>
            <p className="text-sm text-ink-500 mt-0.5">
              {t("settings.enrollTokensSubtitle")}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <DotBadge tone="neutral">{tokens.length}</DotBadge>
            <Button size="sm" onClick={() => setDialogOpen(true)}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("settings.enrollTokensCreate")}
            </Button>
          </div>
        </TableToolbar>

        {q.isError ? (
          <ErrorState
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : q.isLoading ? (
          <Skeleton className="h-20 w-full" />
        ) : tokens.length === 0 ? (
          <EmptyState
            title={t("settings.enrollTokensEmpty")}
            description={t("settings.enrollTokensEmptyHint")}
          />
        ) : (
          <Table>
            <THead>
              <tr>
                <Th>{t("settings.enrollTokenColLabel")}</Th>
                <Th>{t("settings.enrollTokenColKind")}</Th>
                <Th>{t("settings.enrollTokenColExpires")}</Th>
                <Th align="right">{t("alerts.colActions")}</Th>
              </tr>
            </THead>
            <TBody>
              {tokens.map((tok) => (
                <Tr key={tok.id}>
                  <Td>
                    <div className="text-sm font-medium text-ink-900 dark:text-surface-0">
                      {tok.label || (
                        <span className="text-ink-400">—</span>
                      )}
                    </div>
                    <div className="text-xs tabular-nums text-ink-400">
                      {tok.id}
                    </div>
                  </Td>
                  <Td>
                    {tok.permanent ? (
                      <DotBadge tone="success">
                        {t("settings.enrollTokenPermanent")}
                      </DotBadge>
                    ) : (
                      <DotBadge tone="neutral">
                        {t("settings.enrollTokenEphemeral")}
                      </DotBadge>
                    )}
                  </Td>
                  <Td>
                    <EnrollTokenExpiry meta={tok} />
                  </Td>
                  <Td align="right">
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={t("alerts.delete")}
                      onClick={() => setPendingRevoke(tok)}
                      className="text-rose-600 dark:text-rose-400"
                    >
                      <Trash2 className="w-4 h-4" aria-hidden="true"
                      />
                    </Button>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </Table>
        )}
      </TableShell>

      <EnrollTokenDialog
        open={dialogOpen}
        onClose={() => setDialogOpen(false)}
      />

      <ConfirmDialog
        open={!!pendingRevoke}
        title={t("settings.enrollTokenRevokeTitle")}
        message={t("settings.enrollTokenRevokeMessage")}
        confirmLabel={t("settings.enrollTokenRevoke")}
        cancelLabel={t("alerts.cancel")}
        danger
        loading={revoke.isPending}
        onCancel={() => setPendingRevoke(null)}
        onConfirm={() => pendingRevoke && revoke.mutate(pendingRevoke.id)}
      />
    </>
  );
}

/** Enroll token expiry display: "long-term" or "expires in N hours / N days" */
function EnrollTokenExpiry({ meta }: { meta: EnrollTokenMeta }) {
  const { t } = useTranslation();
  if (meta.permanent) {
    return (
      <span className="text-sm text-emerald-600 dark:text-emerald-400">
        {t("settings.enrollTokenPermanentExpiry")}
      </span>
    );
  }
  const remain = meta.expires_at_unix - Math.floor(Date.now() / 1000);
  let label: string;
  if (remain <= 0) label = t("time.secondsAgo", { n: 0 });
  else if (remain < 3600) {
    const m = Math.max(1, Math.ceil(remain / 60));
    label = t("time.minutesAgo", { n: m });
  } else if (remain < 86400) {
    const h = Math.round(remain / 3600);
    label = t("time.hoursAgo", { n: h });
  } else {
    const d = Math.round(remain / 86400);
    label = t("time.daysAgo", { n: d });
  }
  return (
    <span className="text-sm tabular-nums text-ink-700 dark:text-surface-4">
      {t("settings.enrollTokenExpiresIn", { at: label })}
    </span>
  );
}

/** Nanosecond timestamps are converted to ms before formatTime display. */
function AiTokenTime({ tsUnixNano }: { tsUnixNano: number }) {
  const { timezone } = usePrefs();
  const ms = Math.floor(tsUnixNano / 1e6);
  return (
    <span className="text-sm tabular-nums text-ink-700 dark:text-surface-4">
      {formatTime(ms, timezone)}
    </span>
  );
}

function UiSection() {
  const { t } = useTranslation();
  const prefs = usePrefs();
  const toast = useToast();
  return (
    <Card>
      <CardHeader
        icon={<Clock className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("settings.uiTitle")}
        description={t("settings.uiSubtitle")}
      />
      <CardBody compact className="space-y-4">
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
          <div>
            <label
              className="block text-xs text-ink-500 mb-1"
              htmlFor="settings-timezone"
            >
              {t("settings.tzLabel")}
            </label>
            <Select
              id="settings-timezone"
              value={prefs.timezone}
              onChange={(e) => {
                prefs.setTimezone(e.target.value);
                toast.push("success", t("settings.tzSaved"));
              }}
            >
              {TIMEZONES.map((z) => (
                <option key={z.tz} value={z.tz}>
                  {t(`tz.${z.key}`)}
                </option>
              ))}
            </Select>
          </div>
          <div>
            <label
              className="block text-xs text-ink-500 mb-1"
              htmlFor="settings-locale"
            >
              {t("settings.langLabel")}
            </label>
            <Select
              id="settings-locale"
              value={prefs.locale}
              onChange={(e) => {
                const next = e.target.value as Locale;
                prefs.setLocale(next);
                toast.push("success", t("settings.langSaved"));
              }}
            >
              <option value="en-US">{t("settings.lang_en-US")}</option>
              <option value="zh-CN">{t("settings.lang_zh-CN")}</option>
            </Select>
          </div>
        </div>
        <p className="text-xs text-ink-400">{t("settings.uiNote")}</p>
      </CardBody>
    </Card>
  );
}

function Row({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-xs text-ink-400">{label}</dt>
      <dd
        className={cn(
          "mt-1 text-sm text-ink-900 dark:text-surface-0 truncate",
          mono && "tabular-nums",
        )}
      >
        {value}
      </dd>
    </div>
  );
}
