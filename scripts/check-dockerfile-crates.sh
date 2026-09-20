#!/usr/bin/env bash
#
# 校验 Dockerfile 的「依赖桩层」覆盖了 workspace 的全部成员。
#
# 为什么要这个校验：cargo 解析 workspace 时要求 [workspace] members 里的每个
# 目录都存在。Dockerfile 为了做依赖缓存只逐个 COPY 各 crate 的 Cargo.toml，
# 漏一个就会在 **构建镜像时** 才报错：
#
#   error: failed to load manifest for workspace member `/src/crates/ops-server`
#
# 这类错误本地 cargo build 完全测不出来（本地整棵树都在），只有部署才现形，
# 所以用脚本卡在本地。新增 crate 时改完 Dockerfile 先跑它。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

members="$(awk '
  /^members[[:space:]]*=[[:space:]]*\[/ { in_list = 1; next }
  in_list && /\]/                       { in_list = 0 }
  in_list                               { print }
' Cargo.toml | tr -d ' ",' | grep -v '^$' || true)"

if [ -z "$members" ]; then
  echo "错误：没能从 Cargo.toml 解析出 [workspace] members" >&2
  exit 1
fi

missing_copy=()
missing_stub=()
for m in $members; do
  grep -qE "^COPY[[:space:]]+$m/Cargo.toml" Dockerfile || missing_copy+=("$m")
  grep -qE "$m/src" Dockerfile || missing_stub+=("$m")
done

if [ ${#missing_copy[@]} -gt 0 ] || [ ${#missing_stub[@]} -gt 0 ]; then
  echo "Dockerfile 与 workspace 成员不一致：" >&2
  [ ${#missing_copy[@]} -gt 0 ] && printf '  缺 COPY  %s/Cargo.toml\n' "${missing_copy[@]}" >&2
  [ ${#missing_stub[@]} -gt 0 ] && printf '  缺桩目录 %s/src\n' "${missing_stub[@]}" >&2
  echo "  （漏掉会导致部署时报 failed to load manifest for workspace member）" >&2
  exit 1
fi

echo "OK Dockerfile 覆盖全部 $(echo "$members" | wc -l | tr -d ' ') 个 workspace 成员"
