# Deployment Guide

This guide covers deploying ZhiWei's monitor-server to self-hosted servers or
managed platforms (Render / Railway / Northflank). The only difference between
the two: **who terminates TLS** — monitor terminates it itself (default) on
self-hosted; edge terminates it on managed platforms (`--plain-http`).
Node identity relies on Ed25519 request signing, not the transport layer, so
security is equivalent in both cases.

---

## Self-Hosted / On-Premises

### Binary

```sh
cargo build --release --bin zhiwei-monitor --bin zhiwei-ops
./target/release/zhiwei-monitor --data-dir /var/lib/zhiwei --listen 127.0.0.1:8443
```

`zhiwei-monitor` checks `ops_endpoint` (default `http://127.0.0.1:8444/exec`):
if nothing is listening on that port and `zhiwei-ops` exists in the same directory,
the control plane starts automatically. Use `ZHIWEI_OPS_DISABLE=1` to run only
the data plane.

First startup:

1. Generates self-signed root CA in `<data-dir>/ca/` (10-year validity)
2. Issues monitor server certificate (90-day, CA-signed, SAN: localhost / 127.0.0.1 / ::1)
3. Creates SQLite database `<data-dir>/monitor.db` (WAL mode)
4. Prints a one-time bootstrap token in logs (10-minute TTL)

### Docker

```sh
docker build -t zhiwei-monitor .
docker run -d --name zhiwei-monitor \
  -p 8443:8443 \
  -v zhiwei-data:/var/lib/zhiwei \
  zhiwei-monitor
```

View bootstrap token:

```sh
docker logs zhiwei-monitor | grep 'BOOTSTRAP TOKEN'
```

For managed platforms (edge terminates TLS), run with plain HTTP:

```sh
docker run -d --name zhiwei-monitor \
  -e PORT=10000 -e ZHIWEI_PLAIN_HTTP=1 \
  -v zhiwei-data:/var/lib/zhiwei \
  zhiwei-monitor
```

The image entrypoint starts `zhiwei-ops` first, then `exec zhiwei-monitor`,
sharing the data directory. Use `ZHIWEI_OPS_DISABLE=1` for data-plane-only.

### Platform Architecture Note (Apple Silicon)

On Apple Silicon, `docker build` produces **linux/arm64** by default, but
Render / Railway / Northflank run on **amd64**. Build for the target platform:

```sh
docker build --platform linux/amd64 -t zhiwei-monitor .
```

---

## Managed Platforms (Render / Railway / Northflank)

Most PaaS platforms terminate TLS at the edge and don't forward client
certificates to containers:

```
node ──HTTPS──> PaaS edge (TLS terminated) ──plain HTTP──> container
```

Therefore, node identity **cannot rely on mTLS client certificates**.
ZhiWei uses **Ed25519 request signing** instead:

1. Node submits Ed25519 public key at enrollment (not a CSR)
2. All subsequent requests carry signature headers: `x-zhiwei-node` /
   `x-zhiwei-timestamp` / `x-zhiwei-nonce` / `x-zhiwei-signature`
3. Monitor validates signature + time window (±300s) + non-replayed nonce
4. Command channel uses bidirectional signing: ops signs commands, nodes sign responses

| Scenario | Deployment | Node connects to |
| --- | --- | --- |
| Self-hosted | Default (monitor terminates TLS) | `https://monitor.example.com` |
| Render / Railway / Northflank | `--plain-http` or `ZHIWEI_PLAIN_HTTP=1` | Platform-issued `https://<app>.onrender.com` |

### Auto-Detection (v0.0.1+)

Without explicit `--plain-http`, monitor auto-detects managed platforms:

| Platform | Trigger Variable | Injects `PORT` |
| --- | --- | --- |
| Render | `RENDER=true` | Yes |
| Railway | Any `RAILWAY_*` variable | Yes |
| Heroku | `DYNO` | Yes |
| Northflank | No reliable marker | No |

Always **explicitly configure** these two for managed platforms:

```
ZHIWEI_PLAIN_HTTP=1
ZHIWEI_LISTEN=0.0.0.0:<container port>
```

### Northflank Gotcha

Northflank doesn't inject `PORT`. Without configuration, the server binds
`127.0.0.1:8443` (edge can't reach it) and treats plain HTTP as TLS
handshake. Set:

```
ZHIWEI_PLAIN_HTTP=1
ZHIWEI_LISTEN=0.0.0.0:8443
```

Then verify in startup logs: `listen=0.0.0.0:8443 plain_http=true`.
Also ensure the port is protocol **HTTP** (not TCP) and **Public** in
Northflank's port settings.

---

## Configuration Reference

| Source | Key | Description |
| --- | --- | --- |
| Env | `PORT` | PaaS injection. Binds `0.0.0.0:$PORT` when no explicit listen is configured |
| Env | `ZHIWEI_DATA_DIR` | Data directory (overrides config file) |
| Env | `ZHIWEI_LISTEN` | Listen address (overrides config and `PORT`) |
| Env | `ZHIWEI_PLAIN_HTTP` | Plain HTTP mode for edge-terminated TLS |
| Env | `ZHIWEI_ADMIN_TOKEN` | Console credential (min 16 chars) |
| Env | `ZHIWEI_BOOTSTRAP_TOKEN` | Long-lived enrollment token (min 16 chars) |
| Env | `ZHIWEI_OPS_DISABLE` | Disable ops-server (data-plane only) |
| Env | `RUST_LOG` | Log level, default `info,zhiwei=debug` |
| CLI | `--config` | Config file path, default `config/monitor.toml` |
| CLI | `--data-dir` | Same as `ZHIWEI_DATA_DIR` |
| CLI | `--listen` | Same as `ZHIWEI_LISTEN` |
| CLI | `--plain-http` | Same as `ZHIWEI_PLAIN_HTTP` |

Priority: CLI > env > config file > defaults.

---

## Persistence (Required)

`<data-dir>` contains:

| Path | Content | Loss consequence |
| --- | --- | --- |
| `ca/ca.key.pem` | CA private key | All enrolled nodes invalidated, must re-enroll |
| `monitor.db` | Node list + telemetry | All history lost |
| `ops.key` | Ops signing key | Commands rejected until ops restarts with new key |

**Always mount a persistent volume** on managed platforms:

| Platform | Method | Mount point |
| --- | --- | --- |
| Render | Disk (paid) | e.g. `/var/lib/zhiwei` |
| Railway | Volume | e.g. `/var/lib/zhiwei` |
| Northflank | Volume | e.g. `/var/lib/zhiwei` |

Without a volume: every redeploy generates a new CA, all nodes go offline.

---

## Node Deployment

The node agent reads host CPU / memory / disk / network, **not suitable for
containerization**. Deploy as a binary + systemd on each host.

### Install Binary

After a tag is pushed, release workflow produces pre-built packages for
Linux (x86_64/aarch64 × musl/gnu) and macOS (arm64/x86_64).
The install script auto-detects OS and architecture, verifies SHA256:

```sh
curl -fsSL https://raw.githubusercontent.com/zhiwei/zhiwei/main/scripts/install.sh | sh
```

Default installs `zhiwei-node`; use `-s -- --bin monitor` to install monitor.

**China / isolated networks**: set `ZHIWEI_BASE_URL` to your own mirror:

```sh
curl -fsSL https://raw.githubusercontent.com/zhiwei/zhiwei/main/scripts/install.sh \
  | ZHIWEI_BASE_URL=https://mirror.example.com/releases/zhiwei/zhiwei sh
```

### Enroll

```sh
ZHIWEI_MONITOR_URL=https://monitor.example.com \
ZHIWEI_BOOTSTRAP_TOKEN=<enrollment token> \
zhiwei-node --state-dir /var/lib/zhiwei-node --interval 30
```

After first enrollment, subsequent starts skip enrollment (state persisted).

To set up as a systemd/launchd service (auto-restart on boot), use the
install script:

```sh
curl -fsSL https://raw.githubusercontent.com/zhiwei/zhiwei/main/scripts/install-node-service.sh \
  | sudo sh -s -- --token zhi-bt-xxxxxxxx
```

Node local state:

| File | Content | Notes |
| --- | --- | --- |
| `signing.key` | Ed25519 signing private key | 0600 permissions |
| `node.id` | Monitor-assigned node ID | — |
| `ops.pub` | Ops control-plane public key | TOFU on enroll |
| `ca.crt.pem` | Monitor CA certificate | Self-hosted only |

---

## Enrollment via Console

On managed platforms where you don't have repo access, enroll nodes through
the console:

1. Settings → Enrollment Tokens → New Token
2. Choose TTL (1h / 24h / 7d), optional label
3. Copy the provided command

```sh
curl -sSL https://<your-monitor>/install-node.sh \
  | ZHIWEI_MONITOR_URL=https://<your-monitor> \
    ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
    bash -s
```

On the target machine (requires root):

```sh
curl -sSL https://<your-monitor>/install-node.sh | sudo -E bash -s
```

### Upgrade (preserves identity)

```sh
curl -sSL https://<your-monitor>/install-node.sh | sudo bash -s -- --upgrade
```

Upgrades binary only; state-dir identity and config are untouched.

### Reinstall (new identity)

```sh
curl -sSL https://<your-monitor>/install-node.sh | \
  ZHIWEI_MONITOR_URL=https://<your-monitor> \
  ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
  sudo -E bash -s -- --reinstall
```

### Uninstall

```sh
curl -sSL https://<your-monitor>/install-node.sh | sudo bash -s -- --uninstall
```

---

## Pre-Launch Checklist

- [ ] `<data-dir>` has a persistent volume; `ZHIWEI_DATA_DIR` points to it
- [ ] CA private key backed up (loss requires re-enrolling all nodes)
- [ ] Image built with `--platform linux/amd64` (Apple Silicon defaults to arm64)
- [ ] Managed platform: `ZHIWEI_PLAIN_HTTP=1` explicitly set
- [ ] Managed platform: `ZHIWEI_LISTEN=0.0.0.0:<port>` (or platform injects `PORT`)
- [ ] Northflank: port set to **HTTP** + **Public**, `*.code.run` domain assigned
- [ ] Console credential in place (self-hosted: check startup logs; managed: set `ZHIWEI_ADMIN_TOKEN`)
- [ ] Node can reach monitor address (check firewall / security group)
- [ ] `RUST_LOG` adjusted for production (avoid debug spam)
- [ ] Bootstrap token ≥ 16 chars (≥ 32 recommended); delete variable and restart after enrollment

---

## China / Isolated Network Deployment

Two defaults that don't work in China:

| Default | Workaround |
| --- | --- |
| Monitor address: platform domain | Run monitor on a China-hosted server |
| Binary download: GitHub Releases | Set `ZHIWEI_BASE_URL` to your own mirror |

1. **Run monitor in China**: any Linux server, SQLite has no external dependencies.
   Use `ZHIWEI_PLAIN_HTTP=1` if behind an nginx reverse proxy.
2. **Set `ZHIWEI_NODE_BASE_URL` on monitor**: every enrollment command generated
   by the console will auto-include `ZHIWEI_BASE_URL` — operators don't need to add it manually.

Binary source directory layout must match GitHub Release:
```
<ZHIWEI_BASE_URL>/latest/<asset>
<ZHIWEI_BASE_URL>/v<version>/<asset>
```
