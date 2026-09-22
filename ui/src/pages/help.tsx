/**
 * 帮助页。
 *
 * 内容来自后端 `GET /v1/help`：加载 `crates/monitor-server/assets/help.md`
 * 整个 markdown，按 `## ` 二级标题切成一个个 section，每段一张
 * MarkdownCard，右侧保留悬浮目录（点击跳到对应锚点）。
 *
 * locale 现在只有中文版（`/v1/help` 当前固定返回 zh-CN）——英文进入时
 * 顶部提示「翻译暂未提供」，但仍渲染 markdown，避免换页直接空白。
 */
import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { BookOpen, RefreshCw } from "lucide-react";
import { help } from "@/api";
import { MarkdownCard } from "@/components/markdown-card";
import { Button } from "@/components/ui/button";
import {
  EmptyState,
  ErrorState,
  Skeleton,
} from "@/components/ui/feedback";
import { cn, friendlyError } from "@/lib/utils";

/**
 * 二级标题切分。`## X` 留在 section body 里（markdown 自身会渲染成 h2），
 * 仅用 `## X` 那一行作为锚点 id。`# X` 一级标题以及之前的整段作为 hero card。
 */
function splitSections(
  markdown: string,
): { hero: string; rest: { title: string; body: string }[] } {
  const lines = markdown.split("\n");
  const sections: { title: string; body: string[] }[] = [];
  let current: { title: string; body: string[] } | null = null;
  const hero: string[] = [];

  for (const line of lines) {
    const m = /^##\s+(.+?)\s*$/.exec(line);
    if (m) {
      if (current) sections.push(current);
      current = { title: m[1].trim(), body: [] };
    } else if (current) {
      current.body.push(line);
    } else {
      hero.push(line);
    }
  }
  if (current) sections.push(current);

  return {
    hero: hero.join("\n").trim(),
    rest: sections.map((s) => ({
      title: s.title,
      body: s.body.join("\n").trim(),
    })),
  };
}

/**
 * 标题转锚点 id。中文/标点替换成 `-`；emoji 之类非字母数字字符会被剥掉。
 * 不追求 RFC 完备——章节名是后端固定的有限集合。
 */
function slugify(title: string): string {
  return (
    title
      .toLowerCase()
      .replace(/[\s_]+/g, "-")
      .replace(/[^\p{L}\p{N}\-]+/gu, "")
      .replace(/-+/g, "-")
      .replace(/^-|-$/g, "")
    || "section"
  );
}

export function HelpPage() {
  const { t, i18n } = useTranslation();
  const isEn = i18n.language?.toLowerCase().startsWith("en");

  const q = useQuery({
    queryKey: ["help"],
    queryFn: help.fetch,
    // 帮助页内容稳定，让 react-query 缓存更久
    staleTime: 60_000,
  });

  const sections = React.useMemo(() => {
    const body = q.data?.body ?? "";
    return splitSections(body);
  }, [q.data?.body]);

  const [active, setActive] = React.useState<string | null>(null);

  // 滚动高亮：目录要能指示「现在读到哪一节」
  React.useEffect(() => {
    const ids = sections.rest.map((s) => slugify(s.title));
    const els = ids
      .map((id) => document.getElementById(id))
      .filter((e): e is HTMLElement => !!e);
    if (els.length === 0) {
      setActive(null);
      return;
    }
    const ob = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((e) => e.isIntersecting)
          .sort(
            (a, b) =>
              a.boundingClientRect.top - b.boundingClientRect.top,
          );
        if (visible[0]) setActive(visible[0].target.id);
      },
      { rootMargin: "-96px 0px -70% 0px", threshold: 0 },
    );
    els.forEach((e) => ob.observe(e));
    return () => ob.disconnect();
  }, [sections]);

  return (
    <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_200px]">
      <article className="space-y-8 min-w-0">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <h1 className="text-xl font-semibold text-ink-900 dark:text-surface-0">
              {t("help.title")}
            </h1>
          </div>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void q.refetch()}
            loading={q.isFetching}
            aria-label={t("help.refresh")}
          >
            <RefreshCw className="w-4 h-4" aria-hidden="true" />
            {t("help.refresh")}
          </Button>
        </div>

        {isEn && (
          <div
            className={cn(
              "flex items-start gap-2 rounded-lg",
              "bg-surface-2 dark:bg-ink-700/60 px-4 py-3",
            )}
            role="note"
          >
            <BookOpen
              className="w-4 h-4 mt-0.5 shrink-0 text-ink-400"
              aria-hidden="true"
            />
            <p className="text-sm text-ink-500">
              {t("help.untranslatedHint")}
            </p>
          </div>
        )}

        {q.isLoading ? (
          <>
            <Skeleton className="h-8 w-1/2" />
            <Skeleton className="h-40 w-full" />
            <Skeleton className="h-40 w-full" />
          </>
        ) : q.isError ? (
          <ErrorState
            message={t(friendlyError(q.error))}
            onRetry={() => void q.refetch()}
            retrying={q.isFetching}
          />
        ) : !q.data?.body ? (
          <EmptyState
            title={t("help.empty")}
            description={t("help.emptyHint")}
          />
        ) : (
          <>
            {sections.hero && <MarkdownCard>{sections.hero}</MarkdownCard>}
            {sections.rest.length === 0
              ? null
              : sections.rest.map((s) => {
                  const id = slugify(s.title);
                  return (
                    <section
                      key={id}
                      id={id}
                      className="scroll-mt-20"
                      aria-label={s.title}
                    >
                      <MarkdownCard>{`## ${s.title}\n\n${s.body}`}</MarkdownCard>
                    </section>
                  );
                })}
          </>
        )}
      </article>

      {/* 右侧悬浮目录 */}
      {sections.rest.length > 0 && (
        <nav aria-label={t("help.toc")} className="hidden lg:block">
          <div className="sticky top-2">
            <p className="text-xs font-medium text-ink-400 mb-2">
              {t("help.toc")}
            </p>
            <ul className="space-y-1 border-l border-surface-3 dark:border-ink-700">
              {sections.rest.map((s) => {
                const id = slugify(s.title);
                return (
                  <li key={id}>
                    <a
                      href={`#${id}`}
                      className={cn(
                        "-ml-px block border-l-2 pl-3 py-1 text-sm transition-colors",
                        active === id
                          ? "border-brand-600 text-brand-700 dark:text-brand-100 font-medium"
                          : "border-transparent text-ink-500 hover:text-ink-900 dark:hover:text-surface-0",
                      )}
                    >
                      {s.title}
                    </a>
                  </li>
                );
              })}
            </ul>
          </div>
        </nav>
      )}
    </div>
  );
}
