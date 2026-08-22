//! User media asset identifiers and metadata primitives.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque identifier for a user media asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct AssetId(pub Uuid);

impl AssetId {
    /// Create a fresh UUID v7 asset identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create an asset identifier from an existing UUID.
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the wrapped UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for AssetId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for AssetId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl FromStr for AssetId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// Metadata scan state for an asset record.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AssetScanStatus {
    #[default]
    Pending,
    Ready,
    Unsupported {
        reason: String,
    },
    Failed {
        reason: String,
    },
    Unscanned,
}

/// Non-fatal policy warnings attached to an accepted asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetWarning {
    PerAssetSoftCapExceeded { limit_bytes: u64 },
    LibrarySoftCapExceeded { limit_bytes: u64 },
}

/// Persisted metadata for one user media asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct MediaAssetRecord {
    pub id: AssetId,
    pub name: String,
    pub hash_sha256: String,
    pub mime_type: String,
    pub byte_len: u64,
    pub intrinsic_width: Option<u32>,
    pub intrinsic_height: Option<u32>,
    pub duration_us: Option<u64>,
    pub frame_count: Option<u32>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub modified_at: DateTime<Utc>,
    #[serde(default)]
    pub scan_status: AssetScanStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<AssetWarning>,
}
