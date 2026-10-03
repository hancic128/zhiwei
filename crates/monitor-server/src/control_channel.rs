//! Determining "is the command channel still alive?".
//!
//! The command channel is **pulled by the node** (`GET /v1/commands`, 25-second
//! long-poll), so the only reliable evidence on the monitor side is "the last
//! time it was pulled". This evidence deserves its own home because there is
//! a class of failure completely invisible on the console: a node that enrolled
//! before ops public key distribution (or with its state directory wiped) will
//! make `control::run_poll_loop` return immediately at startup, **never sending
//! a single request**; telemetry continues to report normally, the node shows
//! online, metrics draw nicely, but start / stop / restart / fetch-log buttons
//! all silently fail — users have to SSH in and dig through logs.
//!
//! Conversely: persistent agent-side pull failures (TLS, firewall, ops-server
//! down) also go through the same judgment — regardless of cause, "online but
//! not pulling commands" means this machine's control plane is broken.

use std::collections::HashMap;
use std::sync::Mutex;

/// How long a node can go without pulling commands before the channel is
/// considered "possibly unavailable".
///
/// Nodes long-poll every 25 seconds (they immediately re-poll after receiving
/// a command), so 90 seconds means missing 3 consecutive rounds, leaving
/// some headroom for network jitter.
pub const CONTROL_POLL_GRACE_MS: i64 = 90_000;

/// Observation window after monitor startup.
///
/// The table lives in process memory and empties on restart, so for the first
/// few dozen seconds after a cold start every node looks like "hasn't pulled
/// commands" — without a window, every restart would generate a screenful of
/// false-positive failures.
pub const CONTROL_WARMUP_MS: i64 = 120_000;

/// Judgment result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Pulling: channel is available
    Ok,
    /// Node online but not pulling commands: control plane is broken
    Down,
    /// Unclear: monitor just started and hasn't observed enough yet, or the
    /// node itself is offline (that case belongs to the offline alert, don't
    /// report it again here)
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

/// The judgment. All parameters are millisecond timestamps, pure function,
/// to make the boundaries easy to pin down in tests.
///
/// The three conditions are checked in priority order: observation window →
/// node online → poll interval.
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

/// Each node's most recent command-pull time. In-memory only: within a few
/// dozen seconds after a restart it gets refilled by each node, and during
/// that window [`CONTROL_WARMUP_MS`] suppresses false positives — not worth
/// hitting the database every poll just for that.
#[derive(Default)]
pub struct ControlPolls(Mutex<HashMap<String, i64>>);

impl ControlPolls {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one pull. On lock poisoning, recover the inner state (this table
    /// is just a "last seen" hint; not worth crashing the whole process over it).
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
        // Long-poll 25s per round; right at the grace boundary still counts
        // as alive.
        assert_eq!(st(5_000, Some(CONTROL_POLL_GRACE_MS), WARM), Channel::Ok);
    }

    #[test]
    fn online_but_never_polling_is_down() {
        // Never pulled (old node with no ops public key)
        assert_eq!(st(5_000, None, WARM), Channel::Down);
        // Has pulled before, but long past timeout
        assert_eq!(
            st(5_000, Some(CONTROL_POLL_GRACE_MS + 1), WARM),
            Channel::Down
        );
    }

    #[test]
    fn cold_start_does_not_report_anything() {
        // Just restarted: the table is empty, don't report anyone
        assert_eq!(st(5_000, None, 0), Channel::Unknown);
        assert_eq!(
            st(5_000, Some(1_000), CONTROL_WARMUP_MS - 1),
            Channel::Unknown
        );
        assert_eq!(st(5_000, Some(1_000), CONTROL_WARMUP_MS), Channel::Ok);
    }

    #[test]
    fn offline_node_is_left_to_the_offline_alert() {
        // Node itself is unreachable: that's the offline alert's job, don't
        // re-report it here.
        assert_eq!(
            st(crate::todo_api::NODE_OFFLINE_AFTER_MS + 1, None, WARM),
            Channel::Unknown
        );
        // Same for nodes that have never reported telemetry
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
        assert_eq!(polls.last("no such host"), None);
    }
}
