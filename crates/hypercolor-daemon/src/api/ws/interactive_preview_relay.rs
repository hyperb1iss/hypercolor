use std::sync::Arc;

use anyhow::{Context, Result};
use axum::body::Bytes;
use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::input::BrowserInputPublicationId;
use hypercolor_leptos_ext::ws::{
    InteractivePreviewFrame as WireInteractivePreviewFrame,
    PreviewPixelFormat as WirePreviewPixelFormat, PreviewStreamId,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::warn;

use super::preview_encode::{PreviewJpegEncoder, PreviewRawEncoder};
use super::protocol::CanvasFormat;
use super::relays::PreviewOutboundSender;
use crate::interactive_preview::InteractivePreviewFrame;
use crate::preview_runtime::PreviewPixelFormat;

pub(super) fn spawn_interactive_preview_relay(
    preview_id: String,
    publication_id: BrowserInputPublicationId,
    mut frames: watch::Receiver<Option<Arc<InteractivePreviewFrame>>>,
    spec_generation: watch::Receiver<u64>,
    workers: crate::interactive_preview::PreviewWorkerPool,
    outbound: PreviewOutboundSender,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut encoder = Some(InteractivePreviewEncoder::new());
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
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            let worker_frame = Arc::clone(&frame);
            let worker_preview_id = preview_id.clone();
            let mut worker_encoder = encoder
                .take()
                .expect("interactive preview encoder must return before redispatch");
            if workers
                .execute(move || {
                    let result = worker_encoder.encode(&worker_preview_id, &worker_frame);
                    let _ = result_tx.send((worker_encoder, result));
                })
                .is_err()
            {
                warn!(preview_id, "Interactive preview encoder pool closed");
                break;
            }
            let Ok((returned_encoder, encoded)) = result_rx.await else {
                warn!(preview_id, "Interactive preview encoder worker stopped");
                break;
            };
            encoder = Some(returned_encoder);
            let bytes = match encoded {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!(preview_id, %error, "Failed to encode interactive preview frame");
                    continue;
                }
            };
            if frame.spec_generation != *spec_generation.borrow() {
                continue;
            }
            if let Err(error) = outbound.publish(
                PreviewStreamId::Interactive(preview_id.clone()),
                bytes,
                Some(publication_id),
            ) {
                warn!(preview_id, %error, "Rejected interactive preview publication");
            }
        }
    })
}

struct InteractivePreviewEncoder {
    raw: PreviewRawEncoder,
    jpeg: Option<PreviewJpegEncoder>,
}

impl InteractivePreviewEncoder {
    fn new() -> Self {
        Self {
            raw: PreviewRawEncoder::new(),
            jpeg: None,
        }
    }

    fn encode(&mut self, preview_id: &str, frame: &InteractivePreviewFrame) -> Result<Bytes> {
        encode_frame(preview_id, frame, &mut self.raw, &mut self.jpeg)
    }
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
        )?,
        PreviewPixelFormat::Rgba => raw_encoder.encode_scaled_body(
            &canvas,
            CanvasFormat::Rgba,
            1.0,
            frame.width,
            frame.height,
        )?,
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
        width: frame.width,
        height: frame.height,
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
