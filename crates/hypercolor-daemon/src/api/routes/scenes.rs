use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{openapi, scenes};
use crate::app_state::AppState;
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/scenes",
            axum::routing::get(scenes::list_scenes).post(scenes::create_scene),
            [
                OperationDoc::get_list::<hypercolor_types::api::scenes::SceneSummary>(
                    "list_scenes",
                    "scenes",
                    "List scenes",
                ),
                OperationDoc::post::<hypercolor_types::api::scenes::SceneSummary>(
                    "create_scene",
                    "scenes",
                    "Create scene",
                )
                .body::<hypercolor_types::api::scenes::CreateSceneRequest>()
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/scenes/snapshot",
            axum::routing::post(scenes::snapshot_scene),
            [
                OperationDoc::post::<hypercolor_types::api::scenes::SceneSummary>(
                    "snapshot_scene",
                    "scenes",
                    "Snapshot the active scene",
                )
                .body::<hypercolor_types::api::scenes::SnapshotSceneRequest>()
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/scenes/{id}",
            axum::routing::get(scenes::get_scene)
                .put(scenes::update_scene)
                .delete(scenes::delete_scene),
            [
                OperationDoc::get::<hypercolor_types::api::scene::SceneDocument>(
                    "get_scene",
                    "scenes",
                    "Get scene",
                ),
                OperationDoc::put::<hypercolor_types::api::scene::SceneDocument>(
                    "update_scene",
                    "scenes",
                    "Update scene",
                )
                .body::<hypercolor_types::api::scenes::ReplaceSceneRequest>(),
                OperationDoc::delete::<hypercolor_types::api::scenes::DeleteSceneResponse>(
                    "delete_scene",
                    "scenes",
                    "Delete scene",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/scenes/{id}/activate",
            axum::routing::post(scenes::activate_scene),
            [
                OperationDoc::post::<hypercolor_types::api::scenes::ActivateSceneResponse>(
                    "activate_scene",
                    "scenes",
                    "Activate scene",
                )
                .optional_body::<hypercolor_types::api::scenes::ActivateSceneRequest>(),
            ],
        ))
}
