import * as React from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { api, type NodeView } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";
import { friendlyError } from "@/lib/utils";

/** 与后端 crates/monitor-server/src/routes.rs 的上限保持一致 */
const MAX_ALIAS_CHARS = 10;
const MAX_TAGS = 10;
const MAX_TAG_CHARS = 24;
/**
 * 输入标签时的分隔符：空白 / 英文逗号 / 中文逗号 / 顿号。
 *
 * 只在**提交**（回车 / 失焦）时按它切分，不在 `onChange` 里边输边切——
 * 中文输入法在拼音候选期间也会逐字触发 change，边输边切会把「还没上屏的词」
 * 拆成几个标签（用户报的 bug）。
 */
const TAG_SEPARATORS = /[\s,，、]/;

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
  /**
   * 输入法是否正在组词。
   *
   * 组词期间按回车是在「选候选字」，不是在「提交标签」——必须放行，
   * 否则一次中文输入会先被切开、又把没上屏的内容吞掉。
   * `isComposing` 在 Chrome 的 keydown 上可靠，Safari 需要靠组合事件兜底，
   * 所以两者都看。
   */
  const composing = React.useRef(false);

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

  /** 把输入框里的草稿按分隔符拆开并成标签（回车 / 失焦时调用） */
  const commitDraft = React.useCallback(() => {
    setTags((cur) => mergeTags(cur, draft.split(TAG_SEPARATORS)));
    setDraft("");
  }, [draft, mergeTags]);

  const save = useMutation({
    mutationFn: () =>
      api.updateNode(node!.id, {
        alias: alias.trim(),
        // 输入框里还没回车的草稿也要一并带上：点「保存」时 onBlur 的 setState
        // 还没生效，只读 tags 会把刚打完的最后一个标签丢掉。
        tags: mergeTags(tags, draft.split(TAG_SEPARATORS))
          .map((x) => x.trim())
          .filter(Boolean),
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
              onChange={(e) => setDraft(e.target.value)}
              onCompositionStart={() => {
                composing.current = true;
              }}
              onCompositionEnd={() => {
                composing.current = false;
              }}
              onKeyDown={(e) => {
                // 组词中的回车 / 空格是输入法在选字，绝不能当成分隔
                if (composing.current || e.nativeEvent.isComposing) return;
                if (e.key === "Enter") {
                  e.preventDefault();
                  commitDraft();
                } else if (e.key === "Backspace" && !draft && tags.length > 0) {
                  setTags((cur) => cur.slice(0, -1));
                }
              }}
              onBlur={commitDraft}
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

/** 标签列表（只读展示），列表页 / 详情页共用；统一用徽章样式 */
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
        <Badge key={tag} tone="neutral">
          {tag}
        </Badge>
      ))}
    </span>
  );
}
