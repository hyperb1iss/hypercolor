//! Why a Windows release unit was refused.
//!
//! Parsing a unit manifest needs no Windows API, so this error compiles on
//! every host with the manifest; only the store variant waits for the
//! Windows store it wraps.

use std::io;

#[cfg(windows)]
use super::store::InstallStoreError;

/// Release unit validation or staging failure on Windows.
#[derive(Debug, thiserror::Error)]
pub enum ReleasePayloadError {
    /// The manifest exceeded its bounded byte limit.
    #[error("release manifest exceeds {limit} bytes")]
    ManifestTooLarge { limit: usize },
    /// Strict JSON decoding failed.
    #[error("release manifest is not valid strict JSON: {0}")]
    DecodeManifest(#[from] serde_json::Error),
    /// Manifest semantics or inventory are invalid.
    #[error("invalid release manifest: {0}")]
    InvalidManifest(String),
    /// The extracted manifest does not match the verifier-accepted digest.
    #[error("release manifest digest {actual} does not match verified digest {expected}")]
    UnexpectedManifestDigest { expected: String, actual: String },
    /// The extracted source tree does not exactly match the manifest.
    #[error("invalid release source: {0}")]
    InvalidSource(String),
    /// An existing, staged or newly published unit is not exact.
    #[error("invalid immutable release unit: {0}")]
    InvalidUnit(String),
    /// A bounded staging namespace was exhausted.
    #[error("could not allocate a staging directory after {attempts} attempts")]
    StageCollisions { attempts: usize },
    /// A handle-relative filesystem operation failed.
    #[error("failed to {operation}: {source}")]
    Filesystem {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    /// Install-store authority could not be retained.
    #[cfg(windows)]
    #[error(transparent)]
    Store(#[from] InstallStoreError),
    /// Failure cleanup could not remove the staging tree.
    #[error("release staging failed ({original}) and cleanup failed: {cleanup}")]
    Cleanup {
        original: String,
        #[source]
        cleanup: io::Error,
    },
}
