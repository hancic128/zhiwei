#!/bin/sh
# ZhiWei 二进制安装脚本。
#
#   curl -fsSL https://raw.githubusercontent.com/hancic128/zhiwei/main/scripts/install.sh | sh
#
# 自动识别系统与架构，从 GitHub Release 下载对应预编译包并校验 SHA256。
# 默认装 node-agent（`zhiwei-node`）到 /usr/local/bin。
#
# 环境变量 / 参数（参数优先）：
#   ZHIWEI_BIN          node | monitor | ops   默认 node。装哪个二进制；
#                       monitor 会连带装 zhiwei-ops（命令通道的控制平面）。
#   ZHIWEI_VERSION      latest | 0.0.1      默认 latest。
#   ZHIWEI_INSTALL_DIR  /usr/local/bin      默认 /usr/local/bin；不可写时自动 sudo。
#   ZHIWEI_LIBC         musl | gnu          Linux 专用，默认 musl（静态、不挑 glibc）。
#   ZHIWEI_BASE_URL     自定义下载前缀     给镜像站 / 内网分发用；默认走 GitHub Release。
#                       已知可用值：
#                         - https://artifacts.hancic.site/releases/hancic128/zhiwei
#                           （自建制品仓库，国内直连，由 CI 每次 tag 同步）
#
# 例：
#   ... | sh -s -- --bin monitor --version 0.0.1
#
# 例（走自建制品仓库，国内加速）：
#   ZHIWEI_BASE_URL=https://artifacts.hancic.site/releases/hancic128/zhiwei \
#     ... | sh -s -- --bin node --version latest

set -eu

REPO="${ZHIWEI_REPO:-hancic128/zhiwei}"
BIN="${ZHIWEI_BIN:-node}"
VERSION="${ZHIWEI_VERSION:-latest}"
INSTALL_DIR="${ZHIWEI_INSTALL_DIR:-/usr/local/bin}"
LIBC="${ZHIWEI_LIBC:-musl}"

# ---- 参数解析（覆盖同名环境变量）----
while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN="${2:?--bin 需要一个值}"; shift 2 ;;
    --version) VERSION="${2:?--version 需要一个值}"; shift 2 ;;
    --dir) INSTALL_DIR="${2:?--dir 需要一个值}"; shift 2 ;;
    --libc) LIBC="${2:?--libc 需要一个值}"; shift 2 ;;
    -h|--help)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "未知参数：$1（用 --help 看用法）" >&2; exit 2 ;;
  esac
done

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "  $*" >&2; }

# ---- 识别平台 ----
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)
    case "$arch" in
      x86_64|amd64) cpu="x86_64" ;;
      aarch64|arm64) cpu="aarch64" ;;
      *) die "暂不支持的 Linux 架构：${arch}（目前有 x86_64 / aarch64）" ;;
    esac
    case "$LIBC" in
      musl) target="${cpu}-unknown-linux-musl" ;;
      gnu)  target="${cpu}-unknown-linux-gnu" ;;
      *) die "--libc 只能是 musl 或 gnu（收到 ${LIBC}）" ;;
    esac
    ;;
  Darwin)
    case "$arch" in
      x86_64) target="x86_64-apple-darwin" ;;
      arm64|aarch64) target="aarch64-apple-darwin" ;;
      *) die "暂不支持的 macOS 架构：$arch" ;;
    esac
    ;;
  *)
    die "暂不支持的系统：${os}（目前有 Linux / macOS）。Windows 建议用 WSL，或自行 cargo build。"
    ;;
esac

# ---- 选二进制 ----
# monitor 连带装 ops-server：命令通道（删容器 / 拉日志 / 重启主机）要 ops 签名
# 才能下发，只装 monitor 会一直报「ops-server 不可用：Connection refused」。
case "$BIN" in
  node) binary="zhiwei-node"; binaries="zhiwei-node" ;;
  monitor) binary="zhiwei-monitor"; binaries="zhiwei-monitor zhiwei-ops" ;;
  ops) binary="zhiwei-ops"; binaries="zhiwei-ops" ;;
  *) die "--bin 只能是 node / monitor / ops（收到 ${BIN}）" ;;
esac

# ---- 下载工具 ----
fetch() { # fetch <url> <dest>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q ${GITHUB_TOKEN:+--header="Authorization: Bearer $GITHUB_TOKEN"} -O "$2" "$1"
  else
    die "需要 curl 或 wget"
  fi
}

# 资产名不带版本号，所以 latest 有稳定路径可走。
asset="zhiwei-${target}.tar.gz"
if [ -n "${ZHIWEI_BASE_URL:-}" ]; then
  # 镜像站 / 内网分发：BASE_URL 是仓库根（不含 v<tag>），由本脚本拼
  # latest/ 或 v<VERSION>/ 子路径。两种已知来源布局一致：
  #   - 自建制品仓库 (artifacts.hancic.site/releases/hancic128/zhiwei/)
  #   - 任意按 <owner>/<repo>/v<tag>/<file> 排的镜像（ghcr clone / 内部站点）
  if [ "$VERSION" = "latest" ]; then
    base="${ZHIWEI_BASE_URL%/}/latest"
  else
    base="${ZHIWEI_BASE_URL%/}/v${VERSION#v}"
  fi
elif [ "$VERSION" = "latest" ]; then
  base="https://github.com/${REPO}/releases/latest/download"
else
  base="https://github.com/${REPO}/releases/download/v${VERSION#v}"
fi

tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t zhiwei)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

echo "知微安装器" >&2
note "平台   ${target}"
note "目标   ${binary} ${VERSION}"
note "目录   ${INSTALL_DIR}"

echo "" >&2
echo "下载 ${asset} ..." >&2
if ! fetch "${base}/${asset}" "${tmpdir}/${asset}"; then
  cat >&2 <<EOF

下载失败。可能的原因：
  1. 该 release 还没产出这个平台的包（先在 Actions 里看 release 工作流是否跑过）
  2. 仓库仍是私有的——匿名 curl 拿不到 release 资产。
     做法二选一：
       a) 把仓库设为 public（推荐，开源后安装体验最顺）
       b) 带 token 下载：export GITHUB_TOKEN=<你的 PAT> 后重跑
  3. 版本号写错了（当前请求：${VERSION}）

手动下载地址：
  ${base}/${asset}
EOF
  exit 1
fi

# ---- 校验 SHA256（有就校验，没有就跳过）----
if fetch "${base}/${asset}.sha256" "${tmpdir}/${asset}.sha256" 2>/dev/null; then
  expected="$(awk '{print $1}' "${tmpdir}/${asset}.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${tmpdir}/${asset}" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "${tmpdir}/${asset}" | awk '{print $1}')"
  fi
  if [ "$expected" != "$actual" ]; then
    die "SHA256 不匹配（期望 ${expected}，实际 ${actual}）——包可能损坏或被篡改，已中止"
  fi
  note "校验   SHA256 ok"
else
  note "校验   该 release 没有 .sha256，跳过（建议补上）"
fi

# ---- 解包并安装 ----
tar -xzf "${tmpdir}/${asset}" -C "$tmpdir"
[ -f "${tmpdir}/${binary}" ] || die "包内没有 ${binary}，解包结果：$(ls "$tmpdir")"

mkdir -p "$INSTALL_DIR" 2>/dev/null || true
installed_version="$(cat "${tmpdir}/VERSION" 2>/dev/null || echo "$VERSION")"

# 逐个装（monitor 会带上 zhiwei-ops）。旧 release 包里没有 ops 时给提示、不失败：
# 那种包的命令通道本来就用不了，但要拦的是「装不上」而不是「少一个可选件」。
for one in $binaries; do
  if [ ! -f "${tmpdir}/${one}" ]; then
    note "包内没有 ${one}（大概是旧版本 release），跳过；命令通道可能不可用"
    continue
  fi
  chmod +x "${tmpdir}/${one}"
  if [ -w "$INSTALL_DIR" ]; then
    install -m 0755 "${tmpdir}/${one}" "${INSTALL_DIR}/${one}" 2>/dev/null \
      || mv "${tmpdir}/${one}" "${INSTALL_DIR}/${one}"
  else
    note "sudo    ${INSTALL_DIR} 不可写，用 sudo 安装"
    sudo install -m 0755 "${tmpdir}/${one}" "${INSTALL_DIR}/${one}"
  fi
  echo "" >&2
  echo "已安装 ${one} ${installed_version} → ${INSTALL_DIR}/${one}" >&2
done

case "$BIN" in
  node)
    echo "" >&2
    echo "下一步（首次入网）：" >&2
    echo "  ZHIWEI_MONITOR_URL=https://<你的-monitor> \\" >&2
    echo "  ZHIWEI_BOOTSTRAP_TOKEN=<入网令牌> \\" >&2
    echo "  ${INSTALL_DIR}/zhiwei-node --state-dir /var/lib/zhiwei-node" >&2
    ;;
  monitor)
    echo "" >&2
    echo "下一步（自建部署）：" >&2
    echo "  ${INSTALL_DIR}/zhiwei-monitor --data-dir /var/lib/zhiwei --listen 0.0.0.0:8443" >&2
    echo "  zhiwei-monitor 启动时会自动拉起同目录的 zhiwei-ops（控制平面）；" >&2
    echo "  也可以自己先起：${INSTALL_DIR}/zhiwei-ops --data-dir /var/lib/zhiwei &" >&2
    echo "  只想跑数据平面（不要写操作）就设 ZHIWEI_OPS_DISABLE=1。" >&2
    ;;
esac

# 装到非 PATH 目录时提醒一下
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *) echo "" >&2; note "注意   ${INSTALL_DIR} 不在 PATH 里，用绝对路径调用或自行加进 PATH" ;;
esac
