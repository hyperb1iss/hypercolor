//! Automatic display output pipeline for LCD-capable devices.
//!
//! This task renders the latest canvas into layout-mapped display zones without
//! disturbing the existing LED frame routing path.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, RgbaImage};
use tokio::sync::{Mutex, RwLock, watch};
use tokio::task::JoinHandle;
use tracing::{debug, trace, warn};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::device::{DeviceId, DeviceTopologyHint};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition, SpatialLayout};

use crate::discovery::backend_id_for_family;
use crate::logical_devices::{self, LogicalDevice};

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
    /// Active spatial layout used to decide which LCDs should render and how.
    pub spatial_engine: Arc<RwLock<SpatialEngine>>,
    /// Logical-device aliases used to match physical devices to layout zones.
    pub logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
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
    viewport: DisplayViewport,
}

#[derive(Clone, Debug)]
struct DisplayViewport {
    position: NormalizedPosition,
    size: NormalizedPosition,
    rotation: f32,
    scale: f32,
    edge_behavior: EdgeBehavior,
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

        let targets = display_targets(
            &state.device_registry,
            &state.spatial_engine,
            &state.logical_devices,
        )
        .await;
        if targets.is_empty() {
            continue;
        }

        let Some(source) =
            RgbaImage::from_raw(frame.width, frame.height, frame.rgba_bytes().to_vec())
        else {
            warn!(
                width = frame.width,
                height = frame.height,
                payload_bytes = frame.rgba_bytes().len(),
                "canvas frame RGBA payload length does not match its dimensions"
            );
            continue;
        };

        for target in targets {
            let jpeg = match encode_canvas_frame(&source, &target.viewport, &target.geometry) {
                Ok(encoded) => encoded,
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
                maybe_warn_display_error(&mut last_warned_at, &target, &error);
            }
        }
    }
}

async fn display_targets(
    registry: &DeviceRegistry,
    spatial_engine: &Arc<RwLock<SpatialEngine>>,
    logical_devices: &Arc<RwLock<HashMap<String, LogicalDevice>>>,
) -> Vec<DisplayTarget> {
    let layout = {
        let spatial = spatial_engine.read().await;
        spatial.layout().clone()
    };
    let logical_store = logical_devices.read().await;

    let mut targets = registry
        .list()
        .await
        .into_iter()
        .filter(|tracked| tracked.state.is_renderable())
        .filter_map(|tracked| {
            let geometry = display_geometry_for_device(&tracked.info.zones).or_else(|| {
                tracked
                    .info
                    .capabilities
                    .display_resolution
                    .map(|(width, height)| DisplayGeometry {
                        width,
                        height,
                        circular: false,
                    })
            })?;
            let viewport = display_viewport_for_device(&layout, &logical_store, tracked.info.id)?;

            Some(DisplayTarget {
                backend_id: backend_id_for_family(&tracked.info.family),
                device_id: tracked.info.id,
                name: tracked.info.name,
                geometry,
                viewport,
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
    source: &RgbaImage,
    viewport: &DisplayViewport,
    geometry: &DisplayGeometry,
) -> Result<Vec<u8>> {
    let mut rendered = render_display_view(source, viewport, geometry.width, geometry.height);
    if geometry.circular {
        apply_circular_mask(&mut rendered);
    }

    let mut jpeg = Vec::new();
    let image = DynamicImage::ImageRgba8(rendered);
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY);
    encoder
        .encode_image(&image)
        .context("failed to JPEG-encode display frame")?;
    Ok(jpeg)
}

fn display_viewport_for_device(
    layout: &SpatialLayout,
    logical_store: &HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
) -> Option<DisplayViewport> {
    let aliases = layout_device_aliases(logical_store, physical_device_id);
    let zone = layout
        .zones
        .iter()
        .find(|zone| aliases.iter().any(|candidate| candidate == &zone.device_id))?;

    Some(DisplayViewport {
        position: zone.position,
        size: zone.size,
        rotation: zone.rotation,
        scale: zone.scale,
        edge_behavior: zone.edge_behavior.unwrap_or(layout.default_edge_behavior),
    })
}

fn layout_device_aliases(
    logical_store: &HashMap<String, LogicalDevice>,
    physical_device_id: DeviceId,
) -> Vec<String> {
    let mut aliases = logical_devices::list_for_physical(logical_store, physical_device_id)
        .into_iter()
        .map(|entry| entry.id)
        .collect::<Vec<_>>();

    let physical_alias = physical_device_id.to_string();
    if !aliases.iter().any(|candidate| candidate == &physical_alias) {
        aliases.push(physical_alias);
    }

    let legacy_alias = format!("device:{physical_device_id}");
    if !aliases.iter().any(|candidate| candidate == &legacy_alias) {
        aliases.push(legacy_alias);
    }

    aliases
}

#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn render_display_view(
    source: &RgbaImage,
    viewport: &DisplayViewport,
    width: u32,
    height: u32,
) -> RgbaImage {
    let mut rendered = RgbaImage::new(width, height);
    if width == 0 || height == 0 || source.width() == 0 || source.height() == 0 {
        return rendered;
    }

    let width_f32 = width as f32;
    let height_f32 = height as f32;

    for y in 0..height {
        for x in 0..width {
            let local = NormalizedPosition::new(
                (x as f32 + 0.5) / width_f32,
                (y as f32 + 0.5) / height_f32,
            );
            let canvas_pos = viewport_local_to_canvas(local, viewport);
            let pixel = sample_image_bilinear(source, canvas_pos, viewport.edge_behavior);
            rendered.put_pixel(x, y, pixel);
        }
    }

    rendered
}

fn viewport_local_to_canvas(
    local: NormalizedPosition,
    viewport: &DisplayViewport,
) -> NormalizedPosition {
    let sx = (local.x - 0.5) * viewport.size.x * viewport.scale;
    let sy = (local.y - 0.5) * viewport.size.y * viewport.scale;

    let cos_t = viewport.rotation.cos();
    let sin_t = viewport.rotation.sin();
    let rx = sx.mul_add(cos_t, -sy * sin_t);
    let ry = sx.mul_add(sin_t, sy * cos_t);

    NormalizedPosition::new(viewport.position.x + rx, viewport.position.y + ry)
}

fn sample_image_bilinear(
    source: &RgbaImage,
    canvas_pos: NormalizedPosition,
    edge_behavior: EdgeBehavior,
) -> image::Rgba<u8> {
    let sample_x = apply_edge_normalized(canvas_pos.x, edge_behavior).clamp(0.0, 1.0);
    let sample_y = apply_edge_normalized(canvas_pos.y, edge_behavior).clamp(0.0, 1.0);

    let sampled = bilinear_sample(source, sample_x, sample_y);
    apply_fade_to_black(sampled, canvas_pos, edge_behavior)
}

fn apply_edge_normalized(value: f32, edge_behavior: EdgeBehavior) -> f32 {
    match edge_behavior {
        EdgeBehavior::Clamp => value.clamp(0.0, 1.0),
        EdgeBehavior::Wrap => value.rem_euclid(1.0),
        EdgeBehavior::Mirror => {
            let period = value.rem_euclid(2.0);
            if period >= 1.0 { 2.0 - period } else { period }
        }
        EdgeBehavior::FadeToBlack { .. } => value,
    }
}

#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn bilinear_sample(source: &RgbaImage, nx: f32, ny: f32) -> image::Rgba<u8> {
    let fx = nx * (source.width().saturating_sub(1) as f32);
    let fy = ny * (source.height().saturating_sub(1) as f32);

    let x0 = fx.floor() as u32;
    let y0 = fy.floor() as u32;
    let x1 = (x0 + 1).min(source.width().saturating_sub(1));
    let y1 = (y0 + 1).min(source.height().saturating_sub(1));

    let frac_x = fx.fract();
    let frac_y = fy.fract();

    let tl = source.get_pixel(x0, y0).0;
    let tr = source.get_pixel(x1, y0).0;
    let bl = source.get_pixel(x0, y1).0;
    let br = source.get_pixel(x1, y1).0;

    let top_r = lerp_channel(tl[0], tr[0], frac_x);
    let top_g = lerp_channel(tl[1], tr[1], frac_x);
    let top_b = lerp_channel(tl[2], tr[2], frac_x);
    let top_a = lerp_channel(tl[3], tr[3], frac_x);

    let bottom_r = lerp_channel(bl[0], br[0], frac_x);
    let bottom_g = lerp_channel(bl[1], br[1], frac_x);
    let bottom_b = lerp_channel(bl[2], br[2], frac_x);
    let bottom_a = lerp_channel(bl[3], br[3], frac_x);

    image::Rgba([
        lerp_f32(top_r, bottom_r, frac_y).round().clamp(0.0, 255.0) as u8,
        lerp_f32(top_g, bottom_g, frac_y).round().clamp(0.0, 255.0) as u8,
        lerp_f32(top_b, bottom_b, frac_y).round().clamp(0.0, 255.0) as u8,
        lerp_f32(top_a, bottom_a, frac_y).round().clamp(0.0, 255.0) as u8,
    ])
}

fn lerp_channel(left: u8, right: u8, amount: f32) -> f32 {
    lerp_f32(f32::from(left), f32::from(right), amount)
}

fn lerp_f32(left: f32, right: f32, amount: f32) -> f32 {
    left + (right - left) * amount
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn apply_fade_to_black(
    pixel: image::Rgba<u8>,
    canvas_pos: NormalizedPosition,
    edge_behavior: EdgeBehavior,
) -> image::Rgba<u8> {
    let EdgeBehavior::FadeToBlack { falloff } = edge_behavior else {
        return pixel;
    };

    let dx = if canvas_pos.x < 0.0 {
        -canvas_pos.x
    } else if canvas_pos.x > 1.0 {
        canvas_pos.x - 1.0
    } else {
        0.0
    };
    let dy = if canvas_pos.y < 0.0 {
        -canvas_pos.y
    } else if canvas_pos.y > 1.0 {
        canvas_pos.y - 1.0
    } else {
        0.0
    };

    let distance = (dx.mul_add(dx, dy * dy)).sqrt();
    if distance <= 0.0 {
        return pixel;
    }

    let attenuation = (-distance * falloff).exp().clamp(0.0, 1.0);
    image::Rgba([
        (f32::from(pixel[0]) * attenuation)
            .round()
            .clamp(0.0, 255.0) as u8,
        (f32::from(pixel[1]) * attenuation)
            .round()
            .clamp(0.0, 255.0) as u8,
        (f32::from(pixel[2]) * attenuation)
            .round()
            .clamp(0.0, 255.0) as u8,
        pixel[3],
    ])
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
    error: &anyhow::Error,
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
