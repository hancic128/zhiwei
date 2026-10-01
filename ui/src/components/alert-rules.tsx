/**
 * 告警规则管理——从原「告警」页搬来，现落在设置页里。
 *
 * 设计：docs/superpowers/specs/2026-09-19-product-structure-design.md §6
 * （「告警」是从数据源出发的命名；用户要的是「要处理的事」，所以告警降为待办的
 * 一个来源，规则配置属于「对象的属性」，进设置。）
 */
import * as React from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Plus, Pencil, Trash2 } from "lucide-react";
import {
  alertsApi,
  ALERT_METRICS,
  OP_LABEL,
  type AlertRule,
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
  TableSkeleton,
} from "@/components/ui/feedback";
import { Table, TableFooter, TBody, Td, Th, THead, Tr } from "@/components/ui/table";
import { useToast } from "@/components/ui/toast";
import { friendlyError } from "@/lib/utils";

export function RulesTable({
  rules,
  loading,
  error,
  onRetry,
  retrying,
  onToggle,
  onDelete,
  onEdit,
  toggling,
}: {
  rules: AlertRule[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  retrying: boolean;
  onToggle: (r: AlertRule) => void;
  onDelete: (r: AlertRule) => void;
  onEdit: (r: AlertRule) => void;
  /** 有一条规则正在改启停：期间把所有开关置灰，避免连点打出一串请求 */
  toggling?: boolean;
}) {
  const { t } = useTranslation();
  if (loading) return <TableSkeleton rows={3} />;
  if (error)
    return (
      <ErrorState message={t(friendlyError(error))} onRetry={onRetry} retrying={retrying} />
    );
  if (rules.length === 0)
    return (
      <EmptyState title={t("alerts.emptyRules")} description={t("alerts.emptyHintRules")} />
    );

  return (
    <>
      <Table>
        <THead>
          <tr>
            <Th>{t("alerts.ruleName")}</Th>
            <Th className="hidden md:table-cell">{t("alerts.metric")}</Th>
            <Th>{t("alerts.op")}</Th>
            <Th className="hidden lg:table-cell">{t("alerts.duration")}</Th>
            <Th>{t("alerts.severity")}</Th>
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
                <div className="text-xs text-ink-400">#{r.id}</div>
              </Td>
              <Td className="hidden md:table-cell">
                <span className="text-xs tabular-nums text-ink-500">{r.metric}</span>
              </Td>
              <Td>
                <span className="text-sm tabular-nums">
                  {OP_LABEL[r.op] ?? r.op} {r.threshold}
                </span>
              </Td>
              <Td className="hidden lg:table-cell">
                <span className="text-xs tabular-nums text-ink-500">
                  {r.duration_seconds}s
                </span>
              </Td>
              <Td>
                <DotBadge tone={r.severity === "critical" ? "danger" : "warn"}>
                  {t(r.severity === "critical" ? "alerts.critical" : "alerts.warning")}
                </DotBadge>
              </Td>
              <Td align="right">
                <div className="flex items-center justify-end gap-2">
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label={t("action.edit")}
                    onClick={() => onEdit(r)}
                  >
                    <Pencil className="w-4 h-4" aria-hidden="true" />
                  </Button>
                  <Switch
                    checked={r.enabled}
                    onCheckedChange={() => onToggle(r)}
                    disabled={toggling}
                    aria-label={`${r.name} — ${t(
                      r.enabled ? "alerts.disable" : "alerts.enable",
                    )}`}
                  />
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label={t("alerts.delete")}
                    onClick={() => onDelete(r)}
                    className="text-rose-600 dark:text-rose-400"
                  >
                    <Trash2 className="w-4 h-4" aria-hidden="true" />
                  </Button>
                </div>
              </Td>
            </Tr>
          ))}
        </TBody>
      </Table>
      <TableFooter>
        <p className="text-xs text-ink-500">
          {t("alerts.rules")} · {rules.length}
        </p>
      </TableFooter>
    </>
  );
}

interface RuleDialogProps {
  rule?: AlertRule;
  onClose: () => void;
}

export function RuleDialog({ rule, onClose }: RuleDialogProps) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [form, setForm] = React.useState({
    name: rule?.name ?? "",
    metric: rule?.metric ?? (ALERT_METRICS[0] as string),
    op: rule?.op ?? "gt",
    threshold: rule?.threshold ?? 90,
    duration_seconds: rule?.duration_seconds ?? 300,
    severity: rule?.severity ?? "warning",
  });

  const isEdit = !!rule;

  const save = useMutation({
    mutationFn: () =>
      isEdit
        ? alertsApi.updateRule(rule.id, form)
        : alertsApi.createRule(form),
    onSuccess: () => {
      toast.push("success", t(isEdit ? "alerts.updated" : "alerts.created"));
      void qc.invalidateQueries({ queryKey: ["rules"] });
      void qc.invalidateQueries({ queryKey: ["todo"] });
      onClose();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  return (
    <Dialog
      open
      onClose={onClose}
      size="md"
      title={isEdit ? t("alerts.editRule") : t("alerts.newRule")}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("alerts.cancel")}
          </Button>
          <Button
            loading={save.isPending}
            disabled={!form.name.trim()}
            onClick={() => save.mutate()}
          >
            {isEdit ? t("action.save") : t("alerts.create")}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label={t("alerts.ruleName")}>
          <Input
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder={t("alerts.ruleNamePlaceholder")}
          />
        </Field>
        <Field label={t("alerts.metric")}>
          <Select
            value={form.metric}
            onChange={(e) => setForm({ ...form, metric: e.target.value })}
          >
            {ALERT_METRICS.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </Select>
        </Field>
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("alerts.op")}>
            <Select
              value={form.op}
              onChange={(e) => setForm({ ...form, op: e.target.value })}
            >
              {Object.entries(OP_LABEL).map(([k, v]) => (
                <option key={k} value={k}>
                  {v}
                </option>
              ))}
            </Select>
          </Field>
          <Field label={t("alerts.threshold")}>
            <Input
              type="number"
              value={form.threshold}
              onChange={(e) =>
                setForm({ ...form, threshold: Number(e.target.value) })
              }
            />
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("alerts.duration")}>
            <Input
              type="number"
              value={form.duration_seconds}
              onChange={(e) =>
                setForm({ ...form, duration_seconds: Number(e.target.value) })
              }
            />
          </Field>
          <Field label={t("alerts.severity")}>
            <Select
              value={form.severity}
              onChange={(e) => setForm({ ...form, severity: e.target.value })}
            >
              <option value="warning">{t("alerts.warning")}</option>
              <option value="critical">{t("alerts.critical")}</option>
            </Select>
          </Field>
        </div>
      </div>
    </Dialog>
  );
}

export function NewRuleButton({ onNew }: { onNew: () => void }) {
  const { t } = useTranslation();
  return (
    <Button variant="primary" size="sm" onClick={onNew}>
      <Plus className="w-4 h-4" aria-hidden="true" />
      {t("alerts.newRule")}
    </Button>
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
