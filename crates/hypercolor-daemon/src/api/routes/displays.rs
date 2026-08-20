use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, displays, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/displays",
            axum::routing::get(displays::list_displays),
            [OperationDoc::get_vec::<
                hypercolor_types::api::displays::DisplaySummary,
            >(
                "list_displays", "displays", "List display devices"
            )],
        ))
        .routes(openapi::documented_route(
            "/displays/{id}/frame",
            axum::routing::get(displays::get_display_frame),
            [OperationDoc::get::<serde_json::Value>(
                "get_display_frame",
                "displays",
                "Get display preview image",
            )
            .binary("image/jpeg")],
        ))
        .routes(openapi::documented_route(
            "/displays/{id}/face",
            axum::routing::get(displays::get_display_face)
                .put(displays::set_display_face)
                .delete(displays::delete_display_face),
            [
                OperationDoc::get::<Option<hypercolor_types::api::displays::DisplayFaceResponse>>(
                    "get_display_face",
                    "displays",
                    "Get display face assignment",
                ),
                OperationDoc::put::<hypercolor_types::api::displays::DisplayFaceResponse>(
                    "set_display_face",
                    "displays",
                    "Set display face assignment",
                )
                .body::<hypercolor_types::api::displays::SetDisplayFaceRequest>(),
                OperationDoc::delete::<hypercolor_types::api::displays::DeleteDisplayFaceResponse>(
                    "delete_display_face",
                    "displays",
                    "Delete display face assignment",
                )
                .query::<hypercolor_types::api::displays::DisplayFaceScopeQuery>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/displays/{id}/face/controls",
            axum::routing::patch(displays::patch_display_face_controls),
            [
                OperationDoc::patch::<hypercolor_types::api::displays::DisplayFaceResponse>(
                    "patch_display_face_controls",
                    "displays",
                    "Patch display face controls",
                )
                .body::<hypercolor_types::api::displays::UpdateDisplayFaceControlsRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/displays/{id}/face/composition",
            axum::routing::patch(displays::patch_display_face_composition),
            [
                OperationDoc::patch::<hypercolor_types::api::displays::DisplayFaceResponse>(
                    "patch_display_face_composition",
                    "displays",
                    "Patch display face composition",
                )
                .body::<hypercolor_types::api::displays::UpdateDisplayFaceCompositionRequest>(),
            ],
        ))
}
