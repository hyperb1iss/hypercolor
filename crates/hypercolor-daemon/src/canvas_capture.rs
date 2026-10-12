//! One-shot captures of the composed effect canvas.
//!
//! The canvas lane only publishes while something subscribes to it, so a
//! capture registers its own full-resolution demand, waits for a frame the
//! render thread produced after that registration, and encodes it as a
//! standalone image. Dropping the receiver withdraws the demand, so a capture
//! leaves no standing preview cost behind.

use std::time::Duration;

use hypercolor_core::bus::CanvasFrame;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::ws::{CanvasStillFormat, encode_canvas_still};
use crate::domain::DomainError;
use crate::preview_runtime::{
    PreviewFrameReceiver, PreviewPixelFormat, PreviewRuntime, PreviewStreamDemand,
};

/// Most frames one capture may return.
pub const MAX_CAPTURE_FRAMES: u32 = 8;
/// Longest pause a burst may leave between frames.
pub const MAX_CAPTURE_INTERVAL: Duration = Duration::from_secs(2);
/// Largest output side a capture accepts.
pub const MAX_CAPTURE_DIMENSION: u32 = 4096;
/// How long a capture waits for a newly rendered frame before it settles for
/// the latest published one and reports it as not fresh.
pub const FRESH_FRAME_TIMEOUT: Duration = Duration::from_millis(1500);

/// Publication rate the capture asks for while it waits, so the first fresh
/// frame lands within one render tick instead of one preview tick.
const CAPTURE_DEMAND_FPS: u32 = 60;

/// Image encoding for a canvas capture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CanvasImageFormat {
    /// Baseline JPEG: small, slightly soft on hard color edges.
    #[default]
    Jpeg,
    /// Lossless PNG: exact pixel values, several times larger.
    Png,
}

impl CanvasImageFormat {
    /// MIME type of the encoded bytes.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }

    const fn still_format(self) -> CanvasStillFormat {
        match self {
            Self::Jpeg => CanvasStillFormat::Jpeg,
            Self::Png => CanvasStillFormat::Png,
        }
    }
}

/// What one capture should return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanvasCaptureRequest {
    /// Frame count, `1..=MAX_CAPTURE_FRAMES`.
    pub frames: u32,
    /// Pause between burst frames; ignored for a single frame.
    pub interval: Duration,
    /// Output width in pixels; `0` keeps the canvas width.
    pub width: u32,
    /// Output height in pixels; `0` keeps the canvas height.
    pub height: u32,
    /// Image encoding.
    pub format: CanvasImageFormat,
}

impl CanvasCaptureRequest {
    /// One full-resolution frame in the given encoding.
    #[must_use]
    pub const fn single(format: CanvasImageFormat) -> Self {
        Self {
            frames: 1,
            interval: Duration::ZERO,
            width: 0,
            height: 0,
            format,
        }
    }

    fn validate(&self) -> Result<(), DomainError> {
        if !(1..=MAX_CAPTURE_FRAMES).contains(&self.frames) {
            return Err(DomainError::validation_field(
                "frames",
                format!("frames must be between 1 and {MAX_CAPTURE_FRAMES}"),
            ));
        }
        if self.interval > MAX_CAPTURE_INTERVAL {
            return Err(DomainError::validation_field(
                "interval_ms",
                format!(
                    "interval_ms must be at most {}",
                    MAX_CAPTURE_INTERVAL.as_millis()
                ),
            ));
        }
        for (field, value) in [("width", self.width), ("height", self.height)] {
            if value > MAX_CAPTURE_DIMENSION {
                return Err(DomainError::validation_field(
                    field,
                    format!("{field} must be at most {MAX_CAPTURE_DIMENSION}"),
                ));
            }
        }
        Ok(())
    }
}

/// One encoded canvas frame.
#[derive(Debug, Clone)]
pub struct CapturedCanvasFrame {
    /// Render frame counter of the captured canvas.
    pub frame_number: u32,
    /// Daemon-relative timestamp of the captured canvas, in milliseconds.
    pub timestamp_ms: u32,
    /// Encoded image width.
    pub width: u32,
    /// Encoded image height.
    pub height: u32,
    /// Canvas width the frame was rendered at.
    pub source_width: u32,
    /// Canvas height the frame was rendered at.
    pub source_height: u32,
    /// Whether the render thread published this frame after the capture
    /// began. A stale frame is the last one published before the render
    /// thread went quiet, which usually means nothing is rendering.
    pub fresh: bool,
    /// Encoding of [`Self::bytes`].
    pub format: CanvasImageFormat,
    /// Encoded image file.
    pub bytes: Vec<u8>,
}

/// Capture one frame or a short burst of the composed canvas.
///
/// A burst stops early at the first stale frame, because a render thread
/// that stopped publishing would only repeat it.
///
/// # Errors
///
/// Returns a validation error for out-of-range requests, and
/// `ServiceUnavailable` when no canvas frame has been rendered at all.
pub async fn capture_canvas(
    preview: &PreviewRuntime,
    request: CanvasCaptureRequest,
) -> Result<Vec<CapturedCanvasFrame>, DomainError> {
    request.validate()?;

    let mut receiver = preview.internal_canvas_receiver(PreviewStreamDemand {
        fps: CAPTURE_DEMAND_FPS,
        format: PreviewPixelFormat::Rgba,
        width: 0,
        height: 0,
    });

    let mut captured = Vec::with_capacity(usize::try_from(request.frames).unwrap_or(1));
    for index in 0..request.frames {
        if index > 0 && !request.interval.is_zero() {
            tokio::time::sleep(request.interval).await;
        }
        let (frame, fresh) = next_frame(&mut receiver).await;
        if frame.width == 0 || frame.height == 0 {
            return Err(DomainError::service_unavailable(
                "No canvas frame has been rendered yet; apply an effect and capture again",
            ));
        }
        captured.push(encode(frame, fresh, &request).await?);
        if !fresh {
            break;
        }
    }
    Ok(captured)
}

/// Wait for the next frame published after this call, falling back to the
/// latest published frame when none arrives in time.
async fn next_frame(receiver: &mut PreviewFrameReceiver) -> (CanvasFrame, bool) {
    drop(receiver.borrow_and_update());
    let fresh = matches!(
        tokio::time::timeout(FRESH_FRAME_TIMEOUT, receiver.changed()).await,
        Ok(Ok(()))
    );
    let frame = receiver.borrow_and_update().clone();
    (frame, fresh)
}

async fn encode(
    frame: CanvasFrame,
    fresh: bool,
    request: &CanvasCaptureRequest,
) -> Result<CapturedCanvasFrame, DomainError> {
    // Never upscale: a side at or past the source falls back to the source
    // size, and the other side keeps the aspect ratio.
    let width = if request.width >= frame.width {
        0
    } else {
        request.width
    };
    let height = if request.height >= frame.height {
        0
    } else {
        request.height
    };
    let format = request.format;
    tokio::task::spawn_blocking(move || {
        let still = encode_canvas_still(&frame, format.still_format(), width, height)?;
        Ok::<_, anyhow::Error>(CapturedCanvasFrame {
            frame_number: frame.frame_number,
            timestamp_ms: frame.timestamp_ms,
            width: still.width,
            height: still.height,
            source_width: frame.width,
            source_height: frame.height,
            fresh,
            format,
            bytes: still.bytes,
        })
    })
    .await
    .map_err(|error| DomainError::Internal(anyhow::anyhow!("canvas encode task failed: {error}")))?
    .map_err(DomainError::Internal)
}
