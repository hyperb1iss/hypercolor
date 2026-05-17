//! User media asset API client.

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use web_sys::{File, FormData};

use super::{ApiEnvelope, client};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MediaAssetRecord {
    pub id: String,
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
    pub created_at: String,
    pub modified_at: String,
    #[serde(default)]
    pub scan_status: serde_json::Value,
    #[serde(default)]
    pub warnings: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AssetListResponse {
    pub items: Vec<MediaAssetRecord>,
    pub total: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AssetUploadResponse {
    #[serde(flatten)]
    pub record: MediaAssetRecord,
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetUpdateRequest {
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
}

pub async fn list_assets() -> Result<AssetListResponse, String> {
    client::fetch_json("/api/v1/assets")
        .await
        .map_err(Into::into)
}

pub async fn update_asset(
    id: &str,
    request: &AssetUpdateRequest,
) -> Result<MediaAssetRecord, String> {
    client::put_json(&format!("/api/v1/assets/{id}"), request)
        .await
        .map_err(Into::into)
}

pub async fn delete_asset(id: &str) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/assets/{id}"))
        .await
        .map_err(Into::into)
}

pub async fn upload_asset(file: File) -> Result<AssetUploadResponse, String> {
    let form_data = FormData::new().map_err(|error| format!("{error:?}"))?;
    form_data
        .append_with_blob_and_filename("file", &file, &file.name())
        .map_err(|error| format!("{error:?}"))?;

    let response = client::with_auth(Request::post("/api/v1/assets"))
        .body(form_data)
        .map_err(|error| error.to_string())?
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if !(200..300).contains(&response.status()) {
        let fallback = format!("HTTP {}", response.status());
        let payload = response.json::<serde_json::Value>().await.ok();
        let message = payload
            .as_ref()
            .and_then(|value| value["error"]["message"].as_str())
            .map(str::to_owned)
            .unwrap_or(fallback);
        return Err(message);
    }

    response
        .json::<ApiEnvelope<AssetUploadResponse>>()
        .await
        .map(|payload| payload.data)
        .map_err(|error| error.to_string())
}
