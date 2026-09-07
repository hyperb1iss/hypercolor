use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{openapi, simulators};
use crate::app_state::AppState;
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/simulators/displays",
            axum::routing::get(simulators::list_simulated_displays)
                .post(simulators::create_simulated_display),
            [
                OperationDoc::get_vec::<crate::simulators::SimulatedDisplayConfig>(
                    "list_simulated_displays",
                    "displays",
                    "List simulated displays",
                ),
                OperationDoc::post::<crate::simulators::SimulatedDisplayConfig>(
                    "create_simulated_display",
                    "displays",
                    "Create simulated display",
                )
                .body::<hypercolor_types::api::simulators::CreateSimulatedDisplayRequest>()
                .status("201"),
            ],
        ))
        .routes(openapi::documented_route(
            "/simulators/displays/{id}",
            axum::routing::get(simulators::get_simulated_display)
                .patch(simulators::patch_simulated_display)
                .delete(simulators::delete_simulated_display),
            [
                OperationDoc::get::<crate::simulators::SimulatedDisplayConfig>(
                    "get_simulated_display",
                    "displays",
                    "Get simulated display",
                ),
                OperationDoc::patch::<crate::simulators::SimulatedDisplayConfig>(
                    "patch_simulated_display",
                    "displays",
                    "Patch simulated display",
                )
                .body::<hypercolor_types::api::simulators::UpdateSimulatedDisplayRequest>(),
                OperationDoc::delete::<
                    hypercolor_types::api::simulators::DeleteSimulatedDisplayResponse,
                >(
                    "delete_simulated_display",
                    "displays",
                    "Delete simulated display",
                ),
            ],
        ))
        .routes(openapi::documented_route(
            "/simulators/displays/{id}/frame",
            axum::routing::get(simulators::get_simulated_display_frame),
            [OperationDoc::get::<serde_json::Value>(
                "get_simulated_display_frame",
                "displays",
                "Get simulated display frame",
            )
            .binary("image/jpeg")],
        ))
}
