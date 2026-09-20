import * as React from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { setToken } from "@/api";

/** 规范 07-3.2 居中卡片布局 + 3.3 登录页通用规则（无导航栏，保留悬浮面板）。 */
export function LoginPage({ onSubmit }: { onSubmit: () => void }) {
  const { t } = useTranslation();
  const [value, setValue] = React.useState("");
  const [error, setError] = React.useState("");
  const [submitting, setSubmitting] = React.useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const token = value.trim();
    if (!token) {
      setError(t("login.required"));
      return;
    }
    setError("");
    setSubmitting(true);
    try {
      // /v1 永不返回 401（仅靠 body.authenticated 区分鉴权结果），
      // 用它做「先验证再放行」比直接进控制台等 401 友好得多——
      // 也能挡住「粘贴了过期/错位 token」这种典型踩坑。
      const res = await fetch("/v1", {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!res.ok) throw new Error("network");
      const body = (await res.json()) as { authenticated?: boolean };
      if (body.authenticated !== true) throw new Error("invalid");
      setToken(token);
      onSubmit();
    } catch {
      setSubmitting(false);
      setError(t("login.invalid"));
    }
  }

  return (
    <div className="min-h-screen flex items-center justify-center px-4 bg-surface-2 dark:bg-ink-900">
      <div className="w-full max-w-sm">
        <div className="w-12 h-12 rounded-lg bg-brand-600 mx-auto mb-8 flex items-center justify-center">
          <span className="text-white text-lg font-semibold">
            {t("app.name").slice(0, 1)}
          </span>
        </div>

        <div className="bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-700 p-8">
          <h1 className="text-base font-semibold text-ink-900 dark:text-surface-0 text-center">
            {t("login.title")}
          </h1>
          <p className="text-sm text-ink-500 text-center mt-1">
            {t("login.hint")}
          </p>
          <code className="mt-2 block text-center text-xs text-ink-400">
            cat &lt;data-dir&gt;/admin.token
          </code>

          <form className="space-y-4 mt-6" onSubmit={submit}>
            <div>
              <Input
                type="password"
                autoFocus
                autoComplete="current-password"
                placeholder={t("login.placeholder")}
                value={value}
                onChange={(e) => {
                  setValue(e.target.value);
                  if (error) setError("");
                }}
                aria-invalid={!!error}
              />
              {error && (
                <p className="text-xs text-rose-600 mt-1">{error}</p>
              )}
            </div>
            <Button type="submit" className="w-full" disabled={!value.trim() || submitting}>
              {submitting ? t("login.verifying") : t("login.submit")}
            </Button>
          </form>

          <p className="text-xs text-ink-400 text-center mt-6">
            {t("login.footnote")}
          </p>
        </div>
      </div>
    </div>
  );
}
