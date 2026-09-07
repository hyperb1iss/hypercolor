//! Persisted media asset index.

use hypercolor_types::asset::AssetId;
use serde::{Deserialize, Serialize};

// The record vocabulary lives in hypercolor-types so the daemon's asset
// responses and every client name the same struct; these re-exports keep
// `crate::asset::*` paths stable inside core.
pub use hypercolor_types::asset::{AssetScanStatus, AssetWarning, MediaAssetRecord};

/// Current media asset index schema version.
pub const INDEX_VERSION: u32 = 1;

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
