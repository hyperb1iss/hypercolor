//! Automatic display output pipeline for LCD-capable devices.
//!
//! This task renders the latest canvas into layout-mapped display zones without
//! disturbing the existing LED frame routing path.

use std::any::Any;
use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Interval, MissedTickBehavior};
use tracing::{debug, info, trace, warn};
use turbojpeg::{
    Compressor as TurboJpegCompressor, Image as TurboJpegImage,
    PixelFormat as TurboJpegPixelFormat, Subsamp as TurboJpegSubsamp,
    compressed_buf_len as turbojpeg_compressed_buf_len,
};

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendIo, BackendManager, DeviceRegistry};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::device::{DeviceId, DeviceTopologyHint};
use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition, SpatialLayout};

use crate::discovery::backend_id_for_device;
use crate::logical_devices::{self, LogicalDevice};

const DISPLAY_ERROR_WARN_INTERVAL: Duration = Duration::from_secs(5);
const DISPLAY_OUTPUT_MAX_FPS: u32 = 15;
const JPEG_QUALITY: u8 = 85;
const JPEG_SUBSAMP: TurboJpegSubsamp = TurboJpegSubsamp::Sub2x2;
const BILINEAR_WEIGHT_SCALE: u32 = 256;
const BILINEAR_WEIGHT_ROUNDING: u32 = (BILINEAR_WEIGHT_SCALE * BILINEAR_WEIGHT_SCALE) / 2;

/// Handle to the automatic display output task.
pub struct DisplayOutputThread {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: Option<std::thread::JoinHandle<()>>,
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
    target_fps: u32,
    brightness: f32,
    geometry: DisplayGeometry,
    viewport: DisplayViewport,
}

type DisplayWorkerKey = (String, DeviceId);

#[derive(Default)]
struct DisplayTargetCache {
    initialized: bool,
    registry_generation: u64,
    layout_ptr: usize,
    logical_signature: u64,
    targets: Vec<DisplayTarget>,
}

#[derive(Clone)]
struct DisplayWorkItem {
    source: CanvasFrame,
    target: DisplayTarget,
}

struct DisplayWorkerHandle {
    tx: watch::Sender<Option<Arc<DisplayWorkItem>>>,
    join_handle: JoinHandle<()>,
    target_fps: u32,
}

#[derive(Clone, Debug)]
struct DisplayViewport {
    position: NormalizedPosition,
    size: NormalizedPosition,
    rotation: f32,
    scale: f32,
    edge_behavior: EdgeBehavior,
}

struct DisplayEncodeState {
    rgb_buffer: Vec<u8>,
    jpeg_buffer: Vec<u8>,
    jpeg_compressor: TurboJpegCompressor,
    axis_plan: Option<PreparedDisplayPlan>,
}

impl DisplayEncodeState {
    fn new() -> Result<Self> {
        let mut jpeg_compressor =
            TurboJpegCompressor::new().context("failed to initialize TurboJPEG display encoder")?;
        jpeg_compressor
            .set_quality(i32::from(JPEG_QUALITY))
            .context("failed to configure TurboJPEG quality")?;
        jpeg_compressor
            .set_subsamp(JPEG_SUBSAMP)
            .context("failed to configure TurboJPEG chroma subsampling")?;

        Ok(Self {
            rgb_buffer: Vec::new(),
            jpeg_buffer: Vec::new(),
            jpeg_compressor,
            axis_plan: None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedDisplayPlanKey {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    edge_behavior: u8,
    start_x_bits: u32,
    start_y_bits: u32,
    span_x_bits: u32,
    span_y_bits: u32,
}

#[derive(Clone, Debug)]
struct PreparedDisplayPlan {
    key: PreparedDisplayPlanKey,
    samples: Vec<PreparedDisplaySample>,
}

#[derive(Clone, Copy, Debug)]
struct PreparedDisplaySample {
    offsets: [usize; 4],
    x_lower_weight: u16,
    x_upper_weight: u16,
    y_lower_weight: u16,
    y_upper_weight: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DisplayFrameInputState {
    source_width: u32,
    source_height: u32,
    brightness_factor: u16,
    geometry: DisplayGeometry,
    viewport: DisplayViewportSignature,
    source_rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayViewportSignature {
    position_x_bits: u32,
    position_y_bits: u32,
    size_x_bits: u32,
    size_y_bits: u32,
    rotation_bits: u32,
    scale_bits: u32,
    edge_behavior: u8,
    fade_falloff_bits: u32,
}

impl DisplayFrameInputState {
    fn matches(
        &self,
        source: &CanvasFrame,
        viewport: &DisplayViewport,
        geometry: &DisplayGeometry,
        brightness: f32,
    ) -> bool {
        self.source_width == source.width
            && self.source_height == source.height
            && self.brightness_factor == display_brightness_factor(brightness)
            && self.geometry == *geometry
            && self.viewport == display_viewport_signature(viewport)
            && self.source_rgba.as_slice() == source.rgba_bytes()
    }

    fn capture(
        source: &CanvasFrame,
        viewport: &DisplayViewport,
        geometry: &DisplayGeometry,
        brightness: f32,
    ) -> Self {
        Self {
            source_width: source.width,
            source_height: source.height,
            brightness_factor: display_brightness_factor(brightness),
            geometry: geometry.clone(),
            viewport: display_viewport_signature(viewport),
            source_rgba: source.rgba_bytes().to_vec(),
        }
    }
}

impl DisplayWorkerHandle {
    fn spawn(target: &DisplayTarget, backend_io: BackendIo) -> Self {
        let (tx, rx) = watch::channel(None::<Arc<DisplayWorkItem>>);
        let worker_backend_id = target.backend_id.clone();
        let worker_device_id = target.device_id;
        let target_fps = target.target_fps;
        let join_handle = tokio::spawn(run_display_worker(
            backend_io,
            worker_backend_id,
            worker_device_id,
            target_fps,
            rx,
        ));

        Self {
            tx,
            join_handle,
            target_fps,
        }
    }

    fn push(&self, work: DisplayWorkItem) {
        self.tx.send_replace(Some(Arc::new(work)));
    }

    async fn shutdown(self) {
        drop(self.tx);
        let _ = self.join_handle.await;
    }
}

impl DisplayOutputThread {
    /// Spawn the automatic display output task.
    #[must_use]
    pub fn spawn(state: DisplayOutputState) -> Self {
        let canvas_rx = state.event_bus.canvas_receiver();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let join_handle = std::thread::Builder::new()
            .name("hypercolor-display".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("display output runtime should initialize");
                runtime.block_on(run_display_output(state, canvas_rx, shutdown_rx));
            })
            .expect("display output thread should spawn");
        info!("display output thread spawned");
        Self {
            shutdown_tx: Some(shutdown_tx),
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
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        let Some(handle) = self.join_handle.take() else {
            return Ok(());
        };

        tokio::task::spawn_blocking(move || {
            handle.join().map_err(|panic| {
                anyhow!(
                    "display output thread panicked: {}",
                    panic_payload_message(panic.as_ref())
                )
            })
        })
        .await
        .context("failed to join display output thread")??;
        info!("display output thread stopped");
        Ok(())
    }
}

async fn run_display_output(
    state: DisplayOutputState,
    mut canvas_rx: watch::Receiver<CanvasFrame>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let mut workers = HashMap::<DisplayWorkerKey, DisplayWorkerHandle>::new();
    let mut targets_cache = DisplayTargetCache::default();

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                debug!("display output task shutting down");
                break;
            }
            changed = canvas_rx.changed() => {
                if changed.is_err() {
                    debug!("display output task exiting because canvas stream closed");
                    break;
                }
            }
        }

        let has_canvas_frame = {
            let frame = canvas_rx.borrow_and_update();
            frame.width != 0 && frame.height != 0
        };
        if !has_canvas_frame {
            continue;
        }

        let targets = display_targets(
            &state.device_registry,
            &state.spatial_engine,
            &state.logical_devices,
            &mut targets_cache,
        )
        .await;
        reconcile_display_workers(&state, &mut workers, &targets).await;
        if targets.is_empty() {
            continue;
        }

        // `watch` gives us latest-value semantics, so after target discovery we can
        // cheaply snapshot the newest frame instead of cloning every canvas update
        // while no display target is active.
        let frame = canvas_rx.borrow().clone();

        for target in targets {
            let key = display_worker_key(&target);
            if let Some(worker) = workers.get(&key) {
                worker.push(DisplayWorkItem {
                    source: frame.clone(),
                    target,
                });
            }
        }
    }

    for (_, worker) in workers {
        worker.shutdown().await;
    }
}

async fn reconcile_display_workers(
    state: &DisplayOutputState,
    workers: &mut HashMap<DisplayWorkerKey, DisplayWorkerHandle>,
    targets: &[DisplayTarget],
) {
    let expected_keys = targets
        .iter()
        .map(display_worker_key)
        .collect::<HashSet<_>>();
    let stale_keys = workers
        .keys()
        .filter(|key| !expected_keys.contains(*key))
        .cloned()
        .collect::<Vec<_>>();

    for key in stale_keys {
        if let Some(worker) = workers.remove(&key) {
            worker.shutdown().await;
        }
    }

    for target in targets {
        let key = display_worker_key(target);
        let needs_restart = workers
            .get(&key)
            .is_some_and(|worker| worker.target_fps != target.target_fps);
        if needs_restart && let Some(worker) = workers.remove(&key) {
            worker.shutdown().await;
        }

        if workers.contains_key(&key) {
            continue;
        }

        let backend_io = {
            let manager = state.backend_manager.lock().await;
            manager.backend_io(&target.backend_id)
        };

        match backend_io {
            Some(backend_io) => {
                workers.insert(key, DisplayWorkerHandle::spawn(target, backend_io));
            }
            None => {
                warn!(
                    device = %target.name,
                    backend_id = %target.backend_id,
                    device_id = %target.device_id,
                    "display target backend is not registered"
                );
            }
        }
    }
}

async fn run_display_worker(
    backend_io: BackendIo,
    backend_key: String,
    device_id: DeviceId,
    target_fps: u32,
    mut rx: watch::Receiver<Option<Arc<DisplayWorkItem>>>,
) {
    let mut interval = interval_for_target_fps(target_fps);
    let mut last_warned_at = None::<Instant>;
    let mut last_delivered_input = None::<DisplayFrameInputState>;
    let mut encode_state = match DisplayEncodeState::new() {
        Ok(state) => state,
        Err(error) => {
            warn!(
                backend_id = %backend_key,
                device_id = %device_id,
                error = %error,
                "display worker failed to initialize encoder state"
            );
            return;
        }
    };

    loop {
        if rx.changed().await.is_err() {
            break;
        }

        let Some(work) = rx.borrow_and_update().clone() else {
            continue;
        };

        if let Some(ref mut limiter) = interval {
            limiter.tick().await;
        }

        let source = work.source.clone();
        let viewport = work.target.viewport.clone();
        let geometry = work.target.geometry.clone();
        let brightness = work.target.brightness;
        let target = work.target.clone();
        if last_delivered_input
            .as_ref()
            .is_some_and(|previous| previous.matches(&source, &viewport, &geometry, brightness))
        {
            trace!(
                device = %target.name,
                backend_id = %backend_key,
                device_id = %device_id,
                target_fps,
                "skipping unchanged display frame"
            );
            continue;
        }
        let encode_source = source.clone();
        let encode_viewport = viewport.clone();
        let encode_geometry = geometry.clone();
        let encode_result = tokio::task::spawn_blocking(move || {
            let mut encode_state = encode_state;
            let encoded = encode_canvas_frame(
                &encode_source,
                &encode_viewport,
                &encode_geometry,
                brightness,
                &mut encode_state,
            );
            (encode_state, encoded)
        })
        .await;

        let jpeg = match encode_result {
            Ok((returned_state, Ok(encoded))) => {
                encode_state = returned_state;
                Arc::new(encoded)
            }
            Ok((returned_state, Err(error))) => {
                encode_state = returned_state;
                maybe_warn_display_error(&mut last_warned_at, &target, &error);
                continue;
            }
            Err(error) => {
                match DisplayEncodeState::new() {
                    Ok(state) => {
                        encode_state = state;
                    }
                    Err(init_error) => {
                        warn!(
                            backend_id = %backend_key,
                            device_id = %device_id,
                            error = %init_error,
                            "display worker could not recover encoder state after a blocking-task failure"
                        );
                        break;
                    }
                }
                maybe_warn_display_error(
                    &mut last_warned_at,
                    &target,
                    &anyhow!("display encode worker failed: {error}"),
                );
                continue;
            }
        };

        let jpeg_bytes = jpeg.len();
        let write_result = backend_io
            .write_display_frame_owned(device_id, Arc::clone(&jpeg))
            .await;
        if let Some(reusable_jpeg) = Arc::into_inner(jpeg) {
            encode_state.jpeg_buffer = reusable_jpeg;
        }
        if let Err(error) = write_result {
            maybe_warn_display_error(&mut last_warned_at, &target, &error);
            continue;
        }
        last_delivered_input = Some(DisplayFrameInputState::capture(
            &source, &viewport, &geometry, brightness,
        ));

        trace!(
            device = %target.name,
            backend_id = %backend_key,
            device_id = %device_id,
            jpeg_bytes,
            target_fps,
            "display frame delivered"
        );
    }
}

fn display_worker_key(target: &DisplayTarget) -> DisplayWorkerKey {
    (target.backend_id.clone(), target.device_id)
}

fn interval_for_target_fps(target_fps: u32) -> Option<Interval> {
    if target_fps == 0 {
        return None;
    }

    let mut interval = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(target_fps)));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    Some(interval)
}

async fn display_targets(
    registry: &DeviceRegistry,
    spatial_engine: &Arc<RwLock<SpatialEngine>>,
    logical_devices: &Arc<RwLock<HashMap<String, LogicalDevice>>>,
    cache: &mut DisplayTargetCache,
) -> Vec<DisplayTarget> {
    let layout = {
        let spatial = spatial_engine.read().await;
        spatial.layout()
    };
    let logical_store = logical_devices.read().await;
    let registry_generation = registry.generation();
    let layout_ptr = Arc::as_ptr(&layout) as usize;
    let logical_signature = logical_device_store_signature(&logical_store);

    if cache.initialized
        && cache.registry_generation == registry_generation
        && cache.layout_ptr == layout_ptr
        && cache.logical_signature == logical_signature
    {
        return cache.targets.clone();
    }

    let mut targets = Vec::new();
    for tracked in registry
        .list()
        .await
        .into_iter()
        .filter(|tracked| tracked.state.is_renderable())
    {
        let metadata = registry.metadata_for_id(&tracked.info.id).await;
        let Some(geometry) = display_geometry_for_device(&tracked.info.zones).or_else(|| {
            tracked
                .info
                .capabilities
                .display_resolution
                .map(|(width, height)| DisplayGeometry {
                    width,
                    height,
                    circular: false,
                })
        }) else {
            continue;
        };
        let Some(viewport) =
            display_viewport_for_device(layout.as_ref(), &logical_store, tracked.info.id)
        else {
            continue;
        };

        targets.push(DisplayTarget {
            backend_id: backend_id_for_device(&tracked.info.family, metadata.as_ref()),
            device_id: tracked.info.id,
            name: tracked.info.name,
            target_fps: capped_display_target_fps(tracked.info.capabilities.max_fps),
            brightness: tracked.user_settings.brightness,
            geometry,
            viewport,
        });
    }

    targets.sort_by(|left, right| {
        left.backend_id
            .cmp(&right.backend_id)
            .then(left.device_id.to_string().cmp(&right.device_id.to_string()))
    });
    cache.initialized = true;
    cache.registry_generation = registry_generation;
    cache.layout_ptr = layout_ptr;
    cache.logical_signature = logical_signature;
    cache.targets = targets.clone();
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
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    geometry: &DisplayGeometry,
    brightness: f32,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    render_display_view(
        source,
        viewport,
        geometry.width,
        geometry.height,
        &mut encode_state.rgb_buffer,
        &mut encode_state.axis_plan,
    );
    apply_display_brightness(&mut encode_state.rgb_buffer, brightness);
    if geometry.circular {
        apply_circular_mask(
            &mut encode_state.rgb_buffer,
            geometry.width,
            geometry.height,
        );
    }

    encode_rgb_to_jpeg(geometry, encode_state)
}

fn encode_rgb_to_jpeg(
    geometry: &DisplayGeometry,
    encode_state: &mut DisplayEncodeState,
) -> Result<Vec<u8>> {
    let width = usize::try_from(geometry.width).context("display width does not fit usize")?;
    let height = usize::try_from(geometry.height).context("display height does not fit usize")?;
    let pitch = width
        .checked_mul(TurboJpegPixelFormat::RGB.size())
        .context("display row pitch overflow")?;
    let required_len = turbojpeg_compressed_buf_len(width, height, JPEG_SUBSAMP)
        .context("failed to size TurboJPEG display buffer")?;

    let mut jpeg_buffer = std::mem::take(&mut encode_state.jpeg_buffer);
    if jpeg_buffer.len() < required_len {
        jpeg_buffer.resize(required_len, 0);
    } else {
        jpeg_buffer.truncate(required_len);
    }

    let image = TurboJpegImage {
        pixels: encode_state.rgb_buffer.as_slice(),
        width,
        pitch,
        height,
        format: TurboJpegPixelFormat::RGB,
    };
    let jpeg_len = match encode_state
        .jpeg_compressor
        .compress_to_slice(image, jpeg_buffer.as_mut_slice())
    {
        Ok(len) => len,
        Err(error) => {
            encode_state.jpeg_buffer = jpeg_buffer;
            return Err(error).context("failed to TurboJPEG-encode display frame");
        }
    };

    jpeg_buffer.truncate(jpeg_len);
    Ok(jpeg_buffer)
}

fn apply_display_brightness(image: &mut [u8], brightness: f32) {
    let factor = display_brightness_factor(brightness);
    if factor >= u16::from(u8::MAX) {
        return;
    }
    if factor == 0 {
        image.fill(0);
        return;
    }

    for pixel in image.chunks_exact_mut(3) {
        pixel[0] = scale_channel(pixel[0], factor);
        pixel[1] = scale_channel(pixel[1], factor);
        pixel[2] = scale_channel(pixel[2], factor);
    }
}

fn scale_channel(channel: u8, factor: u16) -> u8 {
    let scaled = (u16::from(channel) * factor) / u16::from(u8::MAX);
    u8::try_from(scaled).expect("display brightness scaling should remain within byte range")
}

fn display_brightness_factor(brightness: f32) -> u16 {
    round_unit_to_u16(brightness.clamp(0.0, 1.0))
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
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    width: u32,
    height: u32,
    rendered_rgb: &mut Vec<u8>,
    axis_plan: &mut Option<PreparedDisplayPlan>,
) {
    let Some(render_len) = rgb_buffer_len(width, height) else {
        rendered_rgb.clear();
        return;
    };
    rendered_rgb.clear();
    rendered_rgb.resize(render_len, 0);

    if width == 0 || height == 0 || source.width == 0 || source.height == 0 {
        return;
    }

    if viewport.rotation.abs() <= f32::EPSILON
        && !matches!(viewport.edge_behavior, EdgeBehavior::FadeToBlack { .. })
    {
        render_display_view_axis_aligned(source, viewport, width, height, rendered_rgb, axis_plan);
        return;
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
            write_rgb_pixel(rendered_rgb, width, x, y, pixel);
        }
    }
}

fn render_display_view_axis_aligned(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    width: u32,
    height: u32,
    rendered_rgb: &mut [u8],
    axis_plan: &mut Option<PreparedDisplayPlan>,
) {
    let rgba = source.rgba_bytes();
    let plan_key = prepared_display_plan_key(source, viewport, width, height);
    if axis_plan.as_ref().is_none_or(|plan| plan.key != plan_key) {
        *axis_plan = Some(prepare_display_plan(
            source, viewport, width, height, plan_key,
        ));
    }

    if let Some(plan) = axis_plan.as_ref() {
        let mut output_offset = 0usize;
        for sample in &plan.samples {
            let pixel = sample_prepared_display_rgb(rgba, sample);
            rendered_rgb[output_offset] = pixel[0];
            rendered_rgb[output_offset + 1] = pixel[1];
            rendered_rgb[output_offset + 2] = pixel[2];
            output_offset += 3;
        }
    }
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
    source: &CanvasFrame,
    canvas_pos: NormalizedPosition,
    edge_behavior: EdgeBehavior,
) -> [u8; 3] {
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

fn bilinear_sample(source: &CanvasFrame, nx: f32, ny: f32) -> [u8; 3] {
    let x_sample = axis_sample(nx, source.width);
    let y_sample = axis_sample(ny, source.height);
    bilinear_sample_rgb(source, x_sample, y_sample)
}

#[derive(Clone, Copy)]
struct AxisSample {
    lower: usize,
    upper: usize,
    lower_weight: u16,
    upper_weight: u16,
}

fn prepared_display_plan_key(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    output_width: u32,
    output_height: u32,
) -> PreparedDisplayPlanKey {
    let start_x = viewport.position.x - (viewport.size.x * viewport.scale * 0.5);
    let start_y = viewport.position.y - (viewport.size.y * viewport.scale * 0.5);
    let span_x = viewport.size.x * viewport.scale;
    let span_y = viewport.size.y * viewport.scale;

    PreparedDisplayPlanKey {
        source_width: source.width,
        source_height: source.height,
        output_width,
        output_height,
        edge_behavior: match viewport.edge_behavior {
            EdgeBehavior::Clamp => 0,
            EdgeBehavior::Wrap => 1,
            EdgeBehavior::Mirror => 2,
            EdgeBehavior::FadeToBlack { .. } => 3,
        },
        start_x_bits: start_x.to_bits(),
        start_y_bits: start_y.to_bits(),
        span_x_bits: span_x.to_bits(),
        span_y_bits: span_y.to_bits(),
    }
}

fn prepare_display_plan(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    output_width: u32,
    output_height: u32,
    key: PreparedDisplayPlanKey,
) -> PreparedDisplayPlan {
    let start_x = viewport.position.x - (viewport.size.x * viewport.scale * 0.5);
    let start_y = viewport.position.y - (viewport.size.y * viewport.scale * 0.5);
    let span_x = viewport.size.x * viewport.scale;
    let span_y = viewport.size.y * viewport.scale;
    let x_samples = precompute_axis_samples(
        output_width,
        start_x,
        span_x,
        source.width,
        viewport.edge_behavior,
    );
    let y_samples = precompute_axis_samples(
        output_height,
        start_y,
        span_y,
        source.height,
        viewport.edge_behavior,
    );
    let source_width = usize::try_from(source.width).unwrap_or_default();
    let mut samples = Vec::with_capacity(x_samples.len().saturating_mul(y_samples.len()));
    for y_sample in &y_samples {
        for x_sample in &x_samples {
            samples.push(PreparedDisplaySample {
                offsets: [
                    rgba_offset(source_width, x_sample.lower, y_sample.lower),
                    rgba_offset(source_width, x_sample.upper, y_sample.lower),
                    rgba_offset(source_width, x_sample.lower, y_sample.upper),
                    rgba_offset(source_width, x_sample.upper, y_sample.upper),
                ],
                x_lower_weight: x_sample.lower_weight,
                x_upper_weight: x_sample.upper_weight,
                y_lower_weight: y_sample.lower_weight,
                y_upper_weight: y_sample.upper_weight,
            });
        }
    }

    PreparedDisplayPlan { key, samples }
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn apply_fade_to_black(
    pixel: [u8; 3],
    canvas_pos: NormalizedPosition,
    edge_behavior: EdgeBehavior,
) -> [u8; 3] {
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
    [
        round_to_u8(f32::from(pixel[0]) * attenuation),
        round_to_u8(f32::from(pixel[1]) * attenuation),
        round_to_u8(f32::from(pixel[2]) * attenuation),
    ]
}

fn apply_circular_mask(image: &mut [u8], width: u32, height: u32) {
    let width = i64::from(width);
    let height = i64::from(height);
    let radius = width.min(height);
    let radius_sq = radius.saturating_mul(radius);

    for y in 0..height {
        for x in 0..width {
            let dx = x.saturating_mul(2).saturating_add(1) - width;
            let dy = y.saturating_mul(2).saturating_add(1) - height;
            let distance_sq = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
            if distance_sq > radius_sq {
                let index = rgb_offset(
                    usize::try_from(width).unwrap_or_default(),
                    usize::try_from(x).unwrap_or_default(),
                    usize::try_from(y).unwrap_or_default(),
                );
                image[index..index + 3].fill(0);
            }
        }
    }
}

fn rgb_buffer_len(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(3)
}

#[allow(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "display resampling math operates in normalized float space before producing bounded indices"
)]
fn precompute_axis_samples(
    output_len: u32,
    start: f32,
    span: f32,
    source_len: u32,
    edge_behavior: EdgeBehavior,
) -> Vec<AxisSample> {
    let output_len_f32 = output_len.max(1) as f32;
    (0..output_len)
        .map(|index| {
            let position = start + ((index as f32 + 0.5) / output_len_f32) * span;
            let normalized = apply_edge_normalized(position, edge_behavior).clamp(0.0, 1.0);
            axis_sample(normalized, source_len)
        })
        .collect()
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "axis sampling clamps coordinates and weights into valid output ranges before narrowing"
)]
fn axis_sample(normalized: f32, source_len: u32) -> AxisSample {
    let max_index = source_len.saturating_sub(1);
    let coordinate = normalized * max_index as f32;
    let lower = coordinate as usize;
    let upper = lower
        .saturating_add(1)
        .min(usize::try_from(max_index).unwrap_or_default());
    let upper_weight = (((coordinate - lower as f32) * BILINEAR_WEIGHT_SCALE as f32) + 0.5)
        .clamp(0.0, BILINEAR_WEIGHT_SCALE as f32) as u16;
    let lower_weight = u16::try_from(BILINEAR_WEIGHT_SCALE).unwrap_or(u16::MAX) - upper_weight;
    AxisSample {
        lower,
        upper,
        lower_weight,
        upper_weight,
    }
}

fn bilinear_sample_rgb(
    source: &CanvasFrame,
    x_sample: AxisSample,
    y_sample: AxisSample,
) -> [u8; 3] {
    bilinear_sample_rgba(
        source.rgba_bytes(),
        usize::try_from(source.width).unwrap_or_default(),
        x_sample,
        y_sample,
    )
}

fn bilinear_sample_rgba(
    rgba: &[u8],
    source_width: usize,
    x_sample: AxisSample,
    y_sample: AxisSample,
) -> [u8; 3] {
    let top_left = rgba_offset(source_width, x_sample.lower, y_sample.lower);
    let top_right = rgba_offset(source_width, x_sample.upper, y_sample.lower);
    let bottom_left = rgba_offset(source_width, x_sample.lower, y_sample.upper);
    let bottom_right = rgba_offset(source_width, x_sample.upper, y_sample.upper);

    [
        bilinear_channel(
            rgba[top_left],
            rgba[top_right],
            rgba[bottom_left],
            rgba[bottom_right],
            x_sample,
            y_sample,
        ),
        bilinear_channel(
            rgba[top_left + 1],
            rgba[top_right + 1],
            rgba[bottom_left + 1],
            rgba[bottom_right + 1],
            x_sample,
            y_sample,
        ),
        bilinear_channel(
            rgba[top_left + 2],
            rgba[top_right + 2],
            rgba[bottom_left + 2],
            rgba[bottom_right + 2],
            x_sample,
            y_sample,
        ),
    ]
}

fn sample_prepared_display_rgb(rgba: &[u8], sample: &PreparedDisplaySample) -> [u8; 3] {
    [
        prepared_display_channel(rgba, sample.offsets, 0, sample),
        prepared_display_channel(rgba, sample.offsets, 1, sample),
        prepared_display_channel(rgba, sample.offsets, 2, sample),
    ]
}

fn prepared_display_channel(
    rgba: &[u8],
    offsets: [usize; 4],
    channel: usize,
    sample: &PreparedDisplaySample,
) -> u8 {
    let top = u32::from(rgba[offsets[0] + channel]) * u32::from(sample.x_lower_weight)
        + u32::from(rgba[offsets[1] + channel]) * u32::from(sample.x_upper_weight);
    let bottom = u32::from(rgba[offsets[2] + channel]) * u32::from(sample.x_lower_weight)
        + u32::from(rgba[offsets[3] + channel]) * u32::from(sample.x_upper_weight);
    let blended =
        top * u32::from(sample.y_lower_weight) + bottom * u32::from(sample.y_upper_weight);
    let rounded = blended.saturating_add(BILINEAR_WEIGHT_ROUNDING) >> 16;
    u8::try_from(rounded).expect("bilinear interpolation should remain within byte range")
}

fn bilinear_channel(
    top_left: u8,
    top_right: u8,
    bottom_left: u8,
    bottom_right: u8,
    x_sample: AxisSample,
    y_sample: AxisSample,
) -> u8 {
    let top = blend_channel(top_left, top_right, x_sample);
    let bottom = blend_channel(bottom_left, bottom_right, x_sample);
    let blended =
        top * u32::from(y_sample.lower_weight) + bottom * u32::from(y_sample.upper_weight);
    let rounded = blended.saturating_add(BILINEAR_WEIGHT_ROUNDING) >> 16;
    u8::try_from(rounded).expect("bilinear interpolation should remain within byte range")
}

fn blend_channel(lower: u8, upper: u8, sample: AxisSample) -> u32 {
    u32::from(lower) * u32::from(sample.lower_weight)
        + u32::from(upper) * u32::from(sample.upper_weight)
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the helper bounds finite values to the 0-255 display byte range before narrowing"
)]
fn round_to_u8(value: f32) -> u8 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= 255.0 {
        return u8::MAX;
    }

    (value + 0.5) as u8
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the helper bounds finite unit values to the 0-255 brightness factor range"
)]
fn round_unit_to_u16(value: f32) -> u16 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= 1.0 {
        return u16::from(u8::MAX);
    }

    ((value * f32::from(u8::MAX)) + 0.5) as u16
}

fn display_viewport_signature(viewport: &DisplayViewport) -> DisplayViewportSignature {
    let (edge_behavior, fade_falloff_bits) = match viewport.edge_behavior {
        EdgeBehavior::Clamp => (0, 0),
        EdgeBehavior::Wrap => (1, 0),
        EdgeBehavior::Mirror => (2, 0),
        EdgeBehavior::FadeToBlack { falloff } => (3, falloff.to_bits()),
    };

    DisplayViewportSignature {
        position_x_bits: viewport.position.x.to_bits(),
        position_y_bits: viewport.position.y.to_bits(),
        size_x_bits: viewport.size.x.to_bits(),
        size_y_bits: viewport.size.y.to_bits(),
        rotation_bits: viewport.rotation.to_bits(),
        scale_bits: viewport.scale.to_bits(),
        edge_behavior,
        fade_falloff_bits,
    }
}

fn logical_device_store_signature(store: &HashMap<String, LogicalDevice>) -> u64 {
    let mut combined = u64::try_from(store.len()).unwrap_or(u64::MAX);
    for entry in store.values() {
        let mut hasher = DefaultHasher::new();
        entry.id.hash(&mut hasher);
        entry.physical_device_id.hash(&mut hasher);
        combined ^= hasher.finish().rotate_left(1);
    }

    combined
}

fn write_rgb_pixel(image: &mut [u8], width: u32, x: u32, y: u32, pixel: [u8; 3]) {
    let offset = rgb_offset(
        usize::try_from(width).unwrap_or_default(),
        usize::try_from(x).unwrap_or_default(),
        usize::try_from(y).unwrap_or_default(),
    );
    image[offset] = pixel[0];
    image[offset + 1] = pixel[1];
    image[offset + 2] = pixel[2];
}

fn rgba_offset(width: usize, x: usize, y: usize) -> usize {
    (y * width + x) * 4
}

fn rgb_offset(width: usize, x: usize, y: usize) -> usize {
    (y * width + x) * 3
}

fn capped_display_target_fps(device_max_fps: u32) -> u32 {
    let device_limit = if device_max_fps == 0 {
        DISPLAY_OUTPUT_MAX_FPS
    } else {
        device_max_fps
    };

    device_limit.clamp(1, DISPLAY_OUTPUT_MAX_FPS)
}

fn maybe_warn_display_error(
    last_warned_at: &mut Option<Instant>,
    target: &DisplayTarget,
    error: &anyhow::Error,
) {
    let should_warn =
        last_warned_at.is_none_or(|last| last.elapsed() >= DISPLAY_ERROR_WARN_INTERVAL);
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

    *last_warned_at = Some(Instant::now());
    warn!(
        device = %target.name,
        backend_id = %target.backend_id,
        device_id = %target.device_id,
        error = %error,
        "failed to push automatic display frame"
    );
}

fn panic_payload_message(panic: &(dyn Any + Send + 'static)) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}
