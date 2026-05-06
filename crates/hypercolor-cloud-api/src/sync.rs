use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncEntityKind {
    Pref,
    Scene,
    Layout,
    Favorite,
    Profile,
    OwnedDevice,
    InstalledEffect,
}

impl SyncEntityKind {
    #[must_use]
    pub const fn path_segment(self) -> &'static str {
        match self {
            Self::Pref => "prefs",
            Self::Scene => "scenes",
            Self::Layout => "layouts",
            Self::Favorite => "favorites",
            Self::Profile => "profiles",
            Self::OwnedDevice => "owned-devices",
            Self::InstalledEffect => "installed-effects",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOp {
    Put,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Etag(pub u64);

impl fmt::Display for Etag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncEntity {
    pub kind: SyncEntityKind,
    pub id: String,
    pub etag: Etag,
    pub schema_version: u32,
    pub value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncChange {
    pub seq: i64,
    pub op: SyncOp,
    pub entity_kind: SyncEntityKind,
    pub entity_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<SyncEntity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangesResponse {
    #[serde(rename = "data", alias = "changes")]
    pub changes: Vec<SyncChange>,
    pub next_seq: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncPutRequest {
    pub schema_version: u32,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncConflictResponse {
    pub error: String,
    pub current_etag: Etag,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<SyncEntity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub id: Ulid,
    pub entity_kind: SyncEntityKind,
    pub entity_id: String,
    pub losing_version: Value,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}
