import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/utils";

/**
 * Renders a markdown snippet for the help page. Card styling is used to align visually
 * with other settings sections and avoid long markdown blobs sitting directly on the page background.
 *
 * Built-in markdown styling (h1-h4 / p / ul / ol / code / pre / table / a / hr).
 * Doesn't depend on @tailwindcss/typography (the project doesn't have that plugin);
 * uses the project's own surface / ink / brand tokens for theme consistency.
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
