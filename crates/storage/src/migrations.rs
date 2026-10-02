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

    // Migration 012: AI token（MCP / 外部 AI 客户端用的读端点凭据）
    //
    // 设计：
    // - id 形如 `ait_<12 hex>`，展示用，不参与校验
    // - token_hash 是 SHA-256(明文 token) 的小写 hex，校验时算一遍再比
    // - revoked_at 非空即失效；本次不做过期
    // - 唯一索引只覆盖未撤销的，让「同 hash 撤销后重建」合法
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

    // Migration 013: 节点别名 + 标签
    //
    // alias 是管理员给的简短别称（≤10 字符），hostname 由节点自报、可能很长，
    // 列表与下拉框优先显示 alias。tags_json 是 JSON 数组（≤10 个），用于过滤。
    // 两者都由管理员维护，节点上报不会覆盖。
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

    // Migration 014: 留存按时间删除要用的索引
    //
    // 留存删的是「比某个时间点更早的行」（`ts_unix_nano < ?`），而 001 建的索引
    // 是 (node_id, ts_unix_nano)——首列不是时间，SQLite 只能全表扫描。
    // 现场症状：`INSERT INTO telemetry_batches ... elapsed=2.88s`（写锁被那条
    // DELETE 占住），连带控制台的容器操作「点了没反应」。
    //
    // 注意：大库首次升级时这条 CREATE INDEX 会扫一遍全表，属一次性开销；
    // 之后带时间条件的删除与查询才走得上索引。
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

    // Migration 015: 探针绑定**多个**节点
    //
    // 原 probes.node_id 只能绑一个节点（NULL = 任意节点）。控制台改成多选后，
    // 用 node_ids_json 存节点 id 数组：空数组 = 任意节点（沿用 NULL 的语义，
    // 老数据不用改行为），非空则只在这些节点上执行。
    //
    // 老数据先按单节点展开成数组，再把旧列连同它的索引删掉——留着两份真相迟早
    // 会有人写错一份。`json_array` 依赖 SQLite 的 JSON1（3.38 起内置，sqlx 自带
    // 的 bundled sqlite 是 3.46）。
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

    // Migration 016: 通知渠道收敛为「飞书 / Slack / 通用 webhook」
    //
    // 飞书不再用「自定义机器人 webhook」，改走官方应用接口：App ID + App Secret 换
    // tenant_access_token，再按 receive_id 发消息——所以要多存三个字段。
    // `secret` 一列按类型复用：飞书 = App Secret，通用 webhook = 投递 Token（Bearer）；
    // `url` 只给 Slack / 通用 webhook 用，飞书的地址由 receive_id 决定。
    //
    // 钉钉渠道下线：留着这些行只会变成改不了、也发不出去的僵尸配置（渠道列表里
    // 还显示不出类型名），直接删掉。
    //
    // 老的飞书「自定义机器人」渠道只有 url、没有 App ID，走不通应用接口——把它们
    // 置为停用，让控制台里一眼看出这条要重建，而不是留着一条悄悄不发告警的记录。
    //
    // 号段接着 015 往下取（同号的迁移会被 `has_0xx.is_none()` 静默跳过）。
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

    // Migration 017: 内置告警规则（节点上下线）
    //
    // 这两类事件不走 alert_rules（不是基于指标阈值），但要受统一的「启用 / 停用」
    // 控制。id 是稳定字符串（'node_offline' / 'node_online'），后续发通知与查
    // 状态都用它做主键。enabled 默认 1，老库升级时直接 INSERT OR IGNORE，已有
    // 的配置不会被覆盖。
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
                VALUES ('node_offline', '节点离线', 1, 0),
                       ('node_online',  '节点上线', 1, 0);

            INSERT INTO schema_version (version) VALUES (17);
            "#,
        )
        .execute(pool)
        .await?;
    }

    // Migration 018: cron 任务快照
    //
    // 与容器 / 进程 / 证书并列，同样属于「当前状态」而非时序：节点每 5 分钟扫一次
    // 本机所有用户的 crontab，服务端只留最新一份。单独存一列而不是塞进
    // processes_json，是因为列表端点要按任务逐条过滤 / 回写，字段形态完全不同。
    // 默认 '[]'：老库升级后、节点第一次上报前，列表端点解析出空数组而不是报错。
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

    // Migration 019: 扩充内置告警（服务探针上下线 / 容器启停 / 证书到期）
    //
    // 服务探针与证书到期在此之前已有各自的告警链路（source = probe / cert），
    // 缺的只是「内置告警」页上的统一开关；容器启停是新增事件源。六条默认
    // enabled = 1：对老库而言行为不变（探针 / 证书原来就告警），容器事件是
    // 新能力，用户要在「告警 → 内置告警」页停用才不推。INSERT OR IGNORE
    // 保证老库升级不覆盖已有配置。
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

    // Migration 019b: migrate existing Chinese rule names to English and set default threshold/duration
    let has_019b_done: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 19")
            .fetch_optional(pool)
            .await?;
    if has_019b_done.is_some() {
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
        // Set default threshold for alerts that need it (node_offline: 60s, cert_expiring: 30 days)
        let count: Option<i64> = sqlx::query_scalar(
            "SELECT COUNT(*) FROM builtin_alert_rules WHERE threshold = 0 AND (id = 'node_offline' OR id = 'cert_expiring')"
        )
        .fetch_optional(pool)
        .await?;
        if count.unwrap_or(0) > 0 {
            sqlx::query(
                r#"
                UPDATE builtin_alert_rules SET threshold = 60 WHERE id = 'node_offline';
                UPDATE builtin_alert_rules SET threshold = 30 WHERE id = 'cert_expiring';
                UPDATE builtin_alert_rules SET duration_seconds = 300 WHERE duration_seconds = 0;
                "#,
            )
            .execute(pool)
            .await?;
        }
    }

    // Migration 020: builtin_alert_rules add editable threshold and duration
    //
    // Builtin alerts now support customizing threshold and duration per alert.
    let has_020: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version WHERE version = 20")
            .fetch_optional(pool)
            .await?;
    if has_020.is_none() {
        sqlx::query(
            r#"
            ALTER TABLE builtin_alert_rules ADD COLUMN threshold REAL NOT NULL DEFAULT 0;
            ALTER TABLE builtin_alert_rules ADD COLUMN duration_seconds INTEGER NOT NULL DEFAULT 300;
            INSERT INTO schema_version (version) VALUES (20);
            "#,
        )
        .execute(pool)
        .await?;
    }

    Ok(())
}
