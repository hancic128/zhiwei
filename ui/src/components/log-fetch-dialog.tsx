import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { FileText, Loader2 } from "lucide-react";
import {
  api,
  containersApi,
  controlApi,
  waitForCommand,
  type CommandHistoryRow,
} from "@/api";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { EmptyState, ErrorState } from "@/components/ui/feedback";
import { LogView } from "@/components/log-view";
import { useToast } from "@/components/ui/toast";
import { cn, friendlyError, nodeLabel } from "@/lib/utils";

const MAX_RENDER = 20_000;

/**
 * On-demand log fetch (container / file).
 *
 * The standalone logs menu has been retired; file-log capability lives in this
 * dialog. The containers page toolbar's "view file logs" and a container row's
 * "view latest logs" share the same command channel.
 */
export function LogFetchDialog({
  open,
  onClose,
  defaultNodeId,
  defaultSource = "file",
  defaultContainer,
}: {
  open: boolean;
  onClose: () => void;
  defaultNodeId?: string;
  defaultSource?: "container" | "file";
  defaultContainer?: string;
}) {
  const { t } = useTranslation();
  const toast = useToast();

  const nodesQ = useQuery({ queryKey: ["nodes"], queryFn: api.nodes, enabled: open });
  // Default to a node that was recently online: if the top of the list is an
  // offline machine, the user clicks fetch and waits forever for the receipt.
  const nodes = React.useMemo(
    () =>
      [...(nodesQ.data ?? [])].sort(
        (a, b) => (b.last_seen_ms ?? 0) - (a.last_seen_ms ?? 0),
      ),
    [nodesQ.data],
  );

  const [nodeId, setNodeId] = React.useState(defaultNodeId ?? "");
  const [source, setSource] = React.useState<"container" | "file">(defaultSource);
  const [container, setContainer] = React.useState(defaultContainer ?? "");
  const [path, setPath] = React.useState("");
  const [tail, setTail] = React.useState(200);
  const [timestamps, setTimestamps] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [follow, setFollow] = React.useState(false);
  const [lastAt, setLastAt] = React.useState<number | null>(null);
  // Let "N seconds ago" tick by itself (re-renders every second while following)
  const [, setTick] = React.useState(0);
  const [row, setRow] = React.useState<CommandHistoryRow | null>(null);

  // Reset to the caller's target every time the dialog opens (viewing two
  // containers back-to-back on the same node won't cross-contaminate).
  React.useEffect(() => {
    if (!open) return;
    setNodeId(defaultNodeId ?? "");
    setSource(defaultSource);
    setContainer(defaultContainer ?? "");
    setRow(null);
    setFollow(false);
    setLastAt(null);
  }, [open, defaultNodeId, defaultSource, defaultContainer]);

  React.useEffect(() => {
    if (!nodeId && nodes.length > 0) setNodeId(nodes[0].id);
  }, [nodes, nodeId]);

  const invQ = useQuery({
    queryKey: ["containers", nodeId],
    queryFn: () => containersApi.byNode(nodeId),
    enabled: open && !!nodeId,
  });
  const containers = invQ.data?.containers ?? [];

  React.useEffect(() => {
    if (!container || containers.some((c) => c.name === container)) return;
    setContainer(containers[0]?.name ?? "");
  }, [containers, container]);

  /** silent = an auto-refresh round: don't clear existing output, don't show loading — avoids flicker. */
  const fetchLogs = React.useCallback(async (silent = false) => {
    if (!nodeId) return;
    if (!silent) {
      setBusy(true);
      setRow(null);
    }
    try {
      const params =
        source === "container"
          ? { source: "container", container, tail, timestamps }
          : { source: "file", path, tail };
      const { command_id } = await controlApi.exec(nodeId, "fetch_logs", params);
      const r = await waitForCommand(command_id);
      if (!r) {
        if (!silent) toast.push("warn", t("detail.cmdPending"));
      } else {
        setRow(r);
        setLastAt(Date.now());
      }
    } catch (e) {
      if (!silent) toast.push("error", t(friendlyError(e)));
    } finally {
      if (!silent) setBusy(false);
    }
    // path must be in the deps: missing it closes over a stale value (file logs
    // would keep reporting "path required").
  }, [nodeId, source, container, path, tail, timestamps, t, toast]);

  /**
   * Auto-refresh: **wait 5 seconds after the previous round completes before
   * firing the next one**. No fixed-interval concurrent fetches.
   * The command channel has to wait for the node's 10-second poll + execution
   * + receipt — a single round already takes several seconds. A fixed 2-second
   * interval just queues commands up, which is actually slower.
   */
  const followRef = React.useRef(false);
  followRef.current = follow;
  const inFlight = React.useRef(false);
  // Use a ref to get the latest fetchLogs: otherwise "follow while typing"
  // restarts the loop on every keystroke and fires a separate command each time.
  const fetchRef = React.useRef(fetchLogs);
  fetchRef.current = fetchLogs;
  React.useEffect(() => {
    if (!follow) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      if (cancelled || inFlight.current) return;
      inFlight.current = true;
      try {
        await fetchRef.current(true);
      } finally {
        inFlight.current = false;
      }
      if (!cancelled && followRef.current) {
        timer = window.setTimeout(() => void tick(), 5000);
      }
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer) window.clearTimeout(timer);
    };
  }, [follow]);

  // Stop following when the dialog closes / the target changes — don't keep
  // firing commands in the background.
  React.useEffect(() => {
    if (!open) setFollow(false);
  }, [open]);

  React.useEffect(() => {
    if (!follow) return;
    const id = window.setInterval(() => setTick((n) => n + 1), 1000);
    return () => window.clearInterval(id);
  }, [follow]);

  const canFetch = !!nodeId && (source === "container" ? !!container : !!path.trim());

  const failed = row && (row.state === "failed" || row.result_ok === false);
  const text = row?.result_text ?? "";

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="xl"
      title={t("logs.title")}
      description={t("logs.subtitle")}
    >
      <div className="space-y-4">
        {/* Spec 08/10: node-list load failure must not be silent — show a friendly message + retry inline */}
        {nodesQ.isError && (
          <ErrorState
            compact
            message={t(friendlyError(nodesQ.error))}
            onRetry={() => void nodesQ.refetch()}
            retrying={nodesQ.isFetching}
          />
        )}
        <div className="flex flex-wrap items-end gap-3">
          {/* Fixed width: alias / hostname must display fully, but must not stretch the dialog with content */}
          <Field label={t("logs.node")} className="w-64 shrink-0">
            <Select
              value={nodeId}
              onChange={(e) => setNodeId(e.target.value)}
              aria-label={t("logs.node")}
              disabled={nodesQ.isLoading}
            >
              {nodesQ.isLoading && <option>{t("logs.loadingNodes")}</option>}
              {nodes.map((n) => (
                <option key={n.id} value={n.id}>
                  {nodeLabel(n)}
                </option>
              ))}
            </Select>
          </Field>

          <Field label={t("logs.source")}>
            <div className="flex items-center gap-1 rounded-lg border border-surface-3 dark:border-ink-500 p-1">
              {(["container", "file"] as const).map((s) => (
                <button
                  key={s}
                  type="button"
                  onClick={() => setSource(s)}
                  className={cn(
                    "px-3 py-1.5 rounded-md text-xs font-medium transition-colors",
                    source === s
                      ? "bg-brand-50 text-brand-700 dark:bg-brand-900 dark:text-brand-100"
                      : "text-ink-500 hover:bg-surface-2 dark:hover:bg-ink-700/60",
                  )}
                >
                  {t(s === "container" ? "logs.sourceContainer" : "logs.sourceFile")}
                </button>
              ))}
            </div>
          </Field>

          {source === "container" ? (
            <Field label={t("logs.container")} className="min-w-[200px]">
              {invQ.isError ? (
                <span className="text-xs text-rose-600 dark:text-rose-400">
                  {t(friendlyError(invQ.error))}{" "}
                  <button
                    type="button"
                    className="underline"
                    onClick={() => void invQ.refetch()}
                  >
                    {t("action.retry")}
                  </button>
                </span>
              ) : invQ.isLoading ? (
                <span className="flex items-center gap-2 text-xs text-ink-400">
                  <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />
                  {t("logs.loadingContainers")}
                </span>
              ) : containers.length === 0 ? (
                <span className="text-xs text-ink-400">{t("logs.noContainers")}</span>
              ) : (
                <Select
                  value={container}
                  onChange={(e) => setContainer(e.target.value)}
                  aria-label={t("logs.container")}
                >
                  {containers.map((c) => (
                    <option key={c.id} value={c.name}>
                      {c.name}
                      {c.state === "running" ? "" : ` (${c.state})`}
                    </option>
                  ))}
                </Select>
              )}
            </Field>
          ) : (
            <Field label={t("logs.path")} className="min-w-[240px] flex-1">
              <Input
                value={path}
                onChange={(e) => setPath(e.target.value)}
                placeholder={t("logs.pathPlaceholder")}
              />
            </Field>
          )}

          <Field label={t("logs.tail")} className="w-[110px]">
            <Input
              type="number"
              value={tail}
              onChange={(e) => setTail(Number(e.target.value))}
            />
          </Field>

          {source === "container" && (
            <label className="flex items-center gap-2 pb-2 text-xs text-ink-500">
              <input
                type="checkbox"
                checked={timestamps}
                onChange={(e) => setTimestamps(e.target.checked)}
              />
              {t("logs.timestamps")}
            </label>
          )}

          <label className="flex items-center gap-2 pb-2 text-xs text-ink-500">
            <input
              type="checkbox"
              checked={follow}
              disabled={!canFetch}
              onChange={(e) => setFollow(e.target.checked)}
            />
            {t("logs.follow")}
          </label>

          <Button
            className="mb-0.5"
            loading={busy}
            disabled={!canFetch}
            onClick={() => void fetchLogs()}
          >
            {!busy && <FileText className="w-4 h-4" aria-hidden="true" />}
            {busy ? t("logs.fetching") : t("logs.fetch")}
          </Button>
        </div>

        {busy || !row ? (
          busy ? (
            <div className="flex items-center gap-3 py-8 text-sm text-ink-500">
              <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
              {t("logs.fetching")}
            </div>
          ) : (
            <EmptyState
              icon={<FileText className="w-12 h-12" aria-hidden="true" />}
              title={t("logs.empty")}
              description={t("logs.emptyHint")}
            />
          )
        ) : failed ? (
          <div className="rounded-lg bg-rose-50 dark:bg-rose-700/20 px-4 py-3 text-sm text-rose-700 dark:text-rose-400">
            {row.result_error || t("logs.stateFailed")}
          </div>
        ) : !text ? (
          <EmptyState title={t("containers.logsEmpty")} />
        ) : (
          <>
            {/* How many lines we got: logs are read one screen at a time, give a quick magnitude */}
            {text.trim() && (
              <p className="text-xs text-ink-400">
                {t("logs.lines", {
                  n: text.trimEnd().split("\n").length,
                })}
              </p>
            )}
            {follow && lastAt && (
              <p className="flex items-center gap-2 text-xs text-emerald-600 dark:text-emerald-400">
                <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse" />
                {t("logs.following", { ago: Math.round((Date.now() - lastAt) / 1000) })}
              </p>
            )}
            {text.length > MAX_RENDER && (
              <p className="text-xs text-amber-600 dark:text-amber-400">
                {t("logs.truncated")}
              </p>
            )}
            <LogView text={text.slice(-MAX_RENDER)} follow={follow} />
          </>
        )}
      </div>
    </Dialog>
  );
}

function Field({
  label,
  children,
  className,
}: {
  label: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <label className={cn("block", className)}>
      <span className="block text-xs text-ink-500 mb-1">{label}</span>
      {children}
    </label>
  );
}
