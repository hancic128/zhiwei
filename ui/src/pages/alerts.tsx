/**
 * 告警菜单 —— 顶级页面，承载「内置告警」与「自定义规则」两类规则。
 *
 * 设计：docs/superpowers/specs/2026-09-19-product-structure-design.md §6
 * （「告警」曾被并入设置页；本次把它拆回顶级菜单是因为它现在承载
 *   两类规则 + 用户期望的「看见我在监控什么」入口。）
 *
 * - 内置告警（BuiltinAlertsSection）：平台自带事件（节点上下线等），只能启停
 * - 自定义规则（UserAlertRulesSection）：复用 alert-rules.tsx 里的 RulesTable
 */
import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { DotBadge } from "@/components/ui/badge";
import {
  alertsApi,
  builtinAlertsApi,
  type AlertRule,
  type BuiltinAlertRule,
} from "@/api";
import { NewRuleButton, RuleDialog, RulesTable } from "@/components/alert-rules";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import {
  EmptyState,
  ErrorState,
  Skeleton,
} from "@/components/ui/feedback";
import { Switch } from "@/components/ui/switch";
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

export function Alerts() {
  const { t } = useTranslation();
  return (
    <div className="space-y-4 md:space-y-6">
      <div>
        <h2 className="text-lg font-semibold text-ink-900 dark:text-surface-0">
          {t("alerts.title")}
        </h2>
        <p className="text-sm text-ink-500 mt-0.5">{t("alerts.subtitle")}</p>
      </div>
      <BuiltinAlertsSection />
      <UserAlertRulesSection />
    </div>
  );
}

/**
 * 内置告警 —— 平台自带的事件，UI 上可启用 / 停用。
 *
 * 文案：节点上下线（node_online / node_offline）是当前两条；
 * 后续再加内置规则就在 `builtin_alert_rules` 表里插一行即可。
 *
 * metric 走 `host.online` 是因为 node_offline / node_online 在同一行
 * 通知里争同一个 metric 名（值为 0 / 1），方便控制台在历史告警里做筛选。
 */
/**
 * 内置告警的展示元数据：每条规则对应的「判定表达式」与级别徽标。
 *
 * 以前这里按 `r.id === "node_online"` 硬编码，加一条规则就会错显成节点口径；
 * 现在按 id 查表，未知 id 回落成中性的 warning。
 */
const BUILTIN_META: Record<
  string,
  { metric: string; severity: "warning" | "critical"; tone: "danger" | "warn" | "ok" }
> = {
  node_offline: { metric: "host.online = 0", severity: "critical", tone: "danger" },
  node_online: { metric: "host.online = 1", severity: "warning", tone: "ok" },
  service_offline: { metric: "probe.state = down", severity: "critical", tone: "danger" },
  service_online: { metric: "probe.state = ok", severity: "warning", tone: "ok" },
  container_stopped: { metric: "container.state = stopped", severity: "warning", tone: "warn" },
  container_started: { metric: "container.state = started", severity: "warning", tone: "ok" },
  cert_expired: { metric: "cert.days_left < 0", severity: "critical", tone: "danger" },
  cert_expiring: { metric: "cert.days_left < notify_days_before", severity: "warning", tone: "warn" },
};

const BUILTIN_TONE_CLASS: Record<string, string> = {
  danger: "bg-rose-50 text-rose-700 dark:bg-rose-700/20 dark:text-rose-400",
  warn: "bg-amber-50 text-amber-700 dark:bg-amber-700/20 dark:text-amber-400",
  ok: "bg-emerald-50 text-emerald-700 dark:bg-emerald-700/20 dark:text-emerald-400",
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

function BuiltinAlertsSection() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const q = useQuery({
    queryKey: ["builtin-alerts"],
    queryFn: builtinAlertsApi.list,
  });

  const toggle = useMutation({
    mutationFn: (r: BuiltinAlertRule) =>
      builtinAlertsApi.setEnabled(r.id, !r.enabled),
    onSuccess: () => {
      toast.push("success", t("alerts.updated"));
      void qc.invalidateQueries({ queryKey: ["builtin-alerts"] });
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const rules = q.data ?? [];

  return (
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
                        : "alerts.warning",
                    )}
                  </span>
                </Td>
                <Td align="right">
                  <Switch
                    checked={r.enabled}
                    onCheckedChange={() => toggle.mutate(r)}
                    disabled={toggle.isPending}
                    aria-label={`${r.name} — ${t(
                      r.enabled ? "alerts.disable" : "alerts.enable",
                    )}`}
                  />
                </Td>
              </Tr>
            ))}
          </TBody>
        </Table>
      )}
    </TableShell>
  );
}

/**
 * 自定义规则 —— 复用 alert-rules.tsx 里的 RulesTable + NewRuleButton，
 * 不再嵌在设置页里。
 */
function UserAlertRulesSection() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const rulesQ = useQuery({ queryKey: ["rules"], queryFn: alertsApi.rules });
  const [pendingDelete, setPendingDelete] = React.useState<number | null>(null);
  const [editingRule, setEditingRule] = React.useState<AlertRule | null>(null);
  const [showNewDialog, setShowNewDialog] = React.useState(false);

  const invalidate = () => {
    void qc.invalidateQueries({ queryKey: ["rules"] });
    // 规则变了，待办里的条目也会跟着变
    void qc.invalidateQueries({ queryKey: ["todo"] });
  };

  const remove = useMutation({
    mutationFn: (id: number) => alertsApi.deleteRule(id),
    onSuccess: () => {
      toast.push("success", t("alerts.deleted"));
      setPendingDelete(null);
      invalidate();
    },
    onError: (e) => {
      toast.push("error", t(friendlyError(e)));
      setPendingDelete(null);
    },
  });

  // 启停写回服务端：以前这里只 invalidate 一下、根本不发请求，
  // 于是「停用」按下去永远不变（开关换成 Switch 后更显眼，顺手修掉）。
  const toggle = useMutation({
    mutationFn: (r: AlertRule) => alertsApi.toggleRule(r.id, !r.enabled),
    onSuccess: () => {
      toast.push("success", t("alerts.updated"));
      invalidate();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  return (
    <>
      <TableShell>
        <TableToolbar>
          <div>
            <h2 className="text-base font-semibold text-ink-900 dark:text-surface-0">
              {t("alerts.userTitle")}
            </h2>
            <p className="text-sm text-ink-500 mt-0.5">
              {t("alerts.userSubtitle")}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <DotBadge tone="neutral">{rulesQ.data?.length ?? 0}</DotBadge>
            <NewRuleButton onNew={() => setShowNewDialog(true)} />
          </div>
        </TableToolbar>

        <RulesTable
          rules={rulesQ.data ?? []}
          loading={rulesQ.isPending}
          error={rulesQ.isError ? rulesQ.error : null}
          onRetry={() => void rulesQ.refetch()}
          retrying={rulesQ.isFetching}
          toggling={toggle.isPending}
          onToggle={(r) => toggle.mutate(r)}
          onDelete={(r) => setPendingDelete(r.id)}
          onEdit={(r) => setEditingRule(r)}
        />
      </TableShell>

      {showNewDialog && (
        <RuleDialog onClose={() => setShowNewDialog(false)} />
      )}

      {editingRule && (
        <RuleDialog rule={editingRule} onClose={() => setEditingRule(null)} />
      )}

      <ConfirmDialog
        open={pendingDelete !== null}
        title={t("alerts.deleteConfirmTitle")}
        message={t("alerts.deleteConfirmMessage")}
        confirmLabel={t("alerts.delete")}
        cancelLabel={t("alerts.cancel")}
        danger
        loading={remove.isPending}
        onCancel={() => setPendingDelete(null)}
        onConfirm={() =>
          pendingDelete !== null && remove.mutate(pendingDelete)
        }
      />
    </>
  );
}
