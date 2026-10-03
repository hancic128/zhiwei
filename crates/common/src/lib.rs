//! Common types and utilities shared by all zhiwei binaries.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]
// `multiple_crate_versions` flags transitive deps (e.g. ed25519-dalek pulls
// `rand_core` 0.10 while `rand` 0.8 pulls 0.6; sqlx pulls `thiserror` 2 while
// we pin 1). Not actionable from project code — pinned by upstream crates.
#![allow(clippy::multiple_crate_versions)]

pub mod auth;
pub mod certpath;
pub mod crypto;
pub mod error;
pub mod id;
pub mod time;

pub use auth::{NonceCache, SignedHeaders};
pub use crypto::{KeyPair, PublicKey, Signature};
pub use error::{Error, Result};
pub use id::NodeId;
pub use time::Timestamp;
