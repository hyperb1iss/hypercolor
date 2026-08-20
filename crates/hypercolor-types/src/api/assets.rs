//! Media asset API contracts — `/api/v1/assets/*`.

use serde::{Deserialize, Serialize};

use crate::api::envelope::ListResponse;
use crate::asset::MediaAssetRecord;

/// Response from `GET /api/v1/assets`.
pub type AssetListResponse = ListResponse<MediaAssetRecord>;

/// Response from `POST /api/v1/assets`.
///
/// `duplicate` reports that the bytes already existed in the library, in
/// which case `record` is the pre-existing asset rather than a new one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetUploadResponse {
    #[serde(flatten)]
    pub record: MediaAssetRecord,
    pub duplicate: bool,
}

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
