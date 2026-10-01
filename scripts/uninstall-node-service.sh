#!/bin/sh
# ZhiWei 节点常驻服务卸载器
#
#   curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/uninstall-node-service.sh \
#     | sudo sh
#
# 与 install-node-service.sh uninstall 等价，但独立可跑（不依赖 install 脚本）。
#
# 默认行为：
#   - 停 + 移除 systemd unit / launchd plist / nohup 进程
#   - 删 /etc/zhiwei/node.env（以及空目录 /etc/zhiwei）
#   - 保留 /var/lib/zhiwei-node（里头是 signing.key / node.id，重装不丢身份）
#
# 参数：
#   --purge       连 state-dir 一起删（慎重：节点身份永久失效，要重新 enroll）
#   --keep-binary 保留 /usr/local/bin/zhiwei-node 不动（默认保留）
#   --remove-binary  顺带 rm /usr/local/bin/zhiwei-node
#   --state-dir <dir>  默认 /var/lib/zhiwei-node
#   --env-file   <path> 默认 /etc/zhiwei/node.env
#   -h | --help

set -eu

STATE_DIR="${ZHIWEI_STATE_DIR:-/var/lib/zhiwei-node}"
ENV_FILE="${ZHIWEI_ENV_FILE:-/etc/zhiwei/node.env}"
BIN="${ZHIWEI_INSTALL_DIR:-/usr/local/bin}/zhiwei-node"
PURGE=0
REMOVE_BIN=0

die()  { printf '\033[31m错误:\033[0m %s\n' "$*" >&2; exit 1; }
note() { printf '\033[36m  ·\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[32m  ✓\033[0m %s\n' "$*" >&2; }

usage() { sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --purge)         PURGE=1; shift ;;
    --remove-binary) REMOVE_BIN=1; shift ;;
    --keep-binary)   REMOVE_BIN=0; shift ;;
    --state-dir)     STATE_DIR="${2:?--state-dir 需要一个值}"; shift 2 ;;
    --env-file)      ENV_FILE="${2:?--env-file 需要一个值}"; shift 2 ;;
    -h|--help)       usage; exit 0 ;;
    *) die "未知参数：$1（用 --help 看用法）" ;;
  esac
done

is_root() { [ "$(id -u)" -eq 0 ]; }
have()    { command -v "$1" >/dev/null 2>&1; }

ensure_root() {
  is_root && return 0
  printf '\033[36m==>\033[0m 自动 sudo 提权\n'
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
printf '\033[36m==>\033[0m 知微节点常驻服务卸载器（%s）\n' "$init"

# 1) 停服务
case "$init" in
  systemd)
    systemctl stop    zhiwei-node 2>/dev/null || true
    systemctl disable zhiwei-node 2>/dev/null || true
    rm -f /etc/systemd/system/zhiwei-node.service
    systemctl daemon-reload 2>/dev/null || true
    ok "已停 + 移除 systemd unit"
    ;;
  launchd)
    uid="$(id -u)"
    plist="$HOME/Library/LaunchAgents/com.zhiwei.node.plist"
    launchctl bootout "gui/$uid/com.zhiwei.node" 2>/dev/null \
      || launchctl unload "$plist" 2>/dev/null \
      || true
    rm -f "$plist"
    ok "已停 + 移除 launchd plist"
    ;;
  nohup)
    pkill -f "zhiwei-node --state-dir $STATE_DIR" 2>/dev/null || true
    ok "已停 nohup 后台进程"
    ;;
esac

# 2) 清 env 文件
if [ -f "$ENV_FILE" ]; then
  rm -f "$ENV_FILE"
  rmdir "$(dirname "$ENV_FILE")" 2>/dev/null || true
  ok "已删 $ENV_FILE"
else
  note "$ENV_FILE 不存在，跳过"
fi

# 3) state-dir
if [ "$PURGE" = "1" ] && [ -d "$STATE_DIR" ]; then
  rm -rf "$STATE_DIR"
  ok "已 purge $STATE_DIR（节点身份永久失效，要重新 enroll）"
elif [ -d "$STATE_DIR" ]; then
  note "$STATE_DIR 保留（里头是 signing.key / node.id，重装不丢身份）；--purge 可一并删"
fi

# 4) 二进制
if [ "$REMOVE_BIN" = "1" ] && [ -x "$BIN" ]; then
  rm -f "$BIN"
  ok "已删 $BIN"
else
  note "二进制 $BIN 没动（--remove-binary 可一并删）"
fi

cat <<EOF

\033[32m✓ 卸载完成\033[0m

  下次安装：
    curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install-node-service.sh \\
      | sudo sh -s -- --token zhi-bt-xxx
EOF
