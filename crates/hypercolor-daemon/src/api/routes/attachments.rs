use std::sync::Arc;

use utoipa_axum::router::OpenApiRouter;

use crate::api::openapi::OperationDoc;
use crate::api::{AppState, attachments, openapi};
pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(openapi::documented_route(
        "/attachments/templates",
        axum::routing::get(attachments::list_templates).post(attachments::create_template),
        [
            OperationDoc::get_list::<hypercolor_types::api::attachments::TemplateSummary>(
                "list_templates",
                "attachments",
                "List attachment templates",
            )
            .query::<hypercolor_types::api::attachments::ListTemplatesQuery>(),
            OperationDoc::post::<hypercolor_types::api::attachments::TemplateDetail>(
                "create_template",
                "attachments",
                "Create attachment template",
            )
            .body::<hypercolor_types::attachment::ComponentTemplate>()
            .status("201"),
        ],
    ))
}
