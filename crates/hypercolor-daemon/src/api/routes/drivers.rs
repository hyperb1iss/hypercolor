use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, drivers, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/drivers",
            axum::routing::get(drivers::list_drivers),
            [OperationDoc::get::<drivers::DriverListResponse>(
                "list_drivers",
                "drivers",
                "List driver modules",
            )],
        ))
        .routes(openapi::documented_route(
            "/drivers/{id}/config",
            axum::routing::get(drivers::get_driver_config),
            [OperationDoc::get::<drivers::DriverConfigResponse>(
                "get_driver_config",
                "drivers",
                "Get driver module config",
            )],
        ))
}
