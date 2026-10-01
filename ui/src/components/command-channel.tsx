/**
 * 「命令通道不可用」标记。
 *
 * 控制台上的启停 / 重启 / 拉日志都是「签发命令 → 节点主动来拉」的形态。节点要是
 * 一直不来拉，这些按钮点下去不会有任何反应，而节点页看起来一切正常（指标照常
 * 上报、状态还是「在线」）——用户只能登机器翻日志。
 *
 * 判定在后端 `crates/monitor-server/src/control_channel.rs`，这里只负责把它讲出来。
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
