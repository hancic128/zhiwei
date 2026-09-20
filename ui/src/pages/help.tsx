/**
 * 帮助页。
 *
 * 排版是**正常文档层级**（章节标题 + 正文 + 子标题），不堆卡片——卡片会把
 * 一页连贯的说明切碎，也不利于滚动阅读。右侧是固定的悬浮目录。
 *
 * 只写四件事：是什么 / 怎么快速接入 / AI 怎么用 / 部署与证书。
 * 界面上已经一眼能看到的（菜单在哪、按钮干嘛、数据留多久）不写。
 */
import * as React from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** 目录项的锚点 id —— 与正文里的 <section id> 一一对应 */
const SECTIONS = ["what", "onboard", "ai", "deploy"] as const;

export function HelpPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [active, setActive] = React.useState<string>(SECTIONS[0]);

  // 滚动高亮：目录要能指示「现在读到哪一节」，否则固定在那儿也只是装饰
  React.useEffect(() => {
    const els = SECTIONS.map((id) => document.getElementById(id)).filter(
      (e): e is HTMLElement => !!e,
    );
    if (els.length === 0) return;
    const ob = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((e) => e.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
        if (visible[0]) setActive(visible[0].target.id);
      },
      { rootMargin: "-96px 0px -70% 0px", threshold: 0 },
    );
    els.forEach((e) => ob.observe(e));
    return () => ob.disconnect();
  }, []);

  return (
    <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_200px]">
      <article className="space-y-8 min-w-0">
        <Section id="what" title={t("help.whatTitle")}>
          <p className="text-sm text-ink-700 dark:text-surface-4">
            {t("help.what1")}
          </p>
          <p className="text-sm text-ink-500">{t("help.what2")}</p>
        </Section>

        <Section id="onboard" title={t("help.onboardTitle")}>
          <p className="text-sm text-ink-500">{t("help.onboardSubtitle")}</p>
          <Sub title={t("help.onboard1Title")}>
            <Code>{t("help.onboard1Cmd")}</Code>
            <Note>{t("help.onboard1Note")}</Note>
          </Sub>
          <Sub title={t("help.onboard2Title")}>
            <Code>{t("help.onboard2Cmd")}</Code>
            <Note>{t("help.onboard2Note")}</Note>
          </Sub>
          <Sub title={t("help.onboard3Title")}>
            <Code>{t("help.onboard3Cmd")}</Code>
          </Sub>
        </Section>

        <Section id="ai" title={t("help.aiTitle")}>
          <p className="text-sm text-ink-500">{t("help.ai1")}</p>
          <p className="text-sm text-ink-500">{t("help.ai2")}</p>
          <p className="text-sm text-ink-500">{t("help.ai3")}</p>
          <p className="text-xs text-ink-400">{t("help.aiBoundary")}</p>
          <div>
            <Button variant="secondary" size="sm" onClick={() => navigate("/settings")}>
              {t("help.aiGoSettings")}
            </Button>
          </div>
        </Section>

        <Section id="deploy" title={t("help.deployTitle")}>
          <p className="text-sm text-ink-500">{t("help.deploy1")}</p>
          <p className="text-sm text-ink-500">{t("help.deploy2")}</p>
          <p className="text-sm text-ink-500">{t("help.deploy3")}</p>
        </Section>
      </article>

      {/* 固定悬浮目录 */}
      <nav aria-label={t("help.toc")} className="hidden lg:block">
        <div className="sticky top-2">
          <p className="text-xs font-medium text-ink-400 mb-2">{t("help.toc")}</p>
          <ul className="space-y-1 border-l border-surface-3 dark:border-ink-700">
            {SECTIONS.map((id) => (
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
                  {t(`help.${id}Title`)}
                </a>
              </li>
            ))}
          </ul>
        </div>
      </nav>
    </div>
  );
}

function Section({
  id,
  title,
  children,
}: {
  id: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section id={id} className="scroll-mt-20 space-y-3">
      <h2 className="text-lg font-semibold text-ink-900 dark:text-surface-0">
        {title}
      </h2>
      {children}
    </section>
  );
}

function Sub({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1">
      <h3 className="text-sm font-medium text-ink-900 dark:text-surface-0">
        {title}
      </h3>
      {children}
    </div>
  );
}

function Code({ children }: { children: React.ReactNode }) {
  return (
    <pre className="overflow-x-auto scrollbar-thin rounded-md bg-surface-2 dark:bg-ink-700/60 px-3 py-2 text-xs font-mono text-ink-700 dark:text-surface-4 whitespace-pre-wrap break-all">
      {children}
    </pre>
  );
}

function Note({ children }: { children: React.ReactNode }) {
  return <p className="text-xs text-ink-400">{children}</p>;
}
