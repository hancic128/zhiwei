#!/bin/sh
# ZhiWei node daemon uninstaller
#
#   curl -fsSL https://raw.githubusercontent.com/zhiwei/zhiwei/main/scripts/uninstall-node-service.sh \
#     | sudo sh
#
# Equivalent to `install-node-service.sh uninstall`, but standalone (no
# dependency on the install script).
#
# Default behavior:
#   - Stop + remove the systemd unit / launchd plist / nohup process
#   - Remove /etc/zhiwei/node.env (and the empty /etc/zhiwei directory)
#   - Keep /var/lib/zhiwei-node (holds signing.key / node.id; reinstall keeps the identity)
#
# Arguments:
#   --purge          delete the state-dir too (caution: node identity is permanently lost;
#                    re-enroll is required)
#   --keep-binary    keep /usr/local/bin/zhiwei-node as-is (default)
#   --remove-binary  also rm /usr/local/bin/zhiwei-node
#   --state-dir <dir>  default /var/lib/zhiwei-node
#   --env-file   <path> default /etc/zhiwei/node.env
#   -h | --help

set -eu

STATE_DIR="${ZHIWEI_STATE_DIR:-/var/lib/zhiwei-node}"
ENV_FILE="${ZHIWEI_ENV_FILE:-/etc/zhiwei/node.env}"
BIN="${ZHIWEI_INSTALL_DIR:-/usr/local/bin}/zhiwei-node"
PURGE=0
REMOVE_BIN=0

die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
note() { printf '\033[36m  -\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[32m  +\033[0m %s\n' "$*" >&2; }

usage() { sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --purge)         PURGE=1; shift ;;
    --remove-binary) REMOVE_BIN=1; shift ;;
    --keep-binary)   REMOVE_BIN=0; shift ;;
    --state-dir)     STATE_DIR="${2:?--state-dir requires a value}"; shift 2 ;;
    --env-file)      ENV_FILE="${2:?--env-file requires a value}"; shift 2 ;;
    -h|--help)       usage; exit 0 ;;
    *) die "unknown argument: $1 (use --help for usage)" ;;
  esac
done

is_root() { [ "$(id -u)" -eq 0 ]; }
have()    { command -v "$1" >/dev/null 2>&1; }

ensure_root() {
  is_root && return 0
  printf '\033[36m==>\033[0m auto-escalating with sudo\n'
  exec sudo -E sh "$0" "$@"
}

ensure_root "$@"

detect_init() {
  if [ "$(uname -s)" = "Darwin" ]; then echo "launchd"
  elif have systemctl && [ -d /run/systemd/system ]; then echo "systemd"
  else echo "nohup"
  fi
}

init="$(detect_init)"
printf '\033[36m==>\033[0m ZhiWei node daemon uninstaller (%s)\n' "$init"

# 1) Stop the service
case "$init" in
  systemd)
    systemctl stop    zhiwei-node 2>/dev/null || true
    systemctl disable zhiwei-node 2>/dev/null || true
    rm -f /etc/systemd/system/zhiwei-node.service
    systemctl daemon-reload 2>/dev/null || true
    ok "stopped + removed systemd unit"
    ;;
  launchd)
    uid="$(id -u)"
    plist="$HOME/Library/LaunchAgents/com.zhiwei.node.plist"
    launchctl bootout "gui/$uid/com.zhiwei.node" 2>/dev/null \
      || launchctl unload "$plist" 2>/dev/null \
      || true
    rm -f "$plist"
    ok "stopped + removed launchd plist"
    ;;
  nohup)
    pkill -f "zhiwei-node --state-dir $STATE_DIR" 2>/dev/null || true
    ok "stopped nohup background process"
    ;;
esac

# 2) Remove the env file
if [ -f "$ENV_FILE" ]; then
  rm -f "$ENV_FILE"
  rmdir "$(dirname "$ENV_FILE")" 2>/dev/null || true
  ok "removed $ENV_FILE"
else
  note "$ENV_FILE does not exist; skipped"
fi

# 3) state-dir
if [ "$PURGE" = "1" ] && [ -d "$STATE_DIR" ]; then
  rm -rf "$STATE_DIR"
  ok "purged $STATE_DIR (node identity permanently lost; re-enroll required)"
elif [ -d "$STATE_DIR" ]; then
  note "$STATE_DIR kept (holds signing.key / node.id; reinstall keeps the identity); pass --purge to also remove"
fi

# 4) Binary
if [ "$REMOVE_BIN" = "1" ] && [ -x "$BIN" ]; then
  rm -f "$BIN"
  ok "removed $BIN"
else
  note "binary $BIN untouched (pass --remove-binary to also remove)"
fi

cat <<EOF

\033[32m+ uninstall complete\033[0m

  To reinstall:
    curl -fsSL https://raw.githubusercontent.com/zhiwei/zhiwei/main/scripts/install-node-service.sh \\
      | sudo sh -s -- --token zhi-bt-xxx
EOF