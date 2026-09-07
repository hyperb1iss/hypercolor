//! Error type for host-side OpenRGB operations.

use std::path::PathBuf;

use thiserror::Error;

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, HostError>;

/// Failures while inspecting or preparing the OpenRGB host environment.
#[derive(Debug, Error)]
pub enum HostError {
    /// Reading or creating a path under the managed config directory failed.
    #[error("openrgb host I/O failure at {path}: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// An existing `OpenRGB.json` could not be parsed as JSON.
    #[error("existing OpenRGB config at {path} is not valid JSON: {source}")]
    InvalidExistingConfig {
        /// The config file that failed to parse.
        path: PathBuf,
        /// The parse error.
        #[source]
        source: serde_json::Error,
    },

    /// An existing `OpenRGB.json` parsed, but its root or `Detectors` section
    /// is not a JSON object, so the partition cannot be merged in.
    #[error("existing OpenRGB config at {path} has a non-object {section} section")]
    UnexpectedConfigShape {
        /// The config file with the unexpected shape.
        path: PathBuf,
        /// Which section was malformed (`root`, `Detectors`, or `detectors`).
        section: &'static str,
    },

    /// Serializing the merged config failed.
    #[error("failed to serialize OpenRGB config: {0}")]
    Serialize(#[from] serde_json::Error),

    /// The durable replace of `OpenRGB.json` failed.
    #[error("failed to persist OpenRGB config: {0}")]
    Persist(#[from] hypercolor_persistence::PersistenceError),

    /// The managed config directory cannot be passed to OpenRGB as given.
    #[error("openrgb config path {path} is unusable: {reason}")]
    UnsupportedConfigPath {
        /// The offending directory.
        path: PathBuf,
        /// Why it cannot be passed through (non-UTF-8, or a `:` under Flatpak).
        reason: &'static str,
    },

    /// The embedded detector table did not parse. This is a build defect.
    #[error("embedded OpenRGB detector table is malformed: {0}")]
    DetectorTable(String),
}
