use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{assets, openapi};
use crate::app_state::AppState;
pub(super) fn router(asset_upload_body_limit: usize) -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/assets",
            axum::routing::get(assets::list_assets)
                .post(assets::upload_asset)
                .layer(DefaultBodyLimit::max(asset_upload_body_limit)),
            [
                OperationDoc::get_list::<hypercolor_types::asset::MediaAssetRecord>(
                    "list_assets",
                    "assets",
                    "List media assets",
                ),
                OperationDoc::post::<hypercolor_types::api::assets::AssetUploadResponse>(
                    "upload_asset",
                    "assets",
                    "Upload a media asset",
                )
                .query::<hypercolor_types::api::assets::AssetUploadQuery>()
                .also_status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/assets/{id}",
            axum::routing::get(assets::get_asset)
                .put(assets::update_asset)
                .delete(assets::delete_asset),
            [
                OperationDoc::get::<hypercolor_types::asset::MediaAssetRecord>(
                    "get_asset",
                    "assets",
                    "Get one media asset",
                ),
                OperationDoc::put::<hypercolor_types::asset::MediaAssetRecord>(
                    "update_asset",
                    "assets",
                    "Update one media asset",
                )
                .body::<hypercolor_types::api::assets::AssetUpdateRequest>(),
                OperationDoc::delete::<hypercolor_types::api::assets::DeleteAssetResponse>(
                    "delete_asset",
                    "assets",
                    "Delete one media asset",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/assets/{id}/blob",
            axum::routing::get(assets::get_asset_blob),
            [OperationDoc::get::<serde_json::Value>(
                "get_asset_blob",
                "assets",
                "Download media asset bytes",
            )
            .binary("application/octet-stream")],
        ))
        .routes(openapi::documented_route(
            "/assets/{id}/thumbnail",
            axum::routing::get(assets::get_asset_thumbnail),
            [OperationDoc::get::<serde_json::Value>(
                "get_asset_thumbnail",
                "assets",
                "Get a media asset thumbnail",
            )
            .binary("image/webp")],
        ))
}
