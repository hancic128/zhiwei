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

/** Keep the upper bound in sync with crates/monitor-server/src/routes.rs */
const MAX_ALIAS_CHARS = 10;
const MAX_TAGS = 10;
const MAX_TAG_CHARS = 24;
/**
 * Tag input separators: whitespace / ASCII comma / full-width comma / Chinese enumeration comma.
 *
 * Split only on **commit** (Enter / blur), not on every `onChange` keystroke —
 * CJK input methods fire change events one character at a time during pinyin
 * composition. Splitting eagerly would chop the not-yet-confirmed word into
 * multiple tags (user-reported bug).
 */
const TAG_SEPARATORS = /[\s,,、]/;

/**
 * Edit a node's alias and tags.
 *
 * The alias is a short recognizable name (≤10 chars), shown preferentially
 * in lists and dropdowns. Tags are used for filtering (≤10). Hostname is
 * self-reported by the node and not editable here.
 */
export function NodeMetaDialog({
  node,
  onClose,
}: {
  /** null = closed */
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
   * Whether the IME is currently composing a word.
   *
   * While composing, pressing Enter selects a candidate character — it is
   * not "submit tags". We must let it through; otherwise one Chinese input
   * gets split open AND swallows the uncommitted text.
   * `isComposing` is reliable on Chrome's keydown, but Safari needs the
   * composition events as fallback, so we check both.
   */
  const composing = React.useRef(false);

  // Re-initialize from the node's current values every time the dialog opens,
  // so leftover edits from a previous open don't carry over.
  React.useEffect(() => {
    if (!node) return;
    setAlias(node.alias ?? "");
    setTags(node.tags ?? []);
    setDraft("");
  }, [node]);

  /** Merge a batch of raw tag inputs into the tag list: trim whitespace, dedupe, truncate to the limit */
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

  /** Split the input draft by separators and merge into the tag list (called on Enter / blur) */
  const commitDraft = React.useCallback(() => {
    setTags((cur) => mergeTags(cur, draft.split(TAG_SEPARATORS)));
    setDraft("");
  }, [draft, mergeTags]);

  const save = useMutation({
    mutationFn: () =>
      api.updateNode(node!.id, {
        alias: alias.trim(),
        // The not-yet-Entered draft must be carried along too: when the user
        // clicks Save, the onBlur setState hasn't taken effect yet, so reading
        // `tags` would silently drop the just-typed last tag.
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
          {/* Fixed but generous width: alias limit is 10 chars, w-64 fits the whole string without truncation */}
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
          <div className="flex flex-wrap items-center gap-1.5 rounded-md border border-surface-3 dark:border-ink-500 bg-surface-0 dark:bg-ink-700 px-2 py-1.5">
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
                // Enter / Space during composition is the IME selecting a candidate — never treat it as a separator
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

/** Tag list (read-only display), shared by list page / detail page; uniform badge style */
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
