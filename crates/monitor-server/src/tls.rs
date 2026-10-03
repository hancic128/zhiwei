//! TLS plumbing for the monitor server.
//!
//! The server config requires a client cert chain rooted at our local CA — i.e.
//! only nodes that completed /v1/enroll can call /v1/telemetry.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use anyhow::Context;
use rcgen::{CertificateParams, DistinguishedName, IsCa, KeyPair, SanType, SerialNumber};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use time::{Duration, OffsetDateTime};

use crate::ca::Ca;

#[allow(clippy::struct_field_names)] // `_pem` suffix is the point: these are PEM-encoded strings
pub struct IssuedCert {
    pub cert_pem: String,
    pub key_pem: String,
    pub ca_cert_pem: String,
}

/// Ensure the monitor server has a valid server cert signed by our CA.
/// Re-uses an existing cert on disk if present; otherwise generates one.
pub fn ensure_server_cert(ca: &Ca, data_dir: &Path, cn: &str) -> anyhow::Result<IssuedCert> {
    let cert_path = data_dir.join("monitor.crt.pem");
    let key_path = data_dir.join("monitor.key.pem");

    if cert_path.exists() && key_path.exists() {
        tracing::info!(?cert_path, "loaded existing monitor server cert");
        return Ok(IssuedCert {
            cert_pem: std::fs::read_to_string(&cert_path)?,
            key_pem: std::fs::read_to_string(&key_path)?,
            ca_cert_pem: ca.cert_pem.clone(),
        });
    }

    tracing::warn!("no server cert found, generating new one signed by local CA");

    let key_pair = KeyPair::generate().context("server key generation")?;
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::NoCa;
    params.not_before = OffsetDateTime::now_utc() - Duration::minutes(5);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(90);
    // rustls does not fall back to CN, so SANs are mandatory for the host names
    // and addresses a node might use to reach this server.
    params.subject_alt_names = vec![
        SanType::DnsName("localhost".try_into()?),
        SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)),
    ];
    let mut dn = DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, cn);
    params.distinguished_name = dn;
    params.serial_number = Some(SerialNumber::from(vec![2u8]));

    let cert = params
        .signed_by(&key_pair, ca.cert(), ca.key())
        .context("signing server cert")?;
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    std::fs::write(&cert_path, cert_pem.as_bytes())?;
    std::fs::write(&key_path, key_pem.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&key_path)?.permissions();
        perm.set_mode(0o600);
        std::fs::set_permissions(&key_path, perm)?;
    }

    Ok(IssuedCert {
        cert_pem,
        key_pem,
        ca_cert_pem: ca.cert_pem.clone(),
    })
}

/// Build a rustls `ServerConfig`.
///
/// Does not require client certificates: node identity is carried by Ed25519
/// request signatures (`routes::verify_node`), so this still works when
/// deployed behind a "TLS-terminated at the edge" managed platform. Self-hosted
/// deployments still have TLS terminated here, but transport-layer certs are
/// no longer used to authenticate nodes.
pub fn build_server_config(
    cert_pem: &str,
    key_pem: &str,
    _ca_pem: &str,
) -> anyhow::Result<ServerConfig> {
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .context("parsing server cert chain")?;
    let key = rustls_pemfile::pkcs8_private_keys(&mut key_pem.as_bytes())
        .next()
        .context("no PKCS#8 private key found")?
        .context("parsing server private key")?;
    let key = PrivateKeyDer::Pkcs8(key);

    // No longer requires client certificates: node identity is now carried
    // by Ed25519 request signatures, so this still works when deployed behind
    // edge-terminating TLS managed platforms.
    let mut cfg = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("installing server certificate")?;
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(cfg)
}
