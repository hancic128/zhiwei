# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

## [0.0.1] - 2026-10-03

First public release.

### Highlights

- **Single binary** Rust + SQLite + WAL, ~105 MB Docker image, 5-minute
  setup
- **Ed25519 node identity** instead of mTLS — works behind
  edge-terminated TLS on PaaS platforms
- **Bidirectional command signing**: monitor cannot forge ops commands,
  nodes cannot forge execution receipts
- **Web console** with en-US / zh-CN bilingual UI, theme picker, and a
  side drawer that collapses for small screens

### Resource monitoring

- Per-node CPU / memory / disk / network / load average / uptime / boot
  time
- Per-node process list (top 20 by CPU) with hover tooltip and one-click
  command copy
- Per-node interface inventory with primary IPv4/IPv6 surfaced in the
  node list

### Service health

- HTTP / TCP / TLS probes executed on the node, results stored as
  per-node history
- Probe failures surface in the "today's TODOs" home page alongside
  alerts and certificate expiry

### Certificate management

- Glob-based PEM discovery with optional path list per source
- x509-parser-driven subject / issuer / SAN / validity / serial number
- Expiry bucketing (expired / < 30 days / healthy) with sortable list
  and node-filter dropdown
- Source-level test-fetch + notification channel

### Alerting

- Built-in rules seeded on first launch: CPU / memory / disk / node
  offline
- Threshold + duration state machine (no spam on transient blips)
- Webhook delivery (Slack / Feishu / DingTalk / generic), one-click
  test on each

### Remote operations (control plane)

- `zhiwei-ops` separate process; signing key lives only in its memory
- Whitelisted action set: `noop`, `fetch_logs`, `kill_process`,
  `restart_host`, `shutdown_host`, `container_start|stop|restart`,
  `refresh_inventory`, `scan_certs`
- Log pull is on-demand (no continuous log streaming); responses are
  signed by the executing node

### MCP

- Read-only MCP server exposing node / alert / certificate / probe data
  to AI agents (Claude Desktop, etc.)
- Sensitive actions (reboot / shutdown / container kill) are not
  exposed; they remain on the ops-signed command channel

### Deployment

- One-line install via `curl ... | sh` (Linux x86_64 / aarch64 × musl /
  gnu, macOS arm64 / x86_64)
- Multi-stage Dockerfile with non-root runtime, persistent CA + SQLite
  under `/var/lib/zhiwei`
- `ZHIWEI_PLAIN_HTTP=1` for PaaS deployments that terminate TLS at the
  edge
- systemd unit / launchd plist / nohup fallback via
  `scripts/install-node-service.sh`

### Documentation

- README + `docs/` covering architecture, deploy, service health,
  probes, alerts, certificates, backup, troubleshooting, FAQ, and API
  reference
- `SECURITY.md` with supported-versions table and a `security@` contact
- `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `GOVERNANCE.md`,
  `MAINTAINERS.md`, `SUPPORT.md`

### Known limitations

- First-paint gzip is about 320 KB (over the 200 KB target tracked
  internally); switching ECharts to uPlot could shave ~130 KB but is
  not in this release
- The nonce-replay cache is in-process; running multiple monitor
  instances behind a load balancer requires a shared cache (planned)
- `cargo deny check` is enforced in CI but `cargo audit` and the
  frontend equivalent are not yet wired
- `clippy.toml` contains a few fields that the current Clippy version
  rejects; this is a pre-existing toolchain mismatch unrelated to this
  release
