use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{media, openapi};
use crate::app_state::AppState;

pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(openapi::documented_route(
        "/media/authorize",
        axum::routing::post(media::authorize_media),
        [OperationDoc::post::<media::MediaAuthorizationResponse>(
            "authorize_media",
            "media",
            "Request media Automation authorization",
        )
        .body::<media::MediaAuthorizationRequest>()],
    ))
}
