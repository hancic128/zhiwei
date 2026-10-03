# Troubleshooting

> Common issues and how to resolve them.

## Monitor

### Monitor won't start

**Port already in use**:
```
Error: Address already in use (os error 98)
```
Another process is using port 8443 (or whatever `ZHIWEI_LISTEN` is set to). Stop the conflicting process or change the port.

**Data directory permissions**:
```
Error: Permission denied (os error 13)
```
ZhiWei needs read/write access to the data directory. Check ownership:
```sh
ls -la /var/lib/zhiwei
chown -R $USER:$USER /var/lib/zhiwei
```

**Bootstrap token expired**:
```
Error: bootstrap token expired
```
Bootstrap tokens expire after 10 minutes. Generate a new one from the console (Settings → Enrollment Tokens → New Token).

### Monitor runs but nodes can't connect

**Wrong monitor URL on nodes**:
Nodes must connect to the publicly accessible monitor URL, not `localhost`. If monitor is behind a reverse proxy, ensure `X-Forwarded-Proto` and `Host` headers are set correctly.

**TLS certificate mismatch**:
```
error: tls handshake eof
```
On self-hosted deployments, nodes need the CA certificate (`ca.crt.pem`). The install script fetches it automatically. For manual setup:
```sh
# On the node, copy the CA cert to:
/var/lib/zhiwei-node/ca.crt.pem
```

**Ed25519 signature validation failed**:
```
Error: invalid signature
```
The node's signing key may be corrupted. Re-enroll the node:
```sh
curl -sSL https://<monitor>/install-node.sh | sudo bash -s -- --reinstall
```

### Web UI shows "Not authenticated"

The admin token is missing or incorrect. Check the token:
```sh
cat /var/lib/zhiwei/admin.token
```

For managed platforms, ensure `ZHIWEI_ADMIN_TOKEN` is set to at least 16 characters.

---

## Nodes

### Node not reporting telemetry

1. **Check node status**:
```sh
systemctl status zhiwei-node
journalctl -u zhiwei-node -n 50
```

2. **Verify enrollment**: The node should have `signing.key`, `node.id`, and `ops.pub` in its state directory. If missing, re-enroll:
```sh
curl -sSL https://<monitor>/install-node.sh | sudo bash -s -- --reinstall
```

3. **Check network**: Node must reach monitor at `ZHIWEI_MONITOR_URL`:
```sh
curl -v https://<monitor>/healthz
```

### High CPU / memory usage on node

Node's resource footprint is minimal (~1% CPU, ~50MB RAM). High usage indicates:
- Disk I/O bottleneck (SQLite WAL on slow storage)
- Network latency to monitor (telemetry batches queue up)

Reduce collection frequency:
```sh
zhiwei-node --interval 60  # default is 30
```

### Node offline alert but node is running

- **Cold start suppression**: Monitor waits 120s after restart before triggering `node_offline` alerts
- **Check `last_seen`**: In the UI, check the node's last telemetry timestamp. If it's stale, the node isn't reaching the monitor

### Start / stop / restart buttons do nothing (command channel down)

The node appears online and reports telemetry, but **command buttons have
no effect**. This means the node's command channel is broken — it never
polls `/v1/commands` so ops-signed commands never reach it.

Common causes:
1. **Node enrolled before ops-server was running.** The node never
   received `ops_public_key` at enroll time. Re-enroll to fetch it:
   ```sh
   curl -sSL https://<monitor>/install-node.sh | sudo bash -s -- --reinstall
   ```
2. **State restored from a backup taken before ops-server first started.**
   Same symptom — `ops.pub` on disk is empty or stale. Re-enroll as above.
3. **Clock drift > ±300s.** Signed commands are rejected by the node's
   timestamp window. Sync NTP on both monitor and node.

Verify the channel state in the UI: the node's detail page surfaces the
last successful poll timestamp. If it's been minutes since the last poll
despite recent telemetry, the channel is broken.

---

## Alerts

### Alerts not firing

1. **Check rule is enabled**:
```sh
curl https://<monitor>/v1/rules -H "Authorization: Bearer <token>"
```

2. **Check metric name**: Metric keys are case-sensitive and must match exactly:
   - `host.cpu.usage` (not `cpu_usage` or `CPU`)
   - See [Alerting](./ALERTS.md) for full metric list

3. **Check duration**: If `duration_seconds` is 300, the condition must persist for 5 minutes before firing

4. **Check notification channels**: Ensure at least one channel exists and is enabled:
```sh
curl https://<monitor>/v1/channels -H "Authorization: Bearer <token>"
```

### Webhook not receiving notifications

1. **Test the channel**:
```sh
curl -X POST https://<monitor>/v1/channels/test \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Test","kind":"webhook","url":"https://example.com/webhook"}'
```

2. **Check webhook URL**: Ensure the URL is publicly accessible and accepts POST with JSON body

3. **Check firewall**: Outbound HTTPS (port 443) must be allowed from the monitor

4. **Increase timeout**: Webhook timeout is 10s. For slow endpoints, consider a queue/proxy

### Duplicate alerts

Alerts are deduplicated by `(rule_id, node_id)` for metric rules, or by `source_ref` for built-in alerts. If you're seeing duplicates:
- For metric rules: the same rule may be applied to the same node through multiple paths
- Check [Alerting](./ALERTS.md) → Deduplication section

---

## Certificates

### Certificate scan finds nothing

1. **Add cert sources**: Certificates aren't discovered automatically. Add scan paths:
```sh
curl -X POST https://<monitor>/v1/cert-sources \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"node_id":"<node-uuid>","paths":["/etc/ssl/certs","/var/www/.well-known/acme-challenge"]}'
```

2. **Check node enrollment**: Cert sources require an enrolled node

3. **File permissions**: Node must have read access to the certificate directories

### Certificate alert not firing

- Ensure the cert source has `notify_days_before` set (default may be 0)
- Check `cert_expiring` / `cert_expired` built-in alerts are enabled:
```sh
curl https://<monitor>/v1/builtin-alerts -H "Authorization: Bearer <token>"
```

---

## Database

### SQLite database locked

```
Error: database is locked
```
Only one writer can access SQLite at a time. Ensure only one monitor instance is running:
```sh
ps aux | grep zhiwei-monitor
```

If running multiple monitors for HA, this is not supported — use a single instance with proper process supervision.

### Database corruption

1. **Check integrity**:
```sh
sqlite3 /var/lib/zhiwei/monitor.db "PRAGMA integrity_check;"
```

2. **Recovery**: If corrupted, restore from backup (see [Backup & Restore](#backup--restore))

3. **Prevention**: Always shut down gracefully (`systemctl stop zhiwei-monitor`), avoid hard kills

---

## Performance

### Slow API responses

- **Check disk I/O**: SQLite performance degrades on NFS/network storage
- **Telemetry retention**: Default 7-day retention. Reduce if disk is constrained:
  ```sh
  # In config or env
  ZHIWEI_RETENTION_DAYS=3
  ```
- **Too many nodes**: ZhiWei is designed for ~20 nodes. Beyond that, consider splitting deployments

### High disk usage

Telemetry storage estimate: ~1.6 MB/day per node.

To check current size:
```sh
du -sh /var/lib/zhiwei/monitor.db
du -sh /var/lib/zhiwei/
```

Manual cleanup (monitor must be stopped):
```sh
sqlite3 /var/lib/zhiwei/monitor.db "DELETE FROM telemetry WHERE ts_unix_nano < datetime('now', '-7 days');"
sqlite3 /var/lib/zhiwei/monitor.db "VACUUM;"
```

---

## Getting Help

If the issue persists:

1. Enable debug logging:
   ```sh
   RUST_LOG=debug,zhiwei=trace ./zhiwei-monitor
   ```

2. Check logs for error context

3. For bugs or feature requests, open an issue at:
   https://github.com/zhiwei/zhiwei/issues
