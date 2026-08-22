//! User media asset API client.

use gloo_net::http::Method;
use hypercolor_types::api::ApiResponse;
use web_sys::{File, FormData};

use super::{ApiError, ApiResult, client};

pub use hypercolor_types::api::assets::{
    AssetListResponse, AssetUpdateRequest, AssetUploadResponse,
};
pub use hypercolor_types::asset::{AssetId, MediaAssetRecord};

pub async fn list_assets() -> ApiResult<AssetListResponse> {
    client::fetch_json("/api/v1/assets").await
}

pub async fn update_asset(
    id: AssetId,
    request: &AssetUpdateRequest,
) -> ApiResult<MediaAssetRecord> {
    client::put_json(&format!("/api/v1/assets/{id}"), request).await
}

pub async fn delete_asset(id: AssetId) -> ApiResult<()> {
    client::delete_empty(&format!("/api/v1/assets/{id}")).await
}

pub async fn upload_asset(file: File) -> ApiResult<AssetUploadResponse> {
    let form_data = FormData::new().map_err(|error| ApiError::Serialize(format!("{error:?}")))?;
    form_data
        .append_with_blob_and_filename("file", &file, &file.name())
        .map_err(|error| ApiError::Serialize(format!("{error:?}")))?;

    let request = client::request(Method::POST, "/api/v1/assets")?;
    let response = request
        .body(form_data)
        .map_err(|error| ApiError::Serialize(error.to_string()))?
        .send()
        .await
        .map_err(|error| ApiError::Network(error.to_string()))?;

    if !(200..300).contains(&response.status()) {
        let status = response.status();
        let payload = response.json::<serde_json::Value>().await.ok();
        let message = payload
            .as_ref()
            .and_then(|value| value["error"]["message"].as_str())
            .map(str::to_owned);
        return Err(ApiError::Http { status, message });
    }

    response
        .json::<ApiResponse<AssetUploadResponse>>()
        .await
        .map(|payload| payload.data)
        .map_err(|error| ApiError::Parse(error.to_string()))
}
