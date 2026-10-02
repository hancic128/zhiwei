#!/usr/bin/env bash
#
# Verify the Dockerfile's "dependency stub" layer covers all workspace members.
#
# Why this check exists: cargo resolves the workspace and requires every
# [workspace] member directory to exist. The Dockerfile only COPYs each crate's
# Cargo.toml for dependency caching. Missing one causes an error at **image build time**:
#
#   error: failed to load manifest for workspace member `/src/crates/ops-server`
#
# This error is invisible locally (all files exist), only surfaces on deploy.
# Run this after adding a crate and modifying the Dockerfile.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

members="$(awk '
  /^members[[:space:]]*=[[:space:]]*\[/ { in_list = 1; next }
  in_list && /\]/                       { in_list = 0 }
  in_list                               { print }
' Cargo.toml | tr -d ' ",' | grep -v '^$' || true)"

if [ -z "$members" ]; then
  echo "Error: failed to parse [workspace] members from Cargo.toml" >&2
  exit 1
fi

missing_copy=()
missing_stub=()
for m in $members; do
  grep -qE "^COPY[[:space:]]+$m/Cargo.toml" Dockerfile || missing_copy+=("$m")
  grep -qE "$m/src" Dockerfile || missing_stub+=("$m")
done

if [ ${#missing_copy[@]} -gt 0 ] || [ ${#missing_stub[@]} -gt 0 ]; then
  echo "Dockerfile / workspace member mismatch:" >&2
  [ ${#missing_copy[@]} -gt 0 ] && printf '  Missing COPY  %s/Cargo.toml\n' "${missing_copy[@]}" >&2
  [ ${#missing_stub[@]} -gt 0 ] && printf '  Missing stub %s/src\n' "${missing_stub[@]}" >&2
  echo "  (missing stubs cause 'failed to load manifest for workspace member' at deploy)" >&2
  exit 1
fi

echo "OK Dockerfile covers all $(echo "$members" | wc -l | tr -d ' ') workspace members"
