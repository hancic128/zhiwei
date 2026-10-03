# Roadmap

> Where ZhiWei is heading. Updated periodically.

## Vision

ZhiWei exists for **one person to manage N machines** with minimal cognitive load.
The project prioritizes:
1. **Cognitive lightness** - No dashboards to configure, no query languages to learn
2. **Agent-callable** - Data and operations accessible to AI agents
3. **Self-hosted simplicity** - Single binary, SQLite, 5-minute setup

## Current Status

**v0.0.1** - Initial Public Release (2026-10-03)

Completed in 0.0.1:
- [x] Node enrollment with Ed25519 request signing
- [x] Basic telemetry collection (CPU, memory, disk, network, processes)
- [x] Web UI with dark/light themes, theme picker, en-US / zh-CN locales
- [x] Service health probes (HTTP, HTTPS, TCP, TLS)
- [x] Alert system with threshold rules and webhook notifications
- [x] Certificate tracking and expiry monitoring
- [x] Docker container monitoring
- [x] On-demand container / file log fetching via ops-server
- [x] MCP server tools (read-only)
- [x] Self-signed CA for self-hosted deployments
- [x] Command channel (ops-server) for controlled remote operations

## Upcoming

### Next - Foundation Hardening
- [ ] Status pages (private / public)
- [ ] Notification channel polish (Slack / Feishu / DingTalk / webhook)
- [ ] Multi-step probes and gRPC / DNS / ICMP probe types
- [ ] Service grouping and SLA / SLO tracking
- [ ] ACME client for certificate renewal
- [ ] Tauri desktop console (single-binary GUI, optional)

### Later - Ecosystem & Scale
- [ ] Migration guide from 0.0.x to the next breaking release
- [ ] Production hardening guide
- [ ] Resource usage reduction (uPlot migration, binary size)
- [ ] Performance optimization for high-cardinality fleets

See [GitHub Releases](https://github.com/zhiwei/zhiwei/releases) for
shipped versions.

## Not Planned

These are explicitly out of scope:

| Won't Do | Reason |
| --- | --- |
| Multi-user / RBAC | Single operator is the target |
| HA / Clustering | OPC doesn't need this |
| Prometheus/Grafana alternatives | Different market |
| K8s deep integration | First users use docker-compose/systemd |
| Dashboard editors | Counter to cognitive lightness |
| Agent autonomous actions | Security boundary |

## Long-term Vision (1-2 years)

- **Mature ecosystem**: Multiple platforms, package managers
- **Content**: Technical articles, use cases, comparisons
- **Community**: Active contributors, third-party integrations
- **Evidence**: The project demonstrates architectural principles others can learn from

## How to Influence the Roadmap

1. **Read [GOVERNANCE.md](./GOVERNANCE.md)** — "what's in scope" is the source of truth
2. **Open a discussion** - Before filing a feature request
3. **Contribute** - PRs welcome for aligned features
