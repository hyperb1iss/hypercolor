//! Portable backup transfers and explicit restore confirmation.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use hypercolor_types::api::backups::BackupApplyRequest;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::api::{envelope, extract::ApiJson};
use crate::app_state::AppState;
use crate::domain::{DomainError, portable_setup::PortableSetupService};

#[cfg(all(test, feature = "persistence-test-hooks"))]
mod tests;

pub async fn export_archive(State(state): State<Arc<AppState>>) -> Response {
    let result = async {
        let service = PortableSetupService::new(state)?;
        let file = service.export_archive().await?;
        let length = file.metadata().map_err(internal)?.len();
        Response::builder()
            .header(header::CONTENT_TYPE, "application/x-tar")
            .header(header::CONTENT_LENGTH, length)
            .header(
                header::CONTENT_DISPOSITION,
                "attachment; filename=hypercolor.hypercolor-backup",
            )
            .body(Body::from_stream(ReaderStream::new(
                tokio::fs::File::from_std(file),
            )))
            .map_err(internal)
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

pub async fn preview_archive(State(state): State<Arc<AppState>>, body: Body) -> Response {
    let result = async {
        let service = PortableSetupService::new(state)?;
        let temporary = service.archive_upload_file()?;
        let mut file = tokio::fs::File::from_std(temporary);
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|error| {
                DomainError::malformed(format!("backup upload failed: {error}"))
            })?;
            file.write_all(&bytes).await.map_err(internal)?;
        }
        file.seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(internal)?;
        service.preview_archive(file.into_std().await).await
    }
    .await;
    match result {
        Ok(preview) => envelope::ok(preview),
        Err(error) => error.into_response(),
    }
}

pub async fn apply_archive(
    State(state): State<Arc<AppState>>,
    ApiJson(request): ApiJson<BackupApplyRequest>,
) -> Response {
    let result = async {
        PortableSetupService::new(state)?
            .apply_archive(request.operation_id)
            .await
    }
    .await;
    match result {
        Ok(report) => envelope::ok(report),
        Err(error) => error.into_response(),
    }
}

fn internal(error: impl Into<anyhow::Error>) -> DomainError {
    DomainError::Internal(error.into())
}
