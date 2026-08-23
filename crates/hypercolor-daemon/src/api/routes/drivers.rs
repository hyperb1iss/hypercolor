use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use hypercolor_types::api::drivers::DriverConfigResponse;

use crate::api::openapi::OperationDoc;
use crate::api::{drivers, openapi};
use crate::app_state::AppState;
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/drivers",
            axum::routing::get(drivers::list_drivers),
            [OperationDoc::get_list::<
                hypercolor_types::api::drivers::DriverSummary,
            >(
                "list_drivers", "drivers", "List driver modules"
            )],
        ))
        .routes(openapi::documented_route(
            "/drivers/{id}/config",
            axum::routing::get(drivers::get_driver_config),
            [OperationDoc::get::<DriverConfigResponse>(
                "get_driver_config",
                "drivers",
                "Get driver module config",
            )],
        ))
}
