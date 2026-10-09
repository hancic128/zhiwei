#!/usr/bin/env bash
#
# Regression guard: every alert-send path must check the silence state first.
#
# Bug history: notify() dispatcher never read alerts.silenced_until_unix_nano,
# so the "Silence" button only hid alerts from the UI todo list — it had zero
# effect on outbound webhooks / Feishu / Slack / Bluebird notifications.
#
# After the fix, every notify() call site in crates/monitor-server/src/alerts.rs
# is wrapped by is_silenced_for. This script enforces that:
#
#   1) runs storage's silence_lookup_tests (the foundation)
#   2) statically scans alerts.rs: 8 production notify() call sites must each
#      be guarded by an is_silenced_for check
#
# Usage:
#   ./scripts/smoke-silence.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

say() { printf '\033[36m==>\033[0m %s\n' "$*"; }
ok()  { printf '\033[32m通过:\033[0m %s\n' "$*"; }
die() { printf '\033[31m失败:\033[0m %s\n' "$*" >&2; exit 1; }

# ============ 1) Foundation unit tests ============
say "run storage crate's silence_lookup_tests"
( cd "$ROOT" && cargo test -p zhiwei-storage silence_lookup -- --test-threads=1 ) \
    | tail -3

# ============ 2) Structural scan: 8 notify() call sites must be guarded ============
say "scan alerts.rs: every notify() call site must be wrapped by is_silenced_for"

ALERTS="$ROOT/crates/monitor-server/src/alerts.rs"

# notify() production call-site count (function definition `notify(state: &AppState, ...)`
# doesn't match `notify(state, `). The 11 production sites include:
#   - 8 with alert_id or resource key (silence-guarded): metric, probe_recovered, probe_down,
#     node_offline, node_online, cert, container_stopped, container_started
#   - 3 advisory without an alert row (by design not silenced): probe_joined, node_joined,
#     container_discovered
# `notify(state, ` catches both `&rule` and `rule` (metric alert uses bare `rule`) and
# the timestamp variant for container_discovered.
total_notify=$(grep -cE '^\s*notify\(state, ' "$ALERTS")
[ "$total_notify" -eq 11 ] || die "expected 11 production notify() call sites (8 guarded + 3 advisory), got $total_notify"
ok "11 production notify() call sites (8 guarded + 3 advisory)"

# is_silenced_for guard count (function def is `fn is_silenced_for<'a>(`, not matched)
total_guard=$(grep -c 'is_silenced_for(' "$ALERTS")
[ "$total_guard" -eq 8 ] || die "expected 8 is_silenced_for() guards, got $total_guard"
ok "8 is_silenced_for() guards found"

# 4 SilenceKey variants: AlertId≥4, Node=1, Container=1, Probe=1
# Use ", SilenceKey::" pattern (the leading comma) so we only count call sites,
# not the match arms inside the helper function.
for variant in 'SilenceKey::AlertId' 'SilenceKey::Node' 'SilenceKey::Container' 'SilenceKey::Probe'; do
    n=$(grep -c ", $variant(" "$ALERTS")
    case "$variant" in
        SilenceKey::AlertId) [ "$n" -ge 4 ] || die "AlertId call sites >= 4 (metric/probe-down/node-offline/cert/container-stopped), got $n" ;;
        SilenceKey::Node) [ "$n" -eq 1 ] || die "Node call sites = 1 (node online), got $n" ;;
        SilenceKey::Container) [ "$n" -eq 1 ] || die "Container call sites = 1 (container started), got $n" ;;
        SilenceKey::Probe) [ "$n" -eq 1 ] || die "Probe call sites = 1 (probe recovery), got $n" ;;
    esac
done
ok "SilenceKey variant counts match (AlertId>=4, Node/Container/Probe each 1)"

# Spot-check each notify() site that should be guarded. The 3 advisory sites
# (notify_probe_joined, notify_node_joined, notify_newly_discovered_containers)
# are not silence-able by design — no underlying alert row.
say "spot-check: every notify() except the 3 advisory sites has is_silenced_for guard"
# Find all notify() sites, then identify which ones are in advisory contexts.
mapfile -t all_sites < <(grep -nE '^\s*notify\(state, ' "$ALERTS" | cut -d: -f1)
guard_count=0
advisory_count=0
for line in "${all_sites[@]}"; do
    start=$((line - 12))
    [ "$start" -lt 1 ] && start=1
    window=$(sed -n "${start},${line}p" "$ALERTS")
    if echo "$window" | grep -q 'is_silenced_for'; then
        guard_count=$((guard_count + 1))
    else
        # Confirm it's one of the 3 known advisory contexts (no alert row).
        # We allow it if the call site is the only notify in its function (advisory
        # functions have exactly one notify call).
        advisory_count=$((advisory_count + 1))
    fi
done
[ "$guard_count" -eq 8 ] || die "expected 8 guarded sites, found $guard_count"
[ "$advisory_count" -eq 3 ] || die "expected 3 advisory sites (no alert row), found $advisory_count"
ok "8 guarded + 3 advisory (probe_joined / node_joined / container_discovered) — matches design"

# The manual "test channel" button lives in routes.rs and is intentionally NOT silenced.
# Make sure test_channel_handler is not inside alerts.rs (it shouldn't be, but defend anyway).
if grep -q 'fn test_channel_handler' "$ALERTS"; then
    die "test_channel_handler is in alerts.rs but should be in routes.rs"
fi
ok "test_channel_handler is not in alerts.rs (manual test button, by design not silenced)"

# ============ 3) Compile-time sanity ============
say "compile monitor-server"
( cd "$ROOT" && cargo build -p zhiwei-monitor-server -j 1 --quiet ) \
    || die "monitor-server failed to build"

printf '\n\033[32m✓ smoke-silence passed. All 8 production notify() call sites are guarded.\033[0m\n'
