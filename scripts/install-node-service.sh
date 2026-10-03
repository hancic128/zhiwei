#!/bin/sh
# ZhiWei node daemon installer
#
#   curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install-node-service.sh \
#     | sudo sh -s -- --token zhi-bt-xxxxxxxx
#
#   # All-env-vars version:
#   curl -fsSL ... | sudo env ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxx ZHIWEI_MONITOR_URL=https://x.onrender.com sh
#
#   # Or pass nothing — the script prompts interactively for each value (token is hidden):
#   curl -fsSL ... | sudo sh
#
#   # Upgrade to the latest version (re-downloads the binary and restarts the service; env / state / service config are untouched):
#   curl -fsSL ... | sudo sh -s -- upgrade
#
# This script does four things:
#   1. Runs install.sh to install the zhiwei-node binary into /usr/local/bin
#   2. Writes ZHIWEI_MONITOR_URL / ZHIWEI_BOOTSTRAP_TOKEN to /etc/zhiwei/node.env (0600)
#   3. Installs a platform-appropriate daemon:
#        Linux + systemd   → /etc/systemd/system/zhiwei-node.service
#        Linux no systemd → nohup background process (containers / Alpine)
#        macOS             → ~/Library/LaunchAgents/com.zhiwei.node.plist
#   4. Starts the service and prints follow-up commands (logs, uninstall, etc.)
#
# Subcommands:
#   install   Install (default)
#   upgrade   In-place upgrade: re-download the latest binary and restart the service; env / state-dir / service config are preserved
#   uninstall Stops the service and removes the unit / plist / env files (keeps state-dir)
#   status    Prints service status + last 20 log lines
#   logs      Tails logs continuously
#
# Flags / environment variables (flags take precedence):
#   --token <token>    Bootstrap token (or ZHIWEI_BOOTSTRAP_TOKEN)
#   --url   <url>      Monitor URL (or ZHIWEI_MONITOR_URL; interactive prompt if unset)
#   --state-dir <dir>  Node state directory, default /var/lib/zhiwei-node
#   --interval <sec>   Reporting interval in seconds, default 5
#   --dir <dir>        Binary directory, default /usr/local/bin
#   --repo <o/r>       Default hancic128/zhiwei
#   --branch <name>    Default main (switch for private repos)
#   --no-start         Install but don't start (install only)
#   ZHIWEI_VERSION     Which version to install, default latest (applies to install / upgrade, same as install.sh)
#   ZHIWEI_BASE_URL    Use a self-hosted mirror / artifact repo (same as above; machines in mainland China rely on this to avoid GitHub)
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

die()  { printf '\033[31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }
note() { printf '\033[36m  ·\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[32m  ✓\033[0m %s\n' "$*" >&2; }

usage() {
  # Read the header comment block until the first blank line (more robust than hard-coding line numbers: adding/removing comments doesn't require syncing here)
  sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
}

# ---- Argument parsing ----
while [ $# -gt 0 ]; do
  case "$1" in
    --token)     TOKEN="${2:?--token requires a value}"; shift 2 ;;
    --url)       URL="${2:?--url requires a value}"; shift 2 ;;
    --state-dir) STATE_DIR="${2:?--state-dir requires a value}"; shift 2 ;;
    --interval)  INTERVAL="${2:?--interval requires a value}"; shift 2 ;;
    --dir)       INSTALL_DIR="${2:?--dir requires a value}"; shift 2 ;;
    --repo)      REPO="${2:?--repo requires a value}"; shift 2 ;;
    --branch)    BRANCH="${2:?--branch requires a value}"; shift 2 ;;
    --no-start)  NO_START=1; shift ;;
    install|uninstall|status|logs|upgrade) ACTION="$1"; shift ;;
    -h|--help)   usage; exit 0 ;;
    *) die "Unknown flag: $1 (use --help for usage)" ;;
  esac
done

# ---- Utilities ----
is_root() { [ "$(id -u)" -eq 0 ]; }
have()    { command -v "$1" >/dev/null 2>&1; }
fetch() { # fetch <url> <dest>
  if have curl; then
    curl -fsSL ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} "$1" -o "$2"
  elif have wget; then
    wget -q ${GITHUB_TOKEN:+--header="Authorization: Bearer $GITHUB_TOKEN"} -O "$2" "$1"
  else
    die "curl or wget is required"
  fi
}

# ---- Automatic sudo escalation ----
# install / uninstall need to write to /etc, /var/lib, or ~/Library — either root or sudo is required.
# Collect the token in the current shell first (avoids a mangled tty or env being reset after sudo escalation), then exec sudo.
ensure_root() { # ensure_root "$@" — passes script arguments through to exec sudo
  if is_root; then return; fi
  if [ "$ACTION" = "install" ] && [ -z "$TOKEN" ]; then
    ask_token
  fi
  printf '\033[36m==>\033[0m Automatic sudo escalation\n'
  # shellcheck disable=SC2120
  if [ -n "$TOKEN" ]; then
    exec sudo -E sh "$0" "$@" --token "$TOKEN"
  else
    exec sudo -E sh "$0" "$@"
  fi
}

# ---- Interactive prompts ----
# Actually probe whether a tty is usable (can't use [ -r /dev/tty ] — macOS non-interactive shells pass stat but read fails)
have_tty() {
  exec 3</dev/tty 2>/dev/null || return 1
  exec 3<&-
  return 0
}

ask_url() {
  if [ -n "$URL" ]; then return; fi
  have_tty || die "Neither --url / ZHIWEI_MONITOR_URL was provided nor a tty available to ask"
  printf 'Monitor URL (e.g. https://zhiwei.onrender.com/): '
  read -r URL </dev/tty || die "Read failed"
  [ -n "$URL" ] || die "URL cannot be empty"
}

ask_token() {
  if [ -n "$TOKEN" ]; then return; fi
  have_tty || die "Neither --token / ZHIWEI_BOOTSTRAP_TOKEN was provided nor a tty available to ask"
  printf 'Bootstrap token (input hidden): '
  stty -echo 2>/dev/null || true
  read -r TOKEN </dev/tty || { stty echo 2>/dev/null || true; die "Read failed"; }
  stty echo 2>/dev/null || true
  printf '\n'
  [ -n "$TOKEN" ] || die "Token cannot be empty"
}

# ============================================================
# uninstall / status / logs branches
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

do_uninstall() { # do_uninstall "$@" — passes through to ensure_root
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
      ok "Stopped + removed systemd unit"
      ;;
    launchd)
      uid="$(id -u)"
      plist="$HOME/Library/LaunchAgents/com.zhiwei.node.plist"
      launchctl bootout "gui/$uid/com.zhiwei.node" 2>/dev/null \
        || launchctl unload "$plist" 2>/dev/null \
        || true
      rm -f "$plist"
      ok "Stopped + removed launchd plist"
      ;;
    nohup)
      pkill -f "zhiwei-node --state-dir $STATE_DIR" 2>/dev/null || true
      ok "Stopped nohup background process"
      ;;
  esac

  if [ -f "$ENV_FILE" ]; then
    rm -f "$ENV_FILE"
    rmdir "$(dirname "$ENV_FILE")" 2>/dev/null || true
    ok "Removed $ENV_FILE (state-dir $STATE_DIR is preserved — it holds signing.key / node.id, not lost on reinstall)"
  fi
  echo "Binary ${INSTALL_DIR}/zhiwei-node was not touched; rm -f to remove it"
}

do_status() {
  init="$(detect_init)"
  case "$init" in
    systemd)
      systemctl status zhiwei-node --no-pager || true
      echo "---- Last 20 log lines ----"
      journalctl -u zhiwei-node -n 20 --no-pager || true
      ;;
    launchd)
      uid="$(id -u)"
      launchctl print "gui/$uid/com.zhiwei.node" 2>/dev/null | head -20 || true
      echo "---- Last 20 lines ----"
      log show --predicate 'process == "zhiwei-node"' --last 10m 2>/dev/null \
        | tail -20 || true
      ;;
    nohup)
      if pgrep -f "zhiwei-node --state-dir $STATE_DIR" >/dev/null 2>&1; then
        echo "zhiwei-node is running, PID: $(pgrep -f "zhiwei-node --state-dir $STATE_DIR" | head -1)"
      else
        echo "zhiwei-node is not running"
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

# ============================================================
# upgrade: in-place binary replacement (forces re-download) + service restart.
#
# Only these two things happen:
#   - env / state-dir are identity and config — they should not be touched by an upgrade (to change identity, uninstall + install);
#   - service configuration (unit / plist) is only written by the install branch; upgrade does not rewrite it, so manual hardening is preserved.
#     To refresh everything, re-run install (without --no-start).
# The install branch skips the download when the binary already exists, so upgrade must trigger a fresh download itself.
# ============================================================
do_upgrade() {
  ensure_root "$@"
  init="$(detect_init)"
  BIN="${INSTALL_DIR}/zhiwei-node"

  if [ ! -f "$ENV_FILE" ] && [ ! -x "$BIN" ]; then
    die "No node has been installed on this machine (neither $ENV_FILE nor $BIN exists) — upgrade can only upgrade an already-installed node. Use install for first-time installation"
  fi

  old_ver=""
  if [ -x "$BIN" ]; then
    old_ver="$("$BIN" --version 2>/dev/null | tail -n1 | awk '{print $NF}')"
  fi

  printf '\033[36m==>\033[0m Re-downloading and installing %s\n' "$BIN"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT INT TERM
  fetch "https://raw.githubusercontent.com/${REPO}/${BRANCH}/scripts/install.sh" "$tmpdir/install.sh"
  sh "$tmpdir/install.sh" --bin node --dir "$INSTALL_DIR"
  [ -x "$BIN" ] || die "Binary not installed correctly: $BIN"
  new_ver="$("$BIN" --version 2>/dev/null | tail -n1 | awk '{print $NF}')"
  ok "Binary ${old_ver:-?} → ${new_ver:-?}"

  case "$init" in
    systemd)
      systemctl restart zhiwei-node 2>/dev/null \
        || die "Failed to restart zhiwei-node: check systemctl status zhiwei-node first (if the unit is missing, re-run install)"
      ok "Restarted zhiwei-node"
      ;;
    launchd)
      uid="$(id -u)"
      launchctl kickstart -k "gui/$uid/com.zhiwei.node" 2>/dev/null \
        || die "Failed to restart launchd job: make sure the plist written by the install branch still exists"
      ok "Restarted launchd job"
      ;;
    nohup)
      # Container / Alpine: read URL and interval back from the env file and start it up again (0600, readable by root)
      set -a
      # shellcheck disable=SC1090
      . "$ENV_FILE"
      set +a
      pkill -f "zhiwei-node --state-dir $STATE_DIR" 2>/dev/null || true
      sleep 0.3
      logfile=/var/log/zhiwei-node.log
      touch "$logfile"
      if have setsid; then
        setsid "$BIN" --state-dir "$STATE_DIR" </dev/null >>"$logfile" 2>&1 &
      else
        nohup "$BIN" --state-dir "$STATE_DIR" </dev/null >>"$logfile" 2>&1 &
      fi
      sleep 0.5
      pgrep -f "zhiwei-node --state-dir $STATE_DIR" >/dev/null 2>&1 \
        || die "Start failed; check $logfile"
      ok "Restarted in background; log: $logfile"
      ;;
  esac

  note "env / state-dir / service configuration were not touched; to refresh the unit (e.g. systemd hardening), re-run install"
}

if [ "$ACTION" = "uninstall" ]; then do_uninstall "$@"; exit 0; fi
if [ "$ACTION" = "status"   ]; then do_status;   exit 0; fi
if [ "$ACTION" = "logs"     ]; then do_logs;     exit 0; fi
if [ "$ACTION" = "upgrade"  ]; then do_upgrade "$@"; exit 0; fi

# ============================================================
# install main flow
# ============================================================
printf '\033[36m==>\033[0m ZhiWei node daemon installer\n'
ensure_root "$@"
ask_url
ask_token

# 1) Install the binary: skip the download if it already exists (overwriting env + restarting the service is enough; no reinstall needed)
BIN="${INSTALL_DIR}/zhiwei-node"
if [ -x "$BIN" ]; then
  note "$BIN already exists, skipping download (re-running only updates env / overwrites unit / restarts the service)"
  if "$BIN" --help >/dev/null 2>&1; then
    ok "Existing binary is usable"
  else
    die "$BIN exists but --help failed — it might not be zhiwei-node (delete it and reinstall, or change --dir)"
  fi
else
  printf '\033[36m==>\033[0m Installing zhiwei-node binary\n'
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT INT TERM
  fetch "https://raw.githubusercontent.com/${REPO}/${BRANCH}/scripts/install.sh" "$tmpdir/install.sh"
  sh "$tmpdir/install.sh" --bin node --dir "$INSTALL_DIR"
  [ -x "$BIN" ] || die "Binary not installed correctly: $BIN"
fi

# 2) Write env file
printf '\033[36m==>\033[0m Writing %s\n' "$ENV_FILE"
tmp="$(mktemp)"
printf 'ZHIWEI_MONITOR_URL=%s\nZHIWEI_BOOTSTRAP_TOKEN=%s\nZHIWEI_INTERVAL=%s\n' \
  "$URL" "$TOKEN" "$INTERVAL" > "$tmp"
# Preserve extra variables from the existing file
if [ -f "$ENV_FILE" ]; then
  note "$ENV_FILE already exists; merging URL/token, preserving existing extra variables"
  grep -v '^ZHIWEI_MONITOR_URL=\|^ZHIWEI_BOOTSTRAP_TOKEN=\|^ZHIWEI_INTERVAL=' "$ENV_FILE" >> "$tmp" || true
fi
mkdir -p "$(dirname "$ENV_FILE")"
chmod 0750 "$(dirname "$ENV_FILE")" 2>/dev/null || true
install -m 0600 "$tmp" "$ENV_FILE"
rm -f "$tmp"
ok "env file ready (0600, root-readable only)"

# 3) Install the service
init="$(detect_init)"
printf '\033[36m==>\033[0m Installing daemon (%s)\n' "$init"

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
# interval comes from env ZHIWEI_INTERVAL (see /etc/zhiwei/node.env), not the ExecStart command line
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
# read-only instead of true: the default cert scan paths include /root/nginx-certs;
# ProtectHome=true would mount /root as an empty directory → those certs would never be found (and no error would be raised).
ProtectHome=read-only
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    ok "Wrote $unit"
    if [ "$NO_START" = "0" ]; then
      systemctl enable --now zhiwei-node
      ok "Enabled + started"
    else
      note "Installed but not started (--no-start); start manually: systemctl enable --now zhiwei-node"
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
    ok "Wrote $plist"
    if [ "$NO_START" = "0" ]; then
      launchctl bootstrap "gui/$uid" "$plist" 2>/dev/null \
        || launchctl load -w "$plist"
      ok "Bootstrapped + loaded (auto-start on boot)"
    else
      note "Installed but not started (--no-start); start manually: launchctl load -w $plist"
    fi
    ;;

  nohup)
    # Container / Alpine / any Linux without systemd
    state_parent="$(dirname "$STATE_DIR")"
    mkdir -p "$state_parent"
    logfile=/var/log/zhiwei-node.log
    touch "$logfile"
    pkill -f "zhiwei-node --state-dir $STATE_DIR" 2>/dev/null || true
    sleep 0.3
    # setsid detaches the session + nohup blocks SIGHUP; redirect stdin to /dev/null to prevent tty close from killing it
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
      ok "Started in background; log: $logfile"
    else
      die "Start failed; check $logfile"
    fi
    note "Note: nohup mode does not auto-start on boot; in containers, rely on the orchestrator"
    ;;
esac

# 4) Final summary
printf '\n\033[32m✓ Installation complete\033[0m\n\n'
printf '  Platform %s\n' "$(uname -srm)"
printf '  init     %s\n' "$init"
printf '  Binary   %s\n' "$BIN"
printf '  env      %s\n' "$ENV_FILE"
printf '  state    %s\n' "$STATE_DIR"
printf '\n  To view logs:\n'
case "$init" in
  systemd) echo "    journalctl -u zhiwei-node -f" ;;
  launchd) echo "    tail -f ~/Library/Logs/zhiwei-node.log" ;;
  nohup)   echo "    tail -f /var/log/zhiwei-node.log" ;;
esac

cat <<EOF

  Check status:  $0 status
  View logs:     $0 logs
  Uninstall:     $0 uninstall

  To change the telemetry frequency (default 5 seconds):
    sudo vim $ENV_FILE   # change ZHIWEI_INTERVAL=60
    sudo systemctl restart zhiwei-node
  Adjusting inventory / certs / node name / state-dir follows the same pattern (ZHIWEI_INVENTORY_INTERVAL,
  ZHIWEI_CERT_GLOBS, ZHIWEI_NODE_NAME, ZHIWEI_STATE_DIR).

  After the node has joined successfully, it is recommended to remove ZHIWEI_BOOTSTRAP_TOKEN from $ENV_FILE;
  restart the service to revoke that bootstrap key (the node already has signing.key locally and no longer needs it).
EOF
