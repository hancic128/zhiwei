use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Ed25519 signature (64 bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Signature(pub Vec<u8>);

impl Signature {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Ed25519 public key (32 bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicKey(pub Vec<u8>);

impl PublicKey {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn from_verifying_key(k: &VerifyingKey) -> Self {
        Self(k.to_bytes().to_vec())
    }

    /// Convert to a `VerifyingKey` for signature verification.
    ///
    /// # Errors
    ///
    /// Returns `Error::Crypto` if the key bytes are not exactly 32 bytes or are not a
    /// valid Ed25519 public key.
    pub fn to_verifying_key(&self) -> Result<VerifyingKey> {
        let bytes: [u8; 32] = self
            .0
            .as_slice()
            .try_into()
            .map_err(|_| Error::Crypto("public key must be 32 bytes".into()))?;
        VerifyingKey::from_bytes(&bytes).map_err(|e| Error::Crypto(e.to_string()))
    }
}

/// Ed25519 key pair. Private key is automatically zeroized in memory.
#[derive(Debug)]
pub struct KeyPair {
    signing_key: SigningKey,
}

impl KeyPair {
    #[must_use]
    pub fn generate() -> Self {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        Self { signing_key }
    }
    /// Restore a key pair from its 32-byte private key representation.
    ///
    /// # Errors
    ///
    /// Returns `Error::Crypto` if the input bytes are not exactly 32 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Crypto("ed25519 secret must be 32 bytes".into()))?;
        Ok(Self {
            signing_key: SigningKey::from_bytes(&bytes),
        })
    }

    #[must_use]
    pub fn to_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_verifying_key(&self.signing_key.verifying_key())
    }

    #[must_use]
    pub fn sign(&self, message: &[u8]) -> Signature {
        let sig = self.signing_key.sign(message);
        Signature(sig.to_bytes().to_vec())
    }

    /// Verify an Ed25519 signature over `message`.
    ///
    /// # Errors
    ///
    /// Returns `Error::Crypto` if the public key bytes are invalid, the signature
    /// is not exactly 64 bytes, or the signature does not match.
    pub fn verify(public_key: &PublicKey, message: &[u8], signature: &Signature) -> Result<()> {
        let vk = public_key.to_verifying_key()?;
        let sig_bytes: [u8; 64] = signature
            .0
            .as_slice()
            .try_into()
            .map_err(|_| Error::Crypto("signature must be 64 bytes".into()))?;
        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        vk.verify(message, &sig)
            .map_err(|e| Error::Crypto(e.to_string()))
    }
}
