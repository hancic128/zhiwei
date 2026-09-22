//! AI token 表：MCP / 外部 AI 客户端读端点用的凭据。
//!
//! 设计要点：
//! - 不存明文 token，只存 SHA-256 哈希。校验时哈希一遍比对。
//! - 多值，可命名，可单独撤销（revoked_at 非空即失效）。
//! - 不过期——撤销是唯一失效手段。要做过期再加一列。
//! - last_used_at 用于审计「谁在什么时候调用过」，仅在 read_auth_ok_v2 命中后异步更新。

use sqlx::SqlitePool;

/// 数据库行（哈希字段一律小写十六进制）。
#[derive(Debug, Clone, sqlx::FromRow)]
struct AiTokenRow {
    id: String,
    #[allow(dead_code)]
    token_hash: String,
    name: String,
    created_at_unix_nano: i64,
    last_used_at_unix_nano: Option<i64>,
    revoked_at_unix_nano: Option<i64>,
}

/// 暴露给上层（routes.rs）的视图。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AiToken {
    pub id: String,
    pub name: String,
    pub created_at_unix_nano: i64,
    pub last_used_at_unix_nano: Option<i64>,
    pub revoked_at_unix_nano: Option<i64>,
}

impl AiToken {
    pub fn is_active(&self) -> bool {
        self.revoked_at_unix_nano.is_none()
    }
}

impl From<AiTokenRow> for AiToken {
    fn from(r: AiTokenRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            created_at_unix_nano: r.created_at_unix_nano,
            last_used_at_unix_nano: r.last_used_at_unix_nano,
            revoked_at_unix_nano: r.revoked_at_unix_nano,
        }
    }
}

#[derive(Clone)]
pub struct AiTokensRepo {
    pool: SqlitePool,
}

impl AiTokensRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 列出所有 token（包含已撤销的，UI 自己过滤）。
    pub async fn list_all(&self) -> anyhow::Result<Vec<AiToken>> {
        let rows: Vec<AiTokenRow> = sqlx::query_as(
            "SELECT id, token_hash, name, created_at_unix_nano,
                    last_used_at_unix_nano, revoked_at_unix_nano
             FROM ai_tokens ORDER BY created_at_unix_nano DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(AiToken::from).collect())
    }

    /// 通过 token 明文查 active 行（鉴权路径，只查未撤销的）。
    pub async fn find_active_by_hash(&self, token_hash: &str) -> anyhow::Result<Option<AiToken>> {
        let row: Option<AiTokenRow> = sqlx::query_as(
            "SELECT id, token_hash, name, created_at_unix_nano,
                    last_used_at_unix_nano, revoked_at_unix_nano
             FROM ai_tokens
             WHERE token_hash = ? AND revoked_at_unix_nano IS NULL",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(AiToken::from))
    }

    /// 通过 id 查（删除/UI 详情用）。
    pub async fn find(&self, id: &str) -> anyhow::Result<Option<AiToken>> {
        let row: Option<AiTokenRow> = sqlx::query_as(
            "SELECT id, token_hash, name, created_at_unix_nano,
                    last_used_at_unix_nano, revoked_at_unix_nano
             FROM ai_tokens WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(AiToken::from))
    }

    /// 插入一条新 token。`id` 形如 `ait_<12 hex>`，`token_hash` 为 SHA-256(token) hex。
    pub async fn create(
        &self,
        id: &str,
        token_hash: &str,
        name: &str,
        now_unix_nano: i64,
    ) -> anyhow::Result<AiToken> {
        sqlx::query(
            "INSERT INTO ai_tokens
             (id, token_hash, name, created_at_unix_nano,
              last_used_at_unix_nano, revoked_at_unix_nano)
             VALUES (?, ?, ?, ?, NULL, NULL)",
        )
        .bind(id)
        .bind(token_hash)
        .bind(name)
        .bind(now_unix_nano)
        .execute(&self.pool)
        .await?;

        Ok(AiToken {
            id: id.to_string(),
            name: name.to_string(),
            created_at_unix_nano: now_unix_nano,
            last_used_at_unix_nano: None,
            revoked_at_unix_nano: None,
        })
    }

    /// 撤销：若已撤销则 no-op。返回是否实际改变了状态。
    pub async fn revoke(&self, id: &str, now_unix_nano: i64) -> anyhow::Result<bool> {
        let r = sqlx::query(
            "UPDATE ai_tokens SET revoked_at_unix_nano = ?
             WHERE id = ? AND revoked_at_unix_nano IS NULL",
        )
        .bind(now_unix_nano)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() > 0)
    }

    /// 更新最后使用时间。best-effort，失败不传播（不影响主请求）。
    pub async fn touch_last_used(&self, id: &str, now_unix_nano: i64) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE ai_tokens SET last_used_at_unix_nano = ?
             WHERE id = ? AND revoked_at_unix_nano IS NULL",
        )
        .bind(now_unix_nano)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// 内存库 + 跑一遍迁移，拿到可用的 repo。
    async fn repo() -> AiTokensRepo {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        crate::migrations::run(&pool).await.expect("migrations");
        AiTokensRepo::new(pool)
    }

    #[tokio::test]
    async fn create_then_find_by_hash_round_trips() {
        let r = repo().await;
        let created = r
            .create("ait-abc123", "deadbeef", "claude-home", 1_000)
            .await
            .unwrap();
        assert_eq!(created.id, "ait-abc123");
        assert!(created.is_active());
        assert_eq!(created.last_used_at_unix_nano, None);

        let found = r.find_active_by_hash("deadbeef").await.unwrap();
        let found = found.expect("active row");
        assert_eq!(found.name, "claude-home");
    }

    #[tokio::test]
    async fn unknown_hash_yields_none() {
        let r = repo().await;
        assert!(r.find_active_by_hash("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn revoke_makes_the_token_unfindable_by_hash() {
        let r = repo().await;
        r.create("ait-1", "hash-1", "n1", 1).await.unwrap();
        assert!(r.find_active_by_hash("hash-1").await.unwrap().is_some());

        assert!(r.revoke("ait-1", 2).await.unwrap());
        // find_active_by_hash 只回未撤销的
        assert!(r.find_active_by_hash("hash-1").await.unwrap().is_none());
        // 但 find 仍能查到（审计需要看到 revoked_at）
        let row = r.find("ait-1").await.unwrap().expect("row still present");
        assert!(!row.is_active());
        assert_eq!(row.revoked_at_unix_nano, Some(2));
    }

    #[tokio::test]
    async fn revoking_twice_reports_false_the_second_time() {
        let r = repo().await;
        r.create("ait-2", "hash-2", "n2", 1).await.unwrap();
        assert!(r.revoke("ait-2", 5).await.unwrap());
        assert!(!r.revoke("ait-2", 6).await.unwrap());
    }

    #[tokio::test]
    async fn revoking_unknown_id_reports_false() {
        let r = repo().await;
        assert!(!r.revoke("ait-nope", 1).await.unwrap());
    }

    #[tokio::test]
    async fn touch_last_used_updates_the_row() {
        let r = repo().await;
        r.create("ait-3", "hash-3", "n3", 100).await.unwrap();
        r.touch_last_used("ait-3", 200).await.unwrap();
        let row = r.find("ait-3").await.unwrap().unwrap();
        assert_eq!(row.last_used_at_unix_nano, Some(200));
    }

    #[tokio::test]
    async fn list_all_includes_revoked_and_orders_newest_first() {
        let r = repo().await;
        r.create("ait-old", "h-old", "old", 10).await.unwrap();
        r.create("ait-new", "h-new", "new", 20).await.unwrap();
        r.revoke("ait-old", 30).await.unwrap();

        let all = r.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "ait-new", "newest first");
        assert_eq!(all[1].id, "ait-old");
    }

    #[tokio::test]
    async fn duplicate_hash_is_rejected() {
        let r = repo().await;
        r.create("ait-x", "same-hash", "first", 1).await.unwrap();
        let err = r.create("ait-y", "same-hash", "second", 2).await;
        assert!(err.is_err(), "token_hash UNIQUE 应该拒绝重复");
    }
}
