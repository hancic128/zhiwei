import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/utils";

/**
 * 帮助页里渲染一段 markdown 用。卡片化是为了和其它设置 section 在视觉上对齐，
 * 避免长段 markdown 直接落在页面底色上读着累。
 *
 * 内置 markdown 排版样式（h1-h4 / p / ul / ol / code / pre / table / a / hr）。
 * 不依赖 @tailwindcss/typography（项目没有那个插件），全部用项目自己的
 * surface / ink / brand 令牌，保持主题一致。
 */
export function MarkdownCard({
  children,
  className,
}: {
  children: string;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "bg-surface-0 dark:bg-ink-700 rounded-xl shadow-sm",
        "border border-surface-3 dark:border-ink-700 p-6",
        "markdown-body",
        className,
      )}
    >
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{children}</ReactMarkdown>
    </div>
  );
}
