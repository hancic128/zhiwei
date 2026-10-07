# ZhiWei Help

ZhiWei is an observability platform for server clusters: this process collects
resources, service health, certificates, and alerts; one web console shows
everything. Once a node is enrolled, no more wiring up monitoring or alerts by hand.

## Quick start

### 1. Get the enroll command

Open the **Nodes** page and click "Onboard help" in the top right. The console
will mint a one-time enroll token and open a dialog with the full command,
already copied to your clipboard. Paste it into a shell on the target machine
and run it once.

```
curl -sSL {{BASE_URL}}/install-node.sh | \
  ZHIWEI_MONITOR_URL={{BASE_URL}} \
  ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
  bash -s
```

- The `scheme + host + port` prefix at the start of the command is the address
  you're using to open the console. On an internal IP with plain HTTP it would
  look like `http://10.0.0.5:8443/...` — keep it consistent or you'll point at
  the wrong host.
- **The command contains a plaintext token**, equivalent to an "enroll key".
  Don't share it on public channels. It expires automatically (24h by default),
  and you can revoke it manually from Settings → Enroll Tokens.
- For explicit lifecycle control (naming, setting TTL, seeing how many are
  active, revoking) use Settings → Enroll Tokens → New enroll token.

### 2. Run it on the target machine

Root is required (the script writes `/usr/local/bin` and a systemd unit). It will:
- Download `zhiwei-node` (GitHub Releases by default; for air-gapped networks
  set `ZHIWEI_BASE_URL` to point at your own mirror / artifact store)
- Write `/etc/zhiwei-node.env`
- Register and start a systemd unit called `zhiwei-node.service`
- The node appears in the **Nodes** list within 30 seconds

To give the machine a **name on first enroll**, add two flags (or set env vars
`ZHIWEI_NODE_ALIAS` / `ZHIWEI_NODE_TAGS`):

```
curl -sSL {{BASE_URL}}/install-node.sh | \
  ZHIWEI_MONITOR_URL={{BASE_URL}} \
  ZHIWEI_BOOTSTRAP_TOKEN=zhi-bt-xxxxxxxx \
  bash -s -- --alias beijing-edge --tags "prod bj edge"
```

Tags can be separated by spaces, commas, or full-width commas `、` (same
parsing as the console input box). They are **only reported on first enroll**: once a node
already has `node.id`, enrolling again won't update tags. Edit them from the
console, or pass `--reinstall` to re-enroll (wipes the old binary / env /
state dir and assigns a new node_id).

### 3. (Optional) Reverse enroll

If the node starts before the console (for example, when a CI job spins up
a temporary machine), let it read `ZHIWEI_BOOTSTRAP_TOKEN` and `ZHIWEI_MONITOR_URL`
from environment variables. The first heartbeat enrolls automatically.

### 4. Naming: aliases and tags

Hostname-only enrollment is hard to read (auto-generated cloud names like
`ip-10-0-0-5.ec2.internal` are not). The console lets you add two layers of
metadata to each node — open the edit button to the right of the host column
in the list. You can also attach them on **first enroll**
(`install-node.sh --alias/--tags`, see section 2):

| | Alias | Tags |
|--|------|------|
| Count | 1 per node | up to 10 per node |
| Length | up to 10 characters | up to 24 characters each |
| Purpose | replaces hostname in the UI | grouping / filtering |

- **Alias wins**: the Nodes list, Certificates page, Containers page, node
  detail header, and every node dropdown (container logs, certificate sources,
  container filters) all show the alias; they fall back to hostname only when
  no alias is set. Search matches across alias, hostname, ID, IP, and tags.
- **Tag filtering**: the "All tags" dropdown at the top of the Nodes page filters
  by tag. Tags also participate in sort and search. Tags are user-defined
  (e.g. `prod` / `bj` / `edge`); there are no preset values. Press Enter or
  use a comma to add multiple at once.
- Aliases and tags live only in the console's database; they don't affect
  node identity. Changing them doesn't affect issued certificates, alerts,
  or the command channel.

## Troubleshooting (common errors)

### "ops-server unavailable: cannot connect to …: no process listening on that address"

Operations like viewing container logs / file logs, killing processes,
restarting hosts, and renewing certificates must first be signed by
`zhiwei-ops` (control plane); monitor (data plane) only forwards. This error
means monitor can't reach ops:

- **Self-hosted / bare metal**: `zhiwei-ops` isn't running. It's a separate
  process that listens on `127.0.0.1:8444` by default. The address monitor
  uses is in its own `config/monitor.toml` as `ops_endpoint` (default
  `http://127.0.0.1:8444/exec`, overridable with `--config`). **If you
  installed the distro package** (`install.sh --bin monitor` puts
  `zhiwei-ops` next to `zhiwei-monitor`), monitor notices nothing is listening
  on that port at startup and auto-launches the sibling `zhiwei-ops`. If you
  only kept the `zhiwei-monitor` binary, or the two are in different paths,
  set `ZHIWEI_OPS_BIN=/path/to/zhiwei-ops`, or run `zhiwei-ops` as a separate
  systemd unit yourself.
- **Containers / managed platforms**: the image's entrypoint is supposed to
  start `zhiwei-ops` alongside `zhiwei-monitor` in the same container. If
  you overrode Command in the platform UI and didn't keep `zhiwei-ops` next
  to `zhiwei-monitor`, only the data plane is running — drop the override,
  or append `zhiwei-ops &` to your startup command.
- If you only need telemetry + liveness + certificate scanning (no remote
  commands), the error is harmless — or set `ZHIWEI_OPS_DISABLE=1`
  explicitly and the console will report "command channel disabled" instead.

### "ops public key not held; command channel will not fetch commands"

Node-side security default: without the ops public key, the node refuses to
execute any write command from monitor. Usually monitor's data dir has no
`ops.pub` (ops never started, or the data dir wasn't on a persistent volume).
Fix as above. Note that **without a persistent volume**, every cold start
rotates the signing key, so already-enrolled nodes reject new commands and
the symptom is "commands sent, no reply".

### Machine finished installing, but never appears in the console

Two common causes:

- **`node.id` is stale**: the node only cares whether `<state-dir>/node.id`
  exists. After the console side changed its data dir / database (or the
  peer's data was reset), the old identity keeps getting 401 "node signature
  verification failed". Delete `node.id` (and `ops.pub`) on the node and
  reinstall once.
- **Download source unreachable**: the script didn't error but the node
  didn't start — check `systemctl status zhiwei-node` and
  `/etc/zhiwei-node.env`. For air-gapped networks, set `ZHIWEI_BASE_URL`.

### Container logs / container list is empty

These go down different paths; troubleshooting is different:

- **List**: the low-frequency "snapshot" the node reports contains the
  container inventory (host info + containers + processes + certificates);
  monitor only keeps the latest one. If the node has no `docker` (or can't
  read the socket), the inventory is empty — that's a normal result, not an
  error.
- **Logs**: an on-demand write operation — must go through `zhiwei-ops`
  to sign and dispatch the command, then the node runs `docker logs` and
  returns the result. So if the list has data but logs say "ops-server
  unavailable", the data plane is fine and only the command channel is down
  (see above).
- Logs are fetched on demand, not stored: what you can see depends on
  whether the container still exists (old logs become unreachable after
  the container is recreated / deleted).

## Enroll token vs AI token

These are **completely independent**, with different intents:

| | Enroll token | AI token |
|--|----------|---------|
| Purpose | first-time node enroll | MCP / external AI reads data |
| Credential format | `zhi-bt-...` | `ait_...` |
| Persistence | in-memory (lost on restart) | SQLite (survives restart) |
| Quantity | multiple | multiple |
| Revoke | click in the list | click in the list |
| Expiry | TTL (hours/days) | never expires |

## AI integration

ZhiWei exposes an MCP SSE endpoint (`{{BASE_URL}}/mcp/sse`) so Claude Desktop,
Cursor, Cline, Claude Code, etc. can read and manage cluster data directly.

### 1. Create an AI token

Settings → AI tokens → New → name it (e.g. `claude-desktop-home`) → copy
the plaintext token (**shown only once**).

### 2. Configure the MCP client

In your AI client's MCP settings file:

```json
{
  "mcpServers": {
    "zhiwei": {
      "type": "http",
      "url": "{{BASE_URL}}/mcp/sse",
      "headers": {
        "Authorization": "Bearer ait_xxxxxxxxxxxxxxxx"
      }
    }
  }
}
```

### 3. Available tools

**Node management** (3):
- `list_nodes` — cluster node overview (includes `alias` and `tags`)
- `get_node` — single-node details (host_info + latest metrics)
- `update_node` — update node alias/tags (write)
- `delete_node` — delete a node (write)

**Telemetry** (1):
- `get_telemetry` — single-node time series

**Alerts** (8):
- `list_alerts` — all active + 50 recent resolved
- `silence_alert` — silence an alert for N minutes (write)
- `resolve_alert` — manually resolve an alert (write)
- `list_rules` — custom alert rules
- `create_rule` — create a new alert rule (write)
- `update_rule` — update an alert rule (write)
- `delete_rule` — delete an alert rule (write)
- `list_builtin_rules` — built-in alert rules
- `update_builtin_rule` — enable/disable built-in rules (write)

**Certificates** (5):
- `list_certs` — certificate scan sources
- `create_cert_source` — add a certificate scan source (write)
- `test_cert_source` — test a certificate scan source (write)
- `update_cert_source` — update a certificate source (write)
- `delete_cert_source` — delete a certificate source (write)

**Channels** (5):
- `list_channels` — notification channels
- `create_channel` — create a notification channel (write)
- `test_channel` — test a channel (write)
- `update_channel` — update a channel (write)
- `delete_channel` — delete a channel (write)

**Services** (4):
- `list_services` — service list
- `create_service` — create a service (write)
- `update_service` — update a service (write)
- `delete_service` — delete a service (write)

**Probes** (6):
- `list_probes` — all probes
- `create_probe` — create a probe (write)
- `test_probe` — test a probe configuration (write)
- `update_probe` — update a probe (write)
- `delete_probe` — delete a probe (write)
- `get_probe_results` — probe history results

**Commands** (3):
- `exec_command` — issue a control command to a node (whitelisted actions, write)
- `list_command_history` — command execution history
- `get_command` — command details

**Other** (4):
- `list_containers` — single-node container snapshot
- `list_processes` — single-node process snapshot
- `get_todo` — todo items
- `get_retention` — data retention policy

**Total: 47 tools**

### Security boundary

AI tokens can **read** cluster data and **write** (create/update/delete) alert
rules, channels, services, probes, and certificates. High-risk operations
(executing commands, deleting nodes) should be used with caution.

After revoking an AI token, the next request immediately returns 401.

## Deployment & certificates

ZhiWei ships with its own CA (generated on first start at `<data-dir>/ca/`):
- monitor server certificate is valid for 90 days and includes SAN
- node identity is Ed25519 request signing (no client certs); the CA is only
  used to sign the server certificate when terminating TLS at the edge
- **Losing the disk = losing all nodes**: nodes don't trust a reissued
  cert without the original CA private key

Managed platforms (Render / Railway / Northflank):
- TLS is terminated at the edge; this process listens on plain HTTP
- You must set `ZHIWEI_PLAIN_HTTP=1` and `ZHIWEI_LISTEN=0.0.0.0:<port>`
- Northflank doesn't inject `PORT`, so the listen address must be set explicitly
- **The data directory must be on a persistent volume**: CA private key,
  `ops.key`, SQLite all live there
- The image's entrypoint starts `zhiwei-ops` in the same container before
  `zhiwei-monitor` (disable with `ZHIWEI_OPS_DISABLE=1`, tune wait with
  `ZHIWEI_OPS_WAIT=<seconds>`). **Don't override Command / entrypoint in
  the platform UI** — otherwise the command channel (view logs, kill
  processes, restart services, renew certs) won't work, and you'll see
  "ops-server unavailable".

Self-hosted:
- This process terminates TLS itself; the browser goes straight to
  `https://monitor.example.com`
- You can also front it with nginx / caddy, but monitor's self-signed cert
  will warn in browsers — add the CA to the system trust store
- `zhiwei-ops` is not part of monitor; run it as a separate process (same
  container is fine). When it's down, the data plane keeps working — only
  the command channel is unavailable