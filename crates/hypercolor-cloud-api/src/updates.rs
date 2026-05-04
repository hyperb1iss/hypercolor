use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Stable,
    Beta,
    Nightly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub schema_version: u32,
    pub channel: ReleaseChannel,
    pub current: ReleaseInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_target: Option<RollbackTarget>,
    pub revoked_versions: Vec<String>,
    pub allow_downgrade: bool,
    pub manifest_signature: String,
    pub issued_at: DateTime<Utc>,
    pub manifest_kid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub version: String,
    pub released_at: DateTime<Utc>,
    pub min_supported_from: String,
    pub notes_url: String,
    pub platforms: BTreeMap<String, PlatformArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackTarget {
    pub version: String,
    pub manifest_url: String,
    pub manifest_sha256: String,
    pub manifest_kid: String,
    pub artifact_kid: String,
    pub platforms: BTreeMap<String, PlatformArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformArtifact {
    pub url: String,
    pub size: u64,
    pub blake3: String,
    pub minisign: String,
    pub kind: ArtifactKind,
    pub artifact_kid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    TarballZstd,
    AppBundleTarballZstd,
    Msi,
}
