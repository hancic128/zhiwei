//! 证书路径的解析规则（monitor 与 node 共用同一份实现）。
//!
//! 用户填的是一个「路径」，它可能是三种东西：
//!   1. 目录        → 展开为若干以证书后缀为通配的 glob（非递归，只扫一层）
//!   2. 单个文件    → 原样使用（后缀是 .pem / .crt / .cer / .cert）
//!   3. glob 表达式 → 原样使用
//!
//! **纯词法展开**，不依赖本机文件系统——monitor 侧看不到节点上的目录，
//! 但它要用同一套规则把已上报的证书路径反查回来源，两边必须算出同一个结果。

/// 目录展开时使用的证书后缀。
pub const CERT_EXTENSIONS: &[&str] = &["pem", "crt", "cer", "cert"];

/// 路径长度上限（挡住把整段配置塞进路径的误用）
pub const MAX_PATH_LEN: usize = 512;

/// 校验用户输入的路径。返回规范化后的写法（去首尾空白、去尾部多余 `/`）。
pub fn normalize(raw: &str) -> Result<String, String> {
    let p = raw.trim();
    if p.is_empty() {
        return Err("Path cannot be empty".into());
    }
    if p.len() > MAX_PATH_LEN {
        return Err(format!("Path too long (max {MAX_PATH_LEN} characters)"));
    }
    if !p.starts_with('/') {
        return Err("Path must be absolute (must start with /)".into());
    }
    if p.chars().any(|c| c.is_control()) {
        return Err("Path cannot contain control characters".into());
    }
    if p.split('/').any(|seg| seg == "..") {
        return Err("Path cannot contain ..".into());
    }
    // 目录写法统一去掉尾部斜杠（根目录除外）
    let trimmed = p.trim_end_matches('/');
    Ok(if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    })
}

/// 把一条路径展开成用于扫描 / 匹配的 glob 列表。
///
/// 输入应已过 [`normalize`]；这里对空输入返回空列表而不是报错，调用方
/// （扫描与匹配）对空列表天然是「什么都没命中」的安全行为。
pub fn expand(raw: &str) -> Vec<String> {
    let p = raw.trim();
    if p.is_empty() {
        return Vec::new();
    }
    if contains_glob_meta(p) || has_cert_extension(p) {
        return vec![p.to_string()];
    }
    let base = p.trim_end_matches('/');
    CERT_EXTENSIONS
        .iter()
        .map(|ext| format!("{base}/*.{ext}"))
        .collect()
}

/// 某个已上报的证书路径是否属于这条来源（路径 → 来源反查）。
pub fn matches(raw: &str, cert_path: &str) -> bool {
    // require_literal_separator：`*` 不跨目录，与节点侧一层展开的语义一致
    // （整段写 `**` 的递归 glob 不受影响）
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    expand(raw).iter().any(|pat| {
        glob::Pattern::new(pat)
            .is_ok_and(|p| p.matches_path_with(std::path::Path::new(cert_path), opts))
    })
}

fn contains_glob_meta(p: &str) -> bool {
    p.contains('*') || p.contains('?') || p.contains('[')
}

fn has_cert_extension(p: &str) -> bool {
    let Some((_, ext)) = p.rsplit_once('.') else {
        return false;
    };
    CERT_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rules() {
        assert_eq!(normalize("  /a/b/  ").unwrap(), "/a/b");
        assert_eq!(normalize("/").unwrap(), "/");
        assert!(normalize("relative/path").is_err());
        assert!(normalize("/a/../b").is_err());
        assert!(normalize("/a\nb").is_err());
        assert!(normalize("").is_err());
    }

    #[test]
    fn dir_expands_to_cert_globs() {
        let got = expand("/root/nginx-certs");
        assert_eq!(got.len(), CERT_EXTENSIONS.len());
        assert_eq!(got[0], "/root/nginx-certs/*.pem");
        assert!(got.iter().all(|p| p.starts_with("/root/nginx-certs/*.")));
    }

    #[test]
    fn file_and_glob_pass_through() {
        assert_eq!(expand("/etc/ssl/a.crt"), vec!["/etc/ssl/a.crt"]);
        assert_eq!(expand("/etc/ssl/**/*.pem"), vec!["/etc/ssl/**/*.pem"]);
        assert_eq!(expand("/etc/ssl/*.crt"), vec!["/etc/ssl/*.crt"]);
    }

    #[test]
    fn path_to_source_lookup() {
        assert!(matches("/root/nginx-certs", "/root/nginx-certs/a.crt"));
        assert!(matches(
            "/root/nginx-certs/a.crt",
            "/root/nginx-certs/a.crt"
        ));
        assert!(!matches("/root/nginx-certs", "/etc/ssl/b.crt"));
        // 目录展开是非递归的，子目录不命中
        assert!(!matches("/root/nginx-certs", "/root/nginx-certs/sub/a.crt"));
        // 显式写 `**` 时才递归
        assert!(matches(
            "/root/nginx-certs/**/*.crt",
            "/root/nginx-certs/sub/a.crt"
        ));
    }
}
