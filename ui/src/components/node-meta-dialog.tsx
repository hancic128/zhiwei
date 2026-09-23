import * as React from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { api, type NodeView } from "@/api";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";
import { friendlyError } from "@/lib/utils";

/** 与后端 crates/monitor-server/src/routes.rs 的上限保持一致 */
const MAX_ALIAS_CHARS = 10;
const MAX_TAGS = 10;
const MAX_TAG_CHARS = 24;
/** 输入标签时的分隔符：英文逗号 / 中文逗号 / 顿号 */
const TAG_SEPARATORS = /[,，、]/;

/**
 * 编辑节点的别名与标签。
 *
 * 别名是「一眼认出来」的短名（≤10 字符），列表与下拉框优先显示它；
 * 标签用于过滤（≤10 个）。hostname 由节点自报，不在这里改。
 */
export function NodeMetaDialog({
  node,
  onClose,
}: {
  /** null = 关闭 */
  node: NodeView | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();

  const [alias, setAlias] = React.useState("");
  const [tags, setTags] = React.useState<string[]>([]);
  const [draft, setDraft] = React.useState("");

  // 每次打开都从节点现值重新初始化，避免上一次编辑残留
  React.useEffect(() => {
    if (!node) return;
    setAlias(node.alias ?? "");
    setTags(node.tags ?? []);
    setDraft("");
  }, [node]);

  /** 把一批原始输入并进标签列表：去空白、去重、按上限截断 */
  const mergeTags = React.useCallback(
    (current: string[], additions: string[]) => {
      const out = [...current];
      for (const raw of additions) {
        const tag = raw.trim();
        if (!tag) continue;
        if (out.length >= MAX_TAGS) {
          toast.push("warn", t("nodeMeta.tagsFull", { n: MAX_TAGS }));
          break;
        }
        if (tag.length > MAX_TAG_CHARS) {
          toast.push("warn", t("nodeMeta.tagTooLong", { n: MAX_TAG_CHARS }));
          continue;
        }
        if (!out.includes(tag)) out.push(tag);
      }
      return out;
    },
    [t, toast],
  );

  const save = useMutation({
    mutationFn: () =>
      api.updateNode(node!.id, {
        alias: alias.trim(),
        tags: tags.map((x) => x.trim()).filter(Boolean),
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["nodes"] });
      void qc.invalidateQueries({ queryKey: ["containers"] });
      toast.push("success", t("nodeMeta.saved"));
      onClose();
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  return (
    <Dialog
      open={!!node}
      onClose={onClose}
      title={t("nodeMeta.title")}
      description={node ? `${node.id} · ${node.hostname}` : undefined}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("action.cancel")}
          </Button>
          <Button loading={save.isPending} onClick={() => save.mutate()}>
            {t("action.save")}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <label className="block">
          <span className="block text-xs text-ink-500 mb-1">
            {t("nodeMeta.aliasLabel")}
          </span>
          {/* 宽度固定但足够长：别名上限 10 个字符，w-64 能整段显示不被截断 */}
          <Input
            className="w-64"
            value={alias}
            onChange={(e) => setAlias(e.target.value)}
            maxLength={MAX_ALIAS_CHARS}
            placeholder={t("nodeMeta.aliasPlaceholder")}
            aria-label={t("nodeMeta.aliasLabel")}
          />
          <span className="mt-1 block text-xs text-ink-400">
            {t("nodeMeta.aliasHint", { n: MAX_ALIAS_CHARS })}
          </span>
        </label>

        <div>
          <span className="block text-xs text-ink-500 mb-1">
            {t("nodeMeta.tagsLabel")}
          </span>
          <div className="flex flex-wrap items-center gap-1.5 rounded-md border border-surface-3 dark:border-ink-700 bg-surface-0 dark:bg-ink-700 px-2 py-1.5">
            {tags.map((tag) => (
              <span
                key={tag}
                className="inline-flex items-center gap-1 rounded bg-surface-2 dark:bg-ink-700 px-2 py-0.5 text-xs text-ink-700 dark:text-surface-4"
              >
                {tag}
                <button
                  type="button"
                  aria-label={t("nodeMeta.removeTag", { tag })}
                  className="text-ink-400 hover:text-rose-600 transition-colors"
                  onClick={() => setTags((cur) => cur.filter((x) => x !== tag))}
                >
                  <X className="w-3 h-3" aria-hidden="true" />
                </button>
              </span>
            ))}
            <input
              className="h-6 min-w-[8rem] flex-1 bg-transparent text-sm text-ink-900 dark:text-surface-0 outline-none placeholder:text-ink-400"
              value={draft}
              maxLength={MAX_TAG_CHARS}
              placeholder={t("nodeMeta.tagsPlaceholder")}
              aria-label={t("nodeMeta.tagsLabel")}
              onChange={(e) => {
                const v = e.target.value;
                if (!TAG_SEPARATORS.test(v)) {
                  setDraft(v);
                  return;
                }
                const segs = v.split(TAG_SEPARATORS);
                // 结尾不是分隔符时，最后一段是还没输完的，留在输入框里
                const trailing = TAG_SEPARATORS.test(v.slice(-1))
                  ? ""
                  : (segs.pop() ?? "");
                setTags((cur) => mergeTags(cur, segs));
                setDraft(trailing);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  setTags((cur) => mergeTags(cur, [draft]));
                  setDraft("");
                } else if (e.key === "Backspace" && !draft && tags.length > 0) {
                  setTags((cur) => cur.slice(0, -1));
                }
              }}
              onBlur={() => {
                setTags((cur) => mergeTags(cur, [draft]));
                setDraft("");
              }}
            />
          </div>
          <span className="mt-1 block text-xs text-ink-400">
            {t("nodeMeta.tagsHint", { n: MAX_TAGS })}
          </span>
        </div>
      </div>
    </Dialog>
  );
}

/** 标签列表（只读展示），列表页 / 详情页共用 */
export function TagList({
  tags,
  className,
}: {
  tags: string[];
  className?: string;
}) {
  if (tags.length === 0) return null;
  return (
    <span className={className}>
      {tags.map((tag) => (
        <span
          key={tag}
          className="inline-flex items-center rounded bg-surface-2 dark:bg-ink-700 px-1.5 py-0.5 text-[11px] text-ink-500 dark:text-surface-4"
        >
          {tag}
        </span>
      ))}
    </span>
  );
}
