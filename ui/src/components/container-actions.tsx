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
import { Tooltip } from "@/components/ui/tooltip";
import { useToast } from "@/components/ui/toast";
import { friendlyError } from "@/lib/utils";

/**
 * 容器操作组：启动 / 关闭 / 重启 / 查看最新日志。
 *
 * 节点详情页的容器组与容器页共用这一份——两处各写一遍会让「关闭要二次确认」
 * 「动作完成后让节点立刻重采快照」这类规则走偏。动作完成后由 `onDone` 通知
 * 调用方刷新自己的查询键。
 */
export function ContainerActions({
  nodeId,
  container,
  onDone,
  onError,
}: {
  nodeId: string;
  container: ContainerInfo;
  /** 动作成功（或看日志结束）后调用，用来刷新列表 */
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
   * 正在执行的动作。
   *
   * 之前只用一个布尔量，按钮在等回执的这段时间里既不转圈也不禁用——用户点完
   * 看到界面「没反应」，就会连点好几次（每次都真的下发一条命令）。现在按动作
   * 记名：命中的那个按钮转圈，其余的禁用。
   */
  const [busy, setBusy] = React.useState<
    "start" | "stop" | "restart" | "remove" | "logs" | null
  >(null);
  const [logs, setLogs] = React.useState<{
    name: string;
    text: string;
    loading: boolean;
  } | null>(null);

  const running = container.state === "running";

  /** 动作完成后节点会重采快照（5 分钟周期太久），这里在 ~20s 内反复拉几次 */
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
    // 节点是否真的回话了：一旦回话就说明「控制台这份状态」可能已经过期，
    // 无论成功失败都重采一次快照，让这一行显示真实状态
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

  const showLogs = async () => {
    setBusy("logs");
    setLogs({ name: container.name, text: "", loading: true });
    try {
      const { command_id } = await commandsApi.exec(nodeId, "fetch_logs", {
        source: "container",
        container: container.name,
        tail: 200,
        timestamps: true,
      });
      const row = await waitForCommand(command_id);
      if (!row) {
        setLogs(null);
        toast.push("warn", t("detail.cmdPending"));
      } else if (row.result_ok) {
        setLogs({
          name: container.name,
          text: row.result_text ?? "",
          loading: false,
        });
      } else {
        setLogs(null);
        toast.push("error", row.result_error || t("detail.cmdFailed"));
      }
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
        {/* 删除只在「已经不跑」时出现：运行中的容器请先停掉——
            不在界面上提供「顺手强删一个正在跑的服务」这条路 */}
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
            {/* 等回执可能十几秒，只给骨架屏会被当成「卡住了」 */}
            <p className="text-xs text-ink-400">{t("containers.logsWaiting")}</p>
          </div>
        ) : logs && logs.text.trim() ? (
          <pre className="max-h-[60vh] overflow-auto scrollbar-thin rounded-lg bg-ink-900 text-surface-0 p-4 text-xs font-mono leading-relaxed whitespace-pre-wrap break-all">
            {logs.text}
          </pre>
        ) : (
          <EmptyState title={t("containers.logsEmpty")} />
        )}
      </Dialog>
    </>
  );
}
