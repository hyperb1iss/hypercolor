//! Persisted media asset index.

use chrono::{DateTime, Utc};
use hypercolor_types::asset::AssetId;
use serde::{Deserialize, Serialize};

/// Current media asset index schema version.
pub const INDEX_VERSION: u32 = 1;

/// Metadata scan state for an asset record.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetWarning {
    PerAssetSoftCapExceeded { limit_bytes: u64 },
    LibrarySoftCapExceeded { limit_bytes: u64 },
}

/// Persisted metadata for one user media asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    #[serde(default)]
    pub scan_status: AssetScanStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<AssetWarning>,
}

/// Library event emitted for asset mutations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetEvent {
    Added { record: MediaAssetRecord },
    Modified { record: MediaAssetRecord },
    Removed { asset_id: AssetId },
}

/// JSON index mapping stable asset IDs to content-addressed blobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssetIndex {
    pub version: u32,
    records: Vec<MediaAssetRecord>,
}

impl Default for AssetIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            records: Vec::new(),
        }
    }
}

impl AssetIndex {
    #[must_use]
    pub fn records(&self) -> &[MediaAssetRecord] {
        &self.records
    }

    #[must_use]
    pub fn into_records(self) -> Vec<MediaAssetRecord> {
        self.records
    }

    #[must_use]
    pub fn get(&self, id: AssetId) -> Option<&MediaAssetRecord> {
        self.records.iter().find(|record| record.id == id)
    }

    #[must_use]
    pub fn get_mut(&mut self, id: AssetId) -> Option<&mut MediaAssetRecord> {
        self.records.iter_mut().find(|record| record.id == id)
    }

    #[must_use]
    pub fn by_hash(&self, hash_sha256: &str) -> Option<&MediaAssetRecord> {
        self.records
            .iter()
            .find(|record| record.hash_sha256 == hash_sha256)
    }

    pub fn upsert(&mut self, record: MediaAssetRecord) {
        if let Some(existing) = self.get_mut(record.id) {
            *existing = record;
        } else {
            self.records.push(record);
        }
        self.sort_records();
    }

    pub fn remove(&mut self, id: AssetId) -> Option<MediaAssetRecord> {
        let index = self.records.iter().position(|record| record.id == id)?;
        Some(self.records.remove(index))
    }

    pub(crate) fn replace_records(&mut self, records: Vec<MediaAssetRecord>) {
        self.records = records;
        self.version = INDEX_VERSION;
        self.sort_records();
    }

    fn sort_records(&mut self) {
        self.records.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
        });
    }
}
