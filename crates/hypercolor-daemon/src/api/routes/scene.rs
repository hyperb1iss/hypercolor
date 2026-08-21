use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{openapi, scene};
use crate::app_state::AppState;
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/scene",
            axum::routing::get(scene::get_scene).patch(scene::patch_scene),
            [
                OperationDoc::get::<hypercolor_types::api::scene::SceneDocument>(
                    "get_live_scene",
                    "scenes",
                    "Get the live scene",
                ),
                OperationDoc::patch::<hypercolor_types::api::scene::SceneDocument>(
                    "patch_live_scene",
                    "scenes",
                    "Patch live scene metadata",
                )
                .body::<hypercolor_types::api::scene::ScenePatchRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/deactivate",
            axum::routing::post(scene::deactivate_scene),
            [OperationDoc::post::<
                hypercolor_types::api::scene::SceneDocument,
            >(
                "deactivate_scene",
                "scenes",
                "Return to the default scene",
            )],
        ))
        .routes(openapi::documented_route(
            "/scene/clear",
            axum::routing::post(scene::clear_scene),
            [
                OperationDoc::post::<hypercolor_types::api::scene::SceneDocument>(
                    "clear_scene",
                    "scenes",
                    "Clear live scene layers",
                )
                .optional_body::<hypercolor_types::api::scene::ClearSceneRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/zones",
            axum::routing::post(scene::create_zone),
            [
                OperationDoc::post::<hypercolor_types::api::scene::ZoneResource>(
                    "create_live_zone",
                    "scenes",
                    "Create a live scene zone",
                )
                .body::<hypercolor_types::api::scene::CreateZoneRequest>()
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/zones/{zone}",
            axum::routing::get(scene::get_zone)
                .patch(scene::patch_zone)
                .delete(scene::delete_zone),
            [
                OperationDoc::get::<hypercolor_types::api::scene::ZoneResource>(
                    "get_live_zone",
                    "scenes",
                    "Get a live scene zone",
                ),
                OperationDoc::patch::<hypercolor_types::api::scene::ZoneResource>(
                    "patch_live_zone",
                    "scenes",
                    "Patch a live scene zone",
                )
                .body::<hypercolor_types::api::scene::PatchZoneRequest>(),
                OperationDoc::delete::<hypercolor_types::api::scene::SceneDocument>(
                    "delete_live_zone",
                    "scenes",
                    "Delete a live scene zone",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/zones/{zone}/layout",
            axum::routing::put(scene::put_zone_layout),
            [
                OperationDoc::put::<hypercolor_types::api::scene::ZoneResource>(
                    "put_live_zone_layout",
                    "scenes",
                    "Replace a live zone layout",
                )
                .body::<hypercolor_types::api::scene::ZoneLayoutRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/zones/{zone}/members",
            axum::routing::post(scene::assign_members),
            [
                OperationDoc::post::<hypercolor_types::api::scene::ZoneResource>(
                    "assign_live_zone_members",
                    "scenes",
                    "Assign members to a live zone",
                )
                .body::<hypercolor_types::api::scene::AssignMembersRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/zones/{zone}/members/{member}",
            axum::routing::delete(scene::unassign_member),
            [OperationDoc::delete::<
                hypercolor_types::api::scene::ZoneResource,
            >(
                "unassign_live_zone_member",
                "scenes",
                "Unassign a live zone member",
            )],
        ))
        .routes(openapi::documented_route(
            "/scene/zones/{zone}/layers",
            axum::routing::get(scene::list_layers).post(scene::create_layer),
            [
                OperationDoc::get_list::<hypercolor_types::layer::SceneLayer>(
                    "list_live_zone_layers",
                    "scenes",
                    "List live zone layers",
                ),
                OperationDoc::post::<hypercolor_types::api::scene::ZoneResource>(
                    "create_live_zone_layer",
                    "scenes",
                    "Create a live zone layer",
                )
                .body::<hypercolor_types::api::scene::CreateLayerRequest>()
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/zones/{zone}/layers/order",
            axum::routing::patch(scene::reorder_layers),
            [
                OperationDoc::patch::<hypercolor_types::api::scene::ZoneResource>(
                    "reorder_live_zone_layers",
                    "scenes",
                    "Reorder live zone layers",
                )
                .body::<hypercolor_types::api::scene::ReorderLayersRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/zones/{zone}/layers/{layer}",
            axum::routing::put(scene::replace_layer).delete(scene::delete_layer),
            [
                OperationDoc::put::<hypercolor_types::api::scene::ZoneResource>(
                    "replace_live_zone_layer",
                    "scenes",
                    "Replace a live zone layer",
                )
                .body::<hypercolor_types::api::scene::ReplaceLayerRequest>(),
                OperationDoc::delete::<hypercolor_types::api::scene::ZoneResource>(
                    "delete_live_zone_layer",
                    "scenes",
                    "Delete a live zone layer",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/scene/zones/{zone}/layers/{layer}/controls",
            axum::routing::patch(scene::patch_layer_controls),
            [
                OperationDoc::patch::<hypercolor_types::api::scene::ZoneResource>(
                    "patch_live_layer_controls",
                    "scenes",
                    "Patch live layer controls",
                )
                .body::<hypercolor_types::api::scene::PatchControlsRequest>(),
            ],
        ))
}
