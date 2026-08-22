//! User media asset API client.

use web_sys::File;

use super::client;

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
    let part = client::multipart_file_part("file", &file).await?;
    client::post_multipart("/api/v1/assets", vec![part])
        .await
        .map_err(Into::into)
}
