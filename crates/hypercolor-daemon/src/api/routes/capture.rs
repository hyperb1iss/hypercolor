use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{capture, openapi};
use crate::app_state::AppState;
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/input/authorize",
            axum::routing::post(capture::authorize_input_monitoring),
            [OperationDoc::post::<
                hypercolor_types::api::capture::CaptureAuthorizationResponse,
            >(
                "authorize_input_monitoring",
                "capture",
                "Request Input Monitoring authorization",
            )],
        ))
        .routes(openapi::documented_route(
            "/capture/authorize",
            axum::routing::post(capture::authorize_screen_recording),
            [OperationDoc::post::<
                hypercolor_types::api::capture::CaptureAuthorizationResponse,
            >(
                "authorize_screen_recording",
                "capture",
                "Request screen-capture authorization",
            )],
        ))
        .routes(openapi::documented_route(
            "/capture/source",
            axum::routing::put(capture::set_capture_source),
            [OperationDoc::put::<
                hypercolor_types::api::capture::CapturePickerResponse,
            >(
                "set_capture_source",
                "capture",
                "Open the screen-capture source picker",
            )],
        ))
        .routes(openapi::documented_route(
            "/capture/monitors",
            axum::routing::get(capture::list_capture_monitors),
            [OperationDoc::get_vec::<
                hypercolor_types::api::capture::CaptureMonitor,
            >(
                "list_capture_monitors",
                "capture",
                "List addressable capture displays",
            )],
        ))
}
