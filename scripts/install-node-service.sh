#!/bin/sh
# ZhiWei 节点常驻服务安装器
#
#   curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install-node-service.sh \
#     | sudo sh -s -- --token zhi-bt-xxxxxxxx
#
#   # 全环境变量版：
#   curl -fsSL ... | sudo env ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxx ZHIWEI_MONITOR_URL=https://x.onrender.com sh
#
#   # 啥都不传也行，脚本会逐项交互问（token 隐藏输入）：
#   curl -fsSL ... | sudo sh
#
# 这个脚本做四件事：
#   1. 拉 install.sh 把 zhiwei-node 二进制装到 /usr/local/bin
#   2. 把 ZHIWEI_MONITOR_URL / ZHIWEI_BOOTSTRAP_TOKEN 写到 /etc/zhiwei/node.env（0600）
#   3. 按平台装一个常驻服务：
#        Linux + systemd   → /etc/systemd/system/zhiwei-node.service
#        Linux 无 systemd  → nohup 后台进程（容器 / Alpine）
#        macOS             → ~/Library/LaunchAgents/com.zhiwei.node.plist
#   4. 启动并打印后续命令（看日志、卸载等）
#
# 子命令：
#   install   装（默认）
#   uninstall 停服务并清 unit / plist / env 文件（不删 state-dir）
#   status    打印服务状态 + 最近 20 行日志
#   logs      持续跟日志
#
# 参数 / 环境变量（参数优先）：
#   --token <token>    入网令牌（也可 ZHIWEI_BOOTSTRAP_TOKEN）
#   --url   <url>      monitor URL（也可 ZHIWEI_MONITOR_URL；不传则交互问）
#   --state-dir <dir>  节点状态目录，默认 /var/lib/zhiwei-node
#   --interval <sec>   上报间隔，默认 5
#   --dir <dir>        二进制目录，默认 /usr/local/bin
#   --repo <o/r>       默认 hancic128/zhiwei
#   --branch <name>    默认 main（私有仓库切换）
#   --no-start         装好但不启动（仅 install）
#   -h | --help

set -eu

REPO="${ZHIWEI_REPO:-hancic128/zhiwei}"
BRANCH="${ZHIWEI_BRANCH:-main}"
INSTALL_DIR="${ZHIWEI_INSTALL_DIR:-/usr/local/bin}"
STATE_DIR="${ZHIWEI_STATE_DIR:-/var/lib/zhiwei-node}"
INTERVAL="${ZHIWEI_INTERVAL:-5}"
ENV_FILE="${ZHIWEI_ENV_FILE:-/etc/zhiwei/node.env}"
TOKEN="${ZHIWEI_BOOTSTRAP_TOKEN:-}"
URL="${ZHIWEI_MONITOR_URL:-}"
NO_START=0
ACTION="install"

die()  { printf '\033[31m错误:\033[0m %s\n' "$*" >&2; exit 1; }
note() { printf '\033[36m  ·\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[32m  ✓\033[0m %s\n' "$*" >&2; }

usage() {
  sed -n '2,42p' "$0" | sed 's/^# \{0,1\}//'
}

# ---- 参数解析 ----
while [ $# -gt 0 ]; do
  case "$1" in
    --token)     TOKEN="${2:?--token 需要一个值}"; shift 2 ;;
    --url)       URL="${2:?--url 需要一个值}"; shift 2 ;;
    --state-dir) STATE_DIR="${2:?--state-dir 需要一个值}"; shift 2 ;;
    --interval)  INTERVAL="${2:?--interval 需要一个值}"; shift 2 ;;
    --dir)       INSTALL_DIR="${2:?--dir 需要一个值}"; shift 2 ;;
    --repo)      REPO="${2:?--repo 需要一个值}"; shift 2 ;;
    --branch)    BRANCH="${2:?--branch 需要一个值}"; shift 2 ;;
    --no-start)  NO_START=1; shift ;;
    install|uninstall|status|logs) ACTION="$1"; shift ;;
    -h|--help)   usage; exit 0 ;;
    *) die "未知参数：$1（用 --help 看用法）" ;;
  esac
done

# ---- 工具 ----
is_root() { [ "$(id -u)" -eq 0 ]; }
have()    { command -v "$1" >/dev/null 2>&1; }
fetch() { # fetch <url> <dest>
  if have curl; then
    curl -fsSL ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} "$1" -o "$2"
  elif have wget; then
    wget -q ${GITHUB_TOKEN:+--header="Authorization: Bearer $GITHUB_TOKEN"} -O "$2" "$1"
  else
    die "需要 curl 或 wget"
  fi
}

# ---- sudo 自动提权 ----
# install / uninstall 要写 /etc、/var/lib 或 ~/Library，要么 root 要么 sudo。
# 先在当前 shell 收齐 token（避免 sudo 提权后 tty 走样或 env 被 reset），再 exec sudo。
ensure_root() { # ensure_root "$@" — 透传脚本参数给 exec sudo
  if is_root; then return; fi
  if [ "$ACTION" = "install" ] && [ -z "$TOKEN" ]; then
    ask_token
  fi
  printf '\033[36m==>\033[0m 自动 sudo 提权\n'
  # shellcheck disable=SC2120
  if [ -n "$TOKEN" ]; then
    exec sudo -E sh "$0" "$@" --token "$TOKEN"
  else
    exec sudo -E sh "$0" "$@"
  fi
}

# ---- 交互问 ----
# 真探测 tty 是否可用（不能用 [ -r /dev/tty ] —— macOS 非交互 shell 里 stat 过但 read 失败）
have_tty() {
  exec 3</dev/tty 2>/dev/null || return 1
  exec 3<&-
  return 0
}

ask_url() {
  if [ -n "$URL" ]; then return; fi
  have_tty || die "没传 --url / ZHIWEI_MONITOR_URL，又没有 tty 可问"
  printf 'monitor URL（例如 https://zhiwei.onrender.com/）: '
  read -r URL </dev/tty || die "读取失败"
  [ -n "$URL" ] || die "URL 不能为空"
}

ask_token() {
  if [ -n "$TOKEN" ]; then return; fi
  have_tty || die "没传 --token / ZHIWEI_BOOTSTRAP_TOKEN，又没有 tty 可问"
  printf 'bootstrap token（输入隐藏）: '
  stty -echo 2>/dev/null || true
  read -r TOKEN </dev/tty || { stty echo 2>/dev/null || true; die "读取失败"; }
  stty echo 2>/dev/null || true
  printf '\n'
  [ -n "$TOKEN" ] || die "token 不能为空"
}

# ============================================================
# uninstall / status / logs 分支
# ============================================================
detect_init() {
  if [ "$(uname -s)" = "Darwin" ]; then
    echo "launchd"
  elif have systemctl && [ -d /run/systemd/system ]; then
    echo "systemd"
  else
    echo "nohup"
  fi
}

do_uninstall() { # do_uninstall "$@" — 透传给 ensure_root
  # shellcheck disable=SC2120
  # shellcheck disable=SC2119
  ensure_root "$@"
  init="$(detect_init)"
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

  if [ -f "$ENV_FILE" ]; then
    rm -f "$ENV_FILE"
    rmdir "$(dirname "$ENV_FILE")" 2>/dev/null || true
    ok "已删 $ENV_FILE（state-dir $STATE_DIR 保留，里头是 signing.key / node.id，重装不丢）"
  fi
  echo "二进制 ${INSTALL_DIR}/zhiwei-node 没动，要清就 rm -f"
}

do_status() {
  init="$(detect_init)"
  case "$init" in
    systemd)
      systemctl status zhiwei-node --no-pager || true
      echo "---- 最近 20 行日志 ----"
      journalctl -u zhiwei-node -n 20 --no-pager || true
      ;;
    launchd)
      uid="$(id -u)"
      launchctl print "gui/$uid/com.zhiwei.node" 2>/dev/null | head -20 || true
      echo "---- 最近 20 行 ----"
      log show --predicate 'process == "zhiwei-node"' --last 10m 2>/dev/null \
        | tail -20 || true
      ;;
    nohup)
      if pgrep -f "zhiwei-node --state-dir $STATE_DIR" >/dev/null 2>&1; then
        echo "zhiwei-node 在跑，PID：$(pgrep -f "zhiwei-node --state-dir $STATE_DIR" | head -1)"
      else
        echo "zhiwei-node 没在跑"
      fi
      [ -f /var/log/zhiwei-node.log ] && tail -20 /var/log/zhiwei-node.log
      ;;
  esac
}

do_logs() {
  init="$(detect_init)"
  case "$init" in
    systemd)  exec journalctl -u zhiwei-node -f ;;
    launchd)  exec log stream --predicate 'process == "zhiwei-node"' ;;
    nohup)
      touch /var/log/zhiwei-node.log
      exec tail -f /var/log/zhiwei-node.log
      ;;
  esac
}

if [ "$ACTION" = "uninstall" ]; then do_uninstall "$@"; exit 0; fi
if [ "$ACTION" = "status"   ]; then do_status;   exit 0; fi
if [ "$ACTION" = "logs"     ]; then do_logs;     exit 0; fi

# ============================================================
# install 主流程
# ============================================================
printf '\033[36m==>\033[0m 知微节点常驻服务安装器\n'
ensure_root "$@"
ask_url
ask_token

# 1) 装二进制：已存在就跳过下载（env 覆盖 + 重启服务即可，不必重装）
BIN="${INSTALL_DIR}/zhiwei-node"
if [ -x "$BIN" ]; then
  note "$BIN 已存在，跳过下载（重跑只更新 env / 覆盖 unit / 重启服务）"
  if "$BIN" --help >/dev/null 2>&1; then
    ok "现有二进制可用"
  else
    die "$BIN 存在但 --help 失败，可能不是 zhiwei-node（删掉再重装，或换 --dir）"
  fi
else
  printf '\033[36m==>\033[0m 装 zhiwei-node 二进制\n'
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT INT TERM
  fetch "https://raw.githubusercontent.com/${REPO}/${BRANCH}/scripts/install.sh" "$tmpdir/install.sh"
  sh "$tmpdir/install.sh" --bin node --dir "$INSTALL_DIR"
  [ -x "$BIN" ] || die "二进制没装好：$BIN"
fi

# 2) 写 env 文件
printf '\033[36m==>\033[0m 写 %s\n' "$ENV_FILE"
tmp="$(mktemp)"
printf 'ZHIWEI_MONITOR_URL=%s\nZHIWEI_BOOTSTRAP_TOKEN=%s\nZHIWEI_INTERVAL=%s\n' \
  "$URL" "$TOKEN" "$INTERVAL" > "$tmp"
# 保留老文件里的额外变量
if [ -f "$ENV_FILE" ]; then
  note "已存在 $ENV_FILE，合并 URL/token，旧的额外变量保留"
  grep -v '^ZHIWEI_MONITOR_URL=\|^ZHIWEI_BOOTSTRAP_TOKEN=\|^ZHIWEI_INTERVAL=' "$ENV_FILE" >> "$tmp" || true
fi
mkdir -p "$(dirname "$ENV_FILE")"
chmod 0750 "$(dirname "$ENV_FILE")" 2>/dev/null || true
install -m 0600 "$tmp" "$ENV_FILE"
rm -f "$tmp"
ok "env 文件就绪（0600，仅 root 可读）"

# 3) 装服务
init="$(detect_init)"
printf '\033[36m==>\033[0m 装常驻服务（%s）\n' "$init"

case "$init" in
  systemd)
    unit=/etc/systemd/system/zhiwei-node.service
    cat > "$unit" <<EOF
[Unit]
Description=Zhiwei node agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=$ENV_FILE
# interval 走 env ZHIWEI_INTERVAL（见 /etc/zhiwei/node.env），不再写在 ExecStart 命令行里
ExecStart=$BIN --state-dir $STATE_DIR
Restart=on-failure
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=15

StateDirectory=zhiwei-node
StandardOutput=journal
StandardError=journal
SyslogIdentifier=zhiwei-node

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    ok "已写 $unit"
    if [ "$NO_START" = "0" ]; then
      systemctl enable --now zhiwei-node
      ok "已 enable + start"
    else
      note "装好了但没启动（--no-start），手动起：systemctl enable --now zhiwei-node"
    fi
    ;;

  launchd)
    uid="$(id -u)"
    plist_dir="$HOME/Library/LaunchAgents"
    plist="$plist_dir/com.zhiwei.node.plist"
    mkdir -p "$plist_dir"
    state_parent="$(dirname "$STATE_DIR")"
    mkdir -p "$state_parent"
    cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.zhiwei.node</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN</string>
    <string>--state-dir</string><string>$STATE_DIR</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>ZHIWEI_MONITOR_URL</key><string>$URL</string>
    <key>ZHIWEI_INTERVAL</key><string>$INTERVAL</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict>
    <key>SuccessfulExit</key><false/>
    <key>Crashed</key><true/>
  </dict>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/zhiwei-node.log</string>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/zhiwei-node.log</string>
  <key>ProcessType</key><string>Background</string>
</dict>
</plist>
EOF
    ok "已写 $plist"
    if [ "$NO_START" = "0" ]; then
      launchctl bootstrap "gui/$uid" "$plist" 2>/dev/null \
        || launchctl load -w "$plist"
      ok "已 bootstrap + load（开机自启）"
    else
      note "装好了但没启动（--no-start），手动起：launchctl load -w $plist"
    fi
    ;;

  nohup)
    # 容器 / Alpine / 任何没 systemd 的 Linux
    state_parent="$(dirname "$STATE_DIR")"
    mkdir -p "$state_parent"
    logfile=/var/log/zhiwei-node.log
    touch "$logfile"
    pkill -f "zhiwei-node --state-dir $STATE_DIR" 2>/dev/null || true
    sleep 0.3
    # setsid 脱离会话 + nohup 屏蔽 SIGHUP；stdin 重定向到 /dev/null 防止 tty 关闭
    if have setsid; then
      setsid env \
        ZHIWEI_MONITOR_URL="$URL" \
        ZHIWEI_BOOTSTRAP_TOKEN="$TOKEN" \
        ZHIWEI_INTERVAL="$INTERVAL" \
        "$BIN" --state-dir "$STATE_DIR" \
        </dev/null >>"$logfile" 2>&1 &
    else
      nohup env \
        ZHIWEI_MONITOR_URL="$URL" \
        ZHIWEI_BOOTSTRAP_TOKEN="$TOKEN" \
        "$BIN" --state-dir "$STATE_DIR" \
        </dev/null >>"$logfile" 2>&1 &
    fi
    sleep 0.5
    if pgrep -f "zhiwei-node --state-dir $STATE_DIR" >/dev/null 2>&1; then
      ok "已后台启动，日志：$logfile"
    else
      die "启动失败，看 $logfile"
    fi
    note "注意：nohup 模式不会开机自启；容器里靠编排器托管"
    ;;
esac

# 4) 收尾提示
printf '\n\033[32m✓ 装好了\033[0m\n\n'
printf '  平台     %s\n' "$(uname -srm)"
printf '  init     %s\n' "$init"
printf '  二进制   %s\n' "$BIN"
printf '  env      %s\n' "$ENV_FILE"
printf '  state    %s\n' "$STATE_DIR"
printf '\n  看日志：\n'
case "$init" in
  systemd) echo "    journalctl -u zhiwei-node -f" ;;
  launchd) echo "    tail -f ~/Library/Logs/zhiwei-node.log" ;;
  nohup)   echo "    tail -f /var/log/zhiwei-node.log" ;;
esac

cat <<EOF

  查状态：    $0 status
  看日志：    $0 logs
  卸  载：    $0 uninstall

  想改 telemetry 频率（默认 5 秒）：
    sudo vim $ENV_FILE   # 改 ZHIWEI_INTERVAL=60
    sudo systemctl restart zhiwei-node
  改 inventory / 证书 / 节点名 / state-dir 同理（ZHIWEI_INVENTORY_INTERVAL、
  ZHIWEI_CERT_GLOBS、ZHIWEI_NODE_NAME、ZHIWEI_STATE_DIR）。

  入网成功后，建议从 $ENV_FILE 里删掉 ZHIWEI_BOOTSTRAP_TOKEN，
  重启服务即撤销该入网密钥（节点本地有 signing.key，不再需要它）。
EOF
