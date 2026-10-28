use std::sync::Arc;
use utoipa_axum::router::OpenApiRouter;

use crate::api::{
    backups,
    openapi::{self, OperationDoc},
};
use crate::app_state::AppState;
use hypercolor_types::api::backups::{BackupApplyRequest, BackupApplyResponse, BackupPreview};

pub(super) fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(openapi::documented_route(
            "/backups/export",
            axum::routing::get(backups::export_archive),
            [OperationDoc::get::<serde_json::Value>(
                "export_backup",
                "backups",
                "Download a portable lighting backup",
            )
            .binary(&["application/x-tar"])],
        ))
        .routes(openapi::documented_route(
            "/backups/import",
            axum::routing::post(backups::preview_archive),
            [OperationDoc::post::<BackupPreview>(
                "preview_backup",
                "backups",
                "Stage and preview an uploaded backup",
            )
            .binary_body()],
        ))
        .routes(openapi::documented_route(
            "/backups/apply",
            axum::routing::post(backups::apply_archive),
            [OperationDoc::post::<BackupApplyResponse>(
                "apply_backup",
                "backups",
                "Restore a staged backup",
            )
            .body::<BackupApplyRequest>()],
        ))
}
