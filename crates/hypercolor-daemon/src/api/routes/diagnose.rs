use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, diagnose, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(openapi::documented_route(
        "/diagnose",
        axum::routing::post(diagnose::run_diagnostics),
        [OperationDoc::post::<diagnose::DiagnoseResponse>(
            "run_diagnostics",
            "diagnostics",
            "Run daemon diagnostics",
        )
        .body::<hypercolor_types::api::diagnose::DiagnoseRequest>()],
    ))
}
