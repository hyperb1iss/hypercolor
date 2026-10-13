//! Canvas endpoints — `/api/v1/canvas/*`.

use std::sync::Arc;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::app_state::AppState;
use crate::canvas_capture::{
    CanvasCaptureRequest, CanvasImageFormat, CapturedCanvasFrame, capture_canvas,
};
use crate::domain::DomainError;

/// Query string for `GET /api/v1/canvas/frame`.
#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CanvasFrameQuery {
    /// Image encoding: `jpeg` (default) or `png`.
    #[serde(default)]
    pub format: CanvasImageFormat,
    /// Output width in pixels. Omit or pass 0 for the canvas width; set only
    /// one side to keep the canvas aspect ratio. Never upscales.
    #[serde(default)]
    pub width: u32,
    /// Output height in pixels, with the same rules as `width`.
    #[serde(default)]
    pub height: u32,
}

/// `GET /api/v1/canvas/frame` — one still of the composed effect canvas.
///
/// Waits briefly for a frame rendered after the request so the image reflects
/// the current scene. `X-Canvas-Fresh: 0` marks a fallback to the last frame
/// published before rendering went quiet.
pub async fn get_canvas_frame(
    State(state): State<Arc<AppState>>,
    query: Result<Query<CanvasFrameQuery>, QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(error) => return DomainError::validation(error.body_text()).into_response(),
    };
    let request = CanvasCaptureRequest {
        width: query.width,
        height: query.height,
        ..CanvasCaptureRequest::single(query.format)
    };
    match capture_canvas(&state.preview_runtime, request).await {
        Ok(mut frames) => match frames.pop() {
            Some(frame) => canvas_frame_response(frame),
            None => {
                DomainError::service_unavailable("Canvas capture returned no frame").into_response()
            }
        },
        Err(error) => error.into_response(),
    }
}

fn canvas_frame_response(frame: CapturedCanvasFrame) -> Response {
    let mut response = (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(frame.format.mime_type()),
        )],
        frame.bytes,
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("X-Canvas-Frame-Number", frame.frame_number.into());
    headers.insert("X-Canvas-Timestamp-Ms", frame.timestamp_ms.into());
    headers.insert("X-Canvas-Width", frame.width.into());
    headers.insert("X-Canvas-Height", frame.height.into());
    headers.insert("X-Canvas-Source-Width", frame.source_width.into());
    headers.insert("X-Canvas-Source-Height", frame.source_height.into());
    headers.insert(
        "X-Canvas-Fresh",
        HeaderValue::from_static(if frame.fresh { "1" } else { "0" }),
    );
    response
}
