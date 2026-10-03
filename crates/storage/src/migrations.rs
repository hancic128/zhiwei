use sqlx::SqlitePool;

pub async fn run(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Migration 001: nodes + telemetry_batches
    let has_001: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 1")
            .fetch_optional(pool)
            .await?;
    if has_001.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE nodes (
                id TEXT PRIMARY KEY,
                hostname TEXT NOT NULL,
                labels_json TEXT NOT NULL DEFAULT '{}',
                client_cert_pem TEXT NOT NULL,
                enrolled_at_unix_nano INTEGER NOT NULL,
                last_seen_unix_nano INTEGER
            );

            CREATE TABLE telemetry_batches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                node_id TEXT NOT NULL,
                ts_unix_nano INTEGER NOT NULL,
                interval_seconds INTEGER NOT NULL,
                payload_protobuf BLOB NOT NULL,
                received_at_unix_nano INTEGER NOT NULL,
                FOREIGN KEY (node_id) REFERENCES nodes(id)
            );

            CREATE INDEX idx_telemetry_node_ts
                ON telemetry_batches(node_id, ts_unix_nano);

            INSERT INTO schema_version (version) VALUES (1);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 002: Node host basic info (OS / IP / CPU, etc.), stored as JSON
    let has_002: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 2")
            .fetch_optional(pool)
            .await?;
    if has_002.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE nodes ADD COLUMN host_info_json TEXT NOT NULL DEFAULT '{}';
            INSERT INTO schema_version (version) VALUES (2);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 003: Container and process snapshots (low-frequency, only keep latest, not in time-series table)
    let has_003: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 3")
            .fetch_optional(pool)
            .await?;
    if has_003.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE node_inventory (
                node_id TEXT PRIMARY KEY,
                ts_unix_nano INTEGER NOT NULL,
                containers_json TEXT NOT NULL DEFAULT '[]',
                processes_json TEXT NOT NULL DEFAULT '[]',
                FOREIGN KEY (node_id) REFERENCES nodes(id)
            );
            INSERT INTO schema_version (version) VALUES (3);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 004: Certificate snapshot (along with inventory, only keep latest)
    let has_004: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 4")
            .fetch_optional(pool)
            .await?;
    if has_004.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE node_inventory ADD COLUMN certificates_json TEXT NOT NULL DEFAULT '[]';
            INSERT INTO schema_version (version) VALUES (4);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 005: Alert rules / evaluation state / alert instances / notify channels
    let has_005: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 5")
            .fetch_optional(pool)
            .await?;
    if has_005.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE alert_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                metric TEXT NOT NULL,
                op TEXT NOT NULL,                 -- gt | gte | lt | lte | eq
                threshold REAL NOT NULL,
                duration_seconds INTEGER NOT NULL DEFAULT 0,
                severity TEXT NOT NULL,           -- warning | critical
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at_unix_nano INTEGER NOT NULL,
                updated_at_unix_nano INTEGER NOT NULL
            );

            -- Evaluation state: one row per (rule, node), used to implement "alert only after N seconds"
            CREATE TABLE alert_state (
                rule_id INTEGER NOT NULL,
                node_id TEXT NOT NULL,
                breaching_since_unix_nano INTEGER,
                firing INTEGER NOT NULL DEFAULT 0,
                open_alert_id INTEGER,
                last_value REAL,
                PRIMARY KEY (rule_id, node_id)
            );

            CREATE TABLE alerts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rule_id INTEGER NOT NULL,
                rule_name TEXT NOT NULL,
                node_id TEXT NOT NULL,
                hostname TEXT NOT NULL,
                severity TEXT NOT NULL,
                metric TEXT NOT NULL,
                op TEXT NOT NULL,
                threshold REAL NOT NULL,
                value REAL NOT NULL,
                message TEXT NOT NULL,
                started_at_unix_nano INTEGER NOT NULL,
                resolved_at_unix_nano INTEGER,
                silenced_until_unix_nano INTEGER
            );
            CREATE INDEX idx_alerts_open ON alerts(resolved_at_unix_nano, started_at_unix_nano);

            CREATE TABLE notify_channels (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,               -- webhook
                url TEXT NOT NULL,
                secret TEXT NOT NULL DEFAULT '',
                enabled INTEGER NOT NULL DEFAULT 1,
                min_severity TEXT NOT NULL DEFAULT 'warning',
                created_at_unix_nano INTEGER NOT NULL
            );

            INSERT INTO schema_version (version) VALUES (5);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 006: Control plane (signed commands + receipts + audit)
    let has_006: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 6")
            .fetch_optional(pool)
            .await?;
    if has_006.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE commands (
                id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                action TEXT NOT NULL,
                params_json TEXT NOT NULL DEFAULT '{}',
                payload_protobuf BLOB NOT NULL,   -- Full Command with signature
                issued_at_unix_nano INTEGER NOT NULL,
                ttl_seconds INTEGER NOT NULL,
                -- pending | delivered | done | failed | expired
                state TEXT NOT NULL DEFAULT 'pending',
                result_ok INTEGER,
                result_error TEXT,
                result_payload BLOB,
                result_received_at_unix_nano INTEGER
            );
            CREATE INDEX idx_commands_pending ON commands(node_id, state);

            CREATE TABLE audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at_unix_nano INTEGER NOT NULL,
                actor TEXT NOT NULL,              -- Operator (admin token / ops-cli)
                node_id TEXT NOT NULL,
                command_id TEXT NOT NULL,
                action TEXT NOT NULL,
                params_json TEXT NOT NULL DEFAULT '{}',
                outcome TEXT NOT NULL DEFAULT 'issued'
            );

            INSERT INTO schema_version (version) VALUES (6);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 007: Node identity changed from X.509 cert to Ed25519 public key
    let has_007: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 7")
            .fetch_optional(pool)
            .await?;
    if has_007.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE nodes ADD COLUMN public_key TEXT NOT NULL DEFAULT '';
            -- client_cert_pem kept to avoid breaking existing rows, but code no longer reads/writes it
            INSERT INTO schema_version (version) VALUES (7);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 008: Service health (services / probes / probe state machine / probe result time series)
    let has_008: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 8")
            .fetch_optional(pool)
            .await?;
    if has_008.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE services (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                description TEXT NOT NULL DEFAULT '',
                group_name TEXT NOT NULL DEFAULT '',
                tier INTEGER NOT NULL DEFAULT 2,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at_unix_nano INTEGER NOT NULL,
                updated_at_unix_nano INTEGER NOT NULL
            );

            CREATE TABLE probes (
                id TEXT PRIMARY KEY,
                service_id TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,                       -- http | tcp | tls
                target_json TEXT NOT NULL,
                expect_json TEXT NOT NULL DEFAULT '{}',
                interval_seconds INTEGER NOT NULL DEFAULT 60,
                timeout_ms INTEGER NOT NULL DEFAULT 5000,
                failure_threshold INTEGER NOT NULL DEFAULT 3,
                node_id TEXT,                             -- Owning node; NULL = any node
                location TEXT NOT NULL DEFAULT 'node',    -- node | monitor (monitor reserved)
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at_unix_nano INTEGER NOT NULL,
                updated_at_unix_nano INTEGER NOT NULL
            );
            CREATE INDEX idx_probes_node ON probes(node_id, enabled);

            CREATE TABLE probe_state (
                probe_id TEXT PRIMARY KEY,
                state TEXT NOT NULL,                      -- ok | degraded | down
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                last_change_at_unix_nano INTEGER NOT NULL,
                last_check_at_unix_nano INTEGER NOT NULL,
                last_latency_ms REAL,
                last_error TEXT NOT NULL DEFAULT '',
                updated_at_unix_nano INTEGER NOT NULL
            );

            CREATE TABLE probe_results (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                probe_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                ts_unix_nano INTEGER NOT NULL,
                state TEXT NOT NULL,
                latency_ms REAL,
                status_code INTEGER,
                error TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX idx_probe_results_probe_ts ON probe_results(probe_id, ts_unix_nano);

            INSERT INTO schema_version (version) VALUES (8);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 009: Alert sources (rules / service probes), for closing by source and display differentiation
    let has_009: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 9")
            .fetch_optional(pool)
            .await?;
    if has_009.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE alerts ADD COLUMN source TEXT NOT NULL DEFAULT 'rule';
            ALTER TABLE alerts ADD COLUMN source_ref TEXT NOT NULL DEFAULT '';
            CREATE INDEX idx_alerts_probe ON alerts(source, source_ref, resolved_at_unix_nano);
            INSERT INTO schema_version (version) VALUES (9);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 010: Certificate scan sources (console config "node + path", node pulls and scans)
    let has_010: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 10")
            .fetch_optional(pool)
            .await?;
    if has_010.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE cert_sources (
                id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                path TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                notify_enabled INTEGER NOT NULL DEFAULT 1,
                notify_days_before INTEGER NOT NULL DEFAULT 30,
                created_at_unix_nano INTEGER NOT NULL,
                updated_at_unix_nano INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX idx_cert_sources_node_path ON cert_sources(node_id, path);
            INSERT INTO schema_version (version) VALUES (10);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 011: Hourly aggregation (downsample)
    //
    // Raw 10-second data kept for rolling period (default 14 days), after that only hourly aggregates.
    // See docs/superpowers/specs/2026-09-19-product-structure-design.md §8:
    // Purpose is not to save disk, but to keep long-window query cost constant (30-day window
    // scanning raw table starts at 2.6 million rows) and to make "auto retention" not require user intervention.
    //
    // Stores first/last to enable rate recovery for counter-type metrics (network bytes):
    // rate = (last - first) / 3600.
    let has_011: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 11")
            .fetch_optional(pool)
            .await?;
    if has_011.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE telemetry_hourly (
                node_id TEXT NOT NULL,
                ts_hour_unix_nano INTEGER NOT NULL,
                metric TEXT NOT NULL,
                avg REAL NOT NULL,
                min REAL NOT NULL,
                max REAL NOT NULL,
                first REAL NOT NULL,
                last REAL NOT NULL,
                samples INTEGER NOT NULL,
                PRIMARY KEY (node_id, ts_hour_unix_nano, metric)
            );
            CREATE INDEX idx_telemetry_hourly_node_metric
                ON telemetry_hourly(node_id, metric, ts_hour_unix_nano);
            INSERT INTO schema_version (version) VALUES (11);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 012: AI token (credentials for MCP/external AI clients to read endpoints)
    //
    // Design:
    // - id format `ait_<12 hex>`, for display only, not involved in verification
    // - token_hash is SHA-256(plaintext token) lowercase hex, verified by computing hash and comparing
    // - revoked_at non-null means invalid; no expiration this time
    // - Unique index only covers non-revoked, allowing "same hash, revoked, then recreated" as valid
    let has_012: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 12")
            .fetch_optional(pool)
            .await?;
    if has_012.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE ai_tokens (
                id TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                created_at_unix_nano INTEGER NOT NULL,
                last_used_at_unix_nano INTEGER,
                revoked_at_unix_nano INTEGER
            );
            CREATE INDEX idx_ai_tokens_active
                ON ai_tokens(token_hash) WHERE revoked_at_unix_nano IS NULL;
            INSERT INTO schema_version (version) VALUES (12);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 013: Node alias + tags
    //
    // alias is admin-provided short alias (≤10 chars), hostname is self-reported by node and may be long,
    // list and dropdown prefer showing alias. tags_json is JSON array (≤10 items), used for filtering.
    // Both maintained by admin, node reporting does not overwrite.
    let has_013: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 13")
            .fetch_optional(pool)
            .await?;
    if has_013.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE nodes ADD COLUMN alias TEXT NOT NULL DEFAULT '';
            ALTER TABLE nodes ADD COLUMN tags_json TEXT NOT NULL DEFAULT '[]';
            INSERT INTO schema_version (version) VALUES (13);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 014: Index for time-based deletion in retention
    //
    // Retention deletes "rows earlier than a time point" (`ts_unix_nano < ?`), but the index
    // created by 001 is (node_id, ts_unix_nano) -- first column is not time, SQLite does full table scan.
    // Symptom observed: `INSERT INTO telemetry_batches ... elapsed=2.88s` (write lock held by that
    // DELETE), container operations in console "clicked but no response".
    //
    // Note: On large databases, this CREATE INDEX scans the full table on first upgrade, one-time cost;
    // after that, deletions and queries with time conditions use the index.
    let has_014: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 14")
            .fetch_optional(pool)
            .await?;
    if has_014.is_none() {
        sqlx::query(
            r#"
            CREATE INDEX idx_telemetry_ts ON telemetry_batches(ts_unix_nano);
            CREATE INDEX idx_telemetry_hourly_ts ON telemetry_hourly(ts_hour_unix_nano);
            INSERT INTO schema_version (version) VALUES (14);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 015: Probes bind **multiple** nodes
    //
    // Original probes.node_id could only bind one node (NULL = any node). After console changed to multi-select,
    // use node_ids_json to store node id array: empty array = any node (continues NULL semantics,
    // old data doesn't change behavior), non-empty means only execute on these nodes.
    //
    // Old data first expands single node into array, then drops old column and its index --
    // keeping two sources of truth will eventually cause someone to write the wrong one.
    // `json_array` depends on SQLite's JSON1 (built-in from 3.38, sqlx's bundled sqlite is 3.46).
    let has_015: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 15")
            .fetch_optional(pool)
            .await?;
    if has_015.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE probes ADD COLUMN node_ids_json TEXT NOT NULL DEFAULT '[]';
            UPDATE probes SET node_ids_json = json_array(node_id)
                WHERE node_id IS NOT NULL AND trim(node_id) <> '';
            DROP INDEX IF EXISTS idx_probes_node;
            ALTER TABLE probes DROP COLUMN node_id;
            INSERT INTO schema_version (version) VALUES (15);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 016: Consolidate notify channels to "Feishu / Slack / generic webhook"
    //
    // Feishu no longer uses "custom robot webhook", switched to official app API: App ID + App Secret exchange
    // tenant_access_token, then send messages by receive_id -- so three more fields needed.
    // `secret` column reused by type: Feishu = App Secret, generic webhook = delivery Token (Bearer);
    // `url` only used by Slack/generic webhook, Feishu address determined by receive_id.
    //
    // Dingtalk channel discontinued: keeping those rows only creates zombie configs that can't be
    // changed or sent (channel list can't display type name), just delete them.
    //
    // Old Feishu "custom robot" channels only have url, no App ID, can't go through app API --
    // disable them so console shows at a glance these need rebuilding, instead of silently not sending alerts.
    //
    // Numbers continue from 015 (same-number migration is silently skipped by `has_0xx.is_none()`).
    let has_016: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 16")
            .fetch_optional(pool)
            .await?;
    if has_016.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE notify_channels ADD COLUMN app_id TEXT NOT NULL DEFAULT '';
            ALTER TABLE notify_channels ADD COLUMN receive_id TEXT NOT NULL DEFAULT '';
            ALTER TABLE notify_channels ADD COLUMN receive_id_type TEXT NOT NULL DEFAULT 'chat_id';
            UPDATE notify_channels SET enabled = 0 WHERE kind = 'feishu' AND app_id = '';
            DELETE FROM notify_channels WHERE kind = 'dingtalk';
            INSERT INTO schema_version (version) VALUES (16);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 017: Builtin alert rules (node online/offline)
    //
    // These two event types don't go through alert_rules (not based on metric thresholds), but need
    // unified "enable/disable" control. id is a stable string ('node_offline' / 'node_online'), used as
    // primary key for subsequent notifications and status queries. enabled defaults to 1, INSERT OR IGNORE
    // on old database upgrades, existing config not overwritten.
    let has_017: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 17")
            .fetch_optional(pool)
            .await?;
    if has_017.is_none() {
        sqlx::query(
            r#"
            CREATE TABLE builtin_alert_rules (
                id          TEXT PRIMARY KEY,         -- 'node_offline' / 'node_online'
                name        TEXT NOT NULL,
                enabled     INTEGER NOT NULL DEFAULT 1,
                updated_at_unix_nano INTEGER NOT NULL
            );

            INSERT OR IGNORE INTO builtin_alert_rules (id, name, enabled, updated_at_unix_nano)
                VALUES ('node_offline', 'Node Offline', 1, 0),
                       ('node_online',  'Node Online', 1, 0);

            INSERT INTO schema_version (version) VALUES (17);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 018: Cron job snapshot
    //
    // Alongside containers/processes/certs, also "current state" not time-series: node scans all
    // users' crontabs every 5 minutes, server keeps only latest. Stored in separate column instead of
    // stuffing into processes_json because list endpoints need per-job filtering/writing, column format
    // is completely different. Default '[]': after old database upgrade, before node's first report,
    // list endpoint parses empty array instead of error.
    let has_018: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 18")
            .fetch_optional(pool)
            .await?;
    if has_018.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE node_inventory ADD COLUMN cron_jobs_json TEXT NOT NULL DEFAULT '[]';
            INSERT INTO schema_version (version) VALUES (18);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 019: Expand builtin alerts (service probe online/offline / container start/stop / cert expiry)
    //
    // Service probes and cert expiry already had their own alert paths (source = probe / cert),
    // missing was the unified toggle on the "builtin alerts" page; container start/stop is a new event source.
    // Six rules with enabled = 1 by default: for old databases, behavior unchanged (probes/certs already
    // alerted), container events are new capability, users disable in "Alerts -> Builtin Alerts" to opt out.
    // INSERT OR IGNORE ensures old database upgrades don't overwrite existing config.
    let has_019: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 19")
            .fetch_optional(pool)
            .await?;
    if has_019.is_none() {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO builtin_alert_rules (id, name, enabled, updated_at_unix_nano)
                VALUES ('service_offline',  'Service Down', 1, 0),
                       ('service_online',   'Service Up', 1, 0),
                       ('container_started','Container Started', 1, 0),
                       ('container_stopped','Container Stopped', 1, 0),
                       ('cert_expiring',    'Certificate Expiring', 1, 0),
                       ('cert_expired',     'Certificate Expired', 1, 0);
            INSERT INTO schema_version (version) VALUES (19);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 019b: Migrate Chinese rule names to English (for old databases).
    // The LIKE patterns below match historical Chinese-builtin names that early
    // installations may still carry in their SQLite database. The migration is a
    // no-op for fresh installs.
    let has_019b: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 19")
            .fetch_optional(pool)
            .await?;
    if has_019b.is_some() {
        // Check if any rules still have Chinese names
        let count: Option<i64> = sqlx::query_scalar(
            "SELECT COUNT(*) FROM builtin_alert_rules WHERE name LIKE '%节点%' OR name LIKE '%离线%' OR name LIKE '%上线%' OR name LIKE '%证书%' OR name LIKE '%服务%' OR name LIKE '%容器%'"
        )
        .fetch_optional(pool)
        .await?;
        if count.unwrap_or(0) > 0 {
            sqlx::query(
                r#"
                UPDATE builtin_alert_rules SET name = 'Node Offline' WHERE id = 'node_offline';
                UPDATE builtin_alert_rules SET name = 'Node Online' WHERE id = 'node_online';
                UPDATE builtin_alert_rules SET name = 'Service Down' WHERE id = 'service_offline';
                UPDATE builtin_alert_rules SET name = 'Service Up' WHERE id = 'service_online';
                UPDATE builtin_alert_rules SET name = 'Container Started' WHERE id = 'container_started';
                UPDATE builtin_alert_rules SET name = 'Container Stopped' WHERE id = 'container_stopped';
                UPDATE builtin_alert_rules SET name = 'Certificate Expiring' WHERE id = 'cert_expiring';
                UPDATE builtin_alert_rules SET name = 'Certificate Expired' WHERE id = 'cert_expired';
                "#,
            )
            .execute(pool)
            .await?;
        }
    }

    // Migration 020: Add threshold and duration_seconds columns to builtin_alert_rules
    let has_020: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 20")
            .fetch_optional(pool)
            .await?;
    if has_020.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE builtin_alert_rules ADD COLUMN threshold REAL NOT NULL DEFAULT 0;
            ALTER TABLE builtin_alert_rules ADD COLUMN duration_seconds INTEGER NOT NULL DEFAULT 300;
            -- Set appropriate defaults for specific alert types
            UPDATE builtin_alert_rules SET threshold = 60 WHERE id = 'node_offline';
            UPDATE builtin_alert_rules SET threshold = 30 WHERE id = 'cert_expiring';
            UPDATE builtin_alert_rules SET duration_seconds = 300 WHERE duration_seconds = 0;
            INSERT INTO schema_version (version) VALUES (20);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 021: Add CPU/memory/disk metric alert rules
    let has_021: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 21")
            .fetch_optional(pool)
            .await?;
    if has_021.is_none() {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO builtin_alert_rules (id, name, enabled, threshold, duration_seconds, updated_at_unix_nano)
                VALUES ('cpu_high',  'CPU Usage High',  1, 80.0, 300, 0),
                       ('mem_high',  'Memory Usage High', 1, 85.0, 300, 0),
                       ('disk_high',  'Disk Usage High', 1, 90.0, 300, 0);
            INSERT INTO schema_version (version) VALUES (21);
            "#,
        )
        .execute(pool)
        .await?;
    }

    Ok(())
}
