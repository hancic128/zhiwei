#!/bin/sh
# shellcheck shell=bash
# ZhiWei node onboarding install script.
#
#   # One-shot command: after admin clicks "Generate Onboarding Command" in the UI,
#   # the target machine runs the following line:
#   curl -sSL https://<monitor>/install-node.sh | \
#     ZHIWEI_MONITOR_URL=https://<monitor> \
#     ZHIWEI_BOOTSTRAP_TOKEN=<token> \
#     bash -s
#
#   # Uninstall:
#   curl -sSL https://<monitor>/install-node.sh | sudo bash -s -- --uninstall
#
#   # Clean reinstall (new identity: wipe old binary / env / state files, then onboard again):
#   curl -sSL https://<monitor>/install-node.sh | \
#     ZHIWEI_MONITOR_URL=https://<monitor> \
#     ZHIWEI_BOOTSTRAP_TOKEN=<token> \
#     sudo -E bash -s -- --reinstall
#
#   # Upgrade to latest (in-place binary swap + refresh service config, keep identity and env, no re-onboarding):
#   curl -sSL https://<monitor>/install-node.sh | sudo bash -s -- --upgrade
#
# What it does:
#   1. Download zhiwei-<triple>.tar.gz from GitHub release (contains zhiwei-node + VERSION)
#   2. Write env file (default /etc/zhiwei-node.env, mode 0600, root-only readable)
#   3. Write daemon service config, start it, and enable on boot:
#        Linux  -> /etc/systemd/system/zhiwei-node.service (systemd: enable --now)
#        macOS  -> /Library/LaunchDaemons/com.zhiwei.node.plist (launchd: bootstrap)
#   4. If this run re-onboards (--reinstall / switched monitor / no local identity yet),
#      wipe the previous binary / env / state files before doing a full install, so the
#      new identity is not contaminated by leftover state
#   5. Wait for the node to write back node.id to confirm it really onboarded --
#      "script ran successfully" does NOT mean "node is onboarded"
#
# --upgrade takes the "install new binary + refresh service config" path: it does not delete
# identity, does not rewrite env, and does not re-onboard, so it does not need the monitor
# URL nor the onboarding token.
#
# Platforms: Linux (x86_64 / aarch64, systemd) and macOS (arm64 / x86_64, launchd);
#            must be run as root (please use sudo).
#
# Arguments:
#   --alias <name>   Optional: set alias on onboarding (<=10 chars)
#   --tags <list>    Optional: set tags on onboarding, space / comma / dunhao separator (max 10)
#   --uninstall      Stop service + delete binary / env / unit (keep state-dir data)
#   --reinstall      Clean reinstall and re-onboard: stop service, delete installed binary /
#                    env and node state from state-dir (node.id, signing.key, ops.pub, ca.crt.pem),
#                    then do a full install.
#                    Warning: a new node_id will be assigned; old node records in the console
#                    will not disappear automatically and must be removed manually (history /
#                    alias / tags all stay on the old record). Also a way to recover a missing ops.pub.
#   --upgrade        In-place upgrade: install latest binary + refresh service config
#                    (overwrite only if unit / plist changed), **keeps** state-dir identity
#                    and env file, does not re-onboard, does not need monitor URL / onboarding
#                    token. To pin a version when upgrading: ZHIWEI_VERSION=<version>.
#                    Action arguments (--upgrade / --reinstall / --uninstall) are mutually
#                    exclusive; only one at a time.
#   --no-cache       Ignore local installer cache, re-download (for debugging "is the cache broken")
#   -h | --help
#
# Environment variables:
#   ZHIWEI_MONITOR_URL       Monitor URL (required, install / reinstall only; --upgrade does not need it)
#   ZHIWEI_BOOTSTRAP_TOKEN   Onboarding token (required, install / reinstall only; --upgrade does not need it)
#   ZHIWEI_NODE_ALIAS        Same as --alias
#   ZHIWEI_NODE_TAGS         Same as --tags
#   ZHIWEI_VERSION           Optional, used only for log / fallback; the real version is read from
#                            VERSION in the package. Not setting it is fine (asset name has no version).
#   ZHIWEI_REPO              Repository owner/name (default hancic128/zhiwei)
#   ZHIWEI_INSTALL_DIR       Binary directory (default /usr/local/bin)
#   ZHIWEI_STATE_DIR         State directory (default /var/lib/zhiwei-node)
#   ZHIWEI_ENV_FILE          Env file path (default /etc/zhiwei-node.env)
#   ZHIWEI_SERVICE_FILE      Service config path (default per platform: systemd unit / launchd plist)
#   ZHIWEI_LOG_FILE          macOS only: log file captured by launchd (default /var/log/zhiwei-node.log)
#   ZHIWEI_BASE_URL          Self-hosted download source (same semantics as install.sh). When set,
#                            GitHub Releases is bypassed. Any mirror laid out as
#                            <owner>/<repo>/v<tag>/<file> is supported (ghcr clone / internal
#                            site / self-hosted), e.g.:
#                              - https://mirror.example.com/releases/<owner>/<repo>
#                            When set during install, it is also recorded in the env file; subsequent
#                            `--upgrade` reuses it automatically.
#   ZHIWEI_CACHE_DIR         Installer cache directory (default /var/cache/zhiwei-node). On repeated
#                            onboardings, first fetch the remote .sha256 (a few hundred bytes) and
#                            compare with the cache: identical -> use it; no remote .sha256 -> never
#                            reuse the cache (prefer one more download over a broken/old package).
#   ZHIWEI_NO_CACHE=1        Same as --no-cache
#
# Alias / tags are reported only on this machine's **first onboarding**: machines that already
# have a node.id will not enroll again, so re-running this script cannot change them -- change
# them in the console, or add --reinstall to re-onboard.

set -euo pipefail
umask 077

REPO="${ZHIWEI_REPO:-hancic128/zhiwei}"
VERSION="${ZHIWEI_VERSION:-}"
INSTALL_DIR="${ZHIWEI_INSTALL_DIR:-/usr/local/bin}"
STATE_DIR="${ZHIWEI_STATE_DIR:-/var/lib/zhiwei-node}"
CACHE_DIR="${ZHIWEI_CACHE_DIR:-/var/cache/zhiwei-node}"
ENV_FILE="${ZHIWEI_ENV_FILE:-/etc/zhiwei-node.env}"
BIN_NAME="zhiwei-node"
SERVICE_NAME="zhiwei-node"
SERVICE_LABEL="com.zhiwei.node"

log() { printf '[zhiwei-install] %s\n' "$*" >&2; }
die() { printf '[zhiwei-install] error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Env file is 0600 root-only; works even on macOS without coreutils (install ships with BSD, but don't rely on it)
install_file() { # install_file <mode> <src> <dest>
  if have install; then install -m "$1" "$2" "$3"; else cp "$2" "$3" && chmod "$1" "$3"; fi
}

# Header comment up to the first blank line (more robust than hardcoded line numbers,
# so adding/removing comments does not require syncing usage).
# When piped, $0 is "bash" and the file is not readable -- degrade to a one-line usage
# instead of letting sed fail and break --help.
usage() {
  if [ -r "$0" ]; then
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
  else
    printf 'Usage: curl -sSL <monitor>/install-node.sh | \\\n  ZHIWEI_MONITOR_URL=<url> ZHIWEI_BOOTSTRAP_TOKEN=<token> bash -s [-- --upgrade|--reinstall|--uninstall]\n'
  fi
}

# ---- Argument parsing ----
ACTION="install"
# Action arguments are mutually exclusive: --upgrade / --reinstall / --uninstall have
# incompatible semantics (one upgrades in place, one re-onboards with new identity, one
# uninstalls). Supplying two at once is almost certainly a slip; silently honoring the last
# one would make people think they "upgraded" when they actually "changed identity" --
# failing fast is the right call.
set_action() {
  if [ "$ACTION" != "install" ] && [ "$ACTION" != "$1" ]; then
    die "--$1 and --$ACTION are mutually exclusive; only one action at a time"
  fi
  ACTION="$1"
}
# Installer cache: enabled by default (disabled via ZHIWEI_NO_CACHE=1 or --no-cache)
USE_CACHE=1
[ -n "${ZHIWEI_NO_CACHE:-}" ] && USE_CACHE=0
# Optional metadata: reported on onboarding (also via ZHIWEI_NODE_ALIAS / ZHIWEI_NODE_TAGS)
NODE_ALIAS="${ZHIWEI_NODE_ALIAS:-}"
NODE_TAGS="${ZHIWEI_NODE_TAGS:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --alias)
      [ $# -ge 2 ] || die "--alias requires a value"
      NODE_ALIAS="$2"; shift 2 ;;
    --alias=*) NODE_ALIAS="${1#*=}"; shift ;;
    --tags)
      [ $# -ge 2 ] || die "--tags requires a value"
      NODE_TAGS="$2"; shift 2 ;;
    --tags=*) NODE_TAGS="${1#*=}"; shift ;;
    --uninstall) set_action uninstall; shift ;;
    --reinstall) set_action reinstall; shift ;;
    --upgrade)   set_action upgrade; shift ;;
    # `bash -s -- --alias x`: bash itself consumes the `--`, but `sh install-node.sh -- --alias x`
    # leaves it for us -- just skip it
    --) shift ;;
    -h|--help)   usage; exit 0 ;;
    *) die "unknown argument: $1 (use --help for usage)" ;;
  esac
done

# ---- Root check (must come before any disk write) ----
if [ "$(id -u)" -ne 0 ]; then
  die "must be run as root, please use sudo: curl ... | sudo bash -s"
fi

# ---- Which monitor did the previous install point to (must read before overwriting ${ENV_FILE}) ----
# See the "Old identity" section for usage; identity_state records whether this run kept the
# old identity, which decides whether we can claim "node onboarded" at the end (when the old
# identity is kept, node.id was already there, so it proves nothing).
prev_monitor_url=""
# The download source used by the previous install (machines in CN install with ZHIWEI_BASE_URL;
# upgrade reuses it so we don't have to remember the address every time). On --upgrade, it is
# the fallback when BASE_URL is not explicitly given.
prev_base_url=""
# Binary version before overwrite (for the final report; --upgrade needs to say "from version X to version Y")
prev_version=""
identity_state="none"
if [ -f "${ENV_FILE}" ]; then
  prev_monitor_url="$(sed -n 's/^ZHIWEI_MONITOR_URL=//p' "${ENV_FILE}" | tail -n1)"
  prev_base_url="$(sed -n 's/^ZHIWEI_BASE_URL=//p' "${ENV_FILE}" | tail -n1)"
fi
# The download source actually used this run: explicit env wins, otherwise reuse what the env file recorded
if [ -z "${ZHIWEI_BASE_URL:-}" ]; then
  ZHIWEI_BASE_URL="${prev_base_url}"
fi

# ---- Platform detection ----
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)
    init="systemd"
    case "$arch" in
      x86_64|amd64)   target="x86_64-unknown-linux-musl" ;;
      aarch64|arm64)  target="aarch64-unknown-linux-musl" ;;
      *) die "unsupported architecture: ${arch} (Linux currently supports x86_64 / aarch64)" ;;
    esac
    ;;
  Darwin)
    init="launchd"
    case "$arch" in
      x86_64)         target="x86_64-apple-darwin" ;;
      arm64|aarch64)  target="aarch64-apple-darwin" ;;
      *) die "unsupported architecture: ${arch} (macOS currently supports arm64 / x86_64)" ;;
    esac
    ;;
  *)
    die "unsupported system: ${os} (currently Linux / macOS; for Windows use WSL)"
    ;;
esac

# Service config lands at a path depending on the init system; using LaunchDaemon (not
# LaunchAgent) because this script already requires root, and a daemon does not depend on
# "someone logged into a desktop" to auto-start.
if [ "$init" = "launchd" ]; then
  SERVICE_FILE="${ZHIWEI_SERVICE_FILE:-/Library/LaunchDaemons/${SERVICE_LABEL}.plist}"
  LOG_FILE="${ZHIWEI_LOG_FILE:-/var/log/${SERVICE_NAME}.log}"
else
  SERVICE_FILE="${ZHIWEI_SERVICE_FILE:-/etc/systemd/system/${SERVICE_NAME}.service}"
  LOG_FILE=""
fi

# ---- Tool checks ----
have curl || die "curl is required"
have tar  || die "tar is required"
if [ "$init" = "systemd" ]; then
  have systemctl || die "systemctl is required (systemd)"
  [ -d /run/systemd/system ] || die "/run/systemd/system does not exist; systemd is not running"
else
  have launchctl || die "launchctl is required (macOS launchd)"
fi

# Recent service logs: systemd -> journald, launchd -> StandardErrorPath in the plist
recent_log() {
  if [ "$init" = "systemd" ]; then
    journalctl -u "${SERVICE_NAME}" --since '-2min' --no-pager 2>/dev/null
  else
    tail -n 200 "${LOG_FILE}" 2>/dev/null
  fi
}

# Stop the daemon (shared by uninstall and clean reinstall). systemd also disables it,
# launchd falls back step by step.
# Must be called before deleting node.id / binary: if the process is still running, it may
# immediately write back node.id after deletion, and the new process sees the file and
# skips enroll, so "reinstall" still yields the old identity.
stop_service() {
  if [ "$init" = "systemd" ]; then
    systemctl disable --now "${SERVICE_NAME}" 2>/dev/null || true
    systemctl daemon-reload 2>/dev/null || true
  else
    # LaunchDaemon's domain is system; bootout accepts both "plist path" and "domain/label",
    # older macOS only has unload -- fall back step by step.
    launchctl bootout system "${SERVICE_FILE}" 2>/dev/null \
      || launchctl bootout "system/${SERVICE_LABEL}" 2>/dev/null \
      || launchctl unload "${SERVICE_FILE}" 2>/dev/null \
      || true
  fi
}

# ============================================================
# Uninstall
# ============================================================
if [ "$ACTION" = "uninstall" ]; then
  log "uninstalling ${SERVICE_NAME} (${init})"
  stop_service
  rm -f "${INSTALL_DIR}/${BIN_NAME}" "${ENV_FILE}" "${SERVICE_FILE}"
  log "uninstalled, but kept ${STATE_DIR} data directory (it holds signing.key / node.id; identity survives reinstall)"
  log "to also wipe the data, run manually: rm -rf ${STATE_DIR}"
  log "  or add --reinstall next time (wipes identity and assigns a new node_id)"
  exit 0
fi

# ============================================================
# Install prerequisites: version and required config
# ============================================================
# --upgrade only swaps the binary + refreshes service config; it does not enroll, so it
# does not need the monitor URL or token; but the machine must have something installed
# already, otherwise "upgrading" is meaningless.
if [ "$ACTION" = "upgrade" ]; then
  if [ ! -f "${ENV_FILE}" ] && [ ! -f "${SERVICE_FILE}" ]; then
    die "${SERVICE_NAME} is not installed on this machine (neither ${ENV_FILE} nor ${SERVICE_FILE} exists) -- --upgrade can only upgrade an already-installed node; run the onboarding command for first install"
  fi
else
  if [ -z "${ZHIWEI_MONITOR_URL:-}" ]; then
    die "ZHIWEI_MONITOR_URL is not set (the enroll command injects it automatically)"
  fi
  if [ -z "${ZHIWEI_BOOTSTRAP_TOKEN:-}" ]; then
    die "ZHIWEI_BOOTSTRAP_TOKEN is not set (the enroll command injects it automatically)"
  fi
fi

log "platform ${target} / version ${VERSION:-latest}"

# ============================================================
# Download (with local cache)
#
# Domestic / air-gapped servers can set ZHIWEI_BASE_URL to use a self-hosted mirror;
# otherwise GitHub Releases latest is used.
#
# The onboarding command is often re-run (fix alias / swap token / debug / switch monitor),
# and pulling the full installer package (10+ MB) every time is pure waste. Cache by target:
#   1. First fetch the few-hundred-byte remote .sha256 (the asset itself is small; it serves
#      as a fingerprint of "what's current on the remote")
#   2. If the local cached package's sha256 matches -> reuse the cache, no download
#   3. If it does not match / no cache / cannot fetch .sha256 -> download honestly, validate,
#      then write back to cache
# If the remote .sha256 cannot be fetched, **never reuse the cache**: without trustworthy
# proof that "it is still current", we prefer one more download over installing an old
# (or corrupted) package.
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

sha256_file() { # sha256_file <path> -> hex digest; outputs empty when neither tool is available
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

remote_sha=""
if curl -fsSL -o "${tmpdir}/${asset}.sha256" "${url}.sha256" 2>/dev/null; then
  remote_sha="$(awk '{print $1}' "${tmpdir}/${asset}.sha256")"
fi

cached_pkg="${CACHE_DIR}/${asset}"
reuse_cache=0
if [ "${USE_CACHE}" = "1" ] && [ -n "${remote_sha}" ] && [ -s "${cached_pkg}" ]; then
  if [ "$(sha256_file "${cached_pkg}")" = "${remote_sha}" ]; then
    reuse_cache=1
  fi
fi

if [ "${reuse_cache}" = "1" ]; then
  log "reusing local cache ${cached_pkg} (sha256 matches remote, skipping download)"
  cp "${cached_pkg}" "${tmpdir}/${asset}"
else
  log "downloading ${url}"
  if ! curl -fSL -o "${tmpdir}/${asset}" "$url"; then
    die "download failed: ${url}. Possible causes: release has not produced this platform yet / asset name does not match / repo is private (needs GITHUB_TOKEN)"
  fi
  # SHA256 validation (when .sha256 is available)
  if [ -n "${remote_sha}" ]; then
    actual="$(sha256_file "${tmpdir}/${asset}")"
    if [ -n "${actual}" ] && [ "${remote_sha}" != "${actual}" ]; then
      die "SHA256 mismatch (expected ${remote_sha}, got ${actual})"
    fi
    log "SHA256 verification ok"
  else
    log "! remote has no .sha256, skipping verification (recommended to add one)"
  fi
  # Only write to cache after verification passes -- cache must never contain unverified / partial packages
  if mkdir -p "${CACHE_DIR}" 2>/dev/null && [ -w "${CACHE_DIR}" ]; then
    cp "${tmpdir}/${asset}" "${cached_pkg}" 2>/dev/null \
      && log "cached installer package ${cached_pkg}" || true
  fi
fi

# ============================================================
# Extract + install binary (idempotent: overwrite only after version compare)
# ============================================================
tar -xzf "${tmpdir}/${asset}" -C "$tmpdir"
[ -f "${tmpdir}/${BIN_NAME}" ] || die "package does not contain ${BIN_NAME}, extract result: $(ls "$tmpdir")"
pkg_version="$(cat "${tmpdir}/VERSION" 2>/dev/null || echo "${VERSION:-unknown}")"

# ============================================================
# Reinstall cleanup: whenever this run is about to re-onboard, wipe the previous
# artifacts cleanly before installing a new one.
#
# Both binary and env follow "install a new one" semantics, but the old logic skipped
# overwriting when the version was identical, leaving the previous install's files;
# what a re-onboard with new identity needs is a clean, reproducible set, not a Frankenstein.
#
# The timing of state-file cleanup matters (tripped on 2026-09-22): as long as the node
# sees <state>/node.id it considers itself "onboarded" and skips enroll; and a node_id
# is only valid for the monitor that issued it. An old identity paired with a new monitor
# makes the node keep getting 401 (node signature verification failed), the console never
# sees this machine, yet the script keeps printing success. So stop the service first
# (so the old process doesn't write back node.id after deletion), then delete.
#
# This runs after extract + validation: on download failure / incomplete package, not
# a single byte has been touched.
#
# --upgrade is the exception: the whole point of upgrading is "keep the identity",
# so this whole cleanup block is skipped.
# ============================================================
mkdir -p "${STATE_DIR}"
wipe_reason=""
if [ "$ACTION" = "upgrade" ]; then
  log "upgrade mode: keep existing identity (${STATE_DIR}) and ${ENV_FILE}, do not re-onboard"
elif [ "$ACTION" = "reinstall" ]; then
  wipe_reason="--reinstall clean reinstall"
elif [ ! -f "${STATE_DIR}/node.id" ]; then
  wipe_reason="this machine has no node identity yet, will re-onboard"
elif [ -n "$prev_monitor_url" ] && [ "$prev_monitor_url" != "$ZHIWEI_MONITOR_URL" ]; then
  wipe_reason="monitor changed from ${prev_monitor_url} to ${ZHIWEI_MONITOR_URL}; old identity is invalid"
fi

if [ -n "$wipe_reason" ]; then
  case "${STATE_DIR}" in
    "/"|"") die "ZHIWEI_STATE_DIR is unsafe (${STATE_DIR}), refusing to clean" ;;
  esac
  log "${wipe_reason}: wiping previous binary / env / node state"
  # Record "there was something before": a brand-new machine's first install should
  # not be labeled "clean reinstall"
  had_previous=0
  for f in "${INSTALL_DIR}/${BIN_NAME}" "${ENV_FILE}" \
           "${STATE_DIR}/node.id" "${STATE_DIR}/signing.key" \
           "${STATE_DIR}/ops.pub" "${STATE_DIR}/ca.crt.pem"; do
    if [ -e "$f" ]; then had_previous=1; fi
  done
  stop_service
  rm -f "${INSTALL_DIR}/${BIN_NAME}" "${ENV_FILE}"
  # The node only writes these 4 in state-dir (see node-agent/src/main.rs). Other files
  # are not ours -- listing them preserves them; cleanup does not mean deleting whatever
  # else lives in the same directory.
  rm -f "${STATE_DIR}/node.id" "${STATE_DIR}/signing.key" \
        "${STATE_DIR}/ops.pub" "${STATE_DIR}/ca.crt.pem"
  leftovers="$(ls -A "${STATE_DIR}" 2>/dev/null || true)"
  if [ -n "$leftovers" ]; then
    log "! ${STATE_DIR} still contains non-node files, preserved: $(printf '%s' "$leftovers" | tr '\n' ' ')"
  fi
  if [ "$had_previous" = "1" ] || [ "$ACTION" = "reinstall" ]; then
    identity_state="fresh"
  fi
fi

mkdir -p "$INSTALL_DIR"
if [ -x "${INSTALL_DIR}/${BIN_NAME}" ]; then
  # Already installed: read the version (assume the last field on the last line of
  # binary --version is the version; if unrecognizable, treat as ? and force overwrite)
  #
  # `|| true` must stay: binaries before v0.1.1 don't enable clap's version, so
  # `--version` exits with 2, and `set -o pipefail` makes this whole pipeline fail,
  # the assignment failing makes `set -e` terminate the script right here -- yet
  # `2>/dev/null` swallows the only error message, so only the first few log lines
  # show up, env file / unit / service are never written, yet it appears to have
  # installed successfully (this is exactly how 2026-09-22 tripped).
  installed_ver="$("${INSTALL_DIR}/${BIN_NAME}" --version 2>/dev/null \
    | tail -n1 | awk '{print $NF}' || true)"
  # For the final report (--upgrade needs to say "from version X to version Y")
  prev_version="$installed_ver"
  if [ -n "$installed_ver" ] && [ "$installed_ver" = "$pkg_version" ]; then
    log "${BIN_NAME} ${pkg_version} already installed, skipping overwrite"
  else
    install_file 0755 "${tmpdir}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
    log "overwriting ${BIN_NAME}: ${installed_ver:-?} -> ${pkg_version}"
  fi
else
  install_file 0755 "${tmpdir}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
  log "installed ${BIN_NAME} ${pkg_version} -> ${INSTALL_DIR}/${BIN_NAME}"
fi

# ============================================================
# Write env file (0600, root-only readable)
# ============================================================
# Parent directories first: /etc and /etc/systemd/system already exist, but the path
# can be overridden via env vars; launchd also writes stdout/stderr to LOG_FILE,
# and if the log directory does not exist it won't start.
mkdir -p "$(dirname "${ENV_FILE}")" "$(dirname "${SERVICE_FILE}")"
if [ -n "${LOG_FILE}" ]; then
  mkdir -p "$(dirname "${LOG_FILE}")"
fi
# Upgrade does not touch config: monitor URL / token / alias in the env file stay as-is
# (so an upgrade does not silently swap the token for a freshly-issued one).
if [ "$ACTION" = "upgrade" ]; then
  log "keeping ${ENV_FILE} unchanged (upgrade does not change config)"
else
  log "writing ${ENV_FILE} (mode 0600)"
  # The env file is read by both systemd's EnvironmentFile and macOS's `set -a; . file`;
  # strip characters that break either parser, then double-quote the whole thing, so
  # both can round-trip it as-is.
  env_quote() { printf '"%s"' "$(printf '%s' "$1" | sed 's/["\\]//g')"; }
  {
    echo "# ZhiWei node config (generated by install-node.sh, do not edit by hand; rerunning the script will overwrite)"
    echo "# ZHIWEI_MONITOR_URL:    monitor server address (the target the node reports to)"
    echo "# ZHIWEI_BOOTSTRAP_TOKEN: onboarding token (can be revoked on the monitor after first start; node has signing.key locally, does not need it after)"
    echo "ZHIWEI_MONITOR_URL=${ZHIWEI_MONITOR_URL}"
    echo "ZHIWEI_BOOTSTRAP_TOKEN=${ZHIWEI_BOOTSTRAP_TOKEN}"
    if [ -n "${ZHIWEI_BASE_URL:-}" ]; then
      echo "# ZHIWEI_BASE_URL:       installer package download source (self-hosted mirror / artifact store); --upgrade reuses this line"
      echo "ZHIWEI_BASE_URL=${ZHIWEI_BASE_URL}"
    fi
    if [ -n "${NODE_ALIAS}" ]; then
      echo "# ZHIWEI_NODE_ALIAS:     alias reported on onboarding (only affects this machine's first enroll)"
      echo "ZHIWEI_NODE_ALIAS=$(env_quote "${NODE_ALIAS}")"
    fi
    if [ -n "${NODE_TAGS}" ]; then
      echo "# ZHIWEI_NODE_TAGS:      tags reported on onboarding (only affects this machine's first enroll)"
      echo "ZHIWEI_NODE_TAGS=$(env_quote "${NODE_TAGS}")"
    fi
  } > "${tmpdir}/node.env"
  install_file 0600 "${tmpdir}/node.env" "${ENV_FILE}"
fi

# ============================================================
# Write service config
# systemd: comment lines must be on their own line -- they cannot go at the end of
# the ExecStart= line, otherwise systemd treats # as an argument passed to zhiwei-node
# and triggers a crash loop (see 7a90a8b "move systemd unit ExecStart inline # comment
# to its own line").
# launchd: the plist is 0644 and **must not embed the bootstrap token**, so let
# /bin/sh source the 0600 env file first, then exec the binary (equivalent to systemd's
# EnvironmentFile).
# ============================================================
log "writing ${SERVICE_FILE}"
if [ "$init" = "systemd" ]; then
  tmp_service="${tmpdir}/zhiwei-node.service"
  cat > "$tmp_service" <<EOF
[Unit]
Description=ZhiWei node agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
# Report frequency and monitor URL / bootstrap token go through EnvironmentFile, no longer written in the ExecStart command line
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
# read-only instead of true: the default certificate scan location includes /root/nginx-certs;
# ProtectHome=true would mount /root as an empty directory -> those certs are never found (and no error).
ProtectHome=read-only
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
  # The old install-node-service.sh used the same label to install a LaunchAgent (gui domain),
  # coexisting with this LaunchDaemon (system domain) running one node each -- the console
  # will show two. The old LaunchAgent takes over the same binary, conflicts.
  for d in /Users/*/Library/LaunchAgents /var/root/Library/LaunchAgents; do
    if [ -f "${d}/${SERVICE_LABEL}.plist" ]; then
      log "! found old LaunchAgent ${d}/${SERVICE_LABEL}.plist (installed by install-node-service.sh) --"
      log "  it and this daemon each run a node; the console will show two; recommend uninstalling it first"
    fi
  done
fi

# Idempotent: compare config contents, only overwrite + reload when changed
service_changed=0
if [ -f "${SERVICE_FILE}" ] && diff -q "${SERVICE_FILE}" "$tmp_service" >/dev/null 2>&1; then
  log "service config content unchanged, skipping overwrite"
else
  install_file 0644 "$tmp_service" "${SERVICE_FILE}"
  service_changed=1
  log "updated ${SERVICE_FILE}"
fi

# ============================================================
# Machines keeping old identity (node.id still present, and "reinstall cleanup" above
# decided no wipe): just report here, and note it might not match the current monitor
# -- that's why the node quietly stays out of the console.
# --upgrade is excluded: it does not touch identity by design; saying so would only
# clutter the upgrade output.
# ============================================================
if [ "$ACTION" != "upgrade" ] && [ -f "${STATE_DIR}/node.id" ]; then
  log "this machine already has node identity (kept ${STATE_DIR}/node.id)"
  log "  if you don't see it in the console, this identity likely does not belong to ${ZHIWEI_MONITOR_URL}:"
  log "  add --reinstall and rerun this script (wipes identity + binary + env, then re-onboards)"
  if [ -n "${NODE_ALIAS}${NODE_TAGS}" ]; then
    log "! --alias / --tags are only reported on the first onboarding; this machine already has an identity, so this run will not push these changes"
    log "  to change them, edit in the console (node page -> edit), or add --reinstall to re-onboard"
  fi
  identity_state="kept"
fi

# ============================================================
# Start / restart
# ============================================================
if [ "$init" = "systemd" ]; then
  systemctl daemon-reload
  if [ "$service_changed" = "1" ]; then
    systemctl enable --now "${SERVICE_NAME}"
    log "daemon-reload + enable --now ${SERVICE_NAME}"
  else
    # unit unchanged, but env may have changed (rerun with new token/URL). Restart
    # unconditionally to pick up the new config.
    # First install with no unit file and service not running: enable --now covers it.
    if systemctl is-active --quiet "${SERVICE_NAME}" 2>/dev/null; then
      systemctl restart "${SERVICE_NAME}"
      log "service running, restart to pick up new env"
    else
      systemctl enable --now "${SERVICE_NAME}"
      log "service not running, enable --now"
    fi
  fi
else
  # plist content changed (including token / URL / path): bootout + bootstrap is required
  # to start with the new config; when content is unchanged and we only need to pick up
  # the new env, kickstart -k is enough to restart.
  if [ "$service_changed" = "1" ]; then
    launchctl bootout system "${SERVICE_FILE}" 2>/dev/null \
      || launchctl bootout "system/${SERVICE_LABEL}" 2>/dev/null \
      || true
    launchctl bootstrap system "${SERVICE_FILE}" 2>/dev/null \
      || launchctl load -w "${SERVICE_FILE}"
    log "bootout + bootstrap ${SERVICE_FILE} (auto-start on boot)"
  elif launchctl print "system/${SERVICE_LABEL}" >/dev/null 2>&1; then
    launchctl kickstart -k "system/${SERVICE_LABEL}"
    log "service running, kickstart -k to pick up new env"
  else
    launchctl bootstrap system "${SERVICE_FILE}" 2>/dev/null \
      || launchctl load -w "${SERVICE_FILE}"
    log "service not running, bootstrap"
  fi
fi

# ============================================================
# Confirm real onboarding: "script ran successfully" != "node is onboarded".
# Service not started, token revoked / expired, or old identity not matching the current
# monitor -- any of these can leave the node quietly out of the console while this script
# prints "success" all the way (2026-09-22 lesson).
# Proof of successful onboarding is the node itself writing <state>/node.id; but when the
# old identity is kept, that file was already there, so it does not prove "the current
# monitor recognizes this machine" -- in that case, check the logs for 401.
# ============================================================
i=0
while [ "$i" -lt 10 ] && [ ! -s "${STATE_DIR}/node.id" ]; do
  sleep 1
  i=$((i + 1))
done

if [ "$ACTION" = "upgrade" ]; then
  log "✓ upgrade complete ${prev_version:-?} -> ${pkg_version} (identity, env, and node records all kept)"
  if [ ! -s "${STATE_DIR}/node.id" ]; then
    log "! but this machine has no node identity (${STATE_DIR}/node.id does not exist) -- upgrade does not handle onboarding,"
    log "  send the onboarding command again (or --reinstall) to make it re-onboard"
  fi
elif [ "$identity_state" = "kept" ]; then
  log "✓ install complete, kept this machine's existing identity node_id=$(cat "${STATE_DIR}/node.id" 2>/dev/null), did not re-onboard"
  if recent_log | grep -q 'node signature verification failed'; then
    log "! but this identity is not recognized by ${ZHIWEI_MONITOR_URL} (401 node signature verification failed) --"
    log "  this is why the console cannot see this machine: add --reinstall and rerun this script"
  fi
elif [ -s "${STATE_DIR}/node.id" ]; then
  if [ "$identity_state" = "fresh" ]; then
    log "✓ clean reinstall complete, node has re-onboarded node_id=$(cat "${STATE_DIR}/node.id")"
    log "  this is a new identity: the old record in the console is still there; after confirming the new node reports normally, remove it manually"
  else
    log "✓ install complete, node onboarded node_id=$(cat "${STATE_DIR}/node.id")"
  fi
else
  log "! install complete, but node has not onboarded yet (${STATE_DIR}/node.id not generated) -- do not consider it done"
  log "  common causes: service did not start / onboarding token invalid / old identity does not match current monitor"
  if [ "$init" = "systemd" ]; then
    log "  check logs: journalctl -u ${SERVICE_NAME} -n 50 --no-pager"
  else
    log "  check logs: tail -n 50 ${LOG_FILE}"
  fi
fi
log "  binary  ${INSTALL_DIR}/${BIN_NAME} ${pkg_version}"
log "  env     ${ENV_FILE}"
log "  unit    ${SERVICE_FILE}"
log "  state   ${STATE_DIR}"
log "  cache   ${CACHE_DIR}"
log ""
if [ "$init" = "systemd" ]; then
  log "  check status: systemctl status ${SERVICE_NAME}"
  log "  watch logs:   journalctl -u ${SERVICE_NAME} -f"
else
  log "  check status: sudo launchctl print system/${SERVICE_LABEL}"
  log "  watch logs:   tail -f ${LOG_FILE}"
fi
log "  uninstall:     curl -sSL <monitor>/install-node.sh | sudo bash -s -- --uninstall"
log "  upgrade:       curl -sSL <monitor>/install-node.sh | sudo bash -s -- --upgrade"
log "  clean reinstall: same as above, replace the tail with --reinstall (wipes identity / binary / env, then re-onboards)"