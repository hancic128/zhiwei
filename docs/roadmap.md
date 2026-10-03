# Roadmap

> Where ZhiWei is heading. Updated periodically.

## Vision

ZhiWei exists for **one person to manage N machines** with minimal cognitive load.
The project prioritizes:
1. **Cognitive lightness** - No dashboards to configure, no query languages to learn
2. **Agent-callable** - Data and operations accessible to AI agents
3. **Self-hosted simplicity** - Single binary, SQLite, 5-minute setup

## Current Status

**v0.1.x** - Foundation Phase (In Progress)

Completed:
- [x] Node enrollment with Ed25519 request signing
- [x] Basic telemetry collection (CPU, memory, disk, network, processes)
- [x] Web UI with dark/light themes and i18n
- [x] Service health probes (HTTP, TCP, TLS)
- [x] Alert system with webhook notifications
- [x] Certificate tracking and expiry monitoring
- [x] Docker container monitoring
- [x] Container log streaming
- [x] MCP server tools
- [x] Self-signed CA and mTLS support
- [x] Command channel (ops-server) for controlled remote operations

## Upcoming

### v0.2.x - Alerting & Notifications
- [ ] Multiple notification channels (email, Slack, Discord, custom)
- [ ] Alert routing rules
- [ ] Alert aggregation and deduplication
- [ ] Alert history and statistics

### v0.3.x - Operations
- [ ] Remote command execution via ops-server
- [ ] Container log query and download
- [ ] Process management (signal sending)
- [ ] Certificate renewal automation (ACME)

### v0.4.x - Scale & Polish
- [ ] Performance optimization
- [ ] Resource usage reduction
- [ ] Migration guide
- [ ] Production hardening guide

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

1. **Read the positioning** - Align your request with the project's goals
2. **Open a discussion** - Before filing a feature request
3. **Contribute** - PRs welcome for aligned features

## Changelog

See [GitHub Releases](https://github.com/hancic128/zhiwei/releases) for detailed
version history.
