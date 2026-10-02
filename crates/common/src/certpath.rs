//! Certificate path parsing rules (same implementation shared by monitor and node).
//!
//! User fills in a "path" which can be one of three things:
//!   1. Directory       -> Expand to globs with cert suffixes (non-recursive, one level only)
//!   2. Single file     -> Use as-is (suffix is .pem / .crt / .cer / .cert)
//!   3. Glob expression -> Use as-is
//!
//! **Pure lexical expansion**, does not depend on local filesystem -- monitor side can't see
//! node's directories, but needs to use the same rules to look up reported cert paths back to
//! sources. Both sides must compute the same result.

/// Certificate suffixes used for directory expansion.
pub const CERT_EXTENSIONS: &[&str] = &["pem", "crt", "cer", "cert"];

/// Maximum path length (prevents abuse of stuffing entire config into path)
pub const MAX_PATH_LEN: usize = 512;

/// Validate user input path. Returns normalized form (trim whitespace, remove trailing `/`).
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
    // Normalize directory paths by removing trailing slash (except root)
    let trimmed = p.trim_end_matches('/');
    Ok(if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    })
}

/// Expand one path into a list of globs for scanning/matching.
///
/// Input should already be normalized; here empty input returns empty list instead of error,
/// caller (scan and match) safely handles empty list as "nothing matched".
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

/// Check if a reported cert path belongs to this source (path -> source lookup).
pub fn matches(raw: &str, cert_path: &str) -> bool {
    // require_literal_separator: `*` doesn't cross directories, consistent with node side one-level expansion
    // (recursive glob with `**` is not affected)
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
        // Directory expansion is non-recursive, subdirectory doesn't match
        assert!(!matches("/root/nginx-certs", "/root/nginx-certs/sub/a.crt"));
        // Only explicit `**` makes it recursive
        assert!(matches(
            "/root/nginx-certs/**/*.crt",
            "/root/nginx-certs/sub/a.crt"
        ));
    }
}
