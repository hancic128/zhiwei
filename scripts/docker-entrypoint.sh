#!/bin/sh
#
# 容器入口：在**同一个容器里**先起 ops-server（控制平面），再起 monitor（数据平面）。
#
# 为什么必须同容器：托管平台（Render / Northflank）只跑一个 web service，没有
# 第二个进程的位置；而 monitor 的命令通道要连回环上的 ops-server 才能签发命令
# （看容器日志、杀进程、重启主机…）。缺了它，控制台里这些操作只会报
# 「ops-server 不可用：连接失败：Connection refused」。
#
# 两个进程仍然各自独立，**签名私钥只在 zhiwei-ops 进程内**：monitor 只从
# `<data-dir>/ops.pub` 读公钥下发给节点（TOFU），拿不到私钥也就伪造不了命令。
# 这正是 docs/DESIGN.md 里 monitor / ops 分离的约束，不是把它合掉。
#
# 环境变量：
#   ZHIWEI_OPS_DISABLE=1  不起 ops-server（只想跑数据平面的部署）
#   ZHIWEI_OPS_WAIT=<秒>  等 ops.pub 落盘的最长秒数，默认 5
#   ZHIWEI_OPS_BIN / ZHIWEI_MONITOR_BIN  覆盖二进制路径（自建镜像 / 本地测试用）
#
# 用 `sh` 而不是 bash：debian:bookworm-slim 里 bash 不保证存在。
set -u

OPS_BIN="${ZHIWEI_OPS_BIN:-/usr/local/bin/zhiwei-ops}"
MONITOR_BIN="${ZHIWEI_MONITOR_BIN:-/usr/local/bin/zhiwei-monitor}"
DATA_DIR="${ZHIWEI_DATA_DIR:-/var/lib/zhiwei}"

start_ops() {
  if [ "${ZHIWEI_OPS_DISABLE:-0}" = "1" ]; then
    echo "[entrypoint] ZHIWEI_OPS_DISABLE=1：跳过 ops-server，命令通道不可用" >&2
    return
  fi
  if [ ! -x "$OPS_BIN" ]; then
    echo "[entrypoint] 没找到 $OPS_BIN：命令通道不可用" >&2
    return
  fi

  "$OPS_BIN" &
  ops_pid=$!

  # 等公钥落盘再放行 monitor：monitor 启动时读一次并缓存，读不到就会给
  # 新入网的节点下发空公钥，那些节点永远不拉命令。等不到也照常起 monitor
  # ——它缓存为空时会按需再读一次盘（见 routes::ops_public_key）。
  waited=0
  limit="${ZHIWEI_OPS_WAIT:-5}"
  while [ ! -s "$DATA_DIR/ops.pub" ] && [ "$waited" -lt "$limit" ]; do
    kill -0 "$ops_pid" 2>/dev/null || {
      echo "[entrypoint] ops-server 提前退出（pid $ops_pid），继续起 monitor" >&2
      return
    }
    sleep 1
    waited=$((waited + 1))
  done
  if [ -s "$DATA_DIR/ops.pub" ]; then
    echo "[entrypoint] ops-server 就绪（pid $ops_pid，公钥已在 $DATA_DIR/ops.pub）" >&2
  else
    echo "[entrypoint] 等 ops.pub 超时（${limit}s），继续起 monitor" >&2
  fi
}

start_ops
exec "$MONITOR_BIN" "$@"
