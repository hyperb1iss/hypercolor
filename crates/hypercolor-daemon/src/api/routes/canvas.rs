use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{canvas, openapi};
use crate::app_state::AppState;
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(openapi::documented_route(
        "/canvas/frame",
        axum::routing::get(canvas::get_canvas_frame),
        [OperationDoc::get::<serde_json::Value>(
            "get_canvas_frame",
            "canvas",
            "Capture the composed canvas (JPEG, or PNG with format=png)",
        )
        .query::<canvas::CanvasFrameQuery>()
        .component::<crate::canvas_capture::CanvasImageFormat>()
        .binary(&["image/jpeg", "image/png"])],
    ))
}
