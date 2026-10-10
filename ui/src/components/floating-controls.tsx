import * as React from "react";
import * as Popover from "@radix-ui/react-popover";
import {
  Check,
  Globe,
  LogOut,
  Moon,
  Palette,
  Settings,
  Sun,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { THEMES, type Theme } from "@/lib/prefs";
import { usePrefs } from "@/components/prefs-provider";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { useToast } from "@/components/ui/toast";

/** Spec 7.16.2 / 02: display colors for the 5 theme color dots */
const THEME_SWATCH: Record<Theme, string> = {
  zhiwei: "#c73225", // cinnabar
  indigo: "#4f46e5",
  emerald: "#059669",
  rose: "#e11d48",
  amber: "#d97706",
  slate: "#475569",
};

/**
 * Spec 7.16 + prohibited list:
 *  - Converged to a single main button (Settings icon, brand solid), hover/focus expands sub-button group
 *  - Main button icon rotates 45° when expanded; auto-collapses ~180ms after mouse leaves; Esc collapses immediately
 *  - Sub-buttons are independent, SVG-only icons, no text, no binary toggles
 *  - Theme color popover expands horizontally to the left of the button to avoid covering peer buttons
 *  - Logout must go through ConfirmDialog, distinguished with rose semantic color
 */
export function FloatingControls() {
  const { t } = useTranslation();
  const prefs = usePrefs();
  const toast = useToast();

  const [open, setOpen] = React.useState(false);
  const [confirmLogout, setConfirmLogout] = React.useState(false);
  const closeTimer = React.useRef<number | null>(null);

  const cancelClose = () => {
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  };
  const scheduleClose = () => {
    cancelClose();
    closeTimer.current = window.setTimeout(() => setOpen(false), 180);
  };

  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      cancelClose();
    };
  }, []);

  const subButton = cn(
    "w-11 h-11 rounded-full bg-surface-0 border border-surface-3 shadow-lg",
    "flex items-center justify-center text-ink-700",
    "hover:bg-surface-2 hover:scale-105 transition-all",
    "dark:bg-ink-700 dark:border-ink-500 dark:text-surface-0 dark:hover:bg-ink-700/70",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40",
  );

  const logoutButton = cn(
    "w-11 h-11 rounded-full bg-rose-50 border border-rose-200 shadow-lg",
    "flex items-center justify-center text-rose-600",
    "hover:scale-105 transition-all",
    "dark:bg-rose-700/20 dark:border-rose-700/40 dark:text-rose-400",
    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-rose-500/40",
  );

  return (
    <>
      {/* Hover-to-expand only recognizes the "button body": when the handler is on the outer container,
          the transparent column in collapsed state also counts as hover area, so sweeping the mouse
          above the main button triggers expand. */}
      <div className="fixed bottom-6 right-6 z-30 flex flex-col items-end gap-3">
        {/* Sub-button group: when expanded, top-to-bottom: theme color → light/dark → language → logout.
            When folded, display:none (not just transparent): a non-collapsed transparent column would occupy
            the entire vertical strip, overlapping content at the bottom-right. */}
        <div
          className={cn(
            "flex flex-col items-end gap-3 transition-opacity duration-200",
            open ? "opacity-100" : "hidden",
          )}
          onMouseEnter={cancelClose}
          onMouseLeave={scheduleClose}
        >
          {/* Theme color: popover expands horizontally to the left of the button */}
          <Popover.Root>
            <Popover.Trigger asChild>
              <button
                type="button"
                className={subButton}
                aria-label={t("action.theme")}
              >
                <Palette className="w-5 h-5" aria-hidden="true" />
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                side="left"
                align="center"
                sideOffset={12}
                className="z-50 bg-surface-0 dark:bg-ink-700 rounded-xl shadow-lg border border-surface-3 dark:border-ink-500 p-3 animate-panel-slide"
              >
                <div className="flex items-center gap-2">
                  {THEMES.map((theme) => (
                    <button
                      key={theme}
                      type="button"
                      aria-label={theme}
                      onClick={() => {
                        prefs.setTheme(theme);
                        toast.push("success", t("toast.themeChanged"));
                      }}
                      className={cn(
                        "w-7 h-7 rounded-full hover:scale-110 transition-transform relative",
                        prefs.theme === theme &&
                          "outline outline-2 outline-offset-2 outline-ink-400",
                      )}
                      style={{ backgroundColor: THEME_SWATCH[theme] }}
                    >
                      {prefs.theme === theme && (
                        <Check
                          className="w-4 h-4 text-white absolute inset-0 m-auto"
                          aria-hidden="true"
                        />
                      )}
                    </button>
                  ))}
                </div>
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>

          {/* Light/dark: single button toggles directly, icon follows state */}
          <button
            type="button"
            className={subButton}
            aria-label={t("action.colorScheme")}
            onClick={() =>
              prefs.setColorScheme(
                prefs.colorScheme === "dark" ? "light" : "dark",
              )
            }
          >
            {prefs.colorScheme === "dark" ? (
              <Moon className="w-5 h-5" aria-hidden="true" />
            ) : (
              <Sun className="w-5 h-5" aria-hidden="true" />
            )}
          </button>

          {/* Language: single button toggles directly, badge at bottom-right shows current language */}
          <button
            type="button"
            className={cn(subButton, "relative")}
            aria-label={t("action.language")}
            onClick={() => prefs.setLocale(prefs.locale === "en-US" ? "zh-CN" : "en-US")}
          >
            <Globe className="w-5 h-5" aria-hidden="true" />
            <span className="absolute -bottom-1 -right-1 w-5 h-5 rounded-full bg-brand-600 text-white text-[10px] font-bold flex items-center justify-center">
              {prefs.locale === "en-US" ? "EN" : "中"}
            </span>
          </button>

          {/* Logout: dangerous action, rose semantic color + must confirm */}
          <button
            type="button"
            className={logoutButton}
            aria-label={t("action.logout")}
            onClick={() => setConfirmLogout(true)}
          >
            <LogOut className="w-5 h-5" aria-hidden="true" />
          </button>
        </div>

        {/* Main button: brand solid, icon rotates 45° when expanded.
            Icon uses gear (per request), but it's **not** the "settings page entry" —
            its semantics is "expand quick controls", and accordingly the aria-label
            doesn't say settings, to avoid having two buttons called "settings" in this group
            (settings is the persistent sidebar entry). */}
        <button
          type="button"
          aria-label={t("action.quickControls")}
          aria-expanded={open}
          onMouseEnter={() => {
            cancelClose();
            setOpen(true);
          }}
          onMouseLeave={scheduleClose}
          onClick={() => setOpen((v) => !v)}
          className={cn(
            "w-12 h-12 rounded-full bg-brand-600 text-white shadow-lg",
            "flex items-center justify-center hover:bg-brand-700 transition-all",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand-500/40",
          )}
        >
          <Settings
            className={cn(
              "w-5 h-5 transition-transform duration-200",
              open && "rotate-45",
            )}
            aria-hidden="true"
          />
        </button>
      </div>

      <ConfirmDialog
        open={confirmLogout}
        title={t("confirm.logoutTitle")}
        message={t("confirm.logoutMessage")}
        confirmLabel={t("confirm.logoutConfirm")}
        cancelLabel={t("confirm.cancel")}
        danger
        onCancel={() => setConfirmLogout(false)}
        onConfirm={() => {
          setConfirmLogout(false);
          toast.push("success", t("toast.logoutSuccess"));
          window.setTimeout(() => {
            localStorage.removeItem("zhiwei.adminToken");
            window.location.reload();
          }, 600);
        }}
      />
    </>
  );
}
