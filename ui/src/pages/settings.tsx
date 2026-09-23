import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  BellRing,
  Bot,
  Clock,
  Copy,
  Eye,
  EyeOff,
  Info,
  KeyRound,
  PlugZap,
  Plus,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  aiTokens,
  alertsApi,
  enrollTokens,
  getToken,
  setToken,
  retentionApi,
  settingsApi,
  type AiTokenMeta,
  type AlertRule,
  type EnrollTokenMeta,
  type NotifyChannel,
} from "@/api";
import { NewRuleButton, RulesTable } from "@/components/alert-rules";
import { AiTokenDialog } from "@/components/ai-token-dialog";
import { EnrollTokenDialog } from "@/components/enroll-token-dialog";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardBody, CardHeader } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { EmptyState, ErrorState, Skeleton } from "@/components/ui/feedback";
import { Table, TBody, Td, Th, THead, Tr } from "@/components/ui/table";
import { useToast } from "@/components/ui/toast";
import { usePrefs } from "@/components/prefs-provider";
import { cn, copyText, formatTime, friendlyError, TIMEZONES } from "@/lib/utils";

export function Settings() {
  const { t } = useTranslation();
  return (
    <div className="space-y-4 md:space-y-6">
      <EnrollTokensSection />
      <CaSection />
      <CredentialSection />
      <AlertRulesSection />
      <ChannelsSection />
      <AiSection />
      <AiTokensSection />
      <CollectSection />
      <UiSection />
      <p className="text-xs text-ink-400">{t("settings.caWarn")}</p>
    </div>
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
            {/* 边缘终结 TLS 时这个 CA 不参与任何事——别让人以为它是「集群身份根」 */}
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

function ChannelsSection() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const chQ = useQuery({ queryKey: ["channels"], queryFn: alertsApi.channels });

  const EMPTY = {
    name: "",
    url: "",
    kind: "feishu",
    secret: "",
    min_severity: "warning",
  };
  const [dialogOpen, setDialogOpen] = React.useState(false);
  const [form, setForm] = React.useState(EMPTY);
  const [confirmDelete, setConfirmDelete] = React.useState<NotifyChannel | null>(
    null,
  );
  const [testing, setTesting] = React.useState(false);

  const invalidate = () => void qc.invalidateQueries({ queryKey: ["channels"] });

  const create = useMutation({
    mutationFn: () => settingsApi.createChannel(form),
    onSuccess: () => {
      toast.push("success", t("settings.chCreated"));
      setForm(EMPTY);
      setDialogOpen(false);
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  /** 拿当前填的参数真发一条——保存之前就能知道地址对不对 */
  const test = async () => {
    setTesting(true);
    try {
      const r = await settingsApi.testChannel(form);
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
  const canSubmit = !!form.name.trim() && !!form.url.trim();

  return (
    <Card>
      <CardHeader
        title={t("settings.chTitle")}
        description={t("settings.chSubtitle")}
        action={
          <>
            <DotBadge tone="neutral">{channels.length}</DotBadge>
            <Button size="sm" onClick={() => setDialogOpen(true)}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("settings.chAdd")}
            </Button>
          </>
        }
      />
      <CardBody compact className="space-y-4">
        {chQ.isError ? (
          <ErrorState
            compact
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
          <div className="overflow-hidden rounded-lg border border-surface-3 dark:border-ink-700">
            <Table>
              <THead>
                <tr>
                  <Th>{t("settings.chName")}</Th>
                  <Th className="hidden md:table-cell">{t("settings.chKind")}</Th>
                  <Th className="hidden lg:table-cell">{t("settings.chUrl")}</Th>
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
                      <div className="text-xs text-ink-400">
                        {t("settings.chTokenSet", { n: c.secret ? 1 : 0 })}
                      </div>
                    </Td>
                    <Td className="hidden md:table-cell">
                      <span className="text-xs text-ink-500">
                        {t(`settings.chKind_${c.kind}`)}
                      </span>
                    </Td>
                    <Td className="hidden lg:table-cell">
                      <span className="text-xs text-ink-400 truncate max-w-[240px] block">
                        {c.url}
                      </span>
                    </Td>
                    <Td>
                      <DotBadge
                        tone={c.min_severity === "critical" ? "danger" : "warn"}
                      >
                        {t(
                          c.min_severity === "critical"
                            ? "alerts.critical"
                            : "alerts.warning",
                        )}
                      </DotBadge>
                    </Td>
                    <Td align="right">
                      <div className="flex items-center justify-end gap-2">
                        <button
                          type="button"
                          onClick={() =>
                            toggle.mutate({ id: c.id, enabled: !c.enabled })
                          }
                          className={cn(
                            "px-2.5 py-1 rounded-md text-xs font-medium transition-colors",
                            c.enabled
                              ? "bg-emerald-50 text-emerald-700 dark:bg-emerald-700/20 dark:text-emerald-400"
                              : "bg-surface-2 text-ink-500 dark:bg-ink-700",
                          )}
                        >
                          {t(c.enabled ? "alerts.enabled" : "alerts.disabled")}
                        </button>
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
          </div>
        )}
        <p className="text-xs text-ink-400">{t("settings.chHttpsNote")}</p>
      </CardBody>

      <Dialog
        open={dialogOpen}
        onClose={() => setDialogOpen(false)}
        title={t("settings.chAdd")}
        description={t("settings.chHttpsNote")}
        footer={
          <>
            <Button
              variant="secondary"
              loading={testing}
              disabled={!form.url.trim()}
              onClick={() => void test()}
            >
              {t("settings.chTest")}
            </Button>
            <Button
              variant="secondary"
              onClick={() => setDialogOpen(false)}
            >
              {t("alerts.cancel")}
            </Button>
            <Button
              loading={create.isPending}
              disabled={!canSubmit}
              onClick={() => create.mutate()}
            >
              {t("settings.chAdd")}
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
              >
                <option value="feishu">{t("settings.chKind_feishu")}</option>
                <option value="dingtalk">{t("settings.chKind_dingtalk")}</option>
                <option value="slack">{t("settings.chKind_slack")}</option>
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
                <option value="warning">{t("alerts.warning")}</option>
                <option value="critical">{t("alerts.critical")}</option>
              </Select>
            </label>
          </div>
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("settings.chName")}
            </span>
            <Input
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="ops-webhook"
            />
          </label>
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("settings.chUrl")}
            </span>
            <Input
              value={form.url}
              onChange={(e) => setForm({ ...form, url: e.target.value })}
              placeholder={t("settings.chUrlPlaceholder")}
            />
          </label>
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("settings.chToken")}
            </span>
            <Input
              value={form.secret}
              onChange={(e) => setForm({ ...form, secret: e.target.value })}
              placeholder={t("settings.chTokenPlaceholder")}
            />
            <span className="mt-1 block text-xs text-ink-400">
              {t("settings.chTokenNote")}
            </span>
          </label>
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
    </Card>
  );
}

/**
 * 界面偏好。时区原先在右下角悬浮按钮组里，按本人要求从那组撤掉；
 * 功能不能跟着消失——时间显示到处都要用它，所以落到设置页。
 */
/**
 * 修改控制台凭据。
 *
 * 控制台只有这一把钥匙（节点走签名、不认它），所以要求：带当前凭据 +
 * 新凭据输两遍。改完立即生效，并把本地存的那份一起换掉——否则下一次请求就 401。
 */
function CredentialSection() {
  const { t } = useTranslation();
  const toast = useToast();
  const [open, setOpen] = React.useState(false);
  const [form, setForm] = React.useState({
    current: "",
    next: "",
    confirm: "",
  });

  const mismatch = form.confirm.length > 0 && form.next !== form.confirm;
  const tooShort = form.next.length > 0 && form.next.length < 16;
  const canSubmit =
    !!form.current && form.next.length >= 16 && form.next === form.confirm;

  const save = useMutation({
    mutationFn: () => settingsApi.changeToken(form.current, form.next),
    onSuccess: () => {
      setToken(form.next);
      toast.push("success", t("settings.pwChanged"));
      setForm({ current: "", next: "", confirm: "" });
      setOpen(false);
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  return (
    <Card>
      <CardHeader
        icon={<KeyRound className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("settings.pwTitle")}
        description={t("settings.pwSubtitle")}
        action={
          <Button size="sm" onClick={() => setOpen(true)}>
            {t("settings.pwChange")}
          </Button>
        }
      />
      <CardBody compact>
        <p className="text-xs text-ink-400">{t("settings.pwNote")}</p>
      </CardBody>

      <Dialog
        open={open}
        onClose={() => setOpen(false)}
        size="md"
        title={t("settings.pwChange")}
        description={t("settings.pwNote")}
        footer={
          <>
            <Button variant="secondary" onClick={() => setOpen(false)}>
              {t("alerts.cancel")}
            </Button>
            <Button
              loading={save.isPending}
              disabled={!canSubmit}
              onClick={() => save.mutate()}
            >
              {t("settings.pwSave")}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("settings.pwCurrent")}
            </span>
            <Input
              type="password"
              value={form.current}
              onChange={(e) => setForm({ ...form, current: e.target.value })}
            />
          </label>
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("settings.pwNew")}
            </span>
            <Input
              type="password"
              value={form.next}
              onChange={(e) => setForm({ ...form, next: e.target.value })}
            />
            {(tooShort || mismatch) && (
              <span className="mt-1 block text-xs text-rose-600 dark:text-rose-400">
                {tooShort ? t("settings.pwTooShort") : t("settings.pwMismatch")}
              </span>
            )}
          </label>
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("settings.pwConfirm")}
            </span>
            <Input
              type="password"
              value={form.confirm}
              onChange={(e) => setForm({ ...form, confirm: e.target.value })}
            />
          </label>
        </div>
      </Dialog>
    </Card>
  );
}

/**
 * AI 接入。
 *
 * 定位里的「Agent 可调用」不该只是一句宣言——接口地址、凭据、能读什么、
 * 能做什么、边界在哪，都要在界面上看得见、能一键复制给 agent。
 */
function AiSection() {
  const { t } = useTranslation();
  const toast = useToast();
  const [revealed, setRevealed] = React.useState(false);

  const token = getToken() ?? "";
  const base = window.location.origin;
  const masked = token ? `${token.slice(0, 8)}…${token.slice(-4)}` : "—";

  const copy = (text: string, okKey: string) =>
    void copyText(text).then((ok) =>
      toast.push(ok ? "success" : "error", t(ok ? okKey : "toast.copyFailed")),
    );

  const readOnly = [
    "GET /v1/todo",
    "GET /v1/nodes",
    "GET /v1/nodes/<id>/series?metric=&from=&to=&limit=",
    "GET /v1/nodes/<id>/containers",
    "GET /v1/containers",
    "GET /v1/certificates",
    "GET /v1/services",
  ];
  const actions = [
    "container_start / container_stop / container_restart / container_remove",
    "kill_process(pid, signal) / fetch_logs",
    "restart_host / shutdown_host",
    "refresh_inventory / scan_certs",
  ];

  return (
    <Card>
      <CardHeader
        icon={<Bot className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("settings.aiTitle")}
        description={t("settings.aiSubtitle")}
        action={
          <Button
            variant="primary"
            size="sm"
            onClick={() =>
              copy(
                t("settings.aiBrief", { base, token: token || "<admin token>" }),
                "settings.aiBriefCopied",
              )
            }
          >
            <Copy className="w-4 h-4" aria-hidden="true" />
            {t("settings.aiCopyBrief")}
          </Button>
        }
      />
      <CardBody compact className="space-y-4">
        <dl className="grid gap-4 sm:grid-cols-2">
          <div className="min-w-0">
            <dt className="text-xs text-ink-400">{t("settings.aiBase")}</dt>
            <dd className="mt-1 flex items-center gap-2">
              <code className="text-sm text-ink-900 dark:text-surface-0 truncate">
                {base}
              </code>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("settings.aiCopyBase")}
                onClick={() => copy(base, "settings.aiBaseCopied")}
              >
                <Copy className="w-4 h-4" aria-hidden="true" />
              </Button>
            </dd>
          </div>
          <div className="min-w-0">
            <dt className="text-xs text-ink-400">{t("settings.aiToken")}</dt>
            <dd className="mt-1 flex items-center gap-2">
              <code className="text-sm tabular-nums text-ink-900 dark:text-surface-0">
                {revealed ? token : masked}
              </code>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t(revealed ? "settings.aiHide" : "settings.aiReveal")}
                onClick={() => setRevealed((v) => !v)}
              >
                {revealed ? (
                  <EyeOff className="w-4 h-4" aria-hidden="true" />
                ) : (
                  <Eye className="w-4 h-4" aria-hidden="true" />
                )}
              </Button>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("settings.aiCopyToken")}
                onClick={() => copy(token, "settings.aiTokenCopied")}
              >
                <Copy className="w-4 h-4" aria-hidden="true" />
              </Button>
            </dd>
          </div>
        </dl>

        <div className="grid gap-4 sm:grid-cols-2">
          <div>
            <h4 className="text-xs text-ink-400">{t("settings.aiRead")}</h4>
            <ul className="mt-1 space-y-0.5">
              {readOnly.map((r) => (
                <li
                  key={r}
                  className="text-xs font-mono text-ink-600 dark:text-surface-4 break-all"
                >
                  {r}
                </li>
              ))}
            </ul>
          </div>
          <div>
            <h4 className="text-xs text-ink-400">{t("settings.aiAct")}</h4>
            <ul className="mt-1 space-y-0.5">
              {actions.map((a) => (
                <li
                  key={a}
                  className="text-xs font-mono text-ink-600 dark:text-surface-4 break-all"
                >
                  {a}
                </li>
              ))}
            </ul>
            <p className="mt-2 text-xs text-ink-400 break-all">
              POST /v1/exec {"{"}"node_id","action","params"{"}"}
            </p>
          </div>
        </div>

        <div className="flex items-start gap-2">
          <ShieldCheck
            className="w-4 h-4 mt-0.5 shrink-0 text-ink-400"
            aria-hidden="true"
          />
          <p className="text-xs text-ink-400">{t("settings.aiBoundary")}</p>
        </div>
      </CardBody>
    </Card>
  );
}

/**
 * 入网令牌（enroll-tokens）—— 给目标机器的一次性 `curl | bash` 命令。
 *
 * 列出当前还没过期的入网令牌元信息（**不**含明文 token 字符串）；
 * 长期有效的来自 `ZHIWEI_BOOTSTRAP_TOKEN` 环境变量，单独标 `permanent`。
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
    // 入网操作很罕见，挂着就行，不主动重试
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
    <Card>
      <CardHeader
        icon={<PlugZap className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("settings.enrollTokensTitle")}
        description={t("settings.enrollTokensSubtitle")}
        action={
          <>
            <DotBadge tone="neutral">{tokens.length}</DotBadge>
            <Button size="sm" onClick={() => setDialogOpen(true)}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("settings.enrollTokensCreate")}
            </Button>
          </>
        }
      />
      <CardBody compact className="space-y-4">
        {q.isError ? (
          <ErrorState
            compact
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
          <div className="overflow-hidden rounded-lg border border-surface-3 dark:border-ink-700">
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
          </div>
        )}
        <p className="text-xs text-ink-400">
          {t("settings.enrollTokensNote")}
        </p>
      </CardBody>

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
    </Card>
  );
}

/** 入网令牌的过期时间展示：「长期」或「N 小时 / N 天后过期」 */
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

/**
 * AI 令牌（ai-tokens）—— 外部 agent 读集群数据用的长期凭据。
 *
 * 撤销前必须 confirm：撤销当下所有持有这个 token 的 AI 客户端都会立刻 401。
 * 明文 token **只在创建时**显示一次，本页只管列表。
 */
function AiTokensSection() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [dialogOpen, setDialogOpen] = React.useState(false);
  const [pendingRevoke, setPendingRevoke] = React.useState<AiTokenMeta | null>(
    null,
  );

  const q = useQuery({
    queryKey: ["ai-tokens"],
    queryFn: aiTokens.list,
  });

  const invalidate = () =>
    void qc.invalidateQueries({ queryKey: ["ai-tokens"] });

  const revoke = useMutation({
    mutationFn: (id: string) => aiTokens.revoke(id),
    onSuccess: () => {
      toast.push("success", t("settings.aiTokenRevoked"));
      setPendingRevoke(null);
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const tokens = q.data?.tokens ?? [];

  return (
    <Card>
      <CardHeader
        icon={<Bot className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("settings.aiTokensTitle")}
        description={t("settings.aiTokensSubtitle")}
        action={
          <>
            <DotBadge tone="neutral">{tokens.length}</DotBadge>
            <Button size="sm" onClick={() => setDialogOpen(true)}>
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("settings.aiTokensCreate")}
            </Button>
          </>
        }
      />
      <CardBody compact className="space-y-4">
        {q.isError ? (
          <ErrorState
            compact
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : q.isLoading ? (
          <Skeleton className="h-20 w-full" />
        ) : tokens.length === 0 ? (
          <EmptyState
            title={t("settings.aiTokensEmpty")}
            description={t("settings.aiTokensEmptyHint")}
          />
        ) : (
          <div className="overflow-hidden rounded-lg border border-surface-3 dark:border-ink-700">
            <Table>
              <THead>
                <tr>
                  <Th>{t("settings.aiTokenColName")}</Th>
                  <Th>{t("settings.aiTokenColCreated")}</Th>
                  <Th>{t("settings.aiTokenColLastUsed")}</Th>
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
                        <Trash2 className="w-4 h-4" aria-hidden="true"
                        />
                      </Button>
                    </Td>
                  </Tr>
                ))}
              </TBody>
            </Table>
          </div>
        )}
        <p className="text-xs text-ink-400">{t("settings.aiTokensNote")}</p>
      </CardBody>

      <AiTokenDialog
        open={dialogOpen}
        onClose={() => setDialogOpen(false)}
      />

      <ConfirmDialog
        open={!!pendingRevoke}
        title={t("settings.aiTokenRevokeTitle")}
        message={t("settings.aiTokenRevokeMessage")}
        confirmLabel={t("settings.aiTokenRevoke")}
        cancelLabel={t("alerts.cancel")}
        danger
        loading={revoke.isPending}
        onCancel={() => setPendingRevoke(null)}
        onConfirm={() => pendingRevoke && revoke.mutate(pendingRevoke.id)}
      />
    </Card>
  );
}

/** 纳秒时间戳统一转 ms 后用 formatTime 显示。 */
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
        <div className="max-w-xs">
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
        <p className="text-xs text-ink-400">{t("settings.uiNote")}</p>
      </CardBody>
    </Card>
  );
}

function CollectSection() {
  const { t } = useTranslation();
  const retentionQ = useQuery({ queryKey: ["retention"], queryFn: retentionApi.get });
  return (
    <Card>
      <CardHeader
        title={t("settings.collectTitle")}
        description={t("settings.collectSubtitle")}
      />
      <CardBody compact className="space-y-4">
        <p className="text-sm text-ink-500">{t("settings.collectNote")}</p>
        <div>
          <h3 className="text-sm font-semibold text-ink-900 dark:text-surface-0">
            {t("settings.retentionTitle")}
          </h3>
          <ul className="mt-1 space-y-0.5 text-sm text-ink-500">
            {retentionQ.data && (
              <>
                <li>
                  {t("settings.retentionRaw", { n: retentionQ.data.raw_days })}
                </li>
                <li>
                  {t("settings.retentionHourly", {
                    n: retentionQ.data.hourly_days,
                  })}
                </li>
              </>
            )}
          </ul>
          <p className="mt-1 text-xs text-ink-400">{t("settings.retentionNote")}</p>
        </div>
      </CardBody>
    </Card>
  );
}

/**
 * 告警规则——从原「告警」页搬进来。
 * 规则是「对象的属性」，不是顶级对象；顶级菜单只放对象（设计 §6）。
 */
function AlertRulesSection() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const rulesQ = useQuery({ queryKey: ["rules"], queryFn: alertsApi.rules });
  const [pendingDelete, setPendingDelete] = React.useState<AlertRule | null>(null);

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["rules"] });
    // 规则变了，待办里的条目也会跟着变
    void qc.invalidateQueries({ queryKey: ["todo"] });
  };

  const toggle = useMutation({
    mutationFn: (r: AlertRule) => alertsApi.toggleRule(r.id, !r.enabled),
    onSuccess: () => {
      toast.push("success", t("alerts.updated"));
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const remove = useMutation({
    mutationFn: (id: number) => alertsApi.deleteRule(id),
    onSuccess: () => {
      toast.push("success", t("alerts.deleted"));
      setPendingDelete(null);
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  return (
    <Card>
      <CardHeader
        icon={<BellRing className="w-5 h-5 text-brand-600" aria-hidden="true" />}
        title={t("settings.rulesTitle")}
        description={t("settings.rulesSubtitle")}
        action={<NewRuleButton />}
      />
      <CardBody compact className="space-y-4">
        <div className="overflow-hidden rounded-lg border border-surface-3 dark:border-ink-700">
          <RulesTable
            rules={rulesQ.data ?? []}
            loading={rulesQ.isPending}
            error={rulesQ.isError ? rulesQ.error : null}
            onRetry={() => void rulesQ.refetch()}
            retrying={rulesQ.isFetching}
            onToggle={(r) => toggle.mutate(r)}
            onDelete={(r) => setPendingDelete(r)}
          />
        </div>
      </CardBody>
      <ConfirmDialog
        open={pendingDelete !== null}
        title={t("alerts.deleteConfirmTitle")}
        message={t("alerts.deleteConfirmMessage")}
        confirmLabel={t("alerts.delete")}
        cancelLabel={t("alerts.cancel")}
        danger
        loading={remove.isPending}
        onConfirm={() => pendingDelete && remove.mutate(pendingDelete.id)}
        onCancel={() => setPendingDelete(null)}
      />
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
