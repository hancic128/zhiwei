#!/usr/bin/env bash
#
# 知微本地试验环境
#
#   ./scripts/dev.sh start    构建并启动 monitor，自动入网一个本地 node
#   ./scripts/dev.sh query    列出已入网节点
#   ./scripts/dev.sh watch    轮询某节点的最新 telemetry（默认 2s 一次）
#   ./scripts/dev.sh index    GET / 总览
#   ./scripts/dev.sh token    打印新的 bootstrap token（10 分钟）
#   ./scripts/dev.sh status   进程与日志状态
#   ./scripts/dev.sh stop     停掉 monitor 与 node
#   ./scripts/dev.sh clean    停掉并删除本地数据（含 CA）
#
# 默认走明文 HTTP（TLS 由前置边缘终结），这也是托管平台的部署形态。
# 想试内置 TLS：ZHIWEI_DEV_TLS=1 ./scripts/dev.sh start
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

say() { printf '\033[36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31m错误:\033[0m %s\n' "$*" >&2; exit 1; }

url() {
  if [ -n "$USE_TLS" ]; then echo "https://${LISTEN}"; else echo "http://${LISTEN}"; fi
}

# monitor 的只读接口认 admin token；节点那一侧走请求签名，与这里无关
need_token() {
  [ -f "$TOKEN_FILE" ] || die "还没有 admin token，先跑 ./scripts/dev.sh start"
}

q() { need_token; curl -s -H "Authorization: Bearer $(cat "$TOKEN_FILE")" "$@"; }

build() {
  say "构建（增量，通常 1–3 秒）"
  cargo build --quiet --bin zhiwei-monitor --bin zhiwei-node --bin zhiwei-ops
}

cmd_start() {
  build
  # ops-server 先起：它生成签名密钥与 ops.pub，monitor 在 enroll 时下发给节点
  if pgrep -f 'zhiwei-ops --data-dir' >/dev/null 2>&1; then
    say "ops-server 已在运行"
  else
    mkdir -p "$DATA_DIR"
    nohup "$OPS_BIN" --data-dir "$DATA_DIR" >"$OPS_LOG" 2>&1 &
    sleep 1
    say "ops-server 已启动 → 127.0.0.1:8444（签名私钥仅在它手里）"
  fi

  if pgrep -f 'zhiwei-monitor --data-dir' >/dev/null 2>&1; then
    say "monitor 已在运行"
  else
    mkdir -p "$DATA_DIR"
    local tls_flag=()
    [ -n "$USE_TLS" ] || tls_flag=(--plain-http)
    nohup "$MONITOR_BIN" --data-dir "$DATA_DIR" --listen "$LISTEN" \
      "${tls_flag[@]}" >"$MONITOR_LOG" 2>&1 &
    sleep 2
    if [ -n "$USE_TLS" ]; then
      say "monitor 已启动 → $(url)（内置 TLS，只需服务端证书）"
    else
      say "monitor 已启动 → $(url)（明文 HTTP，TLS 交给前置边缘）"
    fi
  fi

  if pgrep -f 'zhiwei-node --state-dir' >/dev/null 2>&1; then
    say "node 已在运行"
  else
    local token
    token="$(grep -o 'zhi-bt-[a-f0-9]*' "$MONITOR_LOG" | tail -1 || true)"
    [ -n "$token" ] || die "日志里没找到 bootstrap token；先跑 ./scripts/dev.sh token"
    # 节点身份靠 signing.key 的签名，不需要任何证书
    ZHIWEI_MONITOR_URL="$(url)" ZHIWEI_BOOTSTRAP_TOKEN="$token" \
      nohup "$NODE_BIN" --state-dir "$DATA_DIR/node" --interval "$INTERVAL" >"$NODE_LOG" 2>&1 &
    sleep 3
    say "node 已启动（上报间隔 ${INTERVAL}s）"
  fi

  cmd_status
  echo
  say "试试这些："
  echo "  ./scripts/dev.sh query"
  echo "  ./scripts/dev.sh watch"
  echo "  tail -f $MONITOR_LOG"
}

cmd_query() {
  q "$(url)/v1/nodes" | python3 -m json.tool
}

cmd_index() {
  q "$(url)/" | python3 -m json.tool
}

cmd_watch() {
  local node_id="${1:-}"
  if [ -z "$node_id" ]; then
    node_id="$(q "$(url)/v1/nodes" | python3 -c 'import sys,json; print(json.load(sys.stdin)[0]["id"])')"
  fi
  say "跟踪节点 $node_id（Ctrl-C 退出）"
  while true; do
    clear 2>/dev/null || true
    q "$(url)/v1/nodes/$node_id/telemetry?limit=3" | python3 -m json.tool
    sleep 2
  done
}

cmd_token() {
  local token
  token="$(grep -o 'zhi-bt-[a-f0-9]*' "$MONITOR_LOG" | tail -1 || true)"
  [ -n "$token" ] || die "monitor 日志里没有 token"
  echo "$token"
}

cmd_status() {
  say "ops:     $(pgrep -f 'zhiwei-ops --data-dir' >/dev/null && echo 运行中 || echo 未运行)"
  say "monitor: $(pgrep -f 'zhiwei-monitor --data-dir' >/dev/null && echo 运行中 || echo 未运行)"
  say "node:    $(pgrep -f 'zhiwei-node --state-dir' >/dev/null && echo 运行中 || echo 未运行)"
  [ -f "$MONITOR_LOG" ] && say "最近 monitor 日志：" && tail -3 "$MONITOR_LOG"
  [ -f "$NODE_LOG" ] && say "最近 node 日志：" && tail -3 "$NODE_LOG"
}

cmd_stop() {
  pkill -f 'zhiwei-monitor --data-dir' 2>/dev/null || true
  pkill -f 'zhiwei-node --state-dir' 2>/dev/null || true
  pkill -f 'zhiwei-ops --data-dir' 2>/dev/null || true
  say "已停止（含 ops-server）"
}

cmd_clean() {
  cmd_stop
  rm -rf "$DATA_DIR"
  say "已删除 $DATA_DIR（CA 一并删除，下次启动重新生成）"
}

case "${1:-}" in
  start) cmd_start ;;
  query) cmd_query ;;
  index) cmd_index ;;
  watch) shift; cmd_watch "${1:-}" ;;
  token) cmd_token ;;
  status) cmd_status ;;
  stop) cmd_stop ;;
  clean) cmd_clean ;;
  *)
    sed -n '3,15p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac
