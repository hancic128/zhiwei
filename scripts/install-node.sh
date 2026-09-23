#!/bin/sh
# shellcheck shell=bash
# ZhiWei 节点入网安装脚本。
#
#   # 一次性命令：admin在UI里点「生成入网命令」后,目标机器执行下面这一条：
#   curl -sSL https://<monitor>/install-node.sh | \
#     ZHIWEI_MONITOR_URL=https://<monitor> \
#     ZHIWEI_BOOTSTRAP_TOKEN=<token> \
#     bash -s
#
#   # 卸载：
#   curl -sSL https://<monitor>/install-node.sh | sudo bash -s -- --uninstall
#
# 做的事：
#   1. 从 GitHub release 下载 zhiwei-<triple>.tar.gz（内含 zhiwei-node + VERSION）
#   2. 写环境文件（默认 /etc/zhiwei-node.env，mode 0600，仅 root 可读）
#   3. 写常驻服务配置，启动并设置开机自启：
#        Linux  → /etc/systemd/system/zhiwei-node.service（systemd：enable --now）
#        macOS  → /Library/LaunchDaemons/com.zhiwei.node.plist（launchd：bootstrap）
#   4. 换 monitor 时清掉旧节点身份（身份只对签发它的那台 monitor 有效）
#   5. 等节点写回 node.id，确认真的入网了——「脚本跑通」不等于「节点入网」
#
# 平台：Linux（x86_64 / aarch64，systemd）与 macOS（arm64 / x86_64，launchd）；
#       必须 root 运行（请用 sudo）。
#
# 参数：
#   --alias <名字>  可选：入网时一并设置别名（≤10 字符）
#   --tags <列表>   可选：入网时一并设置标签，空格 / 逗号 / 顿号分隔（最多 10 个）
#   --uninstall     停服务 + 删 binary / env / unit（保留 state-dir 数据）
#   -h | --help
#
# 环境变量：
#   ZHIWEI_MONITOR_URL       monitor URL（必填，安装时）
#   ZHIWEI_BOOTSTRAP_TOKEN   入网令牌（必填，安装时）
#   ZHIWEI_NODE_ALIAS        同 --alias
#   ZHIWEI_NODE_TAGS         同 --tags
#   ZHIWEI_VERSION           可选，仅用于日志/兜底；真实版本从包内 VERSION 读。
#                            不设也能装（资产名不含版本号）。
#   ZHIWEI_REPO              仓库 owner/name（默认 hancic128/zhiwei）
#   ZHIWEI_INSTALL_DIR       二进制目录（默认 /usr/local/bin）
#   ZHIWEI_STATE_DIR         状态目录（默认 /var/lib/zhiwei-node）
#   ZHIWEI_ENV_FILE          env 文件路径（默认 /etc/zhiwei-node.env）
#   ZHIWEI_SERVICE_FILE      服务配置路径（默认按平台：systemd unit / launchd plist）
#   ZHIWEI_LOG_FILE          仅 macOS：launchd 捕获的日志文件（默认 /var/log/zhiwei-node.log）
#   ZHIWEI_BASE_URL          自建下载源（同 install.sh 语义）。设了就不走 GitHub Releases。
#                            已知可用值：
#                              - https://artifacts.hancic.site/releases/hancic128/zhiwei
#                                （hancic-artifacts 国内直连，由 CI 每次 tag 同步）
#
# 别名 / 标签只在本机**第一次入网**时上报：已经有 node.id 的机器不会再 enroll，
# 重复执行本脚本改不了它们——请在控制台改，或 rm -rf <state-dir> 后重跑。

set -euo pipefail
umask 077

REPO="${ZHIWEI_REPO:-hancic128/zhiwei}"
VERSION="${ZHIWEI_VERSION:-}"
INSTALL_DIR="${ZHIWEI_INSTALL_DIR:-/usr/local/bin}"
STATE_DIR="${ZHIWEI_STATE_DIR:-/var/lib/zhiwei-node}"
ENV_FILE="${ZHIWEI_ENV_FILE:-/etc/zhiwei-node.env}"
BIN_NAME="zhiwei-node"
SERVICE_NAME="zhiwei-node"
SERVICE_LABEL="com.zhiwei.node"

log() { printf '[zhiwei-install] %s\n' "$*" >&2; }
die() { printf '[zhiwei-install] 错误: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# 环境文件 0600 只给 root；macOS 没有 coreutils 时也能用（install 是 BSD 自带，但别赌）
install_file() { # install_file <mode> <src> <dest>
  if have install; then install -m "$1" "$2" "$3"; else cp "$2" "$3" && chmod "$1" "$3"; fi
}

# 头部注释到第一个空行为止（比写死行号稳，注释加减不用同步改 usage）。
# 管道执行时 $0 是 "bash"，读不到文件——退化成一行用法，别让 sed 报错把 --help 打断。
usage() {
  if [ -r "$0" ]; then
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
  else
    printf '用法: curl -sSL <monitor>/install-node.sh | \\\n  ZHIWEI_MONITOR_URL=<url> ZHIWEI_BOOTSTRAP_TOKEN=<token> bash -s [-- --uninstall]\n'
  fi
}

# ---- 参数解析 ----
ACTION="install"
# 可选元数据：入网时一并上报（也可以走 ZHIWEI_NODE_ALIAS / ZHIWEI_NODE_TAGS）
NODE_ALIAS="${ZHIWEI_NODE_ALIAS:-}"
NODE_TAGS="${ZHIWEI_NODE_TAGS:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --alias)
      [ $# -ge 2 ] || die "--alias 后面要跟一个值"
      NODE_ALIAS="$2"; shift 2 ;;
    --alias=*) NODE_ALIAS="${1#*=}"; shift ;;
    --tags)
      [ $# -ge 2 ] || die "--tags 后面要跟一个值"
      NODE_TAGS="$2"; shift 2 ;;
    --tags=*) NODE_TAGS="${1#*=}"; shift ;;
    --uninstall) ACTION="uninstall"; shift ;;
    # `bash -s -- --alias x` 时 bash 自己吃掉一个 `--`，但直连 `sh install-node.sh -- --alias x`
    # 会把它留给我们，跳过即可
    --) shift ;;
    -h|--help)   usage; exit 0 ;;
    *) die "未知参数: $1（用 --help 看用法）" ;;
  esac
done

# ---- root 检查（必须在所有写盘操作之前）----
if [ "$(id -u)" -ne 0 ]; then
  die "必须 root 运行，请用 sudo: curl ... | sudo bash -s"
fi

# ---- 上一次安装指向哪台 monitor（必须在覆盖 ${ENV_FILE} 之前读）----
# 见「旧节点身份」一节的用途；identity_state 记本次是否沿用了旧身份，
# 决定最后能不能说「节点已入网」（沿用旧身份时 node.id 本来就在，证明不了什么）。
prev_monitor_url=""
identity_state="none"
if [ -f "${ENV_FILE}" ]; then
  prev_monitor_url="$(sed -n 's/^ZHIWEI_MONITOR_URL=//p' "${ENV_FILE}" | tail -n1)"
fi

# ---- 平台检测 ----
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)
    init="systemd"
    case "$arch" in
      x86_64|amd64)   target="x86_64-unknown-linux-musl" ;;
      aarch64|arm64)  target="aarch64-unknown-linux-musl" ;;
      *) die "暂不支持的架构: ${arch}（Linux 目前有 x86_64 / aarch64）" ;;
    esac
    ;;
  Darwin)
    init="launchd"
    case "$arch" in
      x86_64)         target="x86_64-apple-darwin" ;;
      arm64|aarch64)  target="aarch64-apple-darwin" ;;
      *) die "暂不支持的架构: ${arch}（macOS 目前有 arm64 / x86_64）" ;;
    esac
    ;;
  *)
    die "暂不支持的系统: ${os}（目前有 Linux / macOS；Windows 请用 WSL）"
    ;;
esac

# 服务配置落盘位置随 init 系统走；用 LaunchDaemon（不是 LaunchAgent）是因为
# 本脚本本来就要求 root，而 daemon 不依赖「有人登录桌面」也开机自启。
if [ "$init" = "launchd" ]; then
  SERVICE_FILE="${ZHIWEI_SERVICE_FILE:-/Library/LaunchDaemons/${SERVICE_LABEL}.plist}"
  LOG_FILE="${ZHIWEI_LOG_FILE:-/var/log/${SERVICE_NAME}.log}"
else
  SERVICE_FILE="${ZHIWEI_SERVICE_FILE:-/etc/systemd/system/${SERVICE_NAME}.service}"
  LOG_FILE=""
fi

# ---- 工具检查 ----
have curl || die "需要 curl"
have tar  || die "需要 tar"
if [ "$init" = "systemd" ]; then
  have systemctl || die "需要 systemctl（systemd）"
  [ -d /run/systemd/system ] || die "/run/systemd/system 不存在，systemd 未运行"
else
  have launchctl || die "需要 launchctl（macOS launchd）"
fi

# 最近的服务日志：systemd 走 journald，launchd 走 plist 里的 StandardErrorPath
recent_log() {
  if [ "$init" = "systemd" ]; then
    journalctl -u "${SERVICE_NAME}" --since '-2min' --no-pager 2>/dev/null
  else
    tail -n 200 "${LOG_FILE}" 2>/dev/null
  fi
}

# ============================================================
# 卸载
# ============================================================
if [ "$ACTION" = "uninstall" ]; then
  log "卸载 ${SERVICE_NAME}（${init}）"
  if [ "$init" = "systemd" ]; then
    systemctl disable --now "${SERVICE_NAME}" 2>/dev/null || true
    systemctl daemon-reload 2>/dev/null || true
  else
    # LaunchDaemon 的 domain 是 system；bootout 支持「plist 路径」和「domain/label」
    # 两种目标写法，老版本 macOS 只有 unload，逐级回退。
    launchctl bootout system "${SERVICE_FILE}" 2>/dev/null \
      || launchctl bootout "system/${SERVICE_LABEL}" 2>/dev/null \
      || launchctl unload "${SERVICE_FILE}" 2>/dev/null \
      || true
  fi
  rm -f "${INSTALL_DIR}/${BIN_NAME}" "${ENV_FILE}" "${SERVICE_FILE}"
  log "已卸载，但保留 ${STATE_DIR} 数据目录（里头是 signing.key / node.id，重装不丢身份）"
  log "若要连数据一起清，请手动: rm -rf ${STATE_DIR}"
  exit 0
fi

# ============================================================
# 安装前置：版本与必填配置
# ============================================================
if [ -z "${ZHIWEI_MONITOR_URL:-}" ]; then
  die "ZHIWEI_MONITOR_URL 未设置（enroll 命令会自动注入）"
fi
if [ -z "${ZHIWEI_BOOTSTRAP_TOKEN:-}" ]; then
  die "ZHIWEI_BOOTSTRAP_TOKEN 未设置（enroll 命令会自动注入）"
fi

log "平台 ${target} / 版本 ${VERSION:-latest}"

# ============================================================
# 下载
# 国内服务器 / 隔离网络可设 ZHIWEI_BASE_URL 走自建镜像；
# 不设就走 GitHub Releases latest。
# ============================================================
asset="zhiwei-${target}.tar.gz"
if [ -n "${ZHIWEI_BASE_URL:-}" ]; then
  if [ -n "${VERSION}" ]; then
    url="${ZHIWEI_BASE_URL%/}/v${VERSION#v}/${asset}"
  else
    url="${ZHIWEI_BASE_URL%/}/latest/${asset}"
  fi
else
  url="https://github.com/${REPO}/releases/latest/download/${asset}"
fi
tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t zhiwei)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

log "下载 ${url}"
if ! curl -fSL -o "${tmpdir}/${asset}" "$url"; then
  die "下载失败: ${url}。可能原因:release 还没产出该平台 / 资产命名不匹配 / 仓库私有（需要 GITHUB_TOKEN）"
fi

# SHA256 校验（若有 .sha256）
if curl -fSL -o "${tmpdir}/${asset}.sha256" "${url}.sha256" 2>/dev/null; then
  expected="$(awk '{print $1}' "${tmpdir}/${asset}.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${tmpdir}/${asset}" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "${tmpdir}/${asset}" | awk '{print $1}')"
  else
    actual=""
  fi
  if [ -n "$actual" ] && [ "$expected" != "$actual" ]; then
    die "SHA256 不匹配（期望 ${expected}，实际 ${actual}）"
  fi
  log "SHA256 校验 ok"
fi

# ============================================================
# 解压 + 安装二进制（幂等：版本比较后覆盖）
# ============================================================
tar -xzf "${tmpdir}/${asset}" -C "$tmpdir"
[ -f "${tmpdir}/${BIN_NAME}" ] || die "包内没有 ${BIN_NAME}，解包结果: $(ls "$tmpdir")"
pkg_version="$(cat "${tmpdir}/VERSION" 2>/dev/null || echo "${VERSION:-unknown}")"

mkdir -p "$INSTALL_DIR"
if [ -x "${INSTALL_DIR}/${BIN_NAME}" ]; then
  # 已装：读版本（假设 binary --version 最后一行最后字段是版本号；不识别则当作 ? 强制覆盖）
  #
  # `|| true` 是必需的：--version 不是合法参数时 clap 以 2 退出，而 `set -o pipefail`
  # 让这个管道整体失败，赋值语句一失败 `set -e` 就把整个脚本终止在这里——偏偏
  # `2>/dev/null` 把唯一的报错也吞了，于是只有最前面几行日志、后面 env 文件 /
  # unit / 服务全都没写，看起来却像装成功了（2026-09-22 就是这么踩的）。
  installed_ver="$("${INSTALL_DIR}/${BIN_NAME}" --version 2>/dev/null \
    | tail -n1 | awk '{print $NF}' || true)"
  if [ -n "$installed_ver" ] && [ "$installed_ver" = "$pkg_version" ]; then
    log "${BIN_NAME} ${pkg_version} 已装，跳过覆盖"
  else
    install_file 0755 "${tmpdir}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
    log "覆盖 ${BIN_NAME}: ${installed_ver:-?} -> ${pkg_version}"
  fi
else
  install_file 0755 "${tmpdir}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
  log "安装 ${BIN_NAME} ${pkg_version} -> ${INSTALL_DIR}/${BIN_NAME}"
fi

# ============================================================
# 写 env 文件（0600，仅 root 可读）
# ============================================================
# 父目录先备好：/etc 与 /etc/systemd/system 本来就在，但路径可被环境变量改掉；
# launchd 还会往 LOG_FILE 写 stdout/stderr，日志目录不存在它起不来。
mkdir -p "$(dirname "${ENV_FILE}")" "$(dirname "${SERVICE_FILE}")"
if [ -n "${LOG_FILE}" ]; then
  mkdir -p "$(dirname "${LOG_FILE}")"
fi
log "写 ${ENV_FILE}（mode 0600）"
# env 文件既被 systemd 的 EnvironmentFile 读，也被 macOS 的 `set -a; . file` 读；
# 先去会破坏这两种解析的字符，再整体加双引号，两边都能原样取回。
env_quote() { printf '"%s"' "$(printf '%s' "$1" | sed 's/["\\]//g')"; }
{
  echo "# ZhiWei 节点配置（由 install-node.sh 生成，请勿手工编辑；重跑脚本会覆盖）"
  echo "# ZHIWEI_MONITOR_URL:    monitor 服务器地址（节点要上报到的目标）"
  echo "# ZHIWEI_BOOTSTRAP_TOKEN: 入网令牌（首次启动后可在 monitor 撤销；节点本地有 signing.key，不再需要它）"
  echo "ZHIWEI_MONITOR_URL=${ZHIWEI_MONITOR_URL}"
  echo "ZHIWEI_BOOTSTRAP_TOKEN=${ZHIWEI_BOOTSTRAP_TOKEN}"
  if [ -n "${NODE_ALIAS}" ]; then
    echo "# ZHIWEI_NODE_ALIAS:     入网时上报的别名（只影响本机第一次 enroll）"
    echo "ZHIWEI_NODE_ALIAS=$(env_quote "${NODE_ALIAS}")"
  fi
  if [ -n "${NODE_TAGS}" ]; then
    echo "# ZHIWEI_NODE_TAGS:      入网时上报的标签（只影响本机第一次 enroll）"
    echo "ZHIWEI_NODE_TAGS=$(env_quote "${NODE_TAGS}")"
  fi
} > "${tmpdir}/node.env"
install_file 0600 "${tmpdir}/node.env" "${ENV_FILE}"

# ============================================================
# 写服务配置
# systemd：注释行必须独立行 —— 不能放在 ExecStart= 命令行行尾，
# 否则 systemd 会把 # 当成参数传给 zhiwei-node，触发 crash loop
# （见 7a90a8b「systemd unit ExecStart 行内 # 注释挪到独立行」）。
# launchd：plist 是 0644，**不能内嵌 bootstrap token**，所以让 /bin/sh 先
# source 0600 的 env 文件再 exec 二进制（等价 systemd 的 EnvironmentFile）。
# ============================================================
log "写 ${SERVICE_FILE}"
if [ "$init" = "systemd" ]; then
  tmp_service="${tmpdir}/zhiwei-node.service"
  cat > "$tmp_service" <<EOF
[Unit]
Description=ZhiWei node agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
# 上报频率与 monitor URL / bootstrap token 走 EnvironmentFile，不再写在 ExecStart 命令行里
EnvironmentFile=${ENV_FILE}
ExecStart=${INSTALL_DIR}/${BIN_NAME} --state-dir ${STATE_DIR}
Restart=on-failure
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=15
User=root

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
else
  tmp_service="${tmpdir}/${SERVICE_LABEL}.plist"
  cat > "$tmp_service" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>set -a; . ${ENV_FILE}; exec ${INSTALL_DIR}/${BIN_NAME} --state-dir ${STATE_DIR}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key><false/>
  </dict>
  <key>StandardOutPath</key><string>${LOG_FILE}</string>
  <key>StandardErrorPath</key><string>${LOG_FILE}</string>
  <key>ProcessType</key><string>Background</string>
</dict>
</plist>
EOF
  # 老 install-node-service.sh 用同一个 label 装 LaunchAgent（gui 域），
  # 跟这个 LaunchDaemon（system 域）互不干扰地各跑一个节点——控制台里会多一台。
  for d in /Users/*/Library/LaunchAgents /var/root/Library/LaunchAgents; do
    if [ -f "${d}/${SERVICE_LABEL}.plist" ]; then
      log "! 发现旧的 LaunchAgent ${d}/${SERVICE_LABEL}.plist（install-node-service.sh 装的）——"
      log "  它和本 daemon 会各跑一个节点，控制台里会看到两台；建议先卸掉它"
    fi
  done
fi

# 幂等：配置内容比较，变了才覆盖 + reload
service_changed=0
if [ -f "${SERVICE_FILE}" ] && diff -q "${SERVICE_FILE}" "$tmp_service" >/dev/null 2>&1; then
  log "服务配置内容未变，跳过覆盖"
else
  install_file 0644 "$tmp_service" "${SERVICE_FILE}"
  service_changed=1
  log "更新 ${SERVICE_FILE}"
fi

# ============================================================
# 旧节点身份：节点只要看到 <state>/node.id 存在就认为「已入网」，不再 enroll。
# 而一个 node_id 只对签发它的那台 monitor 有效——换了 monitor（或对端数据被
# 重置）却留着旧身份，节点会一直 401「节点签名校验失败」，控制台里永远看不到
# 这台机器，本脚本却一路报成功（2026-09-22 踩过）。目标变了就清掉，让它重新入网。
# ============================================================
# systemd 那份有 StateDirectory=zhiwei-node 会自动建目录，launchd 没有，自己建
mkdir -p "${STATE_DIR}"
if [ -f "${STATE_DIR}/node.id" ]; then
  if [ -n "$prev_monitor_url" ] && [ "$prev_monitor_url" != "$ZHIWEI_MONITOR_URL" ]; then
    log "monitor 由 ${prev_monitor_url} 换成 ${ZHIWEI_MONITOR_URL}，清掉旧节点身份以便重新入网"
    rm -f "${STATE_DIR}/node.id" "${STATE_DIR}/signing.key" \
          "${STATE_DIR}/ops.pub" "${STATE_DIR}/ca.crt.pem"
    identity_state="fresh"
  else
    log "本机已有节点身份（保留 ${STATE_DIR}/node.id）"
    log "  若控制台里看不到它，多半是这个身份不属于 ${ZHIWEI_MONITOR_URL}："
    log "  rm -rf ${STATE_DIR} 后重跑本脚本即可重新入网"
    if [ -n "${NODE_ALIAS}${NODE_TAGS}" ]; then
      log "! --alias / --tags 只在第一次入网时上报；这台已经有身份，本次不会推送这些改动"
      log "  想改它们请在控制台里改（节点页面 → 编辑），或先清掉身份重新入网"
    fi
    identity_state="kept"
  fi
fi

# ============================================================
# 启动 / 重启
# ============================================================
if [ "$init" = "systemd" ]; then
  systemctl daemon-reload
  if [ "$service_changed" = "1" ]; then
    systemctl enable --now "${SERVICE_NAME}"
    log "daemon-reload + enable --now ${SERVICE_NAME}"
  else
    # unit 没变，但 env 可能变了（重跑脚本带新 token/URL）。统一 restart 拉新配置。
    # 首次安装没 unit 文件但 service 也未运行的情况，enable --now 兜底。
    if systemctl is-active --quiet "${SERVICE_NAME}" 2>/dev/null; then
      systemctl restart "${SERVICE_NAME}"
      log "服务在运行，restart 拉新 env"
    else
      systemctl enable --now "${SERVICE_NAME}"
      log "服务未运行，enable --now"
    fi
  fi
else
  # plist 内容变了（含 token / URL / 路径）必须 bootout + bootstrap 才会按新配置起；
  # 内容没变只是要拉新 env 时 kickstart -k 重启就够。
  if [ "$service_changed" = "1" ]; then
    launchctl bootout system "${SERVICE_FILE}" 2>/dev/null \
      || launchctl bootout "system/${SERVICE_LABEL}" 2>/dev/null \
      || true
    launchctl bootstrap system "${SERVICE_FILE}" 2>/dev/null \
      || launchctl load -w "${SERVICE_FILE}"
    log "bootout + bootstrap ${SERVICE_FILE}（开机自启）"
  elif launchctl print "system/${SERVICE_LABEL}" >/dev/null 2>&1; then
    launchctl kickstart -k "system/${SERVICE_LABEL}"
    log "服务在运行，kickstart -k 拉新 env"
  else
    launchctl bootstrap system "${SERVICE_FILE}" 2>/dev/null \
      || launchctl load -w "${SERVICE_FILE}"
    log "服务未运行，bootstrap"
  fi
fi

# ============================================================
# 确认真的入网了：脚本跑通 ≠ 节点入网。
# 服务没起来、令牌被撤 / 过期、旧身份与当前 monitor 对不上，都会让节点安静地
# 留在控制台外面，而脚本这一路全是「成功」提示（2026-09-22 的教训）。
# 入网成功的凭据是节点自己写下的 <state>/node.id；但沿用旧身份时这个文件本来
# 就在，它证明不了「当前 monitor 认这台机器」——那种情况去看日志里的 401。
# ============================================================
i=0
while [ "$i" -lt 10 ] && [ ! -s "${STATE_DIR}/node.id" ]; do
  sleep 1
  i=$((i + 1))
done

if [ "$identity_state" = "kept" ]; then
  log "✓ 安装完成，沿用本机已有身份 node_id=$(cat "${STATE_DIR}/node.id" 2>/dev/null)，未重新入网"
  if recent_log | grep -q '节点签名校验失败'; then
    log "! 但这个身份不被 ${ZHIWEI_MONITOR_URL} 认可（401 节点签名校验失败）——"
    log "  控制台里看不到这台机器就是这个原因：rm -rf ${STATE_DIR} 后重跑本脚本"
  fi
elif [ -s "${STATE_DIR}/node.id" ]; then
  log "✓ 安装完成，节点已入网 node_id=$(cat "${STATE_DIR}/node.id")"
else
  log "! 安装完成，但节点还没入网（${STATE_DIR}/node.id 未生成）——别当成装好了"
  log "  常见原因: 服务没起来 / 入网令牌失效 / 旧身份与当前 monitor 对不上"
  if [ "$init" = "systemd" ]; then
    log "  查日志:   journalctl -u ${SERVICE_NAME} -n 50 --no-pager"
  else
    log "  查日志:   tail -n 50 ${LOG_FILE}"
  fi
fi
log "  二进制  ${INSTALL_DIR}/${BIN_NAME} ${pkg_version}"
log "  env     ${ENV_FILE}"
log "  unit    ${SERVICE_FILE}"
log "  state   ${STATE_DIR}"
log ""
if [ "$init" = "systemd" ]; then
  log "  查状态:   systemctl status ${SERVICE_NAME}"
  log "  看日志:   journalctl -u ${SERVICE_NAME} -f"
else
  log "  查状态:   sudo launchctl print system/${SERVICE_LABEL}"
  log "  看日志:   tail -f ${LOG_FILE}"
fi
log "  卸载:      curl -sSL <monitor>/install-node.sh | sudo bash -s -- --uninstall"
