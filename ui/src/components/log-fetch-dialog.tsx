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
import { useToast } from "@/components/ui/toast";
import { cn, friendlyError, nodeLabel } from "@/lib/utils";

const MAX_RENDER = 20_000;

/**
 * 按需拉取日志（容器 / 文件）。
 *
 * 日志菜单已下线，文件日志能力收进这个对话框：容器页工具栏的
 * 「查看文件日志」与容器行的「查看最新日志」共用同一套命令通道。
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
  // 默认尽量挑「最近还在线的」节点：排在最前的若是离线机器，用户点拉取会一直等回执
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
  // 让「N 秒前更新」自己走字（跟随中每秒重渲染一次）
  const [, setTick] = React.useState(0);
  const [row, setRow] = React.useState<CommandHistoryRow | null>(null);

  // 每次打开时按调用方给的目标重置（同一个节点连续看两个容器不会串）
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

  /** silent = 自动刷新那一轮：不清空已有输出、不显示 loading，避免画面闪烁 */
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
        if (!silent) toast.push("warn", t("logs.cmdPending"));
      } else {
        setRow(r);
        setLastAt(Date.now());
      }
    } catch (e) {
      if (!silent) toast.push("error", t(friendlyError(e)));
    } finally {
      if (!silent) setBusy(false);
    }
    // path 必须在依赖里：漏了它会闭包住旧值（文件日志会一直报「需要 path」）
  }, [nodeId, source, container, path, tail, timestamps, t, toast]);

  /**
   * 自动刷新：**上一轮完成后再等 5 秒发下一轮**，不做固定间隔的并发拉取。
   * 命令通道要等节点轮询（10s 一次）+ 执行 + 回执，一轮本身就要几秒；
   * 固定 2 秒间隔只会把命令堆在队列里，反而更慢。
   */
  const followRef = React.useRef(false);
  followRef.current = follow;
  const inFlight = React.useRef(false);
  // 用 ref 拿最新的 fetchLogs：否则「边打字边跟随」会每敲一个字符就重启循环、各发一条命令
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

  // 对话框关闭 / 切目标时停止跟随，别在后台一直发命令
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
        {/* 规范 08/10：节点列表加载失败不静默——就地给友好文案 + 重试 */}
        {nodesQ.isError && (
          <ErrorState
            compact
            message={t(friendlyError(nodesQ.error))}
            onRetry={() => void nodesQ.refetch()}
            retrying={nodesQ.isFetching}
          />
        )}
        <div className="flex flex-wrap items-end gap-3">
          {/* 固定宽度：别名/主机名都要能整段显示，但不随内容把弹窗撑宽 */}
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
            <div className="flex items-center gap-1 rounded-lg border border-surface-3 dark:border-ink-700 p-1">
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
            {/* 拉到了多少行：日志是一屏一屏看的，先给个量级 */}
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
            <pre className="max-h-[60vh] overflow-auto scrollbar-thin rounded-lg bg-ink-900 px-4 py-3 text-xs leading-relaxed text-surface-0 whitespace-pre-wrap break-all">
              {text.slice(-MAX_RENDER)}
            </pre>
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
