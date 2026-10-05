#!/usr/bin/env bash
#
# ZhiWei local development environment (backend + frontend in one)
#
#   ./scripts/dev.sh start    Build and start the backend + frontend dev server
#   ./scripts/dev.sh query    List enrolled nodes
#   ./scripts/dev.sh watch    Poll latest telemetry for a node (default 2s interval)
#   ./scripts/dev.sh index    GET / overview
#   ./scripts/dev.sh token    Print new bootstrap token (10 minutes)
#   ./scripts/dev.sh status   Process and log status
#   ./scripts/dev.sh stop     Stop the backend and frontend
#   ./scripts/dev.sh reset    Reset: stop, delete data, restart
#   ./scripts/dev.sh clean    Stop and delete all data (including the CA)
#
# Default uses plain HTTP (TLS terminated by edge), same as managed platform deployment.
# To try built-in TLS: ZHIWEI_DEV_TLS=1 ./scripts/dev.sh start
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DATA_DIR="${ZHIWEI_DEV_DATA:-$ROOT/data}"
LISTEN="${ZHIWEI_DEV_LISTEN:-127.0.0.1:8443}"
INTERVAL="${ZHIWEI_DEV_INTERVAL:-10}"
USE_TLS="${ZHIWEI_DEV_TLS:-}"
MONITOR_BIN="$ROOT/target/debug/zhiwei-monitor"
NODE_BIN="$ROOT/target/debug/zhiwei-node"
OPS_BIN="$ROOT/target/debug/zhiwei-ops"
MONITOR_LOG="$DATA_DIR/monitor.log"
OPS_LOG="$DATA_DIR/ops.log"
NODE_LOG="$DATA_DIR/node.log"
TOKEN_FILE="$DATA_DIR/admin.token"
UI_LOG="$DATA_DIR/ui.log"

say() { printf '\033[36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31mError:\033[0m %s\n' "$*" >&2; exit 1; }

url() {
  if [ -n "$USE_TLS" ]; then echo "https://${LISTEN}"; else echo "http://${LISTEN}"; fi
}

# monitor's read endpoints require the admin token; the node side uses request
# signing, which is unrelated to this
need_token() {
  [ -f "$TOKEN_FILE" ] || die "No admin token yet. Run ./scripts/dev.sh start first"
}

q() { need_token; curl -s -H "Authorization: Bearer $(cat "$TOKEN_FILE")" "$@"; }

build() {
  say "Building Rust (incremental, usually 1-3 seconds)"
  cargo build --quiet --bin zhiwei-monitor --bin zhiwei-node --bin zhiwei-ops
}

build_ui() {
  say "Building UI"
  cd "$ROOT/ui" && npm run build 2>&1 | tail -5
  cd "$ROOT"
}

start_ui() {
  # Kill any vite / esbuild left over, regardless of who started them.
  # Otherwise the stale 5173 listener keeps responding and the new code never lands.
  pkill -f 'node.*vite' 2>/dev/null || true
  pkill -f '@esbuild/darwin' 2>/dev/null || true
  sleep 1
  # If anything else is holding port 5173, kill it by port.
  local port_pid
  port_pid="$(lsof -ti :5173 2>/dev/null || true)"
  if [ -n "$port_pid" ]; then
    kill -9 $port_pid 2>/dev/null || true
    sleep 1
  fi

  mkdir -p "$DATA_DIR"
  cd "$ROOT/ui"
  nohup npm run dev >"$UI_LOG" 2>&1 &
  cd "$ROOT"
  say "UI dev server starting → http://localhost:5173"
  sleep 3
  if pgrep -f 'node.*vite' >/dev/null 2>&1; then
    say "UI dev server running"
  else
    say "UI dev server failed to start, check $UI_LOG"
    tail -10 "$UI_LOG"
  fi
}

cmd_start() {
  build
  build_ui

  # ops-server starts first: it generates signing key and ops.pub, monitor distributes to nodes during enroll
  if pgrep -f 'zhiwei-ops --data-dir' >/dev/null 2>&1; then
    say "ops-server already running"
  else
    mkdir -p "$DATA_DIR"
    nohup "$OPS_BIN" --data-dir "$DATA_DIR" >"$OPS_LOG" 2>&1 &
    sleep 1
    say "ops-server started → 127.0.0.1:8444 (signing key only in its process)"
  fi

  if pgrep -f 'zhiwei-monitor --data-dir' >/dev/null 2>&1; then
    say "monitor already running"
  else
    mkdir -p "$DATA_DIR"
    local tls_flag=()
    [ -n "$USE_TLS" ] || tls_flag=(--plain-http)
    nohup "$MONITOR_BIN" --data-dir "$DATA_DIR" --listen "$LISTEN" \
      "${tls_flag[@]}" >"$MONITOR_LOG" 2>&1 &
    sleep 3
    if [ -n "$USE_TLS" ]; then
      say "monitor started → $(url) (built-in TLS, server cert only)"
    else
      say "monitor started → $(url) (plain HTTP, TLS at edge)"
    fi
  fi

  # Wait for monitor to start and print tokens
  local bt_token=""
  local admin_token=""
  local retries=10
  while [ $retries -gt 0 ]; do
    bt_token="$(grep -o 'zhi-bt-[a-f0-9]*' "$MONITOR_LOG" 2>/dev/null | tail -1 || true)"
    admin_token="$(grep -o '\[ADMIN TOKEN\] [a-f0-9]*' "$MONITOR_LOG" 2>/dev/null | tail -1 | sed 's/\[ADMIN TOKEN\] //' || true)"
    [ -n "$bt_token" ] && [ -n "$admin_token" ] && break
    sleep 1
    retries=$((retries - 1))
  done
  if [ -n "$admin_token" ]; then
    say "Admin token: $admin_token"
    echo
  fi
  if [ -n "$bt_token" ]; then
    say "Bootstrap token: $bt_token (valid 10 minutes)"
    echo
  fi

  # Check if docker is available before starting node
  if ! command -v docker &>/dev/null; then
    say "docker not found, skipping node auto-enrollment"
    cmd_status
    echo
    say "To enroll a node manually:"
    echo "  ZHIWEI_MONITOR_URL=$(url) ZHIWEI_BOOTSTRAP_TOKEN=\$(./scripts/dev.sh token) \\"
    echo "    cargo run --bin zhiwei-node -- --state-dir $DATA_DIR/node --interval $INTERVAL"
    start_ui
    return
  fi

  # Only check for local node (specific state-dir path)
  if [ -f "$DATA_DIR/node/state/signing.key" ] && pgrep -f "zhiwei-node --state-dir $DATA_DIR/node" >/dev/null 2>&1; then
    say "local node already enrolled"
  else
    [ -n "$bt_token" ] || die "No bootstrap token in logs. Run ./scripts/dev.sh token first"
    # Node identity via signing.key, no certificates needed
    mkdir -p "$DATA_DIR/node"
    ZHIWEI_MONITOR_URL="$(url)" ZHIWEI_BOOTSTRAP_TOKEN="$bt_token" \
      nohup "$NODE_BIN" --state-dir "$DATA_DIR/node" --interval "$INTERVAL" >"$NODE_LOG" 2>&1 &
    sleep 3
    say "node enrolled (reporting every ${INTERVAL}s)"
  fi

  start_ui
  cmd_status
  echo
  say "Access the console at http://localhost:5173"
  echo
  say "Try these:"
  echo "  ./scripts/dev.sh query"
  echo "  ./scripts/dev.sh watch"
  echo "  tail -f $MONITOR_LOG"
}

cmd_query() {
  need_token
  q "$(url)/v1/nodes" | python3 -m json.tool
}

cmd_index() {
  need_token
  q "$(url)/" | python3 -m json.tool
}

cmd_watch() {
  local node_id="${1:-}"
  if [ -z "$node_id" ]; then
    node_id="$(q "$(url)/v1/nodes" | python3 -c 'import sys,json; print(json.load(sys.stdin)[0]["id"])')"
  fi
  say "Watching node $node_id (Ctrl-C to exit)"
  while true; do
    clear 2>/dev/null || true
    q "$(url)/v1/nodes/$node_id/telemetry?limit=3" | python3 -m json.tool
    sleep 2
  done
}

cmd_token() {
  need_token
  local token
  token="$(grep -o 'zhi-bt-[a-f0-9]*' "$MONITOR_LOG" | tail -1 || true)"
  [ -n "$token" ] || die "No token in monitor log"
  echo "$token"
}

cmd_status() {
  say "ops:     $(pgrep -f 'zhiwei-ops --data-dir' >/dev/null && echo running || echo stopped)"
  say "monitor: $(pgrep -f 'zhiwei-monitor --data-dir' >/dev/null && echo running || echo stopped)"
  # Check for local node in DATA_DIR or system service
  if pgrep -f 'zhiwei-node --state-dir '"$DATA_DIR/node"'' >/dev/null 2>&1; then
    say "node:    running (local)"
  elif pgrep -f 'zhiwei-node' >/dev/null 2>&1; then
    say "node:    running (external)"
  else
    say "node:    stopped"
  fi
  say "ui:      $(pgrep -f 'node.*vite' >/dev/null && echo running || echo stopped)"
  [ -f "$MONITOR_LOG" ] && say "Recent monitor log:" && tail -3 "$MONITOR_LOG"
  [ -f "$NODE_LOG" ] && say "Recent node log:" && tail -3 "$NODE_LOG"
}

cmd_stop() {
  pkill -f 'zhiwei-monitor --data-dir' 2>/dev/null || true
  pkill -f 'zhiwei-node --state-dir '"$DATA_DIR/node"'' 2>/dev/null || true
  pkill -f 'zhiwei-ops --data-dir' 2>/dev/null || true
  pkill -f 'node.*vite' 2>/dev/null || true
  pkill -f '@esbuild/darwin' 2>/dev/null || true
  say "Stopped all processes"
}

cmd_reset() {
  cmd_stop
  # Delete all data (CA, nodes, settings), then restart fresh
  rm -rf "$DATA_DIR"
  cmd_start
}

cmd_clean() {
  cmd_stop
  rm -rf "$DATA_DIR"
  say "Deleted $DATA_DIR (CA deleted, will regenerate on next start)"
}

case "${1:-}" in
  start) cmd_start ;;
  query) cmd_query ;;
  index) cmd_index ;;
  watch) shift; cmd_watch "${1:-}" ;;
  token) cmd_token ;;
  status) cmd_status ;;
  stop) cmd_stop ;;
  reset) cmd_reset ;;
  clean) cmd_clean ;;
  *)
    sed -n '3,17p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac
