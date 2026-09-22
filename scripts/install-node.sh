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
#   2. 写 /etc/zhiwei-node.env（mode 0600，仅 root 可读）
#   3. 写 /etc/systemd/system/zhiwei-node.service 并 enable
#   4. systemctl daemon-reload + enable --now 启动并设置开机自启
#   5. 换 monitor 时清掉旧节点身份（身份只对签发它的那台 monitor 有效）
#   6. 等节点写回 node.id，确认真的入网了——「脚本跑通」不等于「节点入网」
#
# 当前只支持 Linux（macOS 暂不实现 launchd 步骤）；必须 root 运行（请用 sudo）。
#
# 参数：
#   --uninstall    停服务 + 删 binary / env / unit（保留 state-dir 数据）
#   -h | --help
#
# 环境变量：
#   ZHIWEI_MONITOR_URL       monitor URL（必填，安装时）
#   ZHIWEI_BOOTSTRAP_TOKEN   入网令牌（必填，安装时）
#   ZHIWEI_VERSION           可选，仅用于日志/兜底；真实版本从包内 VERSION 读。
#                            不设也能装（资产名不含版本号）。
#   ZHIWEI_REPO              仓库 owner/name（默认 hancic128/zhiwei）
#   ZHIWEI_INSTALL_DIR       二进制目录（默认 /usr/local/bin）
#   ZHIWEI_STATE_DIR         状态目录（默认 /var/lib/zhiwei-node）
#   ZHIWEI_ENV_FILE          env 文件路径（默认 /etc/zhiwei-node.env）
#   ZHIWEI_BASE_URL          自建下载源（同 install.sh 语义）。设了就不走 GitHub Releases。
#                            已知可用值：
#                              - https://artifacts.hancic.site/releases/hancic128/zhiwei
#                                （hancic-artifacts 国内直连，由 CI 每次 tag 同步）

set -euo pipefail
umask 077

REPO="${ZHIWEI_REPO:-hancic128/zhiwei}"
VERSION="${ZHIWEI_VERSION:-}"
INSTALL_DIR="${ZHIWEI_INSTALL_DIR:-/usr/local/bin}"
STATE_DIR="${ZHIWEI_STATE_DIR:-/var/lib/zhiwei-node}"
ENV_FILE="${ZHIWEI_ENV_FILE:-/etc/zhiwei-node.env}"
BIN_NAME="zhiwei-node"
SERVICE_NAME="zhiwei-node"
UNIT_FILE="/etc/systemd/system/${SERVICE_NAME}.service"

log() { printf '[zhiwei-install] %s\n' "$*" >&2; }
die() { printf '[zhiwei-install] 错误: %s\n' "$*" >&2; exit 1; }

usage() { sed -n '2,35p' "$0" | sed 's/^# \{0,1\}//'; }

# ---- 参数解析 ----
ACTION="install"
while [ $# -gt 0 ]; do
  case "$1" in
    --uninstall) ACTION="uninstall"; shift ;;
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
if [ "$os" != "Linux" ]; then
  die "暂不支持 ${os}（当前只支持 Linux；macOS 暂未实现 launchd 步骤）"
fi
case "$arch" in
  x86_64|amd64)   target="x86_64-unknown-linux-musl" ;;
  aarch64|arm64)  target="aarch64-unknown-linux-musl" ;;
  *) die "暂不支持的架构: ${arch}（目前有 x86_64 / aarch64）" ;;
esac

# ---- 工具检查 ----
command -v curl >/dev/null 2>&1       || die "需要 curl"
command -v systemctl >/dev/null 2>&1  || die "需要 systemctl（systemd）"
[ -d /run/systemd/system ]           || die "/run/systemd/system 不存在，systemd 未运行"
command -v tar >/dev/null 2>&1       || die "需要 tar"
command -v install >/dev/null 2>&1   || die "需要 install（coreutils）"

# ============================================================
# 卸载
# ============================================================
if [ "$ACTION" = "uninstall" ]; then
  log "卸载 ${SERVICE_NAME}"
  systemctl disable --now "${SERVICE_NAME}" 2>/dev/null || true
  rm -f "${INSTALL_DIR}/${BIN_NAME}" "${ENV_FILE}" "${UNIT_FILE}"
  systemctl daemon-reload 2>/dev/null || true
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
tmpdir="$(mktemp -d)"
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
    install -m 0755 "${tmpdir}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
    log "覆盖 ${BIN_NAME}: ${installed_ver:-?} -> ${pkg_version}"
  fi
else
  install -m 0755 "${tmpdir}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
  log "安装 ${BIN_NAME} ${pkg_version} -> ${INSTALL_DIR}/${BIN_NAME}"
fi

# ============================================================
# 写 env 文件（0600，仅 root 可读）
# ============================================================
log "写 ${ENV_FILE}（mode 0600）"
tmp_env="$(mktemp)"
{
  echo "# ZhiWei 节点配置（由 install-node.sh 生成，请勿手工编辑；重跑脚本会覆盖）"
  echo "# ZHIWEI_MONITOR_URL:    monitor 服务器地址（节点要上报到的目标）"
  echo "# ZHIWEI_BOOTSTRAP_TOKEN: 入网令牌（首次启动后可在 monitor 撤销；节点本地有 signing.key，不再需要它）"
  echo "ZHIWEI_MONITOR_URL=${ZHIWEI_MONITOR_URL}"
  echo "ZHIWEI_BOOTSTRAP_TOKEN=${ZHIWEI_BOOTSTRAP_TOKEN}"
} > "$tmp_env"
install -m 0600 "$tmp_env" "${ENV_FILE}"
rm -f "$tmp_env"

# ============================================================
# 写 systemd unit
# 注意：注释行必须独立行 —— 不能放在 ExecStart= 命令行行尾，
# 否则 systemd 会把 # 当成参数传给 zhiwei-node，触发 crash loop
# （见 7a90a8b「systemd unit ExecStart 行内 # 注释挪到独立行」）
# ============================================================
log "写 ${UNIT_FILE}"
tmp_unit="$(mktemp)"
cat > "$tmp_unit" <<EOF
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

# 幂等：unit 内容比较，变了才覆盖 + reload
unit_changed=0
if [ -f "${UNIT_FILE}" ] && diff -q "${UNIT_FILE}" "$tmp_unit" >/dev/null 2>&1; then
  log "systemd unit 内容未变，跳过覆盖"
else
  install -m 0644 "$tmp_unit" "${UNIT_FILE}"
  unit_changed=1
  log "更新 systemd unit"
fi
rm -f "$tmp_unit"

# ============================================================
# 旧节点身份：节点只要看到 <state>/node.id 存在就认为「已入网」，不再 enroll。
# 而一个 node_id 只对签发它的那台 monitor 有效——换了 monitor（或对端数据被
# 重置）却留着旧身份，节点会一直 401「节点签名校验失败」，控制台里永远看不到
# 这台机器，本脚本却一路报成功（2026-09-22 踩过）。目标变了就清掉，让它重新入网。
# ============================================================
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
    identity_state="kept"
  fi
fi

# ============================================================
# 启动 / 重启
# ============================================================
systemctl daemon-reload
if [ "$unit_changed" = "1" ]; then
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
  if journalctl -u "${SERVICE_NAME}" --since '-2min' --no-pager 2>/dev/null \
     | grep -q '节点签名校验失败'; then
    log "! 但这个身份不被 ${ZHIWEI_MONITOR_URL} 认可（401 节点签名校验失败）——"
    log "  控制台里看不到这台机器就是这个原因：rm -rf ${STATE_DIR} 后重跑本脚本"
  fi
elif [ -s "${STATE_DIR}/node.id" ]; then
  log "✓ 安装完成，节点已入网 node_id=$(cat "${STATE_DIR}/node.id")"
else
  log "! 安装完成，但节点还没入网（${STATE_DIR}/node.id 未生成）——别当成装好了"
  log "  常见原因: 服务没起来 / 入网令牌失效 / 旧身份与当前 monitor 对不上"
  log "  查日志:   journalctl -u ${SERVICE_NAME} -n 50 --no-pager"
fi
log "  二进制  ${INSTALL_DIR}/${BIN_NAME} ${pkg_version}"
log "  env     ${ENV_FILE}"
log "  unit    ${UNIT_FILE}"
log "  state   ${STATE_DIR}"
log ""
log "  查状态:   systemctl status ${SERVICE_NAME}"
log "  看日志:   journalctl -u ${SERVICE_NAME} -f"
log "  卸载:      curl -sSL <monitor>/install-node.sh | sudo bash -s -- --uninstall"
