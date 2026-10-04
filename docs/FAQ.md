# Frequently Asked Questions

> Short answers to questions that don't warrant a full doc page. If something
> is missing, open a Discussion; once it's answered here it won't need to be
> asked again.

## Project

**Q: Is ZhiWei a Prometheus replacement?**
No. ZhiWei is a self-contained observability platform with its own storage
(SQLite) and UI. It targets the **single-operator, multi-machine** use case
where installing Prometheus + Grafana + an exporter fleet is too much
operational overhead.

**Q: Can I run multiple monitors behind a load balancer?**
Not yet. The nonce-replay cache is in-process, so the second monitor would
accept requests the first already accepted. A shared cache (Redis or
PostgreSQL-backed) is planned. For 0.0.x, scale by adding more nodes to one
monitor, not more monitors.

**Q: Does ZhiWei support multi-user / teams / RBAC?**
No. ZhiWei is **single-operator** — see [GOVERNANCE.md](../GOVERNANCE.md). All
authenticated users share one role (admin) by design. AI tokens are read-only
sub-credentials for MCP clients; they are not user accounts.

**Q: What languages does the UI speak?**
`en-US` (default) and `zh-CN` (opt-in via the floating controls → language
button). Other locales are welcome as PRs to `ui/locales/<locale>/common.json`;
the build's key-count check ensures parity.

**Q: Where does the version come from?**
The workspace root `Cargo.toml` (`version = "0.0.1"`). The release workflow
(`/.github/workflows/release.yml`) reads it when triggered by a tag.

## Deployment

**Q: Why `ZHIWEI_PLAIN_HTTP=1` on Render / Railway / Northflank?**
Those platforms terminate TLS at the edge and proxy plain HTTP to the
container. Without plain-HTTP mode, monitor would try to do its own TLS
handshake over the unencrypted edge → container hop and fail. Node identity
doesn't depend on TLS (it uses Ed25519 request signing), so security is
preserved. See [DEPLOY.md](./DEPLOY.md) §"Managed Platforms".

**Q: Can I run the node agent in a container?**
Technically yes, but the node agent reports host CPU / memory / disk / network
from inside the container, which are usually meaningless (cgroups-isolated).
Run it on the host as a systemd / launchd service via `install-node.sh`.

**Q: I redeployed monitor — why are all my nodes offline?**
The self-signed CA is in `<data-dir>/ca/`. Without a persistent volume mount,
every redeploy regenerates the CA and the stored node signing keys no longer
match. Mount `<data-dir>` to a persistent volume (Render Disk / Railway Volume
/ Northflank Volume). See [DEPLOY.md](./DEPLOY.md) §"Persistence (Required)".

**Q: My `install-node.sh` curl runs but `install-node-service.sh` fails. Why?**
The service installer requires root and a systemd-or-launchd-capable system.
For containers or immutable hosts, run the node directly with `nohup` or a
supervisor of your choice; the data-plane functionality is identical.

## Operations

**Q: Why are start / stop / restart buttons doing nothing on a node that
shows as online?**
The node's command channel is down. Most often this means the node enrolled
*before* `zhiwei-ops` was running (so it never received the ops public key),
or its state was restored from backup. Look in the Todo page for a "command
channel down" entry — that points at the affected node.

**Q: Can I stream container logs continuously?**
No. Logs are **on-demand**: click the button, wait for the next node-poll
cycle (≤10s), and the response is returned as a one-shot UTF-8 payload. This
keeps the command channel small and avoids holding long-lived connections
through PaaS edges.

**Q: What's the difference between silence and resolve?**
- **Silence** — the alert stays open but is hidden from the Todo for the
  chosen window (1h / 3h / 12h / 24h). When the window expires, it reappears
  if the underlying condition is still present.
- **Resolve** — manually marks the alert as resolved. If the condition
  recurs, a new alert instance is opened.

**Q: I added an alert rule but nothing fires even though CPU is high. Why?**
Alert rules have a `duration_seconds` field. A rule with `duration_seconds:
300` only fires after the condition holds for 5 minutes, to filter transient
spikes. Set it to `0` for immediate firing (noisy).

## MCP

**Q: Why does the MCP server return 403 with my admin token?**
MCP must use an **AI token** (`ait_<...>`), not the admin token. This is
intentional — MCP integrations are long-lived, so they should use an
independently revocable credential. Create one in Settings → AI Tokens.

**Q: Can MCP kill processes or restart containers?**
No. MCP is **read-only** by intent. Management operations (kill / restart /
reboot / shutdown) go through the console or `POST /v1/exec`, which uses the
ops-signed command channel — that channel requires the operator's admin
token, not an AI token.