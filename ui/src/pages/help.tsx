/**
 * Help page markdown.
 *
 * Content comes from `GET /v1/help?locale=...` — the backend embeds
 * multi-language versions at compile time from `crates/monitor-server/assets/`
 * and returns the appropriate translation based on the UI locale; on missing
 * or unrecognized translations it silently falls back to zh-CN. MarkdownCard
 * renders the entire markdown as a text block, and the TOC is displayed in
 * the right-side floating panel, split by sections marked with `## `.
 */
import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { help } from "@/api";
import { MarkdownCard } from "@/components/markdown-card";
import {
  EmptyState,
  ErrorState,
  Skeleton,
} from "@/components/ui/feedback";
import { cn } from "@/lib/utils";

/**
 * Split section bodies by `## X` markers in markdown, used for rendering:
 * the section title goes into the TOC and gets a stable id anchor; the hero
 * portion (content before the first `##`) is rendered separately at the top.
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
 * Generate a stable DOM id from the section title (for TOC anchors). Chinese
 * / emoji / punctuation are all allowed; any non-RFC-letter character is
 * replaced with `-`; an empty title falls back to a generic name to avoid
 * duplicate ids.
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

  const locale = React.useMemo(
    () => i18n.language || "en-US",
    [i18n.language],
  );

  const q = useQuery({
    // locale goes into the key: switching the UI language immediately re-fetches
    // for the target locale without clearing the cache.
    queryKey: ["help", locale],
    queryFn: () => help.fetch(locale),
    staleTime: 60_000,
  });

  const sections = React.useMemo(() => {
    const body = q.data?.body ?? "";
    return splitSections(body);
  }, [q.data?.body]);

  const [active, setActive] = React.useState<string | null>(null);

  // Highlight the current section while scrolling: observe each section's
  // distance from the top, pick the topmost visible one.
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
        <h1 className="text-xl font-semibold text-ink-900 dark:text-surface-0">
          {t("help.title")}
        </h1>

        {q.isLoading ? (
          <>
            <Skeleton className="h-8 w-1/2" />
            <Skeleton className="h-40 w-full" />
            <Skeleton className="h-40 w-full" />
          </>
        ) : q.isError ? (
          <ErrorState
            message={t("help.empty")}
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

      {/* Right-side floating panel: table of contents for this page */}
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
