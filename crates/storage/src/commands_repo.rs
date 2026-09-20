use sqlx::SqlitePool;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandRow {
    pub id: String,
    pub node_id: String,
    pub action: String,
    pub params_json: String,
    pub issued_at_unix_nano: i64,
    pub ttl_seconds: i64,
    pub state: String,
    pub result_ok: Option<bool>,
    pub result_error: Option<String>,
    pub result_payload: Option<Vec<u8>>,
    pub result_received_at_unix_nano: Option<i64>,
}

#[derive(Clone)]
pub struct CommandsRepo {
    pool: SqlitePool,
}

type Row = (
    String,
    String,
    String,
    String,
    i64,
    i64,
    String,
    Option<i64>,
    Option<String>,
    Option<Vec<u8>>,
    Option<i64>,
);

impl CommandsRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// ops-server 签发命令时写入（payload 是含签名的完整 Command protobuf）
    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
        &self,
        id: &str,
        node_id: &str,
        action: &str,
        params_json: &str,
        payload: &[u8],
        issued_at_unix_nano: i64,
        ttl_seconds: i64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO commands
               (id, node_id, action, params_json, payload_protobuf, issued_at_unix_nano, ttl_seconds, state)
               VALUES (?, ?, ?, ?, ?, ?, ?, 'pending')"#,
        )
        .bind(id)
        .bind(node_id)
        .bind(action)
        .bind(params_json)
        .bind(payload)
        .bind(issued_at_unix_nano)
        .bind(ttl_seconds)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn audit(
        &self,
        at_unix_nano: i64,
        actor: &str,
        node_id: &str,
        command_id: &str,
        action: &str,
        params_json: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO audit_log (at_unix_nano, actor, node_id, command_id, action, params_json)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(at_unix_nano)
        .bind(actor)
        .bind(node_id)
        .bind(command_id)
        .bind(action)
        .bind(params_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 节点拉取待执行命令（含原始 protobuf，节点自行验签）
    pub async fn pending_for(
        &self,
        node_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT id, payload_protobuf FROM commands WHERE node_id = ? AND state = 'pending' ORDER BY issued_at_unix_nano LIMIT ?",
        )
        .bind(node_id)
        .bind(limit.clamp(1, 50))
        .fetch_all(&self.pool)
        .await?;

        if !rows.is_empty() {
            let ids: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
            // 标记为已投递，避免重复下发
            for id in ids {
                let _ = sqlx::query(
                    "UPDATE commands SET state = 'delivered' WHERE id = ? AND state = 'pending'",
                )
                .bind(id)
                .execute(&self.pool)
                .await;
            }
        }
        Ok(rows)
    }

    /// 命令归属的节点 id。用于校验回执提交者与命令主人一致。
    pub async fn owner_of(&self, id: &str) -> anyhow::Result<Option<String>> {
        let owner: Option<String> = sqlx::query_scalar("SELECT node_id FROM commands WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(owner)
    }

    pub async fn submit_result(
        &self,
        id: &str,
        ok: bool,
        error: &str,
        payload: Option<&[u8]>,
        now: i64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"UPDATE commands SET
                 state = ?, result_ok = ?, result_error = ?, result_payload = ?,
                 result_received_at_unix_nano = ?
               WHERE id = ?"#,
        )
        .bind(if ok { "done" } else { "failed" })
        .bind(if ok { 1 } else { 0 })
        .bind(error)
        .bind(payload)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        sqlx::query("UPDATE audit_log SET outcome = ? WHERE command_id = ?")
            .bind(if ok { "done" } else { "failed" })
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn find(&self, id: &str) -> anyhow::Result<Option<CommandRow>> {
        let row: Option<Row> = sqlx::query_as(
            r#"SELECT id, node_id, action, params_json, issued_at_unix_nano, ttl_seconds, state,
                      result_ok, result_error, result_payload, result_received_at_unix_nano
               FROM commands WHERE id = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(map_row))
    }

    pub async fn recent(&self, limit: i64) -> anyhow::Result<Vec<CommandRow>> {
        let rows: Vec<Row> = sqlx::query_as(
            r#"SELECT id, node_id, action, params_json, issued_at_unix_nano, ttl_seconds, state,
                      result_ok, result_error, result_payload, result_received_at_unix_nano
               FROM commands ORDER BY issued_at_unix_nano DESC LIMIT ?"#,
        )
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(map_row).collect())
    }
}

fn map_row(r: Row) -> CommandRow {
    CommandRow {
        id: r.0,
        node_id: r.1,
        action: r.2,
        params_json: r.3,
        issued_at_unix_nano: r.4,
        ttl_seconds: r.5,
        state: r.6,
        result_ok: r.7.map(|v| v != 0),
        result_error: r.8,
        result_payload: r.9,
        result_received_at_unix_nano: r.10,
    }
}
