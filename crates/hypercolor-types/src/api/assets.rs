//! Media asset API contracts — `/api/v1/assets/*`.

use serde::{Deserialize, Serialize};

use crate::api::envelope::ListResponse;
use crate::asset::AssetId;
use crate::asset::MediaAssetRecord;

/// Response from `GET /api/v1/assets`.
pub type AssetListResponse = ListResponse<MediaAssetRecord>;

/// Response from `POST /api/v1/assets`.
///
/// `duplicate` reports that the bytes already existed in the library, in
/// which case `record` is the pre-existing asset rather than a new one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct AssetUploadResponse {
    #[serde(flatten)]
    pub record: MediaAssetRecord,
    pub duplicate: bool,
}

/// Query parameters for `POST /api/v1/assets` (multipart upload).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema, utoipa::IntoParams))]
pub struct AssetUploadQuery {
    /// Store a byte-identical upload under a fresh name instead of
    /// returning the existing record.
    #[serde(default)]
    #[cfg_attr(feature = "schema", param(required = false))]
    pub rename_duplicate: bool,
    /// Explicit asset type hint, overriding sniffing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

/// The `multipart/form-data` fields `POST /api/v1/assets` reads.
///
/// This documents the wire form for clients and the OpenAPI document; the
/// daemon reads the parts one by one rather than deserializing this type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct AssetUploadForm {
    /// The asset bytes, sent as a file part. Required.
    #[cfg_attr(feature = "schema", schema(value_type = String, format = Binary))]
    pub file: Vec<u8>,
    /// Display name; defaults to the uploaded file name.
    pub name: Option<String>,
    /// Tags, comma separated or as a JSON array of strings.
    pub tags: Option<String>,
    /// Asset type hint (`lottie` or `stream`), overriding the `type` query
    /// parameter and content sniffing.
    pub r#type: Option<String>,
}

/// Request body for `PUT /api/v1/assets/{id}`.
///
/// Omitted fields leave the stored metadata untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct AssetUpdateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Response from `DELETE /api/v1/assets/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeleteAssetResponse {
    pub removed: AssetId,
}
