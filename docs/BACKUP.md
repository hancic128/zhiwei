# Backup & Restore

> How to protect and recover your ZhiWei data.

## What's in the Data Directory

```
<data-dir>/
├── ca/
│   ├── ca.crt.pem          # CA certificate (public)
│   └── ca.key.pem          # CA private key (critical)
├── ops.key                 # Ops signing key (critical)
├── admin.token             # Admin credential (critical)
├── monitor.db              # SQLite database (critical)
└── enroll-tokens/          # One-time enrollment tokens (non-critical)
```

| File | Loss consequence |
|---|---|
| `ca/ca.key.pem` | **All node identities invalidated** — must re-enroll every node |
| `ops.key` | **Commands rejected** — ops server must restart to regenerate |
| `admin.token` | **Console access lost** — can be reset via `ZHIWEI_ADMIN_TOKEN` env var |
| `monitor.db` | **All telemetry + config lost** — history gone, rules/channels deleted |
| `ca.crt.pem` | Nodes can't verify server (but can be re-downloaded) |

---

## Backup Strategy

### Automated Backup (recommended)

Add to crontab for daily backups:

```sh
# Daily backup at 3 AM
0 3 * * * /usr/local/bin/zhiwei-backup.sh
```

Create `/usr/local/bin/zhiwei-backup.sh`:

```sh
#!/bin/bash
set -e

DATA_DIR="/var/lib/zhiwei"
BACKUP_DIR="/var/backups/zhiwei"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RETENTION_DAYS=30

mkdir -p "$BACKUP_DIR"

# Backup everything in data dir
tar czf "$BACKUP_DIR/zhiwei_backup_${TIMESTAMP}.tar.gz" -C "$(dirname "$DATA_DIR")" "$(basename "$DATA_DIR")"

# Remove old backups
find "$BACKUP_DIR" -name "zhiwei_backup_*.tar.gz" -mtime +$RETENTION_DAYS -delete

echo "Backup complete: zhiwei_backup_${TIMESTAMP}.tar.gz"
```

### Database-Only Backup (smaller, faster)

If the data directory is large, backup just the critical files:

```sh
tar czf backup.tar.gz \
  -C /var/lib/zhiwei \
  ca/ca.key.pem \
  ops.key \
  admin.token \
  monitor.db
```

---

## Restore

### Full Restore

1. **Stop monitor**:
```sh
systemctl stop zhiwei-monitor
```

2. **Replace data directory**:
```sh
rm -rf /var/lib/zhiwei
tar xzf backup.tar.gz -C /var/lib/zhiwei
```

3. **Fix permissions**:
```sh
chown -R zhiwei:zhiwei /var/lib/zhiwei
chmod 600 /var/lib/zhiwei/ca/ca.key.pem
chmod 600 /var/lib/zhiwei/ops.key
chmod 600 /var/lib/zhiwei/admin.token
```

4. **Start monitor**:
```sh
systemctl start zhiwei-monitor
```

### CA Key Lost (re-enroll all nodes)

If `ca.key.pem` is lost, all nodes must re-enroll:

1. **Backup current data** (as-is, before changes)
2. **Delete CA**:
```sh
rm -rf /var/lib/zhiwei/ca
```
3. **Restart monitor** — new CA will be generated
4. **Re-enroll each node**:
```sh
curl -sSL https://<monitor>/install-node.sh | sudo bash -s -- --reinstall
```

---

## Disaster Recovery Checklist

- [ ] Backup script runs daily (verify cron)
- [ ] Backups copied to offsite storage (S3, remote server)
- [ ] CA private key backed up securely (password manager, HSM)
- [ ] Admin token stored securely (password manager)
- [ ] Restore tested at least once

---

## Backup on Managed Platforms

| Platform | Method |
|---|---|
| Render | Use persistent Disk; backup via external script to object storage |
| Railway | Use persistent Volume; automate via Railway's backup API |
| Northflank | Use Volume snapshots; backup via external script |
| Self-hosted | Local backups + offsite sync |

For managed platforms, also set `ZHIWEI_ADMIN_TOKEN` as an environment variable (not in the data directory) so you can recover console access even if the volume is lost.
