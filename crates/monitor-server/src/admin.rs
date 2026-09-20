//! Admin token for the human-facing read API.
//!
//! Nodes authenticate with signed requests, but a browser cannot hold a node key
//! certificate, so the console needs a second credential.
//!
//! 三个来源，优先级从高到低：
//!
//! 1. `ZHIWEI_ADMIN_TOKEN` 环境变量——托管平台免费层没有持久磁盘、也没有
//!    Shell，「首次生成的 token 存在数据卷里」这条路走不通。把 token 放进
//!    面板的环境变量里，重启后仍是同一个，也不用去翻日志。
//! 2. `<data-dir>/admin.token` 文件——自建 / 有持久卷时的正常路径。
//! 3. 都没有就随机生成，落盘，并在启动日志里打印一次。

use std::path::Path;

use rand::RngCore;

/// 凭据是从哪来的——决定启动时打印什么（生成的才需要打印出来给用户看）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// `ZHIWEI_ADMIN_TOKEN` 环境变量
    Env,
    /// `<data-dir>/admin.token` 文件
    File,
    /// 本次随机生成并落盘
    Generated,
}

pub async fn load_or_init(data_dir: &Path) -> anyhow::Result<(String, TokenSource)> {
    // 1. 环境变量优先：托管平台用它固定 token，无需 Shell 也能拿到。
    match std::env::var("ZHIWEI_ADMIN_TOKEN") {
        Ok(raw) => {
            // 太短一律拒绝：这是长期有效的凭据，不能「警告一下就照用」。
            // 拒绝后退回文件 / 随机生成，保证服务仍能起来，但绝不暴露弱凭据。
            match validate_env_token("ZHIWEI_ADMIN_TOKEN", &raw) {
                Ok(token) => return Ok((token, TokenSource::Env)),
                Err(reason) => {
                    tracing::error!(
                        %reason,
                        "ZHIWEI_ADMIN_TOKEN 不可用，已忽略；改用文件或随机生成"
                    );
                }
            }
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => {
            // 非 Unicode 等异常值：不静默吞掉，否则用户会以为环境变量生效了
            tracing::warn!(error = %e, "读取 ZHIWEI_ADMIN_TOKEN 失败，改走文件 / 随机生成");
        }
    }

    // 2. 文件（自建 / 有持久卷）。
    let path = data_dir.join("admin.token");

    if path.exists() {
        let token = tokio::fs::read_to_string(&path).await?;
        return Ok((token.trim().to_string(), TokenSource::File));
    }

    // 3. 随机生成并落盘。
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    let token: String = buf.iter().map(|b| format!("{b:02x}")).collect();

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

/// 凭据的最低长度。
///
/// 控制台 admin token 与入网 bootstrap token 都是**长期有效的秘密**，
/// 且都直接暴露在公网边缘之后：前者能读全部数据，后者能注册节点。
/// 短了就是可暴力猜解的，所以低于这条线一律不采用。
pub const MIN_TOKEN_LEN: usize = 16;

/// 校验一个来自环境变量的凭据。
///
/// 返回 `Ok( trimmed )` 才可以用；`Err(原因)` 表示**必须拒绝**，
/// 调用方应当退回更安全的来源（文件 / 随机生成），而不是降级照用。
pub fn validate_env_token(name: &str, raw: &str) -> Result<String, String> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(format!("{name} 是空的"));
    }
    if token.len() < MIN_TOKEN_LEN {
        return Err(format!(
            "{name} 只有 {} 个字符，少于 {MIN_TOKEN_LEN} 个——它是长期有效的秘密，\
             太短会被暴力猜解，已拒绝使用（请换成 ≥32 字符的随机串）",
            token.len()
        ));
    }
    Ok(token.to_string())
}

/// 新凭据的准入检查。
///
/// 控制台的凭据是唯一一道门（节点走签名、不走这里），所以：够长、不含空白与
/// 控制字符（避免存进去之后自己都复制不利索）。
pub fn validate_new_token(token: &str) -> Result<(), String> {
    if token.len() < MIN_TOKEN_LEN {
        return Err(format!("凭据至少要 {MIN_TOKEN_LEN} 个字符"));
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("凭据不能包含空白或控制字符".into());
    }
    Ok(())
}

/// 把新凭据写回 `<data-dir>/admin.token`（0600）。
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
        assert!(validate_new_token("短").is_err());
        assert!(validate_new_token("0123456789abcde").is_err()); // 15
        assert!(validate_new_token("0123456789abcdef").is_ok()); // 16
    }

    #[test]
    fn new_token_must_not_contain_whitespace_or_control_chars() {
        // 带空格 / 换行 / 制表符的凭据存进去之后，命令行与 curl 都容易用错
        assert!(validate_new_token("0123456789abc ef").is_err());
        assert!(validate_new_token("0123456789abc\nef").is_err());
        assert!(validate_new_token("0123456789abc\tef").is_err());
        assert!(validate_new_token("0123456789abcdef-_.~").is_ok());
    }

    #[test]
    fn env_token_shorter_than_minimum_is_rejected_not_warned() {
        // 回归：用户把 ZHIWEI_BOOTSTRAP_TOKEN 设成 4 个字符，之前只 warn 就照用，
        // 等于把一个可暴力猜解的长期入网令牌留在公网边缘后面。必须拒绝。
        assert!(validate_env_token("T", "true").is_err()); // len=4
        assert!(validate_env_token("T", "short").is_err());
        assert!(validate_env_token("T", "0123456789abcde").is_err()); // 15
        assert!(validate_env_token("T", "").is_err());
        assert!(validate_env_token("T", "   ").is_err());
    }

    #[test]
    fn env_token_is_trimmed_and_accepted_at_minimum() {
        // 部署面板里粘贴很容易带上首尾空白，trim 后要能用
        let ok = validate_env_token("T", "  0123456789abcdef  ").unwrap();
        assert_eq!(ok, "0123456789abcdef");
        // 刚好到线就放行
        assert!(validate_env_token("T", "0123456789abcdef").is_ok());
    }

    /// 三个来源的优先级与副作用。
    ///
    /// 刻意写成**一个**测试：这些断言会改进程级环境变量，而 cargo test 默认
    /// 并行跑测试，拆成两个会互相踩。合在一起就天然串行。
    #[tokio::test]
    async fn token_sources_are_env_then_file_then_generated() {
        let dir = std::env::temp_dir()
            .join(format!("zhiwei-admin-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let token_path = dir.join("admin.token");
        let _ = tokio::fs::remove_file(&token_path).await;

        // --- 环境变量优先：去空白，且**不写盘** ---
        // 不写盘是有意的：托管平台的环境变量才是真相来源，落盘反而会让下一轮
        // 重启读到一个与部署面板不一致的旧值。
        std::env::set_var("ZHIWEI_ADMIN_TOKEN", "  env-token-0123456789abcdef  ");
        let (token, source) = load_or_init(&dir).await.unwrap();
        std::env::remove_var("ZHIWEI_ADMIN_TOKEN");

        assert_eq!(token, "env-token-0123456789abcdef", "首尾空白应被去掉");
        assert_eq!(source, TokenSource::Env);
        assert!(
            !token_path.exists(),
            "环境变量来源不应写盘——否则重启后会读到陈旧值"
        );

        // --- 没有环境变量：首次随机生成 ---
        let (first, source) = load_or_init(&dir).await.unwrap();
        assert_eq!(source, TokenSource::Generated);
        assert!(!first.is_empty());

        // --- 再次调用：同一个 token 从文件读回，不重新生成 ---
        let (second, source) = load_or_init(&dir).await.unwrap();
        assert_eq!(source, TokenSource::File);
        assert_eq!(first, second, "重启后 token 必须保持不变");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
