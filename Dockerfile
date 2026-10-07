# syntax=docker/dockerfile:1
#
# The official `rust:` images ship a full build toolchain (buildpack-deps),
# which is everything `ring` and our crates need — so the builder stage runs
# no `apt-get` at all. The runtime stage installs only `curl` (for the compose
# healthcheck) and reuses the CA bundle from the builder.

# ---- builder ----
FROM rust:1.88-bookworm AS builder

WORKDIR /src

# Optional crates.io mirror for slow networks (e.g. mainland China):
#   docker build --build-arg CARGO_MIRROR=https://rsproxy.cn/index/ .
ARG CARGO_MIRROR=""
RUN if [ -n "$CARGO_MIRROR" ]; then \
      printf '[source.crates-io]\nreplace-with = "mirror"\n\n[source.mirror]\nregistry = "sparse+%s"\n' "$CARGO_MIRROR" \
         > /usr/local/cargo/config.toml ; \
    fi

# Layer 1: resolve and compile dependencies against stub sources, so that
# editing application code does not recompile the whole dependency tree.
#
# ⚠️ 这里的 crate 列表必须与根 Cargo.toml 的 [workspace] members 完全一致：
# cargo 解析 workspace 时要求所有成员都存在，漏一个会直接报
# `failed to load manifest for workspace member`（新增 crate 时别忘同步）。
# 有 `scripts/check-dockerfile-crates.sh` 做这个校验，本地改完 Dockerfile 先跑它。
COPY Cargo.toml Cargo.lock ./
COPY proto/ proto/
COPY crates/common/Cargo.toml         crates/common/
COPY crates/proto/Cargo.toml          crates/proto/
COPY crates/storage/Cargo.toml        crates/storage/
COPY crates/monitor-server/Cargo.toml crates/monitor-server/
COPY crates/node-agent/Cargo.toml     crates/node-agent/
COPY crates/ops-server/Cargo.toml     crates/ops-server/
RUN mkdir -p crates/common/src crates/proto/src crates/storage/src \
             crates/monitor-server/src crates/node-agent/src crates/ops-server/src \
    && echo 'fn main() {}' > crates/monitor-server/src/main.rs \
    && echo 'fn main() {}' > crates/node-agent/src/main.rs \
    && echo 'fn main() {}' > crates/ops-server/src/main.rs \
    && : > crates/common/src/lib.rs \
    && : > crates/proto/src/lib.rs \
    && : > crates/storage/src/lib.rs \
    && cargo build --release --bin zhiwei-monitor --bin zhiwei-ops \
    && rm -rf crates/*/src

# Layer 2: real sources.
#
# `touch` is required, not cosmetic: COPY preserves the source files' mtime,
# which can be *older* than the stub files cargo fingerprinted in layer 1 —
# cargo would then consider the crate unchanged and reuse the stub artifacts.
COPY crates/ crates/
# install-node.sh 由 monitor 用 include_str! 编进二进制（单一来源，不在
# assets/ 留副本），构建阶段必须能读到仓库根的这份。
COPY scripts/install-node.sh scripts/install-node.sh
RUN find crates -name '*.rs' -exec touch {} + \
    && cargo build --release --bin zhiwei-monitor --bin zhiwei-ops

# ---- ui ----
# 控制台（React SPA）单独用一个 node 阶段构建：monitor 在 ZHIWEI_UI_DIR
# 存在时把它挂在 /，没有的话部署上去就只有一个 API（根路径 404）。
FROM node:20-bookworm-slim AS ui-builder

WORKDIR /ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci
COPY ui/ ./
RUN npm run build

# ---- runtime ----
FROM debian:bookworm-slim AS runtime

# The CA bundle is reused from the builder rather than installed. `curl` is the
# one package we do install: the compose healthcheck runs
# `curl -f http://127.0.0.1:8443/healthz`, and stock debian:bookworm-slim ships
# neither curl nor wget — without it the healthcheck exits 127 and the container
# is permanently "unhealthy" even while the service serves traffic.
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /src/target/release/zhiwei-monitor /usr/local/bin/zhiwei-monitor
# 控制平面：和 monitor 同一个容器、同一个数据目录，双进程各持其职
# （签名私钥只在 zhiwei-ops 内）。见 scripts/docker-entrypoint.sh。
COPY --from=builder /src/target/release/zhiwei-ops /usr/local/bin/zhiwei-ops
COPY scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
COPY --from=ui-builder /ui/dist /app/ui/dist

RUN chmod +x /usr/local/bin/docker-entrypoint.sh \
    && useradd --create-home --home-dir /var/lib/zhiwei --uid 10001 \
      --shell /usr/sbin/nologin zhiwei \
    && mkdir -p /var/lib/zhiwei \
    && chown -R zhiwei:zhiwei /var/lib/zhiwei

# Persistent state: SQLite DB + local CA + server cert live here. Mount a
# volume at this path — losing the CA forces every node to re-enroll.
# See docs/DEPLOY.md.
ENV ZHIWEI_DATA_DIR=/var/lib/zhiwei
ENV ZHIWEI_UI_DIR=/app/ui/dist
VOLUME ["/var/lib/zhiwei"]

USER zhiwei
# ⚠️ 别给它加协议后缀。Northflank 靠 EXPOSE 自动探测端口，且按协议决定默认可见性：
#   EXPOSE 8443        → HTTP，默认 public，会自动分配 *.code.run 域名
#   EXPOSE 8443/tcp    → TCP，private，**拿不到域名**（TCP 要单独买 L4 负载均衡）
# Render / Railway 不读这行，但这行决定了 Northflank 上开箱能不能访问。
EXPOSE 8443

# entrypoint 负责先拉起 zhiwei-ops 再 exec zhiwei-monitor（同容器双进程）；
# 传进来的参数会原样转给 monitor，`docker run … --plain-http` 依旧可用。
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
