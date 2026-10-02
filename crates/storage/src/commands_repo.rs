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
        self.audit_with_outcome(
            at_unix_nano,
            actor,
            node_id,
            command_id,
            action,
            params_json,
            "issued",
        )
        .await
    }

    /// 同 [`Self::audit`]，但显式指定 outcome（`issued` / `done` / `failed` / `cancelled`）。
    /// 「作废未发出的命令再删节点」要留痕，用的就是这个。
    #[allow(clippy::too_many_arguments)]
    pub async fn audit_with_outcome(
        &self,
        at_unix_nano: i64,
        actor: &str,
        node_id: &str,
        command_id: &str,
        action: &str,
        params_json: &str,
        outcome: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO audit_log
               (at_unix_nano, actor, node_id, command_id, action, params_json, outcome)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(at_unix_nano)
        .bind(actor)
        .bind(node_id)
        .bind(command_id)
        .bind(action)
        .bind(params_json)
        .bind(outcome)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 某节点「还在路上且仍然有效」的 pending 条数。
    ///
    /// 只数仍然有效的：过了 TTL 的命令节点侧一律拒收（见 [`PENDING_LIVE_SQL`]），
    /// 拿它们挡住删除只会把节点永久锁死。
    pub async fn count_pending_for_node(&self, node_id: &str, now_ns: i64) -> anyhow::Result<i64> {
        let n: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM commands WHERE node_id = ? AND state = 'pending' AND {PENDING_LIVE_SQL}"
    ))
    .bind(node_id)
    .bind(now_ns)
    .fetch_one(&self.pool)
    .await?;
        Ok(n)
    }

    /// 作废某节点全部未发出的命令（`pending` → `expired`），返回被作废的行——
    /// 调用方要拿它们写 audit_log（节点删掉后命令历史会跟着没了，审计得单独留）。
    ///
    /// 与 [`expire_overdue_for_node`] 的区别：这里不看 TTL，是「运维明确要求作废」。
    pub async fn cancel_pending_for_node(&self, node_id: &str) -> anyhow::Result<Vec<CommandRow>> {
        let rows = sqlx::query_as::<_, Row>(
            r#"SELECT id, node_id, action, params_json, issued_at_unix_nano, ttl_seconds, state,
                  result_ok, result_error, result_payload, result_received_at_unix_nano
           FROM commands WHERE node_id = ? AND state = 'pending' ORDER BY issued_at_unix_nano"#,
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(Vec::new());
        }

        let mut cancelled = Vec::with_capacity(rows.len());
        for row in rows {
            let r = sqlx::query(
                "UPDATE commands SET state = 'expired' WHERE id = ? AND state = 'pending'",
            )
            .bind(&row.0)
            .execute(&self.pool)
            .await?;
            // 并发里被节点拉走了（pending → delivered）就跳过：那条已经不是「未发出」了
            if r.rows_affected() > 0 {
                cancelled.push(map_row(row));
            }
        }
        Ok(cancelled)
    }

    /// 节点拉取待执行命令（含原始 protobuf，节点自行验签）。
    ///
    /// 先清掉本节点已过 TTL 的 pending（节点拿到也会拒收，见 [`PENDING_LIVE_SQL`]），
    /// 再取仍然有效的，最后标记为已投递避免重复下发。
    pub async fn pending_for(
        &self,
        node_id: &str,
        limit: i64,
        now_ns: i64,
    ) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let _ = expire_overdue_for_node(&self.pool, node_id, now_ns).await;

        let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(&format!(
            "SELECT id, payload_protobuf FROM commands
              WHERE node_id = ? AND state = 'pending' AND {PENDING_LIVE_SQL}
              ORDER BY issued_at_unix_nano LIMIT ?"
        ))
        .bind(node_id)
        .bind(now_ns)
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

/// 「这条 pending 命令还有效吗」——判断必须与节点侧逐字一致
/// （`node-agent/src/control.rs::verify`）：`ttl_seconds <= 0` 视为不过期，
/// 否则签发时刻 + TTL 之后节点一律拒收（默认 TTL 只有 60s，见 ops-server `--ttl`）。
///
/// 为什么 monitor 也要判一次：原先它完全不看 TTL，只要有过命令没被拉走，
/// 节点就永久卡在 `DELETE /v1/nodes/:id` 的前置检查上；而「等节点拉完」在节点
/// 重装过（`--reinstall` 换掉 node_id）或命令通道坏掉时根本不可能发生。
const PENDING_LIVE_SQL: &str =
    "(ttl_seconds <= 0 OR issued_at_unix_nano + ttl_seconds * 1000000000 > ?)";

/// 把某节点「已过 TTL 却还挂着 pending」的行落成 `expired`。
///
/// 做在拉取路径上：节点来拉命令的那一刻，正是判断「还有效吗」的唯一时机。
/// 不这么做这些行会永远停在 pending——节点早就不会再拉，状态也就再也改不了。
async fn expire_overdue_for_node(
    pool: &SqlitePool,
    node_id: &str,
    now_ns: i64,
) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "UPDATE commands SET state = 'expired'
          WHERE node_id = ? AND state = 'pending' AND ttl_seconds > 0
            AND issued_at_unix_nano + ttl_seconds * 1000000000 <= ?",
    )
    .bind(node_id)
    .bind(now_ns)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Storage;

    const NOW: i64 = 1_700_000_000_000_000_000;
    const SEC: i64 = 1_000_000_000;

    /// 每个测试一个独立库文件——同一进程里并行跑，文件名必须区分开
    /// （这个仓库没有 tempfile，跟 admin.rs 的测试一样用 temp_dir + pid）。
    async fn repo(tag: &str) -> CommandsRepo {
        let dir =
            std::env::temp_dir().join(format!("zhiwei-commands-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Storage::open(dir.join("t.db")).await.unwrap().commands()
    }

    async fn issue(r: &CommandsRepo, id: &str, node: &str, age_secs: i64, ttl: i64) {
        r.insert(
            id,
            node,
            "refresh_inventory",
            "{}",
            format!("payload-{id}").as_bytes(),
            NOW - age_secs * SEC,
            ttl,
        )
        .await
        .unwrap();
    }

    async fn state_of(r: &CommandsRepo, id: &str) -> String {
        r.find(id).await.unwrap().unwrap().state
    }

    /// 过了 TTL 的命令节点侧一律拒收（control.rs::verify），所以在「还有没有
    /// 未发出的命令」这件事上也不该再算它们——否则节点永久删不掉。
    #[tokio::test]
    async fn pending_count_skips_commands_whose_ttl_already_passed() {
        let r = repo("count").await;
        issue(&r, "live", "n1", 30, 60).await;
        issue(&r, "stale", "n1", 61, 60).await;
        issue(&r, "no-ttl", "n1", 86_400, 0).await;
        issue(&r, "other-node", "n2", 30, 60).await;

        assert_eq!(r.count_pending_for_node("n1", NOW).await.unwrap(), 2);
        assert_eq!(r.count_pending_for_node("n2", NOW).await.unwrap(), 1);
    }

    /// 拉取时不该把节点注定拒收的命令塞给它；顺手把这些行落成 expired，
    /// 否则它们会永远停在 pending（节点早就不会再拉，状态再也改不了）。
    #[tokio::test]
    async fn pending_for_skips_and_expires_overdue_ones() {
        let r = repo("pending").await;
        issue(&r, "live", "n1", 30, 60).await;
        issue(&r, "stale", "n1", 61, 60).await;
        // 已投递过的过期命令不该被这次清理碰到
        issue(&r, "delivered", "n1", 86_400, 60).await;
        r.pending_for("n1", 1, NOW - 86_400 * SEC).await.unwrap();

        let got = r.pending_for("n1", 10, NOW).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "live");
        assert_eq!(got[0].1, b"payload-live".to_vec());

        assert_eq!(state_of(&r, "stale").await, "expired");
        assert_eq!(state_of(&r, "delivered").await, "delivered");
        // 拉走的那条标成已投递，不会被重复下发
        assert!(r.pending_for("n1", 10, NOW).await.unwrap().is_empty());
        assert_eq!(state_of(&r, "live").await, "delivered");
    }

    /// 强制删除前的作废：不看 TTL，把所有还没发出的都作废并返回，供调用方写审计。
    #[tokio::test]
    async fn cancel_pending_for_node_expires_every_pending_row() {
        let r = repo("cancel").await;
        // 先造一条「已投递」的：拉走之后就不再是「未发出」了
        issue(&r, "delivered", "n1", 30, 60).await;
        r.pending_for("n1", 1, NOW).await.unwrap();
        // 再补一条过期的 pending：它不会再被拉取路径扫到（节点不会来拉了）
        issue(&r, "overdue", "n1", 86_400, 60).await;
        issue(&r, "live", "n1", 30, 60).await;
        issue(&r, "other", "n2", 30, 60).await;

        let cancelled = r.cancel_pending_for_node("n1").await.unwrap();
        let mut ids: Vec<&str> = cancelled.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["live", "overdue"]);
        assert!(cancelled.iter().all(|c| c.action == "refresh_inventory"));

        assert_eq!(state_of(&r, "live").await, "expired");
        assert_eq!(state_of(&r, "overdue").await, "expired");
        // 已投递的不受影响（它已经不是「未发出」），别人的命令也不动
        assert_eq!(state_of(&r, "delivered").await, "delivered");
        assert_eq!(r.count_pending_for_node("n2", NOW).await.unwrap(), 1);
        // 幂等：再作废一次没有可作废的行
        assert!(r.cancel_pending_for_node("n1").await.unwrap().is_empty());
    }

    /// 作废要留痕：audit_log 里 outcome=cancelled，且不影响既有的 issued 行。
    #[tokio::test]
    async fn cancelled_commands_are_audited_with_outcome() {
        let r = repo("audit").await;
        issue(&r, "c1", "n1", 30, 60).await;
        r.audit(NOW, "console", "n1", "c1", "refresh_inventory", "{}")
            .await
            .unwrap();
        r.audit_with_outcome(
            NOW,
            "admin",
            "n1",
            "c1",
            "refresh_inventory",
            "{}",
            "cancelled",
        )
        .await
        .unwrap();

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT actor, outcome FROM audit_log WHERE command_id = ? ORDER BY id")
                .bind("c1")
                .fetch_all(r_pool(&r).await)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("console".to_string(), "issued".to_string()),
                ("admin".to_string(), "cancelled".to_string()),
            ]
        );
    }

    /// 测试里要直接查 audit_log：借 repo 的池子（同 crate，直接拿 pool）
    async fn r_pool(r: &CommandsRepo) -> &SqlitePool {
        &r.pool
    }
}
