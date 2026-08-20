use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, layouts, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/layouts",
            axum::routing::get(layouts::list_layouts).post(layouts::create_layout),
            [
                OperationDoc::get_list::<hypercolor_types::api::layouts::LayoutSummary>(
                    "list_layouts",
                    "layouts",
                    "List layouts",
                )
                .query::<hypercolor_types::api::layouts::LayoutListQuery>(),
                OperationDoc::post::<hypercolor_types::api::layouts::LayoutSummary>(
                    "create_layout",
                    "layouts",
                    "Create layout",
                )
                .body::<hypercolor_types::api::layouts::CreateLayoutRequest>()
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/layouts/active",
            axum::routing::get(layouts::get_active_layout),
            [
                OperationDoc::get::<hypercolor_types::spatial::SpatialLayout>(
                    "get_active_layout",
                    "layouts",
                    "Get active layout",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/layouts/active/preview",
            axum::routing::put(layouts::preview_layout),
            [
                OperationDoc::put::<hypercolor_types::api::layouts::PreviewLayoutResponse>(
                    "preview_layout",
                    "layouts",
                    "Preview active layout",
                )
                .body::<hypercolor_types::spatial::SpatialLayout>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/layouts/{id}",
            axum::routing::get(layouts::get_layout)
                .put(layouts::update_layout)
                .delete(layouts::delete_layout),
            [
                OperationDoc::get::<hypercolor_types::spatial::SpatialLayout>(
                    "get_layout",
                    "layouts",
                    "Get layout",
                ),
                OperationDoc::put::<hypercolor_types::api::layouts::LayoutSummary>(
                    "update_layout",
                    "layouts",
                    "Update layout",
                )
                .body::<hypercolor_types::api::layouts::UpdateLayoutRequest>(),
                OperationDoc::delete::<hypercolor_types::api::layouts::DeleteLayoutResponse>(
                    "delete_layout",
                    "layouts",
                    "Delete layout",
                )
                .also_status("202"),
            ],
        ))
        .routes(openapi::documented_route(
            "/layouts/{id}/apply",
            axum::routing::post(layouts::apply_layout),
            [
                OperationDoc::post::<hypercolor_types::api::layouts::ApplyLayoutResponse>(
                    "apply_layout",
                    "layouts",
                    "Apply layout",
                )
                .also_status("202"),
            ],
        ))
}
