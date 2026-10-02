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

    /// Insert when ops-server issues a command (payload is the full Command protobuf with signature)
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

    /// Same as [`Self::audit`], but with explicit outcome (`issued` / `done` / `failed` / `cancelled`).
    /// Used when "invalidating unsent commands and deleting a node" needs to leave a trace.
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

    /// Number of pending commands for a node that are "still in transit and still valid".
    ///
    /// Only counts still-valid ones: commands past TTL are always rejected by the node side
    /// (see [`PENDING_LIVE_SQL`]). Including them would permanently lock the node from deletion.
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

    /// Invalidate all unsent commands for a node (`pending` -> `expired`), returns invalidated rows --
    /// caller needs them to write to audit_log (after node deletion, command history disappears,
    /// audit needs to be stored separately).
    ///
    /// Difference from [`expire_overdue_for_node`]: this ignores TTL, it's "ops explicitly requested invalidation".
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
            // Skip if already pulled by node in concurrent scenario (pending -> delivered means it's no longer "unsent")
            if r.rows_affected() > 0 {
                cancelled.push(map_row(row));
            }
        }
        Ok(cancelled)
    }

    /// Node pulls pending commands to execute (includes raw protobuf, node verifies signature itself).
    ///
    /// First clears this node's overdue pending commands (node would reject them anyway, see [`PENDING_LIVE_SQL`]),
    /// then gets the still-valid ones, finally marks them as delivered to avoid duplicate dispatch.
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
            // Mark as delivered to avoid duplicate dispatch
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

    /// Node ID that owns this command. Used to verify receipt submitter matches command owner.
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

/// "Is this pending command still valid" -- logic must match node side verbatim
/// (`node-agent/src/control.rs::verify`): `ttl_seconds <= 0` means never expires,
/// otherwise node always rejects after issued_at + TTL (default TTL is 60s, see ops-server `--ttl`).
///
/// Why monitor also checks: previously it completely ignored TTL, if any command wasn't pulled,
/// the node would be permanently stuck on `DELETE /v1/nodes/:id` precheck; and "wait for node to pull"
/// is impossible when node was reinstalled (`--reinstall` changes node_id) or command channel is broken.
const PENDING_LIVE_SQL: &str =
    "(ttl_seconds <= 0 OR issued_at_unix_nano + ttl_seconds * 1000000000 > ?)";

/// Mark rows "past TTL but still stuck on pending" as `expired`.
///
/// Done on the pull path: the moment the node comes to pull commands is the only time
/// we can judge "is it still valid?". Without this, these rows would stay on pending forever --
/// the node won't pull again, so the state can never change.
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

    /// Each test gets an independent database file -- parallel tests in same process,
    /// filenames must be distinct (this repo has no tempfile, uses temp_dir + pid like admin.rs tests).
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

    /// Commands past TTL are always rejected by the node side (control.rs::verify), so they
    /// shouldn't be counted as "unsent commands" either -- otherwise the node can never be deleted.
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

    /// Pull should not give the node commands it will definitely reject; also expire those rows
    /// so they don't stay on pending forever (node won't pull again, state can never change).
    #[tokio::test]
    async fn pending_for_skips_and_expires_overdue_ones() {
        let r = repo("pending").await;
        issue(&r, "live", "n1", 30, 60).await;
        issue(&r, "stale", "n1", 61, 60).await;
        // Already-delivered overdue command should not be affected by this cleanup
        issue(&r, "delivered", "n1", 86_400, 60).await;
        r.pending_for("n1", 1, NOW - 86_400 * SEC).await.unwrap();

        let got = r.pending_for("n1", 10, NOW).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "live");
        assert_eq!(got[0].1, b"payload-live".to_vec());

        assert_eq!(state_of(&r, "stale").await, "expired");
        assert_eq!(state_of(&r, "delivered").await, "delivered");
        // The pulled one is marked as delivered, won't be re-dispatched
        assert!(r.pending_for("n1", 10, NOW).await.unwrap().is_empty());
        assert_eq!(state_of(&r, "live").await, "delivered");
    }

    /// Invalidation before forced deletion: ignores TTL, expires all unsent commands and returns them
    /// for the caller to write audit.
    #[tokio::test]
    async fn cancel_pending_for_node_expires_every_pending_row() {
        let r = repo("cancel").await;
        // First create a "delivered" one: after being pulled it's no longer "unsent"
        issue(&r, "delivered", "n1", 30, 60).await;
        r.pending_for("n1", 1, NOW).await.unwrap();
        // Then add an overdue pending: it won't be hit by the pull path anymore (node won't come)
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
        // Delivered is not affected (it's no longer "unsent"), others' commands untouched
        assert_eq!(state_of(&r, "delivered").await, "delivered");
        assert_eq!(r.count_pending_for_node("n2", NOW).await.unwrap(), 1);
        // Idempotent: invalidating again when there's nothing to invalidate
        assert!(r.cancel_pending_for_node("n1").await.unwrap().is_empty());
    }

    /// Invalidation must leave a trace: audit_log has outcome=cancelled, doesn't affect existing issued rows.
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

    /// Tests need direct access to audit_log: borrow the repo's pool (same crate, direct pool access)
    async fn r_pool(r: &CommandsRepo) -> &SqlitePool {
        &r.pool
    }
}
