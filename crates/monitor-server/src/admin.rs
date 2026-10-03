//! Admin token for the human-facing read API.
//!
//! Nodes authenticate with signed requests, but a browser cannot hold a node key
//! certificate, so the console needs a second credential.
//!
//! Three sources, in priority order (high to low):
//!
//! 1. `ZHIWEI_ADMIN_TOKEN` environment variable — managed platform free tiers
//!    have no persistent disk and no Shell, so the "store first-generated token
//!    on a data volume" approach doesn't work. Putting the token in the
//!    dashboard's environment variable keeps it the same across restarts and
//!    avoids digging through logs.
//! 2. `<data-dir>/admin.token` file — the normal path for self-hosted /
//!    persistent-volume deployments.
//! 3. If neither is set, generate a random one, persist it, and print it
//!    once in the startup log.

use std::fmt::Write as _;
use std::path::Path;

use rand::RngCore;

/// Where the credential comes from — determines what to print at startup
/// (only generated tokens need to be shown to the user).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// `ZHIWEI_ADMIN_TOKEN` environment variable
    Env,
    /// `<data-dir>/admin.token` file
    File,
    /// Randomly generated this time and persisted to disk
    Generated,
}

pub async fn load_or_init(data_dir: &Path) -> anyhow::Result<(String, TokenSource)> {
    // 1. Environment variable wins: managed platforms use it to pin the token,
    //    no Shell needed to retrieve it.
    match std::env::var("ZHIWEI_ADMIN_TOKEN") {
        Ok(raw) => {
            // Reject anything too short: this is a long-lived credential,
            // never "warn and use it anyway". After rejecting, fall back to
            // file / random generation so the service still starts, but never
            // expose a weak credential.
            match validate_env_token("ZHIWEI_ADMIN_TOKEN", &raw) {
                Ok(token) => return Ok((token, TokenSource::Env)),
                Err(reason) => {
                    tracing::error!(
                        %reason,
                        "ZHIWEI_ADMIN_TOKEN is unusable, ignored; falling back to file or random generation"
                    );
                }
            }
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => {
            // Non-Unicode and other abnormal values: don't silently swallow
            // them, or the user will think the env var took effect.
            tracing::warn!(error = %e, "Failed to read ZHIWEI_ADMIN_TOKEN, falling back to file / random generation");
        }
    }

    // 2. File (self-hosted / persistent volume).
    let path = data_dir.join("admin.token");

    if path.exists() {
        let token = tokio::fs::read_to_string(&path).await?;
        return Ok((token.trim().to_string(), TokenSource::File));
    }

    // 3. Randomly generate and persist.
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    let token = buf.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    });

    tokio::fs::write(&path, token.as_bytes()).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = tokio::fs::metadata(&path).await?.permissions();
        perm.set_mode(0o600);
        tokio::fs::set_permissions(&path, perm).await?;
    }

    Ok((token, TokenSource::Generated))
}

/// Length-checked, content-constant-time comparison.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Minimum credential length.
///
/// Both the console admin token and the enrollment bootstrap token are
/// **long-lived secrets**, and both sit directly behind the public-facing edge:
/// the former can read all data, the latter can enroll nodes.
pub const MIN_TOKEN_LEN: usize = 16;

/// Validate a credential from an environment variable.
///
/// Returns `Ok(trimmed)` when usable; `Err(reason)` means it **must be rejected**,
/// and the caller should fall back to a safer source (file / random generation)
/// instead of degrading and using it anyway.
pub fn validate_env_token(name: &str, raw: &str) -> Result<String, String> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(format!("{name} is empty"));
    }
    if token.len() < MIN_TOKEN_LEN {
        return Err(format!(
            "{name} has only {} characters, fewer than {MIN_TOKEN_LEN} — it is a long-lived secret \
             and a short one is brute-forceable, so it has been rejected \
             (please use a random string of ≥32 characters instead)",
            token.len()
        ));
    }
    Ok(token.to_string())
}

/// Admission check for new credentials.
///
/// The console credential is the only gate (nodes use signatures, not this),
/// so: long enough, no whitespace or control characters (so that once stored
/// it's easy to copy correctly).
pub fn validate_new_token(token: &str) -> Result<(), String> {
    if token.len() < MIN_TOKEN_LEN {
        return Err(format!(
            "credential must be at least {MIN_TOKEN_LEN} characters"
        ));
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("credential must not contain whitespace or control characters".into());
    }
    Ok(())
}

/// Write a new credential back to `<data-dir>/admin.token` (0600).
pub async fn save(data_dir: &Path, token: &str) -> anyhow::Result<()> {
    let path = data_dir.join("admin.token");
    tokio::fs::write(&path, token.as_bytes()).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = tokio::fs::metadata(&path).await?.permissions();
        perm.set_mode(0o600);
        tokio::fs::set_permissions(&path, perm).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_token_must_be_long_enough() {
        assert!(validate_new_token("short").is_err());
        assert!(validate_new_token("0123456789abcde").is_err()); // 15
        assert!(validate_new_token("0123456789abcdef").is_ok()); // 16
    }

    #[test]
    fn new_token_must_not_contain_whitespace_or_control_chars() {
        // Credentials with spaces / newlines / tabs are easy to misuse in
        // command lines and curl once stored.
        assert!(validate_new_token("0123456789abc ef").is_err());
        assert!(validate_new_token("0123456789abc\nef").is_err());
        assert!(validate_new_token("0123456789abc\tef").is_err());
        assert!(validate_new_token("0123456789abcdef-_.~").is_ok());
    }

    #[test]
    fn env_token_shorter_than_minimum_is_rejected_not_warned() {
        // Regression: users setting ZHIWEI_BOOTSTRAP_TOKEN to 4 characters
        // used to only emit a warning and still use it, which meant leaving
        // a brute-forceable long-lived enrollment token behind the public
        // edge. Must be rejected.
        assert!(validate_env_token("T", "true").is_err()); // len=4
        assert!(validate_env_token("T", "short").is_err());
        assert!(validate_env_token("T", "0123456789abcde").is_err()); // 15
        assert!(validate_env_token("T", "").is_err());
        assert!(validate_env_token("T", "   ").is_err());
    }

    #[test]
    fn env_token_is_trimmed_and_accepted_at_minimum() {
        // Pasting in a deploy dashboard often picks up leading/trailing whitespace;
        // after trimming it should still work.
        let ok = validate_env_token("T", "  0123456789abcdef  ").unwrap();
        assert_eq!(ok, "0123456789abcdef");
        // Right at the minimum: allow.
        assert!(validate_env_token("T", "0123456789abcdef").is_ok());
    }

    /// Priority and side effects of the three sources.
    ///
    /// Deliberately written as **one** test: these assertions mutate
    /// process-level environment variables, and cargo test runs in parallel
    /// by default — splitting them would stomp on each other. Combined,
    /// they naturally run sequentially.
    #[tokio::test]
    async fn token_sources_are_env_then_file_then_generated() {
        let dir = std::env::temp_dir().join(format!("zhiwei-admin-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let token_path = dir.join("admin.token");
        let _ = tokio::fs::remove_file(&token_path).await;

        // --- Env var wins: whitespace stripped, **not written to disk** ---
        // Not writing is intentional: the managed platform's env var is the
        // source of truth; persisting it would make a restart read a stale
        // value that disagrees with the deploy dashboard.
        std::env::set_var("ZHIWEI_ADMIN_TOKEN", "  env-token-0123456789abcdef  ");
        let (token, source) = load_or_init(&dir).await.unwrap();
        std::env::remove_var("ZHIWEI_ADMIN_TOKEN");

        assert_eq!(
            token, "env-token-0123456789abcdef",
            "leading/trailing whitespace should be trimmed"
        );
        assert_eq!(source, TokenSource::Env);
        assert!(
            !token_path.exists(),
            "env-var source should not write to disk — otherwise a restart reads a stale value"
        );

        // --- No env var: first call generates randomly ---
        let (first, source) = load_or_init(&dir).await.unwrap();
        assert_eq!(source, TokenSource::Generated);
        assert!(!first.is_empty());

        // --- Call again: same token read back from file, not regenerated ---
        let (second, source) = load_or_init(&dir).await.unwrap();
        assert_eq!(source, TokenSource::File);
        assert_eq!(first, second, "token must stay stable across restarts");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
