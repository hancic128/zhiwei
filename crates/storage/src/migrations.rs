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

    // Migration 002: 节点主机基本信息（操作系统 / IP / CPU 等），JSON 存储
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

    // Migration 003: 容器与进程快照（低频、只留最新一份，不进时序表）
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

    // Migration 004: 证书快照（随 inventory 一起，只留最新一份）
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

    // Migration 005: 告警规则 / 评估状态 / 告警实例 / 通知渠道
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

            -- 评估状态：每个 (规则, 节点) 一行，用于实现「持续 N 秒才告警」
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

    // Migration 006: 控制平面（签名命令 + 回执 + 审计）
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
                payload_protobuf BLOB NOT NULL,   -- 含签名的完整 Command
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
                actor TEXT NOT NULL,              -- 操作者（admin token / ops-cli）
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

    // Migration 007: 节点身份从 X.509 证书改为 Ed25519 公钥
    let has_007: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 7")
            .fetch_optional(pool)
            .await?;
    if has_007.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE nodes ADD COLUMN public_key TEXT NOT NULL DEFAULT '';
            -- client_cert_pem 保留列以免破坏既有行，但代码不再读写它
            INSERT INTO schema_version (version) VALUES (7);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 008: 服务健康度（服务 / 探针 / 探针状态机 / 探针结果时序）
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
                node_id TEXT,                             -- 归属节点；NULL = 任意节点
                location TEXT NOT NULL DEFAULT 'node',    -- node | monitor（monitor 预留）
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

    // Migration 009: 告警来源（规则 / 服务探针），便于按来源关闭与区分展示
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

    // Migration 010: 证书扫描来源（控制台配置「节点 + 路径」，节点侧拉取后扫描）
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

    // Migration 011: 小时聚合（降采样）
    //
    // 原始 10 秒数据滚动保留（默认 14 天），之后只留小时级聚合。
    // 见 docs/superpowers/specs/2026-09-19-product-structure-design.md §8：
    // 目的不是省磁盘，是让长窗口查询的代价恒定（30 天窗口直接扫原始表要
    // 260 万行起），以及让「自动留存」这件事不需要用户操心。
    //
    // 存 first/last 是为了计数器类指标（网络字节数）还能还原速率：
    // rate = (last - first) / 3600。
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

    Ok(())
}
