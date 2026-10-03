import * as React from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import {
  certSourcesApi,
  daysLeft,
  expiryTone,
  waitForCommand,
  type CertScanResult,
  type CertSourceView,
  type NodeView,
} from "@/api";
import { DotBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { EmptyState, ErrorState } from "@/components/ui/feedback";
import { useToast } from "@/components/ui/toast";
import { formatTime, friendlyError, nodeLabel } from "@/lib/utils";
import { usePrefs } from "@/components/prefs-provider";

/** Run one node-side scan (via the scan_certs command + receipt), returns parsed result or error text */
async function runCertScan(
  nodeId: string,
  path: string,
): Promise<{ result?: CertScanResult; error?: string }> {
  const { command_id } = await certSourcesApi.test(nodeId, path);
  const row = await waitForCommand(command_id);
  if (!row) return { error: "pending" };
  if (!row.result_ok) return { error: row.result_error || "failed" };
  try {
    return { result: JSON.parse(row.result_text ?? "{}") as CertScanResult };
  } catch {
    return { error: "bad_payload" };
  }
}

/** Scan result panel: matched certificates list / failure reason (shared by in-form and inline "Test") */
export function CertScanPanel({
  state,
  onRetry,
}: {
  state: { loading: boolean; result?: CertScanResult; error?: string };
  onRetry?: () => void;
}) {
  const { t } = useTranslation();
  const { timezone } = usePrefs();

  if (state.loading) {
    return (
      <div className="flex items-center gap-2 text-sm text-ink-500 py-4">
        <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
        {t("certs.sources.testRunning")}
      </div>
    );
  }

  if (state.error === "pending") {
    return (
      <div className="rounded-lg bg-amber-50 dark:bg-amber-700/20 px-4 py-3 text-sm text-amber-700 dark:text-amber-300">
        {t("certs.sources.testPending")}
      </div>
    );
  }
  if (state.error) {
    return (
      <ErrorState
        compact
        message={
          state.error === "bad_payload" || state.error === "failed"
            ? t("certs.sources.testFailed")
            : state.error
        }
        onRetry={onRetry ?? (() => undefined)}
      />
    );
  }
  const result = state.result;
  if (!result || result.matched === 0) {
    return (
      <EmptyState
        title={t("certs.sources.testNoMatch")}
        description={t("certs.sources.testNoMatchHint", {
          patterns: (result?.patterns ?? []).join(" "),
        })}
      />
    );
  }

  return (
    <div className="rounded-lg border border-surface-3 dark:border-ink-700 divide-y divide-surface-2 dark:divide-ink-700">
      {result.certs.map((c) => (
        <div key={c.path} className="px-4 py-3 flex items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="text-sm text-ink-900 dark:text-surface-0 truncate">
              {c.parse_error ? c.path : c.domains[0] || c.subject}
            </div>
            <div className="text-xs text-ink-400 truncate" title={c.path}>
              {c.parse_error ? c.error : c.path}
            </div>
          </div>
          <div className="text-right shrink-0">
            {c.parse_error ? (
              <DotBadge tone="danger">{t("certs.parseError")}</DotBadge>
            ) : (
              <>
                <DotBadge tone={expiryTone(daysLeft(c.not_after_unix_nano))}>
                  {fmtDays(daysLeft(c.not_after_unix_nano), t)}
                </DotBadge>
                <div className="mt-1 text-xs text-ink-400 tabular-nums">
                  {formatTime(c.not_after_unix_nano / 1e6, timezone)}
                </div>
              </>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

function fmtDays(days: number, t: (k: string, o?: Record<string, unknown>) => string) {
  if (days < 0) return t("certs.expired", { n: Math.abs(Math.floor(days)) });
  if (days < 1) return t("certs.today");
  return t("certs.daysLeft", { n: Math.floor(days) });
}

const emptyScan = { loading: false } as {
  loading: boolean;
  result?: CertScanResult;
  error?: string;
};

/** Add / edit certificate path form dialog (includes "Test") */
export function CertSourceDialog({
  open,
  onClose,
  nodes,
  initial,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  nodes: NodeView[];
  /** Has value = editing; no value = adding */
  initial?: CertSourceView | null;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [nodeId, setNodeId] = React.useState("");
  // For an "all nodes" source, when testing we need to pick one real machine to run on
  const [testNodeId, setTestNodeId] = React.useState("");
  /** Whether the user has manually picked a node (distinguishes "not yet picked" from "explicitly chose all nodes") */
  const [nodeTouched, setNodeTouched] = React.useState(false);
  const [path, setPath] = React.useState("");
  const [enabled, setEnabled] = React.useState(true);
  const [notifyEnabled, setNotifyEnabled] = React.useState(true);
  const [notifyDays, setNotifyDays] = React.useState(30);
  const [scan, setScan] = React.useState(emptyScan);
  const [saving, setSaving] = React.useState(false);

  // Only reset the form on "open / change target".
  // Don't include nodes in the deps: it's a query result, refetching changes the array identity,
  // which would clear the form while the user is typing (this has bitten us in practice) —
  // default node fallback is handled in the effect below.
  const nodesRef = React.useRef(nodes);
  nodesRef.current = nodes;
  React.useEffect(() => {
    if (!open) return;
    // New: default to a specific node (wildcard is an explicit option, not the default)
    setNodeId(initial ? initial.node_id : (nodesRef.current[0]?.id ?? ""));
    setTestNodeId("");
    setNodeTouched(false);
    setPath(initial?.path ?? "");
    setEnabled(initial?.enabled ?? true);
    setNotifyEnabled(initial?.notify_enabled ?? true);
    setNotifyDays(initial?.notify_days_before ?? 30);
    setScan(emptyScan);
  }, [open, initial]);

  // Node list arrives late (not yet loaded when dialog opened) → fill in the default node, but only if user hasn't manually picked one
  React.useEffect(() => {
    if (open && !initial && !nodeTouched && !nodeId && nodes.length > 0) {
      setNodeId(nodes[0].id);
    }
  }, [open, initial, nodeTouched, nodeId, nodes]);

  React.useEffect(() => {
    if (open && !testNodeId && nodes.length > 0) setTestNodeId(nodes[0].id);
  }, [open, testNodeId, nodes]);

  const test = async () => {
    const target = nodeId || testNodeId;
    if (!target || !path.trim()) return;
    setScan({ loading: true });
    try {
      const { result, error } = await runCertScan(target, path.trim());
      setScan({ loading: false, result, error });
    } catch (e) {
      setScan({ loading: false, error: t(friendlyError(e)) });
    }
  };

  const save = async () => {
    setSaving(true);
    try {
      if (initial) {
        await certSourcesApi.patch(initial.id, {
          path: path.trim(),
          enabled,
          notify_enabled: notifyEnabled,
          notify_days_before: notifyDays,
        });
      } else {
        await certSourcesApi.create({
          node_id: nodeId,
          path: path.trim(),
          notify_enabled: notifyEnabled,
          notify_days_before: notifyDays,
        });
      }
      toast.push("success", t("toast.saved"));
      onSaved();
      onClose();
    } catch (e) {
      toast.push("error", t(friendlyError(e)));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title={initial ? t("certs.sources.editTitle") : t("certs.sources.addTitle")}
      description={t("certs.sources.formHint")}
      footer={
        <div className="flex items-center justify-between gap-3 w-full">
          <Button
            variant="secondary"
            disabled={!nodeId || !path.trim() || scan.loading}
            onClick={() => void test()}
          >
            {t("certs.sources.test")}
          </Button>
          <div className="flex items-center gap-2">
            <Button variant="ghost" onClick={onClose}>
              {t("action.cancel")}
            </Button>
            <Button
              loading={saving}
              disabled={!nodeId || !path.trim()}
              onClick={() => void save()}
            >
              {t("action.save")}
            </Button>
          </div>
        </div>
      }
    >
      <div className="space-y-4">
        <div className="flex flex-wrap gap-3">
          {/* Fixed width: node name (alias preferred) must display in full, but doesn't widen the dialog with content */}
          <label className="block w-64 shrink-0">
            <span className="block text-xs text-ink-500 mb-1">
              {t("certs.sources.node")}
            </span>
            <Select
              value={nodeId}
              onChange={(e) => {
                setNodeTouched(true);
                setNodeId(e.target.value);
              }}
            >
              <option value="">{t("certs.sources.allNodes")}</option>
              {nodes.map((n) => (
                <option key={n.id} value={n.id}>
                  {nodeLabel(n)}
                </option>
              ))}
            </Select>
          </label>
          <label className="block min-w-[240px] flex-[2]">
            <span className="block text-xs text-ink-500 mb-1">
              {t("certs.sources.path")}
            </span>
            <Input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder={t("certs.sources.pathPlaceholder")}
            />
          </label>
        </div>

        {nodeId === "" && (
          <div className="rounded-lg bg-surface-1 dark:bg-ink-700/40 px-4 py-3 flex flex-wrap items-center gap-3">
            <span className="text-xs text-ink-500">{t("certs.sources.allNodesHint")}</span>
            <Select
              wrapperClassName="w-64"
              className="h-8 text-xs"
              value={testNodeId}
              onChange={(e) => setTestNodeId(e.target.value)}
              aria-label={t("certs.sources.testNode")}
            >
              {nodes.map((n) => (
                <option key={n.id} value={n.id}>
                  {t("certs.sources.testOn", { node: nodeLabel(n) })}
                </option>
              ))}
            </Select>
          </div>
        )}

        <div className="flex flex-wrap items-center gap-4">
          <label className="flex items-center gap-2 text-sm text-ink-700 dark:text-surface-4">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => setEnabled(e.target.checked)}
            />
            {t("certs.sources.enabled")}
          </label>
          <label className="flex items-center gap-2 text-sm text-ink-700 dark:text-surface-4">
            <input
              type="checkbox"
              checked={notifyEnabled}
              onChange={(e) => setNotifyEnabled(e.target.checked)}
            />
            {t("certs.sources.notify")}
          </label>
          <label className="flex items-center gap-2 text-sm text-ink-700 dark:text-surface-4">
            {t("certs.sources.notifyBefore")}
            <Input
              className="w-20"
              type="number"
              min={1}
              max={365}
              value={notifyDays}
              disabled={!notifyEnabled}
              onChange={(e) => setNotifyDays(Number(e.target.value))}
            />
            {t("certs.sources.days")}
          </label>
        </div>

        {(scan.loading || scan.result || scan.error) && (
          <div className="pt-1">
            <div className="text-xs text-ink-400 mb-2">
              {t("certs.sources.testResult")}
            </div>
            <CertScanPanel state={scan} onRetry={() => void test()} />
          </div>
        )}
      </div>
    </Dialog>
  );
}

/** Inline "Test": opens and immediately runs one scan of this source */
export function CertScanDialog({
  source,
  nodes,
  onClose,
}: {
  source: CertSourceView;
  nodes: NodeView[];
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [scan, setScan] = React.useState<{ loading: boolean; result?: CertScanResult; error?: string }>(
    { loading: true },
  );
  // Wildcard source has no single target node: default to the first one, allow switching to test against another
  const [nodeId, setNodeId] = React.useState(
    source.all_nodes ? (nodes[0]?.id ?? "") : source.node_id,
  );

  const run = React.useCallback(async () => {
    if (!nodeId) return;
    setScan({ loading: true });
    try {
      const { result, error } = await runCertScan(nodeId, source.path);
      setScan({ loading: false, result, error });
    } catch (e) {
      setScan({ loading: false, error: friendlyError(e) });
    }
  }, [nodeId, source.path]);

  React.useEffect(() => {
    void run();
  }, [run]);

  return (
    <Dialog
      open
      onClose={onClose}
      size="lg"
      title={t("certs.sources.testTitle", {
        path: source.path,
        node: source.node_hostname ?? t("certs.sources.allNodes"),
      })}
      description={t("certs.sources.testSubtitle")}
    >
      {source.all_nodes && (
        <div className="mb-3 flex items-center gap-2">
          <span className="text-xs text-ink-500">{t("certs.sources.testNode")}</span>
          <Select
            wrapperClassName="w-48"
            className="h-8 text-xs"
            value={nodeId}
            onChange={(e) => setNodeId(e.target.value)}
            aria-label={t("certs.sources.testNode")}
          >
            {nodes.map((n) => (
              <option key={n.id} value={n.id}>
                {n.hostname}
              </option>
            ))}
          </Select>
        </div>
      )}
      <CertScanPanel state={scan} onRetry={() => void run()} />
    </Dialog>
  );
}
