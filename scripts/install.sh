#!/bin/sh
# ZhiWei binary installer.
#
#   curl -fsSL https://raw.githubusercontent.com/zhiwei/zhiwei/main/scripts/install.sh | sh
#
# Auto-detects the OS and architecture, downloads the matching prebuilt
# tarball from a GitHub Release, and verifies its SHA256. By default installs
# the node-agent (`zhiwei-node`) to /usr/local/bin.
#
# Environment variables / arguments (arguments win):
#   ZHIWEI_BIN          node | monitor | ops   default node. Which binary to
#                       install; "monitor" also installs zhiwei-ops (the
#                       control plane for the command channel).
#   ZHIWEI_VERSION      latest | 0.0.1       default latest.
#   ZHIWEI_INSTALL_DIR  /usr/local/bin        default /usr/local/bin; auto-sudo
#                                             when not writable.
#   ZHIWEI_LIBC         musl | gnu            Linux-only; default musl (static,
#                                             glibc-independent).
#   ZHIWEI_BASE_URL     custom download prefix for mirrors / air-gapped setups.
#                       Default: GitHub Releases. Any mirror laid out as
#                       <owner>/<repo>/v<tag>/<file> works (ghcr clone,
#                       internal artifact server, self-built). Example:
#                         - https://mirror.example.com/releases/<owner>/<repo>
#
# Examples:
#   ... | sh -s -- --bin monitor --version 0.0.1
#
# Custom artifact mirror (faster installs in restricted regions):
#   ZHIWEI_BASE_URL=https://mirror.example.com/releases/<owner>/<repo> \
#     ... | sh -s -- --bin node --version latest

set -eu

REPO="${ZHIWEI_REPO:-zhiwei/zhiwei}"
BIN="${ZHIWEI_BIN:-node}"
VERSION="${ZHIWEI_VERSION:-latest}"
INSTALL_DIR="${ZHIWEI_INSTALL_DIR:-/usr/local/bin}"
LIBC="${ZHIWEI_LIBC:-musl}"

# ---- Argument parsing (overrides the same-name env vars) ----
while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN="${2:?--bin requires a value}"; shift 2 ;;
    --version) VERSION="${2:?--version requires a value}"; shift 2 ;;
    --dir) INSTALL_DIR="${2:?--dir requires a value}"; shift 2 ;;
    --libc) LIBC="${2:?--libc requires a value}"; shift 2 ;;
    -h|--help)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "unknown argument: $1 (use --help for usage)" >&2; exit 2 ;;
  esac
done

die() { echo "error: $*" >&2; exit 1; }
note() { echo "  $*" >&2; }

# ---- Detect platform ----
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)
    case "$arch" in
      x86_64|amd64) cpu="x86_64" ;;
      aarch64|arm64) cpu="aarch64" ;;
      *) die "unsupported Linux architecture: ${arch} (currently x86_64 / aarch64)" ;;
    esac
    case "$LIBC" in
      musl) target="${cpu}-unknown-linux-musl" ;;
      gnu)  target="${cpu}-unknown-linux-gnu" ;;
      *) die "--libc must be musl or gnu (got ${LIBC})" ;;
    esac
    ;;
  Darwin)
    case "$arch" in
      x86_64) target="x86_64-apple-darwin" ;;
      arm64|aarch64) target="aarch64-apple-darwin" ;;
      *) die "unsupported macOS architecture: $arch" ;;
    esac
    ;;
  *)
    die "unsupported OS: ${os} (currently Linux / macOS). For Windows, use WSL or build with cargo yourself."
    ;;
esac

# ---- Pick the binary ----
# "monitor" also installs ops-server: the command channel (delete containers /
# fetch logs / restart hosts) needs the ops-signed commands to flow. Installing
# monitor alone leaves the console reporting "ops-server unavailable:
# Connection refused" forever.
case "$BIN" in
  node) binary="zhiwei-node"; binaries="zhiwei-node" ;;
  monitor) binary="zhiwei-monitor"; binaries="zhiwei-monitor zhiwei-ops" ;;
  ops) binary="zhiwei-ops"; binaries="zhiwei-ops" ;;
  *) die "--bin must be node / monitor / ops (got ${BIN})" ;;
esac

# ---- Download tool ----
fetch() { # fetch <url> <dest>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q ${GITHUB_TOKEN:+--header="Authorization: Bearer $GITHUB_TOKEN"} -O "$2" "$1"
  else
    die "need curl or wget"
  fi
}

# Asset names do not include the version, so "latest" has a stable path.
asset="zhiwei-${target}.tar.gz"
if [ -n "${ZHIWEI_BASE_URL:-}" ]; then
  # Mirror / air-gapped: BASE_URL is the repo root (no v<tag>); this script
  # appends "latest/" or "v<VERSION>/". Any mirror laid out as
  # <owner>/<repo>/v<tag>/<file> works (ghcr clone / internal / self-built).
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

echo "ZhiWei installer" >&2
note "platform ${target}"
note "target   ${binary} ${VERSION}"
note "dir      ${INSTALL_DIR}"

echo "" >&2
echo "downloading ${asset} ..." >&2
if ! fetch "${base}/${asset}" "${tmpdir}/${asset}"; then
  cat >&2 <<EOF

download failed. Possible causes:
  1. This release did not produce an asset for this platform (check Actions
     for the release workflow status first)
  2. The repository is still private — anonymous curl cannot fetch release
     assets. Pick one:
       a) make the repository public (recommended once open-sourced)
       b) provide a token: export GITHUB_TOKEN=<your PAT> then retry
  3. Wrong version (current request: ${VERSION})

Manual download URL:
  ${base}/${asset}
EOF
  exit 1
fi

# ---- SHA256 verification (verify when present, skip otherwise) ----
if fetch "${base}/${asset}.sha256" "${tmpdir}/${asset}.sha256" 2>/dev/null; then
  expected="$(awk '{print $1}' "${tmpdir}/${asset}.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${tmpdir}/${asset}" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "${tmpdir}/${asset}" | awk '{print $1}')"
  fi
  if [ "$expected" != "$actual" ]; then
    die "SHA256 mismatch (expected ${expected}, got ${actual}) — package may be corrupt or tampered with; aborting"
  fi
  note "verify   SHA256 ok"
else
  note "verify   no .sha256 in this release; skipped (consider adding one)"
fi

# ---- Extract and install ----
tar -xzf "${tmpdir}/${asset}" -C "$tmpdir"
[ -f "${tmpdir}/${binary}" ] || die "package does not contain ${binary}; contents: $(ls "$tmpdir")"

mkdir -p "$INSTALL_DIR" 2>/dev/null || true
installed_version="$(cat "${tmpdir}/VERSION" 2>/dev/null || echo "$VERSION")"

# Install each binary (monitor pulls in zhiwei-ops). Older releases may lack
# ops: warn but do not fail — the command channel will not work, but the
# install itself should still succeed.
for one in $binaries; do
  if [ ! -f "${tmpdir}/${one}" ]; then
    note "package does not contain ${one} (probably an old release); skipping; command channel may be unavailable"
    continue
  fi
  chmod +x "${tmpdir}/${one}"
  if [ -w "$INSTALL_DIR" ]; then
    install -m 0755 "${tmpdir}/${one}" "${INSTALL_DIR}/${one}" 2>/dev/null \
      || mv "${tmpdir}/${one}" "${INSTALL_DIR}/${one}"
  else
    note "sudo     ${INSTALL_DIR} is not writable; installing with sudo"
    sudo install -m 0755 "${tmpdir}/${one}" "${INSTALL_DIR}/${one}"
  fi
  echo "" >&2
  echo "installed ${one} ${installed_version} → ${INSTALL_DIR}/${one}" >&2
done

case "$BIN" in
  node)
    echo "" >&2
    echo "next step (first enrollment):" >&2
    echo "  ZHIWEI_MONITOR_URL=https://<your-monitor> \\" >&2
    echo "  ZHIWEI_BOOTSTRAP_TOKEN=<enrollment token> \\" >&2
    echo "  ${INSTALL_DIR}/zhiwei-node --state-dir /var/lib/zhiwei-node" >&2
    ;;
  monitor)
    echo "" >&2
    echo "next step (self-hosted):" >&2
    echo "  ${INSTALL_DIR}/zhiwei-monitor --data-dir /var/lib/zhiwei --listen 0.0.0.0:8443" >&2
    echo "  zhiwei-monitor starts zhiwei-ops from the same directory automatically (control plane);" >&2
    echo "  or start it yourself first: ${INSTALL_DIR}/zhiwei-ops --data-dir /var/lib/zhiwei &" >&2
    echo "  set ZHIWEI_OPS_DISABLE=1 for data-plane-only deployments (no control actions)." >&2
    ;;
esac

# Reminder when the install dir is not on PATH
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *) echo "" >&2; note "note     ${INSTALL_DIR} is not on PATH; invoke via absolute path or add it" ;;
esac