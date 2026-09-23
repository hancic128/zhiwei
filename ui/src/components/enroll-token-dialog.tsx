import * as React from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Copy, Loader2, Plus } from "lucide-react";
import { enrollTokens, type EnrollTokenCreated } from "@/api";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { useToast } from "@/components/ui/toast";
import { cn, copyText, friendlyError } from "@/lib/utils";

/**
 * 生成一次性入网命令对话框。
 *
 * 三档 TTL（1h / 24h / 7d，默认 24h）。提交成功展示整段可直接粘贴的
 * `curl | bash` 命令；过期时间走「n 小时 / n 天」相对描述，避免
 * 在用户跨时区时算不准。
 *
 * 复用现有 `<Dialog>` 模板（7.10 节）：遮罩 + 标题 + 关闭按钮 + 危险操作提示。
 */
const TTL_OPTIONS = [
  { value: 3600, key: "ttl1h" },
  { value: 86_400, key: "ttl24h" },
  { value: 7 * 86_400, key: "ttl7d" },
] as const;

/** 用相对时长短语表达「N 小时 / N 天后过期」 */
function formatExpiresIn(seconds: number): string {
  if (seconds <= 0) return "0";
  if (seconds < 3600) {
    const m = Math.max(1, Math.ceil(seconds / 60));
    return `${m}m`;
  }
  if (seconds < 86_400) {
    const h = Math.round(seconds / 3600);
    return `${h}h`;
  }
  const d = Math.round(seconds / 86_400);
  return `${d}d`;
}

export function EnrollTokenDialog({
  open,
  onClose,
  autoCreate = false,
}: {
  open: boolean;
  onClose: () => void;
  /**
   * 「接入帮助」用：打开即用默认 TTL 生成命令并自动复制，省掉
   * 「选 TTL → 点创建 → 再点复制」三步。普通入口仍走表单。
   */
  autoCreate?: boolean;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [ttl, setTtl] = React.useState<number>(86_400);
  const [label, setLabel] = React.useState("");
  const [created, setCreated] = React.useState<EnrollTokenCreated | null>(null);

  // 关闭时清掉输入 + 已展示的命令，避免下次打开还看见上一份结果
  React.useEffect(() => {
    if (!open) {
      setLabel("");
      setCreated(null);
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      enrollTokens.create({
        ttl_secs: ttl,
        label: label.trim() ? label.trim() : undefined,
      }),
    onSuccess: (r) => setCreated(r),
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const copy = (text: string, okMsg: string) =>
    void copyText(text).then((ok) =>
      toast.push(ok ? "success" : "error", ok ? okMsg : t("toast.copyFailed")),
    );

  // 自动生成：只在每次打开后触发一次（ref 挡住 effect 的重复执行）
  const autoStarted = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      autoStarted.current = false;
      return;
    }
    if (!autoCreate || autoStarted.current) return;
    autoStarted.current = true;
    create.mutate();
    // create.mutate 在 react-query v5 里是稳定引用，不必进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, autoCreate]);

  // 自动复制：命令生成后立刻写入剪贴板，失败也不阻断（弹窗里还有复制按钮）
  const autoCopied = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      autoCopied.current = false;
      return;
    }
    if (!autoCreate || !created || autoCopied.current) return;
    autoCopied.current = true;
    copy(created.enroll_command, t("dialog.commandAutoCopied"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, autoCreate, created]);

  const close = () => {
    if (create.isPending) return;
    onClose();
  };

  return (
    <Dialog
      open={open}
      onClose={close}
      size="lg"
      title={
        created
          ? t("dialog.enrollTokenCreatedTitle")
          : t("dialog.createEnrollToken")
      }
      description={
        created
          ? t("dialog.enrollTokenCreatedDesc")
          : t("dialog.createEnrollTokenDesc")
      }
      footer={
        created ? (
          <Button variant="secondary" onClick={close}>
            {t("action.close")}
          </Button>
        ) : autoCreate ? (
          <Button variant="secondary" onClick={close}>
            {t("action.close")}
          </Button>
        ) : (
          <>
            <Button variant="secondary" onClick={close}>
              {t("action.cancel")}
            </Button>
            <Button
              loading={create.isPending}
              onClick={() => create.mutate()}
            >
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("action.create")}
            </Button>
          </>
        )
      }
    >
      {created ? (
        <div className="space-y-4">
          <p className="text-sm text-ink-700 dark:text-surface-4">
            {t("dialog.expiresIn", {
              at: formatExpiresIn(
                created.expires_at_unix - Math.floor(Date.now() / 1000),
              ),
            })}
            {created.label && (
              <span className="ml-2 text-xs text-ink-400">
                ({created.label})
              </span>
            )}
          </p>
          <div className="relative">
            <pre
              className={cn(
                "overflow-x-auto scrollbar-thin rounded-md",
                "bg-surface-2 dark:bg-ink-700/60 px-3 py-2 pr-12",
                "text-xs font-mono text-ink-700 dark:text-surface-4",
                "whitespace-pre-wrap break-all",
              )}
            >
              {created.enroll_command}
            </pre>
            <Button
              variant="secondary"
              size="sm"
              aria-label={t("dialog.copyCommand")}
              className="absolute top-2 right-2"
              onClick={() =>
                copy(created.enroll_command, t("dialog.commandCopied"))
              }
            >
              <Copy className="w-4 h-4" aria-hidden="true" />
            </Button>
          </div>
          <p className="text-xs text-ink-400">
            {t("dialog.enrollTokenSecretWarn")}
          </p>
          {/* 节点侧也能自带别名 / 标签，这里点一句，免得用户装完再一台台补 */}
          <p className="text-xs text-ink-400">
            {t("dialog.enrollOptionalMeta")}
          </p>
        </div>
      ) : autoCreate ? (
        <div className="py-8 text-center text-sm text-ink-500">
          {create.isError ? (
            <p className="text-rose-600 dark:text-rose-400">
              {t(friendlyError(create.error))}
            </p>
          ) : (
            <span className="inline-flex items-center gap-2">
              <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
              {t("dialog.enrollTokenGenerating")}
            </span>
          )}
        </div>
      ) : (
        <div className="space-y-4">
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("dialog.ttlLabel")}
            </span>
            <Select
              value={String(ttl)}
              onChange={(e) => setTtl(Number(e.target.value))}
              aria-label={t("dialog.ttlLabel")}
            >
              {TTL_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {t(`dialog.${o.key}`)}
                </option>
              ))}
            </Select>
          </label>
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("dialog.labelLabel")}
            </span>
            <Input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={t("dialog.labelPlaceholder")}
              maxLength={64}
            />
            <span className="mt-1 block text-xs text-ink-400">
              {t("dialog.labelHint")}
            </span>
          </label>
          {create.isError && (
            <p className="text-sm text-rose-600 dark:text-rose-400">
              {t(friendlyError(create.error))}
            </p>
          )}
        </div>
      )}
    </Dialog>
  );
}
