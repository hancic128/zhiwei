# Contributing

## Getting Started

### Prerequisites

- Rust 1.75+
- Node 18+ (frontend only changes don't require Node)

### Running Locally

```sh
./scripts/dev.sh start     # Start monitor + ops + auto-enroll a local node
./scripts/dev.sh status    # Check process and log status
./scripts/dev.sh stop
```

Console at <http://127.0.0.1:8443/>, admin token at `data/admin.token`.
Frontend changes require `cd ui && npm install && npm run build`;
`scripts/dev.sh` automatically serves `ui/dist`. See [README](./README.md) for full details.

## Pre-commit Checklist

All checks must pass before submitting:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

cd ui && npm run build      # Includes language pack consistency check + tsc
```

These four are hard requirements; CI will block on failure.
When Dockerfile changes are involved, also run:
`./scripts/check-dockerfile-crates.sh` (workspace members and image COPY list must stay in sync).

## Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(nodes): add node list pagination and sorting
fix(config): accept 1/true/yes/on for ZHIWEI_PLAIN_HTTP
docs(api): document new enrollment endpoint
```

Common scopes: `auth` `api` `ui` `node` `ops` `storage` `alerts` `certs` `services`
`logs` `docker` `deploy` `docs` `config`.

## Code Conventions

**Rust**: New database schema changes go in `crates/storage/src/migrations.rs`,
increment the version number, idempotent via `schema_version`. Never modify existing migrations.

**Frontend**: Follow the UI constraints spec — use only `brand-*` / `surface-*` / `ink-*`
tokens (no hardcoded colors), Lucide SVG icons only (no emoji), all text through `t()`.
The default locale is **en-US**; an opt-in **zh-CN** translation is shipped for
the UI strings. New user-facing text must come with an `en-US` entry; a matching
`zh-CN` entry is appreciated but optional. `npm run build` includes a locale
key-count check (both files must have the same set of keys).

**Tests**: New behavior should come with reproducible verification. Describe what
commands you ran and what output you saw — coverage numbers are less useful than a
working demo in the PR.

## PR Process

1. One PR, one focus — keep it small and targeted.
2. Describe: what it solves, how you verified it (command + actual output), known limitations.
3. CI green before requesting review.

## Issue Guidelines

Include: OS/deployment method/version, expected behavior, actual behavior.
Paste logs and error messages directly — don't paraphrase.

## License

Apache-2.0. By contributing, you agree to license your work under this license.
