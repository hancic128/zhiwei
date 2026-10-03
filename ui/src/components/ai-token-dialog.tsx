import * as React from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Copy, KeyRound, Plus } from "lucide-react";
import { aiTokens } from "@/api";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";
import { cn, copyText, friendlyError } from "@/lib/utils";

/**
 * Create AI token.
 *
 * Two-stage: first stage lets the user fill in the name (required, max 64); second stage can **only be viewed once** —
 * the plaintext token is returned by the backend only this one time; reopening only allows creating a new one.
 *
 * Warning uses AlertTriangle + a wide `warn` light-background block; the copy button is prominent in the second stage.
 */
interface Created {
  id: string;
  name: string;
  token: string;
}

export function AiTokenDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [name, setName] = React.useState("");
  const [created, setCreated] = React.useState<Created | null>(null);

  React.useEffect(() => {
    if (!open) {
      setName("");
      setCreated(null);
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () => aiTokens.create(name.trim()),
    onSuccess: (r) => {
      setCreated({ id: r.id, name: r.name, token: r.token });
      setName("");
    },
    onError: (e) => toast.push("error", t(friendlyError(e))),
  });

  const copy = (text: string, okMsg: string) =>
    void copyText(text).then((ok) =>
      toast.push(ok ? "success" : "error", ok ? okMsg : t("toast.copyFailed")),
    );

  const close = () => {
    if (create.isPending) return;
    onClose();
  };

  const trimmed = name.trim();
  const canSubmit = trimmed.length > 0 && trimmed.length <= 64;

  return (
    <Dialog
      open={open}
      onClose={close}
      size="lg"
      title={
        created
          ? t("dialog.aiTokenCreatedTitle")
          : t("dialog.createAiToken")
      }
      description={
        created
          ? t("dialog.aiTokenCreatedDesc")
          : t("dialog.createAiTokenDesc")
      }
      footer={
        created ? (
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
              disabled={!canSubmit}
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
          <div
            className={cn(
              "flex items-start gap-2 rounded-md",
              "bg-amber-50 dark:bg-amber-700/20",
              "border border-amber-100 dark:border-amber-700/40",
              "px-3 py-2",
            )}
            role="alert"
          >
            <AlertTriangle
              className="w-4 h-4 mt-0.5 shrink-0 text-amber-600 dark:text-amber-400"
              aria-hidden="true"
            />
            <p className="text-sm text-amber-700 dark:text-amber-400">
              {t("dialog.tokenShownOnce")}
            </p>
          </div>
          <div>
            <span className="block text-xs text-ink-500 mb-1">
              {t("dialog.aiTokenLabel")}
            </span>
            <div className="relative">
              <pre
                className={cn(
                  "overflow-x-auto scrollbar-thin rounded-md",
                  "bg-surface-2 dark:bg-ink-700/60 px-3 py-2 pr-12",
                  "text-xs font-mono text-ink-700 dark:text-surface-4",
                  "whitespace-pre-wrap break-all",
                )}
              >
                {created.token}
              </pre>
              <Button
                variant="secondary"
                size="sm"
                aria-label={t("dialog.copyToken")}
                className="absolute top-2 right-2"
                onClick={() =>
                  copy(created.token, t("dialog.tokenCopied"))
                }
              >
                <Copy className="w-4 h-4" aria-hidden="true" />
              </Button>
            </div>
          </div>
          <p className="text-xs text-ink-400">
            {t("dialog.aiTokenUseHint", { name: created.name })}
          </p>
        </div>
      ) : (
        <div className="space-y-4">
          <label className="block">
            <span className="block text-xs text-ink-500 mb-1">
              {t("dialog.aiTokenNameLabel")}
            </span>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("dialog.aiTokenNamePlaceholder")}
              maxLength={64}
              autoFocus
            />
            <span className="mt-1 block text-xs text-ink-400">
              {t("dialog.aiTokenNameHint")}
            </span>
          </label>
          <div
            className={cn(
              "flex items-start gap-2 rounded-md",
              "bg-surface-2 dark:bg-ink-700/60",
              "px-3 py-2",
            )}
          >
            <KeyRound
              className="w-4 h-4 mt-0.5 shrink-0 text-ink-400"
              aria-hidden="true"
            />
            <p className="text-xs text-ink-500">
              {t("dialog.aiTokenPrivacyHint")}
            </p>
          </div>
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
