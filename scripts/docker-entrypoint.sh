#!/bin/sh
#
# Container entrypoint: in the **same container** start ops-server (control
# plane) first, then monitor (data plane).
#
# Why the same container: managed platforms (Render / Northflank) only run a
# single web service with no slot for a second process. Monitor's command
# channel needs ops-server on loopback to sign commands (fetch container logs,
# kill processes, restart hosts, ...). Without it, those console actions just
# fail with "ops-server unavailable: connection refused".
#
# The two processes remain independent; **the signing private key lives only
# inside zhiwei-ops**: monitor reads the public key from `<data-dir>/ops.pub`
# and hands it to nodes on enroll (TOFU), so monitor cannot forge commands.
# This is the monitor/ops separation constraint documented in docs/architecture.md
# — not a candidate for merging them.
#
# Environment variables:
#   ZHIWEI_OPS_DISABLE=1   skip ops-server (data-plane-only deployments)
#   ZHIWEI_OPS_WAIT=<sec>  max seconds to wait for ops.pub; default 5
#   ZHIWEI_OPS_BIN / ZHIWEI_MONITOR_BIN  override binary paths (custom images / local tests)
#
# Uses `sh` rather than `bash`: debian:bookworm-slim does not guarantee bash.
set -u

OPS_BIN="${ZHIWEI_OPS_BIN:-/usr/local/bin/zhiwei-ops}"
MONITOR_BIN="${ZHIWEI_MONITOR_BIN:-/usr/local/bin/zhiwei-monitor}"
DATA_DIR="${ZHIWEI_DATA_DIR:-/var/lib/zhiwei}"

start_ops() {
  if [ "${ZHIWEI_OPS_DISABLE:-0}" = "1" ]; then
    echo "[entrypoint] ZHIWEI_OPS_DISABLE=1: skipping ops-server; command channel unavailable" >&2
    return
  fi
  if [ ! -x "$OPS_BIN" ]; then
    echo "[entrypoint] $OPS_BIN not found: command channel unavailable" >&2
    return
  fi

  "$OPS_BIN" &
  ops_pid=$!

  # Wait for ops.pub before letting monitor start. Monitor reads it once at
  # startup and caches the value — if it's empty, every freshly enrolled node
  # gets an empty key and never polls for commands. If we time out, we still
  # start monitor; its route handler reads the file lazily on each request.
  waited=0
  limit="${ZHIWEI_OPS_WAIT:-5}"
  while [ ! -s "$DATA_DIR/ops.pub" ] && [ "$waited" -lt "$limit" ]; do
    kill -0 "$ops_pid" 2>/dev/null || {
      echo "[entrypoint] ops-server exited early (pid $ops_pid); starting monitor anyway" >&2
      return
    }
    sleep 1
    waited=$((waited + 1))
  done
  if [ -s "$DATA_DIR/ops.pub" ]; then
    echo "[entrypoint] ops-server ready (pid $ops_pid, public key at $DATA_DIR/ops.pub)" >&2
  else
    echo "[entrypoint] timed out waiting for ops.pub (${limit}s); starting monitor anyway" >&2
  fi
}

start_ops
exec "$MONITOR_BIN" "$@"