//! Media asset API contracts — `/api/v1/assets/*`.

use serde::{Deserialize, Serialize};

/// Query parameters for `POST /api/v1/assets` (multipart upload).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetUploadQuery {
    /// Store a byte-identical upload under a fresh name instead of
    /// returning the existing record.
    #[serde(default)]
    pub rename_duplicate: bool,
    /// Explicit asset type hint, overriding sniffing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

/// Request body for `PUT /api/v1/assets/{id}`.
///
/// Omitted fields leave the stored metadata untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetUpdateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}
