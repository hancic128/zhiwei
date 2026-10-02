//! 「命令通道还活着吗」的判定。
//!
//! 命令通道是**节点主动拉**的（`GET /v1/commands`，25 秒长轮询），所以 monitor
//! 手里唯一可靠的证据就是「最近一次被拉走的时刻」。这个证据值得单独收着，因为有
//! 一类故障在控制台上完全不可见：节点入网早于 ops 公钥分发（或者 state 目录被
//! 清过），`control::run_poll_loop` 会在启动时直接 return，**一个请求都不发**；
//! telemetry 照常上报，节点显示在线、指标画得好好的，可启停 / 重启 / 拉日志
//! 点下去全都石沉大海——用户只能登机器翻日志。
//!
//! 反过来说：Agent 侧拉取持续失败（TLS、防火墙、ops-server 挂了）也走同一条
//! 判定——不管哪种原因，「在线却一直不来拉命令」都说明这台机器的控制面是坏的。

use std::collections::HashMap;
use std::sync::Mutex;

/// 节点多久没来拉命令就算「通道可能不可用」。
///
/// 节点是 25 秒长轮询（拿到命令会立刻再拉一次），90 秒相当于连漏 3 轮，
/// 给网络抖动留了余量。
pub const CONTROL_POLL_GRACE_MS: i64 = 90_000;

/// monitor 启动后的观察窗口。
///
/// 表在进程内存里、重启即空，冷启动那几十秒里每个节点都「没拉过命令」——
/// 不设窗口的话每次重启都会报一屏假故障。
pub const CONTROL_WARMUP_MS: i64 = 120_000;

/// 判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// 在拉：通道可用
    Ok,
    /// 节点在线，却一直不来拉命令：控制面是坏的
    Down,
    /// 说不好：monitor 刚起来还没观察够，或节点本身不在线
    /// （那种情况归上下线告警管，别在这里重复报一遍）
    Unknown,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Ok => "ok",
            Channel::Down => "down",
            Channel::Unknown => "unknown",
        }
    }
}

/// 判定。参数都是毫秒时间戳，纯函数，便于把边界钉在测试里。
///
/// 三个条件的先后顺序就是优先级：观察窗口 → 节点在线 → 拉取间隔。
pub fn channel_state(
    node_last_seen_ms: Option<i64>,
    last_poll_ms: Option<i64>,
    monitor_uptime_ms: i64,
    now_ms: i64,
) -> Channel {
    if monitor_uptime_ms < CONTROL_WARMUP_MS {
        return Channel::Unknown;
    }
    let node_online = matches!(
        node_last_seen_ms,
        Some(seen) if now_ms - seen < crate::todo_api::NODE_OFFLINE_AFTER_MS
    );
    if !node_online {
        return Channel::Unknown;
    }
    match last_poll_ms {
        Some(poll) if now_ms - poll <= CONTROL_POLL_GRACE_MS => Channel::Ok,
        _ => Channel::Down,
    }
}

/// 每个节点最近一次拉命令的时刻。只放内存：重启后几十秒内会被各节点重新填满，
/// 期间靠 [`CONTROL_WARMUP_MS`] 压住误报——不值得为此每次轮询都写一次库。
#[derive(Default)]
pub struct ControlPolls(Mutex<HashMap<String, i64>>);

impl ControlPolls {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次拉取。锁中毒了就取回内层（这张表只是「最近见过」的提示，
    /// 不值得为它把整个进程搞崩）。
    pub fn note(&self, node_id: &str, now_ms: i64) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(node_id.to_string(), now_ms);
    }

    pub fn last(&self, node_id: &str) -> Option<i64> {
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.get(node_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;
    const WARM: i64 = CONTROL_WARMUP_MS + 1;

    fn st(seen_ago_ms: i64, polled_ago_ms: Option<i64>, uptime_ms: i64) -> Channel {
        channel_state(
            Some(NOW - seen_ago_ms),
            polled_ago_ms.map(|a| NOW - a),
            uptime_ms,
            NOW,
        )
    }

    #[test]
    fn polling_regularly_is_ok() {
        assert_eq!(st(5_000, Some(5_000), WARM), Channel::Ok);
        // 长轮询 25s 一轮，卡在宽限边界上也还算活着
        assert_eq!(st(5_000, Some(CONTROL_POLL_GRACE_MS), WARM), Channel::Ok);
    }

    #[test]
    fn online_but_never_polling_is_down() {
        // 从没拉过（没 ops 公钥的老节点）
        assert_eq!(st(5_000, None, WARM), Channel::Down);
        // 拉过，但已经超时很久
        assert_eq!(
            st(5_000, Some(CONTROL_POLL_GRACE_MS + 1), WARM),
            Channel::Down
        );
    }

    #[test]
    fn cold_start_does_not_report_anything() {
        // 刚重启：表是空的，谁都别报
        assert_eq!(st(5_000, None, 0), Channel::Unknown);
        assert_eq!(
            st(5_000, Some(1_000), CONTROL_WARMUP_MS - 1),
            Channel::Unknown
        );
        assert_eq!(st(5_000, Some(1_000), CONTROL_WARMUP_MS), Channel::Ok);
    }

    #[test]
    fn offline_node_is_left_to_the_offline_alert() {
        // 节点本身失联：这是上下线告警的事，不该在这里再报一遍
        assert_eq!(
            st(crate::todo_api::NODE_OFFLINE_AFTER_MS + 1, None, WARM),
            Channel::Unknown
        );
        // 从没有过 telemetry 的节点同理
        assert_eq!(channel_state(None, None, WARM, NOW), Channel::Unknown);
    }

    #[test]
    fn polls_map_remembers_per_node() {
        let polls = ControlPolls::new();
        assert_eq!(polls.last("a"), None);
        polls.note("a", 111);
        polls.note("b", 222);
        assert_eq!(polls.last("a"), Some(111));
        polls.note("a", 333);
        assert_eq!(polls.last("a"), Some(333));
        assert_eq!(polls.last("b"), Some(222));
        assert_eq!(polls.last("没有这台机器"), None);
    }
}
