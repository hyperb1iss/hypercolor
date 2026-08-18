//! User media asset API client.

use gloo_net::http::Method;
use web_sys::{File, FormData};

use super::{ApiEnvelope, client};

pub use hypercolor_types::api::assets::{
    AssetListResponse, AssetUpdateRequest, AssetUploadResponse,
};
pub use hypercolor_types::asset::{AssetId, MediaAssetRecord};

pub async fn list_assets() -> Result<AssetListResponse, String> {
    client::fetch_json("/api/v1/assets")
        .await
        .map_err(Into::into)
}

pub async fn update_asset(
    id: AssetId,
    request: &AssetUpdateRequest,
) -> Result<MediaAssetRecord, String> {
    client::put_json(&format!("/api/v1/assets/{id}"), request)
        .await
        .map_err(Into::into)
}

pub async fn delete_asset(id: AssetId) -> Result<(), String> {
    client::delete_empty(&format!("/api/v1/assets/{id}"))
        .await
        .map_err(Into::into)
}

pub async fn upload_asset(file: File) -> Result<AssetUploadResponse, String> {
    let form_data = FormData::new().map_err(|error| format!("{error:?}"))?;
    form_data
        .append_with_blob_and_filename("file", &file, &file.name())
        .map_err(|error| format!("{error:?}"))?;

    let request = client::request(Method::POST, "/api/v1/assets").map_err(String::from)?;
    let response = request
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
