//! Canvas tools: let an agent see what the engine is rendering.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, ToolImage, ToolResponse, output_schema, serialize_result};
use crate::app_state::AppState;
use crate::canvas_capture::{
    CanvasCaptureRequest, CanvasImageFormat, MAX_CAPTURE_DIMENSION, MAX_CAPTURE_FRAMES,
    MAX_CAPTURE_INTERVAL, capture_canvas,
};
use crate::mcp::results::{CanvasCaptureFrameResult, CanvasCaptureResult};

/// Default output width. Large enough to judge an LED effect, small enough
/// that a burst stays cheap for a vision model.
const DEFAULT_CAPTURE_WIDTH: u64 = 320;
const DEFAULT_CAPTURE_INTERVAL_MS: u64 = 250;
const MIN_CAPTURE_WIDTH: u64 = 16;

pub(super) fn build_capture_canvas() -> ToolDefinition {
    ToolDefinition {
        name: "capture_canvas".into(),
        title: "Capture Canvas".into(),
        description: "Look at what Hypercolor is rendering right now. Returns the composed effect canvas as images, one per frame, plus frame metadata. Use frames above 1 with interval_ms to judge motion and speed. The canvas is what effects draw at full brightness before LED sampling; the physical LEDs show a sampled, brightness-scaled version of it. A frame with fresh=false is the last one published before rendering went quiet, so nothing is currently drawing.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "frames": {
                    "type": "integer",
                    "description": "How many frames to capture",
                    "default": 1,
                    "minimum": 1,
                    "maximum": MAX_CAPTURE_FRAMES
                },
                "interval_ms": {
                    "type": "integer",
                    "description": "Pause between frames in a burst, in milliseconds",
                    "default": DEFAULT_CAPTURE_INTERVAL_MS,
                    "minimum": 0,
                    "maximum": MAX_CAPTURE_INTERVAL.as_millis()
                },
                "width": {
                    "type": "integer",
                    "description": "Output width in pixels; height follows the canvas aspect ratio. Never upscales past the canvas.",
                    "default": DEFAULT_CAPTURE_WIDTH,
                    "minimum": MIN_CAPTURE_WIDTH,
                    "maximum": MAX_CAPTURE_DIMENSION
                },
                "format": {
                    "type": "string",
                    "enum": ["jpeg", "png"],
                    "description": "jpeg is small; png keeps exact pixel values for color checks",
                    "default": "jpeg"
                }
            },
            "additionalProperties": false
        }),
        output_schema: output_schema::<CanvasCaptureResult>(),
        read_only: true,
        destructive: false,
        idempotent: false,
    }
}

pub(super) async fn handle_capture_canvas_with_state(
    params: &Value,
    state: &AppState,
) -> Result<ToolResponse, ToolError> {
    let frames = params.get("frames").and_then(Value::as_u64).unwrap_or(1);
    let interval_ms = params
        .get("interval_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_CAPTURE_INTERVAL_MS);
    let width = params
        .get("width")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_CAPTURE_WIDTH);
    let format = match params.get("format").and_then(Value::as_str) {
        None | Some("jpeg") => CanvasImageFormat::Jpeg,
        Some("png") => CanvasImageFormat::Png,
        Some(other) => {
            return Err(ToolError::InvalidParam {
                param: "format".into(),
                reason: format!("unsupported format '{other}'"),
            });
        }
    };

    let request = CanvasCaptureRequest {
        frames: u32::try_from(frames).unwrap_or(u32::MAX),
        interval: Duration::from_millis(interval_ms),
        width: u32::try_from(width).unwrap_or(u32::MAX),
        height: 0,
        format,
    };
    let captured = capture_canvas(&state.preview_runtime, request).await?;

    let (source_width, source_height) = captured
        .first()
        .map_or((0, 0), |frame| (frame.source_width, frame.source_height));
    let mut images = Vec::with_capacity(captured.len());
    let mut frames = Vec::with_capacity(captured.len());
    for frame in captured {
        frames.push(CanvasCaptureFrameResult {
            frame_number: frame.frame_number,
            timestamp_ms: frame.timestamp_ms,
            width: frame.width,
            height: frame.height,
            fresh: frame.fresh,
            mime_type: frame.format.mime_type().to_owned(),
        });
        images.push(ToolImage {
            data: BASE64_STANDARD.encode(&frame.bytes),
            mime_type: frame.format.mime_type(),
        });
    }

    let payload = serialize_result(CanvasCaptureResult {
        frames,
        source_width,
        source_height,
    })?;
    Ok(ToolResponse { payload, images })
}
