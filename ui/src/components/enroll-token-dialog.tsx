import * as React from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Copy, Loader2, Plus } from "lucide-react";
import { enrollTokens, type EnrollTokenCreated } from "@/api";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { useToast } from "@/components/ui/toast";
import { cn, copyText, friendlyError } from "@/lib/utils";

/**
 * One-time enroll command generation dialog.
 *
 * Three TTL tiers (1h / 24h / 7d, default 24h). On success, display the full
 * paste-ready `curl | bash` command. Expiration is rendered as a relative
 * "n hours / n days" phrase to avoid time-zone math mistakes by the user.
 *
 * Reuses the existing `<Dialog>` template (section 7.10): backdrop + title +
 * close button + danger-action hint.
 */
const TTL_OPTIONS = [
  { value: 3600, key: "ttl1h" },
  { value: 86_400, key: "ttl24h" },
  { value: 7 * 86_400, key: "ttl7d" },
] as const;

/** Express "expires in N hours / N days" using a relative duration phrase */
function formatExpiresIn(seconds: number): string {
  if (seconds <= 0) return "0";
  if (seconds < 3600) {
    const m = Math.max(1, Math.ceil(seconds / 60));
    return `${m}m`;
  }
  if (seconds < 86_400) {
    const h = Math.round(seconds / 3600);
    return `${h}h`;
  }
  const d = Math.round(seconds / 86_400);
  return `${d}d`;
}

export function EnrollTokenDialog({
  open,
  onClose,
  autoCreate = false,
}: {
  open: boolean;
  onClose: () => void;
  /**
   * Used by "enroll help": on open, generate a command with the default TTL
   * and auto-copy it, skipping the "choose TTL -> click create -> click copy"
   * three-step flow. The normal entry point still uses the form.
   */
  autoCreate?: boolean;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [ttl, setTtl] = React.useState<number>(86_400);
  const [label, setLabel] = React.useState("");
  const [created, setCreated] = React.useState<EnrollTokenCreated | null>(null);

  // On close, clear the input and any displayed command so reopening doesn't show stale results
  React.useEffect(() => {
    if (!open) {
      setLabel("");
      setCreated(null);
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      enrollTokens.create({
        ttl_secs: ttl,
        label: label.trim() ? label.trim() : undefined,
      }),
    onSuccess: (r) => setCreated(r),
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const copy = (text: string, okMsg: string) =>
    void copyText(text).then((ok) =>
      toast.push(ok ? "success" : "error", ok ? okMsg : t("toast.copyFailed")),
    );

  // Auto-generate: only triggers once per open (the ref blocks the effect from re-running)
  const autoStarted = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      autoStarted.current = false;
      return;
    }
    if (!autoCreate || autoStarted.current) return;
    autoStarted.current = true;
    create.mutate();
    // create.mutate is a stable reference in react-query v5; no need to add it as a dep
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, autoCreate]);

  // Auto-copy: once the command is generated, write it to the clipboard
  // immediately; failure doesn't block (the dialog still has a copy button)
  const autoCopied = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      autoCopied.current = false;
      return;
    }
    if (!autoCreate || !created || autoCopied.current) return;
    autoCopied.current = true;
    copy(created.enroll_command, t("dialog.commandAutoCopied"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, autoCreate, created]);

  const close = () => {
    if (create.isPending) return;
    onClose();
  };

  return (
    <Dialog
      open={open}
      onClose={close}
      size="lg"
      title={
        created
          ? t("dialog.enrollTokenCreatedTitle")
          : t("dialog.createEnrollToken")
      }
      description={
        created
          ? t("dialog.enrollTokenCreatedDesc")
          : t("dialog.createEnrollTokenDesc")
      }
      footer={
        created ? (
          <Button variant="secondary" onClick={close}>
            {t("action.close")}
          </Button>
        ) : autoCreate ? (
          <Button variant="secondary" onClick={close}>
            {t("action.close")}
          </Button>
        ) : (
          <>
            <Button variant="secondary" onClick={close}>
              {t("action.cancel")}
            </Button>
            <Button
              loading={create.isPending}
              onClick={() => create.mutate()}
            >
              <Plus className="w-4 h-4" aria-hidden="true" />
              {t("action.create")}
            </Button>
          </>
        )
      }
    >
      {created ? (
        <div className="space-y-4">
          <p className="text-sm text-ink-700 dark:text-surface-4">
            {t("dialog.expiresIn", {
              at: formatExpiresIn(
                created.expires_at_unix - Math.floor(Date.now() / 1000),
              ),
            })}
            {created.label && (
              <span className="ml-2 text-xs text-ink-400">
                ({created.label})
              </span>
            )}
          </p>
          <div className="relative">
            <pre
              className={cn(
                "overflow-x-auto scrollbar-thin rounded-md",
                "bg-surface-2 dark:bg-ink-700/60 px-3 py-2 pr-12",
                "text-xs font-mono text-ink-700 dark:text-surface-4",
                "whitespace-pre-wrap break-all",
              )}
            >
              {created.enroll_command}
            </pre>
            <Button
              variant="secondary"
              size="sm"
              aria-label={t("dialog.copyCommand")}
              className="absolute top-2 right-2"
              onClick={() =>
                copy(created.enroll_command, t("dialog.commandCopied"))
              }
            >
              <Copy className="w-4 h-4" aria-hidden="true" />
            </Button>
          </div>
          <p className="text-xs text-ink-400">
            {t("dialog.enrollTokenSecretWarn")}
          </p>
          {/* The node side can carry its own alias / labels; mention this so users don't have to fill them in one machine at a time after install */}
          <p className="text-xs text-ink-400">
            {t("dialog.enrollOptionalMeta")}
          </p>
        </div>
      ) : autoCreate ? (
        <div className="py-8 text-center text-sm text-ink-500">
          {create.isError ? (
            <p className="text-rose-600 dark:text-rose-400">
              {t(friendlyError(create.error))}
            </p>
          ) : (
            <span className="inline-flex items-center gap-2">
              <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
              {t("dialog.enrollTokenGenerating")}
            </span>
          )}
        </div>
      ) : (
        <div className="space-y-4">
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("dialog.ttlLabel")}
            </span>
            <Select
              value={String(ttl)}
              onChange={(e) => setTtl(Number(e.target.value))}
              aria-label={t("dialog.ttlLabel")}
            >
              {TTL_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {t(`dialog.${o.key}`)}
                </option>
              ))}
            </Select>
          </label>
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("dialog.labelLabel")}
            </span>
            <Input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={t("dialog.labelPlaceholder")}
              maxLength={64}
            />
            <span className="mt-1 block text-xs text-ink-400">
              {t("dialog.labelHint")}
            </span>
          </label>
          {create.isError && (
            <p className="text-sm text-rose-600 dark:text-rose-400">
              {t(friendlyError(create.error))}
            </p>
          )}
        </div>
      )}
    </Dialog>
  );
}
