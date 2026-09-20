//! 本机证书扫描。
//!
//! 两条来源合并成一份结果：
//!   1. **控制台配置的证书路径**（`GET /v1/cert-config`）——每条带 `source_id`，
//!      命中它的证书会带回这个 id，服务端据此做「归属哪条来源」的判定与到期通知；
//!   2. **本机基线**（`--cert-globs` 或内置默认位置）——`source_id` 为空。
//!
//! 只做「发现 + 解析」，不涉及续签（ACME 在 P2-4 后续阶段）。

use std::collections::BTreeMap;

use anyhow::Context;
use x509_parser::prelude::{FromDer, X509Certificate};
use zhiwei_proto::telemetry::CertInfo;

/// 服务端配置的一条证书路径（节点侧只认「id + 用户填的路径」）
#[derive(Debug, Clone)]
pub struct CertSourceSpec {
    pub id: String,
    pub path: String,
}

/// 一条扫描结果：证书本体 + 解析失败时的原因（失败原因只用于「测试」展示）
#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub info: CertInfo,
    pub error: String,
}

/// 默认扫描位置：覆盖 Let's Encrypt、nginx 自管目录与常见系统证书目录。
/// 注意 /etc/ssl/certs 这类系统信任库会有上百个根证书，默认不扫。
pub const DEFAULT_GLOBS: &[&str] = &[
    "/etc/letsencrypt/live/*/cert.pem",
    "/etc/letsencrypt/live/*/fullchain.pem",
    "/root/nginx-certs/*.crt",
    "/root/nginx-certs/*.pem",
    "/etc/nginx/ssl/*.crt",
    "/etc/nginx/ssl/*.pem",
    "/etc/pki/tls/certs/*.crt",
];

/// 合并扫描：服务端来源在前（同一路径以来源为准），本机基线在后。
/// 结果按证书路径去重（同一路径被多个 glob 命中也只报一次）。
pub fn scan_with_sources(sources: &[CertSourceSpec], baseline_globs: &[String]) -> Vec<CertInfo> {
    let mut patterns: Vec<(String, String)> = Vec::new();
    for s in sources {
        for p in zhiwei_common::certpath::expand(&s.path) {
            patterns.push((p, s.id.clone()));
        }
    }
    patterns.extend(baseline_patterns(baseline_globs));
    scan_entries(&patterns)
        .into_values()
        .map(|e| e.info)
        .collect()
}

/// 「测试」用：把一条路径展开后真扫一遍，连解析失败原因一起返回。
pub fn scan_path(path: &str) -> (Vec<String>, Vec<ScanEntry>) {
    let patterns = zhiwei_common::certpath::expand(path);
    let entries = scan_entries(&baseline_patterns(&patterns));
    (patterns, entries.into_values().collect())
}

/// 按 (pattern, source_id) 扫描，路径去重；同一路径先到者胜（来源优先）。
fn scan_entries(patterns: &[(String, String)]) -> BTreeMap<String, ScanEntry> {
    let mut by_path: BTreeMap<String, ScanEntry> = BTreeMap::new();

    for (pattern, source_id) in patterns {
        let entries = match glob::glob(pattern) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(pattern, error = %e, "证书 glob 无效");
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.to_string_lossy().to_string();
            if by_path.contains_key(&path) {
                continue;
            }
            match read_cert(&path) {
                Ok(Some(mut info)) => {
                    info.source_id = source_id.clone();
                    by_path.insert(
                        path,
                        ScanEntry {
                            info,
                            error: String::new(),
                        },
                    );
                }
                // 私钥 / 无关的 .pem 文件：不是证书，直接跳过。
                // 否则 nginx 那种 `key.pem + cert.pem` 并排的目录会满屏「解析失败」，
                // 也会让「命中数」虚高。
                Ok(None) => {
                    tracing::debug!(path, "不是证书文件，跳过");
                }
                Err(e) => {
                    let reason = format!("{e}");
                    tracing::debug!(path, error = %reason, "证书解析失败");
                    by_path.insert(
                        path.clone(),
                        ScanEntry {
                            info: CertInfo {
                                path,
                                parse_error: true,
                                source_id: source_id.clone(),
                                ..Default::default()
                            },
                            error: reason,
                        },
                    );
                }
            }
        }
    }

    by_path
}

/// 基线 glob（不含来源归属：本机默认位置与 `--cert-globs`）
fn baseline_patterns(globs: &[String]) -> Vec<(String, String)> {
    globs.iter().map(|g| (g.clone(), String::new())).collect()
}

/// 读一个文件里的证书：`Ok(None)` = 文件里没有 CERTIFICATE 块（如私钥）。
fn read_cert(path: &str) -> anyhow::Result<Option<CertInfo>> {
    let pem = std::fs::read_to_string(path).with_context(|| format!("读取 {path}"))?;

    // 一个 PEM 文件可能含证书链，取第一张（叶子证书）
    let Some(der) = first_cert_der(&pem) else {
        return Ok(None);
    };
    let (_, cert) =
        X509Certificate::from_der(&der).map_err(|e| anyhow::anyhow!("解析 X.509 失败: {e}"))?;

    let domains = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    x509_parser::extensions::GeneralName::DNSName(d) => Some((*d).to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(Some(CertInfo {
        path: path.to_string(),
        subject: cert.subject().to_string(),
        issuer: cert.issuer().to_string(),
        // 用 saturating：有些证书（自签测试证书、超长有效期）的到期时间能到
        // 公元 4096 年，秒 → 纳秒会溢出 i64；这里夹到上限而不是 panic / 回绕
        not_after_unix_nano: cert
            .validity()
            .not_after
            .timestamp()
            .saturating_mul(1_000_000_000),
        not_before_unix_nano: cert
            .validity()
            .not_before
            .timestamp()
            .saturating_mul(1_000_000_000),
        domains,
        serial: cert.raw_serial_as_string(),
        parse_error: false,
        source_id: String::new(),
    }))
}

/// 从 PEM 文本中取出第一段 DER 证书。
fn first_cert_der(pem: &str) -> Option<Vec<u8>> {
    let mut block = Vec::new();
    let mut inside = false;
    for line in pem.lines() {
        let line = line.trim();
        if line.contains("BEGIN CERTIFICATE") {
            inside = true;
            block.clear();
            continue;
        }
        if line.contains("END CERTIFICATE") {
            if inside {
                return pem_base64_decode(&block.join(""));
            }
            inside = false;
            continue;
        }
        if inside {
            block.push(line.to_string());
        }
    }
    None
}

fn pem_base64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zhiwei-certs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 目录里混着私钥与真证书时：只报证书，私钥静默跳过
    #[test]
    fn skips_non_certificate_pem() {
        let dir = tmpdir("skip");
        let pair = rcgen::generate_simple_self_signed(vec!["zhiwei.test".to_string()]).unwrap();
        std::fs::write(dir.join("leaf.pem"), pair.cert.pem()).unwrap();
        std::fs::write(dir.join("leaf.key.pem"), pair.key_pair.serialize_pem()).unwrap();
        std::fs::write(dir.join("broken.pem"), "not a pem at all").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored by glob").unwrap();

        let (patterns, entries) = scan_path(dir.to_str().unwrap());
        assert_eq!(patterns.len(), 4, "目录应展开成 4 个后缀 glob");
        assert_eq!(
            entries.len(),
            1,
            "只有 leaf.pem 是证书（broken.pem 连 PEM 都不是）"
        );
        assert_eq!(entries[0].info.path, dir.join("leaf.pem").to_string_lossy());
        assert!(!entries[0].info.parse_error);
        assert_eq!(entries[0].info.domains, vec!["zhiwei.test".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 服务端来源优先：同一路径被来源与基线同时命中时，source_id 以来源为准
    #[test]
    fn source_id_wins_over_baseline() {
        let dir = tmpdir("src");
        let pair = rcgen::generate_simple_self_signed(vec!["src.test".to_string()]).unwrap();
        let file = dir.join("a.crt");
        std::fs::write(&file, pair.cert.pem()).unwrap();

        let sources = vec![CertSourceSpec {
            id: "src-1".to_string(),
            path: dir.to_string_lossy().to_string(),
        }];
        let baseline = vec![format!("{}/*.crt", dir.to_string_lossy())];
        let certs = scan_with_sources(&sources, &baseline);
        assert_eq!(certs.len(), 1);
        assert_eq!(certs[0].source_id, "src-1");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
