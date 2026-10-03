import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { FileText, Play, RotateCw, Square, Trash2 } from "lucide-react";
import {
  commandsApi,
  type ContainerInfo,
  waitForCommand,
} from "@/api";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog } from "@/components/ui/dialog";
import { EmptyState, Skeleton } from "@/components/ui/feedback";
import { LogView } from "@/components/log-view";
import { Tooltip } from "@/components/ui/tooltip";
import { useToast } from "@/components/ui/toast";
import { friendlyError } from "@/lib/utils";

/**
 * Container action group: start / stop / restart / view latest logs.
 *
 * The node detail page's container group and the containers page share this —
 * writing it twice would let rules like "stop requires secondary confirmation"
 * and "node re-snapshots immediately after action" drift apart. After completion,
 * `onDone` notifies the caller to refresh its own query keys.
 */
export function ContainerActions({
  nodeId,
  container,
  onDone,
  onError,
}: {
  nodeId: string;
  container: ContainerInfo;
  /** Called after the action succeeds (or after viewing logs ends), used to refresh the list */
  onDone?: () => void;
  onError?: (message: string) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [pending, setPending] = React.useState<
    "stop" | "restart" | "remove" | null
  >(null);
  /**
   * The action currently being executed.
   *
   * Previously this was a single boolean: while waiting for the receipt the button neither spun nor disabled —
   * users clicked and saw "no response", so they clicked several more times (each one really did dispatch a command).
   * Now we track by action: the targeted button spins, the others disable.
   */
  const [busy, setBusy] = React.useState<
    "start" | "stop" | "restart" | "remove" | "logs" | null
  >(null);
  const [logs, setLogs] = React.useState<{
    name: string;
    text: string;
    loading: boolean;
  } | null>(null);
  // Realtime follow: wait 5s after each log pull before sending the next (no fixed-interval concurrency —
  // the command channel waits for the node to poll every 10s, stacking commands only slows things down).
  // Stop when the dialog is closed.
  const [follow, setFollow] = React.useState(false);
  const [lastAt, setLastAt] = React.useState<number | null>(null);
  // Let "Updated N seconds ago" tick by itself
  const [, setTick] = React.useState(0);

  const running = container.state === "running";

  /** After the action completes the node re-snapshots (5-min cycle is too long), here we re-pull a few times within ~20s */
  const scheduleRefresh = React.useCallback(() => {
    for (let i = 1; i <= 8; i++) {
      window.setTimeout(() => {
        void qc.invalidateQueries({ queryKey: ["containers"] });
      }, i * 2500);
    }
  }, [qc]);

  const runAction = async (
    action: string,
    key: "start" | "stop" | "restart" | "remove",
  ) => {
    setBusy(key);
    // Whether the node actually answered: once it answers, "the console's status" may be stale,
    // so re-snapshot regardless of success/failure to make this row show the true state
    let answered = false;
    try {
      const { command_id } = await commandsApi.exec(nodeId, action, {
        container: container.name,
      });
      const row = await waitForCommand(command_id);
      if (!row) toast.push("warn", t("detail.cmdPending"));
      else {
        answered = true;
        if (row.result_ok) {
          toast.push("success", row.result_text || t("detail.cmdOk"));
        } else toast.push("error", row.result_error || t("detail.cmdFailed"));
      }
    } catch (e) {
      toast.push("error", t(friendlyError(e)));
    } finally {
      setBusy(null);
      setPending(null);
      if (answered) {
        void commandsApi
          .exec(nodeId, "refresh_inventory", {})
          .catch(() => undefined);
        scheduleRefresh();
        onDone?.();
      }
    }
  };

  const PENDING: Record<
    "stop" | "restart" | "remove",
    { action: string; title: string; message: string; label: string; danger: boolean }
  > = {
    stop: {
      action: "container_stop",
      title: t("containers.stopTitle", { name: container.name }),
      message: t("containers.stopMessage"),
      label: t("containers.actStop"),
      danger: true,
    },
    restart: {
      action: "container_restart",
      title: t("containers.restartTitle", { name: container.name }),
      message: t("containers.restartMessage"),
      label: t("containers.actRestart"),
      danger: false,
    },
    remove: {
      action: "container_remove",
      title: t("containers.removeTitle", { name: container.name }),
      message: t("containers.removeMessage"),
      label: t("containers.actRemove"),
      danger: true,
    },
  };

  /** Pull logs once. silent = a realtime-follow round: keep existing output, no toast on failure. */
  const fetchLogs = React.useCallback(
    async (name: string, silent: boolean) => {
      const { command_id } = await commandsApi.exec(nodeId, "fetch_logs", {
        source: "container",
        container: name,
        tail: 200,
        timestamps: true,
      });
      const row = await waitForCommand(command_id);
      if (!row) {
        if (!silent) {
          setLogs(null);
          toast.push("warn", t("detail.cmdPending"));
        }
        return;
      }
      if (!row.result_ok) {
        if (!silent) {
          setLogs(null);
          toast.push("error", row.result_error || t("detail.cmdFailed"));
        }
        return;
      }
      setLogs({ name, text: row.result_text ?? "", loading: false });
      setLastAt(Date.now());
    },
    [nodeId, t, toast],
  );

  const showLogs = async () => {
    setBusy("logs");
    setFollow(false);
    setLastAt(null);
    setLogs({ name: container.name, text: "", loading: true });
    try {
      await fetchLogs(container.name, false);
    } catch (e) {
      setLogs(null);
      const msg = t(friendlyError(e));
      onError?.(msg);
      toast.push("error", msg);
    } finally {
      setBusy(null);
      onDone?.();
    }
  };

  // Realtime follow loop: schedule the next round only after the previous completes (see comments in state)
  const followRef = React.useRef(false);
  followRef.current = follow;
  const fetchRef = React.useRef(fetchLogs);
  fetchRef.current = fetchLogs;
  const logsName = logs?.name;
  React.useEffect(() => {
    if (!follow || !logsName) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      try {
        await fetchRef.current(logsName, true);
      } catch {
        // Follow-round silently fails: don't spam toasts on network jitter, the next round will retry
      }
      if (!cancelled && followRef.current) {
        timer = window.setTimeout(() => void tick(), 5000);
      }
    };
    timer = window.setTimeout(() => void tick(), 5000);
    return () => {
      cancelled = true;
      if (timer) window.clearTimeout(timer);
    };
  }, [follow, logsName]);

  // Stop following when dialog closes (don't keep firing commands in the background)
  React.useEffect(() => {
    if (!logs) setFollow(false);
  }, [logs]);

  // "Updated N seconds ago" tick
  React.useEffect(() => {
    if (!follow) return;
    const id = window.setInterval(() => setTick((n) => n + 1), 1000);
    return () => window.clearInterval(id);
  }, [follow]);

  return (
    <>
      <div className="flex items-center justify-end gap-1">
        {!running && (
          <Tooltip content={t("containers.actStart")}>
            <Button
              variant="ghost"
              size="icon"
              aria-label={t("containers.actStart")}
              loading={busy === "start"}
              disabled={busy !== null}
              onClick={() => void runAction("container_start", "start")}
            >
              <Play className="w-4 h-4" aria-hidden="true" />
            </Button>
          </Tooltip>
        )}
        {running && (
          <>
            <Tooltip content={t("containers.actStop")}>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("containers.actStop")}
                loading={busy === "stop"}
                disabled={busy !== null}
                onClick={() => setPending("stop")}
              >
                <Square className="w-4 h-4" aria-hidden="true" />
              </Button>
            </Tooltip>
            <Tooltip content={t("containers.actRestart")}>
              <Button
                variant="ghost"
                size="icon"
                aria-label={t("containers.actRestart")}
                loading={busy === "restart"}
                disabled={busy !== null}
                onClick={() => setPending("restart")}
              >
                <RotateCw className="w-4 h-4" aria-hidden="true" />
              </Button>
            </Tooltip>
          </>
        )}
        {/* Delete only appears when "already not running": please stop running containers first —
            the UI doesn't offer a "casually force-remove a running service" path */}
        {!running && (
          <Tooltip content={t("containers.actRemove")}>
            <Button
              variant="ghost"
              size="icon"
              aria-label={t("containers.actRemove")}
              loading={busy === "remove"}
              disabled={busy !== null}
              onClick={() => setPending("remove")}
              className="text-rose-600 dark:text-rose-400"
            >
              <Trash2 className="w-4 h-4" aria-hidden="true" />
            </Button>
          </Tooltip>
        )}
        <Tooltip content={t("containers.actLogs")}>
          <Button
            variant="ghost"
            size="icon"
            aria-label={t("containers.actLogs")}
            loading={busy === "logs"}
            disabled={busy !== null}
            onClick={() => void showLogs()}
          >
            <FileText className="w-4 h-4" aria-hidden="true" />
          </Button>
        </Tooltip>
      </div>

      {pending && (
        <ConfirmDialog
          open
          danger={PENDING[pending].danger}
          loading={busy === pending}
          title={PENDING[pending].title}
          message={PENDING[pending].message}
          confirmLabel={PENDING[pending].label}
          cancelLabel={t("action.cancel")}
          onCancel={() => setPending(null)}
          onConfirm={() => void runAction(PENDING[pending].action, pending)}
        />
      )}

      <Dialog
        open={!!logs}
        onClose={() => setLogs(null)}
        size="xl"
        title={t("containers.logsTitle", { name: logs?.name ?? "" })}
        description={t("containers.logsSubtitle")}
      >
        {logs?.loading ? (
          <div className="space-y-3">
            <Skeleton className="h-40 w-full" />
            {/* Waiting for receipt can take over a dozen seconds; just a skeleton would be read as "stuck" */}
            <p className="text-xs text-ink-400">{t("containers.logsWaiting")}</p>
          </div>
        ) : logs ? (
          <div className="space-y-2">
            <div className="flex flex-wrap items-center gap-3">
              <label className="flex items-center gap-2 text-xs text-ink-500">
                <input
                  type="checkbox"
                  checked={follow}
                  onChange={(e) => setFollow(e.target.checked)}
                />
                {t("logs.realtime")}
              </label>
              {follow && lastAt && (
                <span className="flex items-center gap-2 text-xs text-emerald-600 dark:text-emerald-400">
                  <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse" />
                  {t("logs.following", {
                    ago: Math.round((Date.now() - lastAt) / 1000),
                  })}
                </span>
              )}
            </div>
            {logs.text.trim() ? (
              <LogView text={logs.text} follow={follow} />
            ) : (
              <EmptyState title={t("containers.logsEmpty")} />
            )}
          </div>
        ) : (
          <EmptyState title={t("containers.logsEmpty")} />
        )}
      </Dialog>
    </>
  );
}
