//! Automatic display output pipeline for LCD-capable devices.
//!
//! This task mirrors the latest render canvas onto connected display devices
//! without disturbing the existing LED frame routing path.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::{DynamicImage, RgbaImage};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_types::device::{DeviceId, DeviceTopologyHint};

use crate::discovery::backend_id_for_family;

const DISPLAY_ERROR_WARN_INTERVAL: Duration = Duration::from_secs(5);
const JPEG_QUALITY: u8 = 85;

/// Handle to the automatic display output task.
pub struct DisplayOutputThread {
    join_handle: Option<JoinHandle<()>>,
}

/// Shared state for the automatic display output task.
#[derive(Clone)]
pub struct DisplayOutputState {
    /// Direct device writer used for JPEG frame delivery.
    pub backend_manager: Arc<Mutex<BackendManager>>,
    /// Live registry used to discover currently renderable display devices.
    pub device_registry: DeviceRegistry,
    /// Event bus canvas stream produced by the render thread.
    pub event_bus: Arc<HypercolorBus>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct DisplayGeometry {
    width: u32,
    height: u32,
    circular: bool,
}

#[derive(Clone, Debug)]
struct DisplayTarget {
    backend_id: String,
    device_id: DeviceId,
    name: String,
    geometry: DisplayGeometry,
}

impl DisplayOutputThread {
    /// Spawn the automatic display output task.
    #[must_use]
    pub fn spawn(state: DisplayOutputState) -> Self {
        let canvas_rx = state.event_bus.canvas_receiver();
        let join_handle = tokio::spawn(run_display_output(state, canvas_rx));
        debug!("display output task spawned");
        Self {
            join_handle: Some(join_handle),
        }
    }

    /// Stop the automatic display output task.
    ///
    /// The task waits on canvas updates indefinitely, so shutdown aborts the
    /// task directly after the render thread has stopped producing frames.
    ///
    /// # Errors
    ///
    /// Returns an error if task shutdown fails unexpectedly.
    pub async fn shutdown(&mut self) -> Result<()> {
        let Some(handle) = self.join_handle.take() else {
            return Ok(());
        };

        handle.abort();
        match handle.await {
            Ok(()) => Ok(()),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(anyhow::Error::from(error))
                .context("display output task failed during shutdown"),
        }
    }
}

async fn run_display_output(
    state: DisplayOutputState,
    mut canvas_rx: watch::Receiver<CanvasFrame>,
) {
    let mut last_warned_at: HashMap<(String, DeviceId), Instant> = HashMap::new();

    loop {
        if canvas_rx.changed().await.is_err() {
            debug!("display output task exiting because canvas stream closed");
            break;
        }

        let frame = canvas_rx.borrow_and_update().clone();
        if frame.width == 0 || frame.height == 0 {
            continue;
        }

        let targets = display_targets(&state.device_registry).await;
        if targets.is_empty() {
            continue;
        }

        let mut encoded_frames = HashMap::<DisplayGeometry, Vec<u8>>::new();

        for target in targets {
            let jpeg = if let Some(existing) = encoded_frames.get(&target.geometry) {
                existing.clone()
            } else {
                match encode_canvas_frame(
                    &frame,
                    target.geometry.width,
                    target.geometry.height,
                    target.geometry.circular,
                ) {
                    Ok(encoded) => {
                        encoded_frames.insert(target.geometry.clone(), encoded.clone());
                        encoded
                    }
                    Err(error) => {
                        warn!(
                            device = %target.name,
                            backend_id = %target.backend_id,
                            device_id = %target.device_id,
                            error = %error,
                            "failed to encode display frame"
                        );
                        continue;
                    }
                }
            };

            let backend_io = {
                let manager = state.backend_manager.lock().await;
                manager.backend_io(&target.backend_id)
            };

            let result = match backend_io {
                Some(backend_io) => {
                    backend_io
                        .write_display_frame(target.device_id, &jpeg)
                        .await
                }
                None => Err(anyhow::anyhow!(
                    "backend '{}' is not registered",
                    target.backend_id
                )),
            };

            if let Err(error) = result {
                maybe_warn_display_error(&mut last_warned_at, &target, error);
            }
        }
    }
}

async fn display_targets(registry: &DeviceRegistry) -> Vec<DisplayTarget> {
    let mut targets = registry
        .list()
        .await
        .into_iter()
        .filter(|tracked| tracked.state.is_renderable())
        .filter_map(|tracked| {
            display_geometry_for_device(&tracked.info.zones)
                .or_else(|| {
                    tracked
                        .info
                        .capabilities
                        .display_resolution
                        .map(|(width, height)| DisplayGeometry {
                            width,
                            height,
                            circular: false,
                        })
                })
                .map(|geometry| DisplayTarget {
                    backend_id: backend_id_for_family(&tracked.info.family),
                    device_id: tracked.info.id,
                    name: tracked.info.name,
                    geometry,
                })
        })
        .collect::<Vec<_>>();

    targets.sort_by(|left, right| {
        left.backend_id
            .cmp(&right.backend_id)
            .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
    });
    targets
}

fn display_geometry_for_device(
    zones: &[hypercolor_types::device::ZoneInfo],
) -> Option<DisplayGeometry> {
    zones.iter().find_map(|zone| match zone.topology {
        DeviceTopologyHint::Display {
            width,
            height,
            circular,
        } => Some(DisplayGeometry {
            width,
            height,
            circular,
        }),
        _ => None,
    })
}

fn encode_canvas_frame(
    frame: &CanvasFrame,
    width: u32,
    height: u32,
    circular: bool,
) -> Result<Vec<u8>> {
    let source = RgbaImage::from_raw(frame.width, frame.height, frame.rgba_bytes().to_vec())
        .context("canvas frame RGBA payload length does not match its dimensions")?;
    let cropped = crop_to_aspect(&source, width, height);
    let mut resized = imageops::resize(&cropped, width, height, FilterType::Triangle);
    if circular {
        apply_circular_mask(&mut resized);
    }

    let mut jpeg = Vec::new();
    let image = DynamicImage::ImageRgba8(resized);
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY);
    encoder
        .encode_image(&image)
        .context("failed to JPEG-encode display frame")?;
    Ok(jpeg)
}

fn crop_to_aspect(image: &RgbaImage, target_width: u32, target_height: u32) -> RgbaImage {
    if image.width() == 0 || image.height() == 0 || target_width == 0 || target_height == 0 {
        return image.clone();
    }

    let source_width = u64::from(image.width());
    let source_height = u64::from(image.height());
    let target_width_u64 = u64::from(target_width);
    let target_height_u64 = u64::from(target_height);

    let (crop_width, crop_height) = if source_width.saturating_mul(target_height_u64)
        > source_height.saturating_mul(target_width_u64)
    {
        let width = ((source_height.saturating_mul(target_width_u64)) / target_height_u64).max(1);
        (
            u32::try_from(width).unwrap_or(u32::MAX).min(image.width()),
            image.height(),
        )
    } else {
        let height = ((source_width.saturating_mul(target_height_u64)) / target_width_u64).max(1);
        (
            image.width(),
            u32::try_from(height)
                .unwrap_or(u32::MAX)
                .min(image.height()),
        )
    };

    let offset_x = image.width().saturating_sub(crop_width) / 2;
    let offset_y = image.height().saturating_sub(crop_height) / 2;
    imageops::crop_imm(image, offset_x, offset_y, crop_width, crop_height).to_image()
}

fn apply_circular_mask(image: &mut RgbaImage) {
    let width = i64::from(image.width());
    let height = i64::from(image.height());
    let radius = width.min(height);
    let radius_sq = radius.saturating_mul(radius);

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = i64::from(x).saturating_mul(2).saturating_add(1) - width;
        let dy = i64::from(y).saturating_mul(2).saturating_add(1) - height;
        let distance_sq = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
        if distance_sq > radius_sq {
            *pixel = image::Rgba([0, 0, 0, 255]);
        }
    }
}

fn maybe_warn_display_error(
    last_warned_at: &mut HashMap<(String, DeviceId), Instant>,
    target: &DisplayTarget,
    error: anyhow::Error,
) {
    let key = (target.backend_id.clone(), target.device_id);
    let should_warn = last_warned_at
        .get(&key)
        .is_none_or(|last| last.elapsed() >= DISPLAY_ERROR_WARN_INTERVAL);
    if !should_warn {
        trace!(
            device = %target.name,
            backend_id = %target.backend_id,
            device_id = %target.device_id,
            error = %error,
            "suppressing repeated display write error"
        );
        return;
    }

    last_warned_at.insert(key, Instant::now());
    warn!(
        device = %target.name,
        backend_id = %target.backend_id,
        device_id = %target.device_id,
        error = %error,
        "failed to push automatic display frame"
    );
}
