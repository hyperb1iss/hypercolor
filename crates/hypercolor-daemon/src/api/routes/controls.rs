use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, controls, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/drivers/{id}/controls",
            axum::routing::get(controls::get_driver_control_surface),
            [OperationDoc::get::<
                hypercolor_types::controls::ControlSurfaceDocument,
            >(
                "get_driver_control_surface",
                "controls",
                "Get driver control surface",
            )],
        ))
        .routes(openapi::documented_route(
            "/devices/{id}/controls",
            axum::routing::get(controls::get_device_control_surface),
            [OperationDoc::get::<
                hypercolor_types::controls::ControlSurfaceDocument,
            >(
                "get_device_control_surface",
                "controls",
                "Get device control surface",
            )],
        ))
        .routes(openapi::documented_route(
            "/control-surfaces",
            axum::routing::get(controls::list_control_surfaces),
            [OperationDoc::get::<controls::ControlSurfaceListResponse>(
                "list_control_surfaces",
                "controls",
                "List control surfaces",
            )
            .query::<hypercolor_types::api::controls::ControlSurfaceListQuery>()
            .component::<hypercolor_types::controls::ControlValue>()],
        ))
        .routes(openapi::documented_route(
            "/control-surfaces/{id}",
            axum::routing::get(controls::get_control_surface),
            [OperationDoc::get::<
                hypercolor_types::controls::ControlSurfaceDocument,
            >(
                "get_control_surface", "controls", "Get control surface"
            )],
        ))
        .routes(openapi::documented_route(
            "/control-surfaces/{id}/values",
            axum::routing::patch(controls::apply_control_surface_values),
            [
                OperationDoc::patch::<hypercolor_types::controls::ApplyControlChangesResponse>(
                    "apply_control_surface_values",
                    "controls",
                    "Apply control surface values",
                )
                .body::<hypercolor_types::controls::ApplyControlChangesRequest>(),
            ],
        ))
        .routes(openapi::documented_route(
            "/control-surfaces/{id}/actions/{action}",
            axum::routing::post(controls::invoke_control_surface_action),
            [
                OperationDoc::post::<hypercolor_types::controls::ControlActionResult>(
                    "invoke_control_surface_action",
                    "controls",
                    "Invoke control surface action",
                )
                .body::<hypercolor_types::api::controls::InvokeControlActionRequest>(),
            ],
        ))
}
