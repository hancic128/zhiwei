import * as React from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { LogoMark } from "@/components/ui/logo";
import { setToken } from "@/api";

/** Spec 07-3.2 centered card layout + 3.3 login page common rules (no navbar, keep floating panel). */
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
      // /v1 never returns 401 (it only signals auth result via body.authenticated),
      // using it for "verify before letting in" is much friendlier than entering
      // the console and waiting for a 401 — and it also blocks the typical
      // pitfall of pasting an expired or mismatched token.
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
        {/* Same logo as the tab icon / sidebar: no backdrop, no border, color follows the theme */}
        <LogoMark className="w-12 h-12 mx-auto mb-8 text-brand-600 dark:text-brand-500" />

        <div className="bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-700 p-8">
          <h1 className="text-base font-semibold text-ink-900 dark:text-surface-0 text-center">
            {t("login.title")}
          </h1>

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
        </div>
      </div>
    </div>
  );
}
