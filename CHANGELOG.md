# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Complete open-source preparation artifacts
- CODE_OF_CONDUCT.md (Contributor Covenant v2.1)
- SECURITY.md with vulnerability reporting policy
- GOVERNANCE.md and MAINTAINERS.md
- CI workflows (fmt, clippy, test, deny, labeler)
- GitHub issue and PR templates
- CODEOWNERS for automated review assignment
- Rustfmt, Clippy, and Cargo-deny configurations
- .editorconfig for consistent formatting
- Architecture documentation
- Roadmap documentation

## [0.1.0] - 2024-XX-XX

Initial alpha release.

### Added
- Node enrollment with Ed25519 request signing
- Basic telemetry collection (CPU, memory, disk, network, processes)
- Web UI with dark/light themes and i18n
- Service health probes (HTTP, TCP, TLS)
- Alert system with webhook notifications
- Certificate tracking and expiry monitoring
- Docker container monitoring
- Container log streaming
- MCP server tools
- Self-signed CA and mTLS support
- Command channel (ops-server) for controlled remote operations
- Multi-platform binary releases (Linux, macOS)
- Docker image

[unreleased]: https://github.com/hancic128/zhiwei/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hancic128/zhiwei/releases/tag/v0.1.0
