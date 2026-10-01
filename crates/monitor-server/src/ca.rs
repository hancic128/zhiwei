//! Local CA for the monitor server.
//!
//! Generates a self-signed root CA on first run and persists it under
//! `<data_dir>/ca/`. Clients are pinned to this CA via the `ca_cert_pem`
//! returned by /v1/enroll (no public PKI involved).

use std::path::Path;

use anyhow::Context;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, IsCa, KeyPair,
    KeyUsagePurpose, SerialNumber,
};
use time::{Duration, OffsetDateTime};

pub struct Ca {
    /// PEM of the CA certificate — handed to nodes at enroll time.
    pub cert_pem: String,
    cert: Certificate,
    key: KeyPair,
}

impl Ca {
    pub async fn load_or_init(data_dir: &Path) -> anyhow::Result<Self> {
        let ca_dir = data_dir.join("ca");
        tokio::fs::create_dir_all(&ca_dir).await?;
        let cert_path = ca_dir.join("ca.crt.pem");
        let key_path = ca_dir.join("ca.key.pem");

        if cert_path.exists() && key_path.exists() {
            tracing::info!(?cert_path, "loaded existing CA");
        } else {
            tracing::warn!("no CA found, generating new self-signed root");

            let key_pair = KeyPair::generate().context("CA key generation")?;
            let mut params = CertificateParams::default();
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
            params.not_before = OffsetDateTime::now_utc() - Duration::hours(1);
            params.not_after = OffsetDateTime::now_utc() + Duration::days(365 * 10);
            let mut dn = DistinguishedName::new();
            dn.push(rcgen::DnType::CommonName, "ZhiWei Monitor CA");
            dn.push(rcgen::DnType::OrganizationName, "ZhiWei");
            params.distinguished_name = dn;
            params.serial_number = Some(SerialNumber::from(vec![1u8]));

            let cert = params.self_signed(&key_pair).context("self-signing CA")?;
            tokio::fs::write(&cert_path, cert.pem().as_bytes()).await?;
            tokio::fs::write(&key_path, key_pair.serialize_pem().as_bytes()).await?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perm = tokio::fs::metadata(&key_path).await?.permissions();
                perm.set_mode(0o600);
                tokio::fs::set_permissions(&key_path, perm).await?;
            }
        }

        let cert_pem = tokio::fs::read_to_string(&cert_path).await?;
        let key_pem = tokio::fs::read_to_string(&key_path).await?;

        // Rebuild the in-memory handles used for signing: rcgen keeps signing
        // state in the `Certificate`/`KeyPair` pair, so parse the stored params
        // and re-derive the issuer handle with the same key.
        let key = KeyPair::from_pem(&key_pem).context("parsing CA private key")?;
        let params =
            CertificateParams::from_ca_cert_pem(&cert_pem).context("parsing CA certificate")?;
        let cert = params
            .self_signed(&key)
            .context("rebuilding CA certificate handle")?;

        Ok(Self {
            cert_pem,
            cert,
            key,
        })
    }

    pub fn cert(&self) -> &Certificate {
        &self.cert
    }

    pub fn key(&self) -> &KeyPair {
        &self.key
    }
}
