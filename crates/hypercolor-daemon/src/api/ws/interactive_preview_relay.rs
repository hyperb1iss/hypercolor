use std::sync::Arc;

use anyhow::{Context, Result};
use axum::body::Bytes;
use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::input::BrowserInputPublicationId;
use hypercolor_leptos_ext::ws::{
    InteractivePreviewFrame as WireInteractivePreviewFrame,
    PreviewPixelFormat as WirePreviewPixelFormat,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::warn;

use super::preview_encode::{PreviewJpegEncoder, PreviewRawEncoder};
use super::protocol::CanvasFormat;
use crate::interactive_preview::InteractivePreviewFrame;
use crate::preview_runtime::PreviewPixelFormat;

pub(super) struct InteractivePreviewOutbound {
    pub(super) preview_id: String,
    pub(super) publication_id: BrowserInputPublicationId,
    pub(super) bytes: Bytes,
}

pub(super) fn spawn_interactive_preview_relay(
    preview_id: String,
    publication_id: BrowserInputPublicationId,
    mut frames: watch::Receiver<Option<Arc<InteractivePreviewFrame>>>,
    outbound: mpsc::Sender<InteractivePreviewOutbound>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut raw_encoder = PreviewRawEncoder::new();
        let mut jpeg_encoder = None;
        while frames.changed().await.is_ok() {
            let Some(frame) = frames.borrow_and_update().clone() else {
                continue;
            };
            if frame.publication_id != publication_id {
                warn!(
                    preview_id,
                    expected_publication_id = publication_id.get(),
                    actual_publication_id = frame.publication_id.get(),
                    "Dropped interactive preview frame with mismatched publication"
                );
                continue;
            }
            let bytes = match encode_frame(&preview_id, &frame, &mut raw_encoder, &mut jpeg_encoder)
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!(preview_id, %error, "Failed to encode interactive preview frame");
                    continue;
                }
            };
            let message = InteractivePreviewOutbound {
                preview_id: preview_id.clone(),
                publication_id,
                bytes,
            };
            if matches!(
                outbound.try_send(message),
                Err(mpsc::error::TrySendError::Closed(_))
            ) {
                break;
            }
        }
    })
}

fn encode_frame(
    preview_id: &str,
    frame: &InteractivePreviewFrame,
    raw_encoder: &mut PreviewRawEncoder,
    jpeg_encoder: &mut Option<PreviewJpegEncoder>,
) -> Result<Bytes> {
    let canvas = CanvasFrame::from_surface(frame.surface.clone());
    let payload = match frame.format {
        PreviewPixelFormat::Rgb => raw_encoder.encode_scaled_body(
            &canvas,
            CanvasFormat::Rgb,
            1.0,
            frame.width,
            frame.height,
        ),
        PreviewPixelFormat::Rgba => raw_encoder.encode_scaled_body(
            &canvas,
            CanvasFormat::Rgba,
            1.0,
            frame.width,
            frame.height,
        ),
        PreviewPixelFormat::Jpeg => {
            if jpeg_encoder.is_none() {
                *jpeg_encoder = Some(PreviewJpegEncoder::new()?);
            }
            jpeg_encoder
                .as_mut()
                .context("interactive preview JPEG encoder was not initialized")?
                .encode_scaled_body(&canvas, 1.0, frame.width, frame.height)?
        }
    };
    WireInteractivePreviewFrame {
        preview_id: preview_id.to_owned(),
        frame_number: frame.frame_number,
        timestamp_ms: frame.timestamp_ms,
        width: u16::try_from(frame.width)
            .context("interactive preview width exceeds wire limit")?,
        height: u16::try_from(frame.height)
            .context("interactive preview height exceeds wire limit")?,
        format: wire_format(frame.format),
        payload: Bytes::from(payload),
    }
    .encode()
    .context("failed to encode interactive preview wire frame")
}

const fn wire_format(format: PreviewPixelFormat) -> WirePreviewPixelFormat {
    match format {
        PreviewPixelFormat::Rgb => WirePreviewPixelFormat::Rgb,
        PreviewPixelFormat::Rgba => WirePreviewPixelFormat::Rgba,
        PreviewPixelFormat::Jpeg => WirePreviewPixelFormat::Jpeg,
    }
}
