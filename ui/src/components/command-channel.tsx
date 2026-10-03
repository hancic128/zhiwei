/**
 * "Command channel unavailable" indicator.
 *
 * Start / stop / restart / fetch-logs in the console all work in the form of
 * "issue command -> node pulls it actively". If a node stops pulling, these
 * buttons do nothing while the node page looks perfectly normal (metrics keep
 * arriving, state still shows "online") — users have to SSH in and read logs
 * to figure out what happened.
 *
 * The determination lives in the backend at
 * `crates/monitor-server/src/control_channel.rs`; this component only
 * surfaces it.
 */
import { useTranslation } from "react-i18next";
import { PlugZap } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Tooltip } from "@/components/ui/tooltip";
import type { CommandChannel } from "@/api";
import { relativeTime } from "@/lib/utils";

export function ChannelDownBadge({
  channel,
  className,
}: {
  channel?: CommandChannel;
  className?: string;
}) {
  const { t } = useTranslation();
  if (channel?.state !== "down") return null;
  const hint =
    channel.last_poll_age_ms === null
      ? t("nodes.channelDownHintNever")
      : t("nodes.channelDownHint", {
          ago: relativeTime(Date.now() - channel.last_poll_age_ms, t),
        });
  return (
    <Tooltip content={hint}>
      <span className={className}>
        <Badge tone="danger">
          <PlugZap className="w-3 h-3" aria-hidden="true" />
          {t("nodes.channelDown")}
        </Badge>
      </span>
    </Tooltip>
  );
}
