//! Local certificate scanning.
//!
//! Two sources merged into one result:
//!   1. **Console-configured cert paths** (`GET /v1/cert-config`) -- each with `source_id`,
//!      matched certs carry this id back, server uses it to determine "which source this belongs to"
//!      and send expiry notifications;
//!   2. **Local baseline** (`--cert-globs` or built-in default locations) -- `source_id` is empty.
//!
//! Only does "discovery + parsing", no renewal involved (ACME in P2-4 later phases).

use std::collections::BTreeMap;

use anyhow::Context;
use x509_parser::prelude::{FromDer, X509Certificate};
use zhiwei_proto::telemetry::CertInfo;

/// One cert path from server config (node side only recognizes "id + user-filled path")
#[derive(Debug, Clone)]
pub struct CertSourceSpec {
    pub id: String,
    pub path: String,
}

/// One scan result: cert body + reason if parsing failed (failure reason only for "test" display)
#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub info: CertInfo,
    pub error: String,
}

/// Default scan locations: covers Let's Encrypt, nginx self-managed directories, and common system cert directories.
/// Note: /etc/ssl/certs and similar system trust stores have hundreds of root certs, not scanned by default.
pub const DEFAULT_GLOBS: &[&str] = &[
    "/etc/letsencrypt/live/*/cert.pem",
    "/etc/letsencrypt/live/*/fullchain.pem",
    "/etc/nginx/ssl/*.crt",
    "/etc/nginx/ssl/*.pem",
    "/etc/pki/tls/certs/*.crt",
    "/etc/pki/tls/certs/*.pem",
];

/// Merge scan: server sources first (same path prioritized by source), local baseline after.
/// Results deduplicated by cert path (same path matched by multiple globs reported only once).
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

/// "Test" use: expand one path and actually scan it, return parsing failure reason too.
pub fn scan_path(path: &str) -> (Vec<String>, Vec<ScanEntry>) {
    let patterns = zhiwei_common::certpath::expand(path);
    let entries = scan_entries(&baseline_patterns(&patterns));
    (patterns, entries.into_values().collect())
}

/// Merge default globs with user-provided globs: defaults first, user appends after.
///
/// Same path deduplicated by "first wins" in `scan_entries`, so no deduplication here.
/// Empty user input treated as "only scan default locations" -- not "scan nothing".
pub fn merge_globs(user_csv: &str) -> Vec<String> {
    let mut out: Vec<String> = DEFAULT_GLOBS.iter().map(|s| s.to_string()).collect();
    if !user_csv.trim().is_empty() {
        out.extend(
            user_csv
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    out
}

/// Scan by (pattern, source_id), deduplicate by path; same path first match wins (source prioritized).
fn scan_entries(patterns: &[(String, String)]) -> BTreeMap<String, ScanEntry> {
    let mut by_path: BTreeMap<String, ScanEntry> = BTreeMap::new();

    for (pattern, source_id) in patterns {
        let entries = match glob::glob(pattern) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(pattern, error = %e, "Invalid cert glob pattern");
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
                // Private key / unrelated .pem files: not certs, skip directly.
                // Otherwise nginx's `key.pem + cert.pem` side-by-side directories would be full of "parse failed",
                // and "matched count" would be inflated.
                Ok(None) => {
                    tracing::debug!(path, "Not a cert file, skipping");
                }
                Err(e) => {
                    let reason = format!("{e}");
                    tracing::debug!(path, error = %reason, "Cert parsing failed");
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

/// Baseline globs (no source attribution: local default locations and `--cert-globs`)
fn baseline_patterns(globs: &[String]) -> Vec<(String, String)> {
    globs.iter().map(|g| (g.clone(), String::new())).collect()
}

/// Read certs from one file: `Ok(None)` = no CERTIFICATE block in file (e.g. private key).
fn read_cert(path: &str) -> anyhow::Result<Option<CertInfo>> {
    let pem = std::fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;

    // One PEM file may contain a cert chain, take the first (leaf cert)
    let Some(der) = first_cert_der(&pem) else {
        return Ok(None);
    };
    let (_, cert) =
        X509Certificate::from_der(&der).map_err(|e| anyhow::anyhow!("X.509 parse failed: {e}"))?;

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
        // Use saturating: some certs (self-signed test certs, very long validity) have expiry
        // as far as year 4096, seconds -> nanoseconds would overflow i64; clamp to max instead of panic/wrap
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

/// Extract first DER certificate from PEM text.
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

    /// When directory has private keys mixed with real certs: only report certs, private keys silently skipped
    #[test]
    fn skips_non_certificate_pem() {
        let dir = tmpdir("skip");
        let pair = rcgen::generate_simple_self_signed(vec!["zhiwei.test".to_string()]).unwrap();
        std::fs::write(dir.join("leaf.pem"), pair.cert.pem()).unwrap();
        std::fs::write(dir.join("leaf.key.pem"), pair.key_pair.serialize_pem()).unwrap();
        std::fs::write(dir.join("broken.pem"), "not a pem at all").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored by glob").unwrap();

        let (patterns, entries) = scan_path(dir.to_str().unwrap());
        assert_eq!(patterns.len(), 4, "directory should expand to 4 suffix globs");
        assert_eq!(
            entries.len(),
            1,
            "only leaf.pem is a cert (broken.pem isn't even PEM)"
        );
        assert_eq!(entries[0].info.path, dir.join("leaf.pem").to_string_lossy());
        assert!(!entries[0].info.parse_error);
        assert_eq!(entries[0].info.domains, vec!["zhiwei.test".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Server source takes priority: when same path matched by source and baseline, source_id is from source
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

    /// `--cert-globs` / `ZHIWEI_CERT_GLOBS` changed to append instead of replace default locations.
    /// Default locations already cover Let's Encrypt / nginx self-managed directories, etc.;
    /// users configuring custom paths 99% of the time want to add a few extra places,
    /// let them keep the defaults.
    #[test]
    fn merge_globs_appends_user_to_defaults() {
        let merged = merge_globs("/opt/custom/*.pem");
        let default_count = DEFAULT_GLOBS.len();
        assert_eq!(
            merged.len(),
            default_count + 1,
            "user glob should append after defaults"
        );
        assert_eq!(merged[0], DEFAULT_GLOBS[0], "default globs first");
        assert_eq!(
            merged.last().unwrap(),
            "/opt/custom/*.pem",
            "user glob at end"
        );

        // Multiple user globs comma-separated, empty segments dropped
        let merged = merge_globs("/opt/a/*.pem, ,/opt/b/*.crt");
        assert_eq!(merged.len(), default_count + 2);
        assert!(merged.ends_with(&[
            "/opt/a/*.pem".to_string(),
            "/opt/b/*.crt".to_string()
        ]));

        // Empty = only scan defaults (not "scan nothing")
        let merged = merge_globs("");
        assert_eq!(merged.len(), default_count);
    }
}
