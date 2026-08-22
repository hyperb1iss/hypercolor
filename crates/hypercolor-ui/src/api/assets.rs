//! User media asset API client.

use web_sys::File;

use super::{ApiResult, client};

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
    let part = client::multipart_file_part("file", &file).await?;
    client::post_multipart("/api/v1/assets", vec![part]).await
}
