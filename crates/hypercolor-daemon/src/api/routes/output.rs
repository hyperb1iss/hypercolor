use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, openapi, output};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(openapi::documented_route(
        "/output",
        axum::routing::get(output::get_output).patch(output::patch_output),
        [
            OperationDoc::get::<hypercolor_types::api::output::OutputResource>(
                "get_output",
                "output",
                "Get global output power and brightness",
            ),
            OperationDoc::patch::<hypercolor_types::api::output::OutputResource>(
                "patch_output",
                "output",
                "Set global output power, brightness, or both",
            )
            .body::<hypercolor_types::api::output::OutputPatchRequest>(),
        ],
    ))
}
