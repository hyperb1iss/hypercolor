use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tracing::warn;

use hypercolor_core::scene::interpolate_color;
#[cfg(test)]
use hypercolor_core::spatial::generate_positions;
#[cfg(test)]
use hypercolor_core::spatial::is_led_sampled_zone;
use hypercolor_core::spatial::{PreparedZonePlan, SpatialEngine};
use hypercolor_types::canvas::{Canvas, Rgb};
#[cfg(test)]
use hypercolor_types::event::FrameData;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::ColorInterpolation;
#[cfg(feature = "wgpu")]
use hypercolor_types::scene::ZoneId;
#[cfg(test)]
use hypercolor_types::spatial::Output;
use hypercolor_types::spatial::SpatialLayout;

use super::frame_composer::RenderStageStats;
#[cfg(feature = "wgpu")]
use super::pipeline_runtime::GpuFallbackSampling;
use super::pipeline_runtime::{
    CachedTransitionLayout, RetainedZoneFrame, SamplingRuntime, SceneTransitionKey,
};
#[cfg(feature = "wgpu")]
use super::producer_queue::{GpuTextureFrame, ProducerFrame};
use super::scene_snapshot::{FrameSceneSnapshot, SceneTransitionSnapshot};
use super::sparkleflinger::{PendingZoneSampling, ZoneSamplingDispatch, ZoneSamplingRequest};
#[cfg(feature = "wgpu")]
use super::sparkleflinger::{SparkleFlinger, ZoneTextureSample};
use super::{RenderThreadState, micros_between};

/// Per-zone LED sampling for a multi-zone scene on the GPU backend: each
/// zone's LEDs read that zone's own composed pixels with that zone's own plan
/// (Spec 86).
#[cfg(feature = "wgpu")]
pub(super) struct ZoneSamplingSet {
    /// Every scene zone's outputs merged, used to shape and route the result.
    pub(super) layout: Arc<SpatialLayout>,
    /// Scene zones in zone order.
    pub(super) zones: Vec<ZoneSampling>,
    /// The `Gpu` zones' textures in zone order: what one dispatch reads.
    pub(super) gpu_textures: Vec<ZoneTextureSample>,
}

#[cfg(feature = "wgpu")]
pub(super) struct ZoneSampling {
    pub(super) prepared_zones: Arc<[PreparedZonePlan]>,
    pub(super) source: ZoneSampleSource,
}

#[cfg(feature = "wgpu")]
pub(super) enum ZoneSampleSource {
    /// Sampled from the zone's snapshot in the frame's GPU dispatch.
    Gpu,
    /// A CPU-backed zone, already sampled on the CPU from its canvas.
    Cpu(Vec<ZoneColors>),
    /// A zone with nothing to sample yet (no frame, or an imported GPU frame
    /// the sampler cannot bind): its outputs hold their published colors when
    /// the shape matches, and are black otherwise.
    Held,
}

/// How the zones of a frame's per-zone sampling set took their colors, by
/// source. All zero when the frame sampled no zone set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ZoneSourceCounts {
    pub(crate) gpu: u32,
    pub(crate) cpu: u32,
    pub(crate) held: u32,
}

#[cfg(feature = "wgpu")]
impl ZoneSamplingSet {
    pub(super) fn source_counts(&self) -> ZoneSourceCounts {
        let mut counts = ZoneSourceCounts::default();
        for zone in &self.zones {
            let count = match zone.source {
                ZoneSampleSource::Gpu => &mut counts.gpu,
                ZoneSampleSource::Cpu(_) => &mut counts.cpu,
                ZoneSampleSource::Held => &mut counts.held,
            };
            *count = count.saturating_add(1);
        }
        counts
    }

    fn output_shape(&self) -> impl Iterator<Item = (&str, usize)> + '_ {
        self.zones
            .iter()
            .flat_map(|zone| zone.prepared_zones.iter())
            .map(prepared_zone_shape)
    }

    fn samples_only_on_gpu(&self) -> bool {
        self.zones
            .iter()
            .all(|zone| matches!(zone.source, ZoneSampleSource::Gpu))
    }

    /// Rebuild `zones` in output order from a GPU result holding only the
    /// `Gpu` zones' colors, splicing in CPU-sampled and held zones.
    pub(super) fn assemble_into(&self, zones: &mut Vec<ZoneColors>, published: &[ZoneColors]) {
        if self.samples_only_on_gpu() {
            return;
        }
        let mut gpu_colors = std::mem::take(zones).into_iter();
        for zone in &self.zones {
            match &zone.source {
                ZoneSampleSource::Gpu => {
                    zones.extend(gpu_colors.by_ref().take(zone.prepared_zones.len()));
                }
                ZoneSampleSource::Cpu(colors) => zones.extend(colors.iter().cloned()),
                ZoneSampleSource::Held => {
                    zones.extend(zone.prepared_zones.iter().map(|plan| {
                        published
                            .iter()
                            .find(|published| {
                                published.zone_id == plan.zone_id
                                    && published.colors.len() == plan.sample_positions.len()
                            })
                            .cloned()
                            .unwrap_or_else(|| ZoneColors {
                                zone_id: plan.zone_id.clone(),
                                colors: vec![[0, 0, 0]; plan.sample_positions.len()],
                            })
                    }));
                }
            }
        }
    }

    /// Black output in the published shape, for frames whose GPU sample is
    /// unavailable and whose published frame cannot be held.
    fn black_into(&self, zones: &mut Vec<ZoneColors>) {
        zones.clear();
        zones.extend(
            self.zones
                .iter()
                .flat_map(|zone| zone.prepared_zones.iter())
                .map(|plan| ZoneColors {
                    zone_id: plan.zone_id.clone(),
                    colors: vec![[0, 0, 0]; plan.sample_positions.len()],
                }),
        );
    }
}

/// Unassigned outputs sampled from the fallback zone's texture in the same
/// dispatch as the scene's own LEDs (Spec 86 §4.3). Their colors are the
/// tail of the GPU result.
#[cfg(feature = "wgpu")]
struct FallbackPass {
    zone_id: ZoneId,
    /// The textures the dispatch reads besides the compositor output: a zone
    /// set's own textures, then the fallback zone's texture.
    textures: Vec<ZoneTextureSample>,
    ranges: usize,
}

#[cfg(feature = "wgpu")]
impl FallbackPass {
    /// The pass for `fallback` over `frame`, when `frame` is the GPU texture
    /// the registered plan was laid out for.
    fn new(
        base: &[ZoneTextureSample],
        frame: &ProducerFrame,
        fallback: &GpuFallbackSampling,
    ) -> Option<Self> {
        let ProducerFrame::GpuTexture(texture) = frame else {
            return None;
        };
        let layout = fallback.engine.layout();
        let prepared_zones = fallback.engine.sampling_plan();
        if texture.width != layout.canvas_width
            || texture.height != layout.canvas_height
            || prepared_zones.is_empty()
            || !SparkleFlinger::gpu_samples_plan_modes(&prepared_zones)
        {
            return None;
        }
        let ranges = prepared_zones.len();
        let mut textures = Vec::with_capacity(base.len().saturating_add(1));
        textures.extend_from_slice(base);
        textures.push(ZoneTextureSample {
            frame: GpuTextureFrame {
                immutable_lease: None,
                ..texture.clone()
            },
            prepared_zones,
        });
        Some(Self {
            zone_id: fallback.zone_id,
            textures,
            ranges,
        })
    }
}

/// What `resolve_led_sampling` samples on the GPU: the compositor's output
/// with one engine, or a zone set's textures with their own engines.
enum GpuSampleTarget {
    Output {
        engine: SpatialEngine,
        prepared_zones: Arc<[PreparedZonePlan]>,
        #[cfg(feature = "wgpu")]
        fallback: Option<FallbackPass>,
    },
    #[cfg(feature = "wgpu")]
    Zones {
        set: Arc<ZoneSamplingSet>,
        fallback: Option<FallbackPass>,
    },
}

impl GpuSampleTarget {
    fn layout(&self) -> Arc<SpatialLayout> {
        match self {
            Self::Output { engine, .. } => engine.layout(),
            #[cfg(feature = "wgpu")]
            Self::Zones { set, .. } => Arc::clone(&set.layout),
        }
    }

    fn request(&self) -> ZoneSamplingRequest<'_> {
        match self {
            Self::Output {
                prepared_zones,
                #[cfg(feature = "wgpu")]
                fallback,
                ..
            } => ZoneSamplingRequest::Output {
                prepared_zones,
                #[cfg(feature = "wgpu")]
                textures: fallback.as_ref().map_or(&[], |pass| &pass.textures),
            },
            #[cfg(feature = "wgpu")]
            Self::Zones { set, fallback } => {
                ZoneSamplingRequest::ZoneTextures(dispatched_textures(set, fallback.as_ref()))
            }
        }
    }

    /// Whether `zones` has the shape of this target's published output.
    fn output_shape_matches(&self, zones: &[ZoneColors]) -> bool {
        match self {
            Self::Output { prepared_zones, .. } => {
                can_hold_zone_colors_for_prepared_sampling(prepared_zones, zones)
            }
            #[cfg(feature = "wgpu")]
            Self::Zones { set, .. } => zone_shapes_match(set.output_shape(), zones),
        }
    }

    /// Whether `zones` has the shape of one GPU sample of this target.
    fn gpu_shape_matches(&self, zones: &[ZoneColors]) -> bool {
        match self {
            #[cfg(feature = "wgpu")]
            Self::Output {
                prepared_zones,
                fallback: Some(pass),
                ..
            } => zone_shapes_match(
                prepared_zones.iter().map(prepared_zone_shape).chain(
                    pass.textures
                        .iter()
                        .flat_map(|texture| texture.prepared_zones.iter())
                        .map(prepared_zone_shape),
                ),
                zones,
            ),
            Self::Output { prepared_zones, .. } => {
                can_hold_zone_colors_for_prepared_sampling(prepared_zones, zones)
            }
            #[cfg(feature = "wgpu")]
            Self::Zones { set, fallback } => zone_shapes_match(
                dispatched_textures(set, fallback.as_ref())
                    .iter()
                    .flat_map(|texture| texture.prepared_zones.iter())
                    .map(prepared_zone_shape),
                zones,
            ),
        }
    }

    fn output_shape_signature(&self) -> u64 {
        match self {
            Self::Output { prepared_zones, .. } => {
                zone_shape_signature_for_prepared_sampling(prepared_zones)
            }
            #[cfg(feature = "wgpu")]
            Self::Zones { set, .. } => hash_zone_shapes(set.output_shape()),
        }
    }
}

#[cfg(feature = "wgpu")]
impl GpuSampleTarget {
    fn fallback(&self) -> Option<&FallbackPass> {
        match self {
            Self::Output { fallback, .. } | Self::Zones { fallback, .. } => fallback.as_ref(),
        }
    }
}

/// The textures one dispatch of a zone set reads.
#[cfg(feature = "wgpu")]
fn dispatched_textures<'a>(
    set: &'a ZoneSamplingSet,
    fallback: Option<&'a FallbackPass>,
) -> &'a [ZoneTextureSample] {
    fallback.map_or(&set.gpu_textures, |pass| &pass.textures)
}

#[cfg(feature = "wgpu")]
fn prepared_zone_shape(zone: &PreparedZonePlan) -> (&str, usize) {
    (zone.zone_id.as_str(), zone.sample_positions.len())
}

#[derive(Clone)]
pub(super) enum LedSamplingStrategy {
    SparkleFlinger(SpatialEngine),
    /// Per-zone GPU sampling of a multi-zone scene (Spec 86).
    #[cfg(feature = "wgpu")]
    SparkleFlingerZones(Arc<ZoneSamplingSet>),
    PreSampled(Arc<SpatialLayout>),
    RetainedPreSampled {
        layout: Arc<SpatialLayout>,
        zones: Arc<[ZoneColors]>,
    },
    ReusePublished(Arc<SpatialLayout>),
}

impl LedSamplingStrategy {
    pub(super) fn requires_full_composition(&self, transition_active: bool) -> bool {
        transition_active || matches!(self, Self::SparkleFlinger(_))
    }

    pub(super) fn sparkleflinger_engine(&self) -> Option<&SpatialEngine> {
        match self {
            Self::SparkleFlinger(spatial_engine) => Some(spatial_engine),
            #[cfg(feature = "wgpu")]
            Self::SparkleFlingerZones(_) => None,
            Self::PreSampled(_) | Self::RetainedPreSampled { .. } | Self::ReusePublished(_) => None,
        }
    }

    pub(super) fn from_retained(retained: &RetainedLedSamplingStrategy) -> Self {
        match retained {
            RetainedLedSamplingStrategy::SparkleFlinger(spatial_engine) => {
                Self::SparkleFlinger(spatial_engine.clone())
            }
            RetainedLedSamplingStrategy::PreSampled { layout, zones } => Self::RetainedPreSampled {
                layout: Arc::clone(layout),
                zones: Arc::clone(zones),
            },
        }
    }

    pub(super) fn retain(
        &self,
        zones: &[ZoneColors],
        recycled: Option<RetainedLedSamplingStrategy>,
    ) -> RetainedLedSamplingStrategy {
        match self {
            Self::PreSampled(layout) => RetainedLedSamplingStrategy::PreSampled {
                layout: Arc::clone(layout),
                zones: recycle_retained_zones(recycled, zones),
            },
            Self::SparkleFlinger(spatial_engine) => {
                RetainedLedSamplingStrategy::SparkleFlinger(spatial_engine.clone())
            }
            // A zone set's colors resolve on the GPU after the scene renders,
            // and its snapshots move on every frame, so a retained zone set
            // holds the published frame instead of re-sampling (Spec 86 §4.2).
            #[cfg(feature = "wgpu")]
            Self::SparkleFlingerZones(set) => RetainedLedSamplingStrategy::PreSampled {
                layout: Arc::clone(&set.layout),
                zones: Arc::new([]),
            },
            Self::RetainedPreSampled { layout, zones } => RetainedLedSamplingStrategy::PreSampled {
                layout: Arc::clone(layout),
                zones: Arc::clone(zones),
            },
            Self::ReusePublished(layout) => RetainedLedSamplingStrategy::PreSampled {
                layout: Arc::clone(layout),
                zones: Arc::new([]),
            },
        }
    }
}

/// Reuses the previous retained zone allocation when it is uniquely owned and
/// shape-compatible; `clone_from` keeps the per-zone String and color Vec
/// capacities, so steady-state retention of an animating scene allocates
/// nothing.
fn recycle_retained_zones(
    recycled: Option<RetainedLedSamplingStrategy>,
    zones: &[ZoneColors],
) -> Arc<[ZoneColors]> {
    if let Some(RetainedLedSamplingStrategy::PreSampled {
        zones: mut previous,
        ..
    }) = recycled
        && previous.len() == zones.len()
        && let Some(slots) = Arc::get_mut(&mut previous)
    {
        for (slot, zone) in slots.iter_mut().zip(zones) {
            slot.zone_id.clone_from(&zone.zone_id);
            slot.colors.clone_from(&zone.colors);
        }
        return previous;
    }
    zones.to_vec().into()
}

#[derive(Clone)]
pub(super) enum RetainedLedSamplingStrategy {
    SparkleFlinger(SpatialEngine),
    PreSampled {
        layout: Arc<SpatialLayout>,
        zones: Arc<[ZoneColors]>,
    },
}

pub(crate) struct LedSamplingOutcome {
    pub(crate) layout: Arc<SpatialLayout>,
    pub(crate) gpu_zone_sampling: bool,
    pub(crate) gpu_sample_deferred: bool,
    pub(crate) gpu_sample_stale: bool,
    pub(crate) gpu_sample_retry_hit: bool,
    pub(crate) gpu_sample_queue_saturated: bool,
    pub(crate) gpu_sample_wait_blocked: bool,
    pub(crate) gpu_sample_cpu_fallback: bool,
    pub(crate) refresh_reused_frame_metadata: bool,
    pub(crate) reuses_published_frame: bool,
    pub(crate) zone_shape_signature: u64,
    /// The sources of the zone set this frame's LEDs were resolved from.
    pub(crate) zone_sources: ZoneSourceCounts,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CpuSamplingCanvasStatus {
    Ready,
    Unavailable,
}

const CPU_SAMPLING_CANVAS_WARN_INTERVAL: u64 = 300;

/// Rate-limits the no-resident-canvas fallback warning. With the GPU
/// compositor's one-frame sampling readback latch this state is expected at
/// most once at stream start (the latch primes on the first composed frame),
/// so steady-state repetition indicates a real regression without spamming a
/// warn per frame.
fn warn_cpu_sampling_canvas_unavailable(message: &str) {
    static OCCURRENCES: AtomicU64 = AtomicU64::new(0);
    let occurrence = OCCURRENCES.fetch_add(1, Ordering::Relaxed);
    if occurrence.is_multiple_of(CPU_SAMPLING_CANVAS_WARN_INTERVAL) {
        warn!(occurrence, "{message}");
    }
}

#[cfg(test)]
pub(crate) fn can_reuse_published_frame_for_deferred_sampling(
    render_stage: &RenderStageStats,
    layout: &SpatialLayout,
    published_frame: &FrameData,
) -> bool {
    render_stage.screen_retained
        && can_hold_published_frame_for_deferred_sampling(layout, published_frame)
}

#[cfg(test)]
pub(crate) fn can_hold_published_frame_for_deferred_sampling(
    layout: &SpatialLayout,
    published_frame: &FrameData,
) -> bool {
    can_hold_zone_colors_for_deferred_sampling(layout, &published_frame.zones)
}

#[cfg(test)]
pub(crate) fn can_hold_zone_colors_for_deferred_sampling(
    layout: &SpatialLayout,
    zones: &[ZoneColors],
) -> bool {
    zone_shapes_match(
        layout
            .zones
            .iter()
            .filter(|zone| is_led_sampled_zone(zone))
            .map(|zone| (zone.id.as_str(), expected_zone_sample_count(zone))),
        zones,
    )
}

pub(crate) fn can_hold_zone_colors_for_prepared_sampling(
    prepared_zones: &[PreparedZonePlan],
    zones: &[ZoneColors],
) -> bool {
    zone_shapes_match(
        prepared_zones
            .iter()
            .map(|zone| (zone.zone_id.as_str(), zone.sample_positions.len())),
        zones,
    )
}

pub(crate) fn zone_shape_signature_for_prepared_sampling(
    prepared_zones: &[PreparedZonePlan],
) -> u64 {
    hash_zone_shapes(
        prepared_zones
            .iter()
            .map(|zone| (zone.zone_id.as_str(), zone.sample_positions.len())),
    )
}

pub(crate) fn zone_shape_signature_for_zone_colors(zones: &[ZoneColors]) -> u64 {
    hash_zone_shapes(
        zones
            .iter()
            .map(|zone| (zone.zone_id.as_str(), zone.colors.len())),
    )
}

fn zone_shapes_match<'a>(
    expected: impl IntoIterator<Item = (&'a str, usize)>,
    zones: &[ZoneColors],
) -> bool {
    let mut published_zones = zones.iter();
    for (zone_id, sample_count) in expected {
        let Some(zone_colors) = published_zones.next() else {
            return false;
        };
        if zone_colors.zone_id != zone_id || zone_colors.colors.len() != sample_count {
            return false;
        }
    }
    published_zones.next().is_none()
}

fn hash_zone_shapes<'a>(shapes: impl IntoIterator<Item = (&'a str, usize)>) -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut count = 0_usize;
    for (zone_id, sample_count) in shapes {
        zone_id.hash(&mut hasher);
        sample_count.hash(&mut hasher);
        count = count.saturating_add(1);
    }
    count.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
fn expected_zone_sample_count(zone: &Output) -> usize {
    if zone.led_positions.is_empty() {
        return generate_positions(&zone.topology).len();
    }
    zone.led_positions.len()
}

fn try_retire_stale_zone_sampling(
    sampling: &mut SamplingRuntime<'_>,
    pending: PendingZoneSampling,
) -> Option<PendingZoneSampling> {
    sampling.retire_or_return(pending)
}

fn discard_zone_sampling_backlog(sampling: &mut SamplingRuntime<'_>) {
    sampling.discard_backlog();
}

fn scene_transition_key(transition: &SceneTransitionSnapshot) -> Option<SceneTransitionKey> {
    Some(SceneTransitionKey {
        epoch: transition.epoch,
        from_scene: transition.from_scene?,
        to_scene: transition.to_scene?,
    })
}

fn current_scene_sampled_zones<'a>(
    led_sampling_strategy: &'a LedSamplingStrategy,
    recycled_zones: &'a [ZoneColors],
) -> Option<&'a [ZoneColors]> {
    match led_sampling_strategy {
        LedSamplingStrategy::PreSampled(_) => Some(recycled_zones),
        LedSamplingStrategy::RetainedPreSampled { zones, .. } => Some(zones.as_ref()),
        #[cfg(feature = "wgpu")]
        LedSamplingStrategy::SparkleFlingerZones(_) => None,
        LedSamplingStrategy::SparkleFlinger(_) | LedSamplingStrategy::ReusePublished(_) => None,
    }
}

fn ensure_cpu_sampling_canvas(
    state: &RenderThreadState,
    _sampling: &mut SamplingRuntime<'_>,
    render_stage: &mut RenderStageStats,
) -> CpuSamplingCanvasStatus {
    if render_stage.composed_frame.sampling_canvas.is_some() {
        return CpuSamplingCanvasStatus::Ready;
    }

    let canvas_width = state.canvas_dims.width();
    let canvas_height = state.canvas_dims.height();
    if let Some(surface) = render_stage
        .composed_frame
        .sampling_surface
        .as_ref()
        .filter(|surface| surface.width() == canvas_width && surface.height() == canvas_height)
    {
        render_stage.composed_frame.sampling_canvas = Some(Canvas::from_published_surface(surface));
        return CpuSamplingCanvasStatus::Ready;
    }

    if let Some(surface) = render_stage
        .composed_frame
        .preview_surface
        .as_ref()
        .filter(|surface| surface.width() == canvas_width && surface.height() == canvas_height)
    {
        render_stage.composed_frame.sampling_canvas = Some(Canvas::from_published_surface(surface));
        return CpuSamplingCanvasStatus::Ready;
    }

    CpuSamplingCanvasStatus::Unavailable
}

/// The transition's routing layout, built once and reused while the same
/// transition runs between the same two layouts. A fresh layout every frame
/// would miss every downstream cache keyed by layout identity (routing,
/// unassigned outputs, the fallback plan GPU sampling keys on).
fn cached_transition_layout(
    cache: &mut Option<CachedTransitionLayout>,
    base_layout: &Arc<SpatialLayout>,
    current_layout: &Arc<SpatialLayout>,
    transition_key: SceneTransitionKey,
) -> Arc<SpatialLayout> {
    if let Some(cached) = cache.as_ref()
        && cached.key == transition_key
        && Arc::ptr_eq(&cached.base, base_layout)
        && Arc::ptr_eq(&cached.current, current_layout)
    {
        return Arc::clone(&cached.layout);
    }
    let layout = build_transition_layout(base_layout, current_layout, transition_key);
    *cache = Some(CachedTransitionLayout {
        key: transition_key,
        base: Arc::clone(base_layout),
        current: Arc::clone(current_layout),
        layout: Arc::clone(&layout),
    });
    layout
}

pub(crate) fn build_transition_layout(
    base_layout: &SpatialLayout,
    current_layout: &SpatialLayout,
    transition_key: SceneTransitionKey,
) -> Arc<SpatialLayout> {
    let mut layout = current_layout.clone();
    layout.id = format!(
        "scene-transition:{}->{}",
        transition_key.from_scene, transition_key.to_scene
    );
    layout.name = "Scene Transition".into();
    layout.description = Some("Blended transition routing layout".into());

    let existing_zone_ids = layout
        .zones
        .iter()
        .map(|zone| zone.id.as_str())
        .collect::<HashSet<_>>();
    let base_only_zones = base_layout
        .zones
        .iter()
        .filter(|zone| !existing_zone_ids.contains(zone.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    layout.zones.extend(base_only_zones);
    Arc::new(layout)
}

fn blend_zone_rgb(
    from: [u8; 3],
    to: [u8; 3],
    progress: f32,
    color_interpolation: &ColorInterpolation,
) -> [u8; 3] {
    let mixed = interpolate_color(
        &Rgb::new(from[0], from[1], from[2]).to_linear(),
        &Rgb::new(to[0], to[1], to[2]).to_linear(),
        progress,
        color_interpolation,
    )
    .to_encoded();
    [mixed.r, mixed.g, mixed.b]
}

pub(crate) fn blend_scene_zone_frames(
    from_zones: &[ZoneColors],
    to_zones: &[ZoneColors],
    transition_layout: &SpatialLayout,
    progress: f32,
    color_interpolation: &ColorInterpolation,
    target: &mut Vec<ZoneColors>,
) {
    let from_zones_by_id = from_zones
        .iter()
        .map(|zone| (zone.zone_id.as_str(), zone))
        .collect::<HashMap<_, _>>();
    let to_zones_by_id = to_zones
        .iter()
        .map(|zone| (zone.zone_id.as_str(), zone))
        .collect::<HashMap<_, _>>();
    let black = [0, 0, 0];

    target.clear();
    target.reserve(transition_layout.zones.len());
    for layout_zone in &transition_layout.zones {
        let from_zone = from_zones_by_id.get(layout_zone.id.as_str()).copied();
        let to_zone = to_zones_by_id.get(layout_zone.id.as_str()).copied();
        let led_count = from_zone
            .map_or(0, |zone| zone.colors.len())
            .max(to_zone.map_or(0, |zone| zone.colors.len()));
        let colors = (0..led_count)
            .map(|index| {
                let from = from_zone
                    .and_then(|zone| zone.colors.get(index))
                    .copied()
                    .unwrap_or(black);
                let to = to_zone
                    .and_then(|zone| zone.colors.get(index))
                    .copied()
                    .unwrap_or(black);
                blend_zone_rgb(from, to, progress, color_interpolation)
            })
            .collect();
        target.push(ZoneColors {
            zone_id: layout_zone.id.clone(),
            colors,
        });
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "sampling orchestration keeps the deferred GPU and retained-scene state machine in one place"
)]
pub(crate) fn resolve_led_sampling(
    state: &RenderThreadState,
    sampling: &mut SamplingRuntime<'_>,
    scene_snapshot: &FrameSceneSnapshot,
    render_stage: &mut RenderStageStats,
    completed_deferred_sampling: Option<PendingZoneSampling>,
    mut stale_deferred_sampling: Option<PendingZoneSampling>,
) -> LedSamplingOutcome {
    let gpu_sample_target = match &render_stage.led_sampling_strategy {
        LedSamplingStrategy::SparkleFlinger(spatial_engine) => Some(GpuSampleTarget::Output {
            engine: spatial_engine.clone(),
            prepared_zones: spatial_engine.sampling_plan(),
            #[cfg(feature = "wgpu")]
            fallback: None,
        }),
        #[cfg(feature = "wgpu")]
        LedSamplingStrategy::SparkleFlingerZones(set) => Some(GpuSampleTarget::Zones {
            set: Arc::clone(set),
            fallback: None,
        }),
        LedSamplingStrategy::PreSampled(_)
        | LedSamplingStrategy::RetainedPreSampled { .. }
        | LedSamplingStrategy::ReusePublished(_) => None,
    };
    // Whether this frame renders a multi-zone scene; retained and reused
    // frames say nothing new.
    let multi_zone_frame = match &render_stage.led_sampling_strategy {
        LedSamplingStrategy::PreSampled(_) => Some(true),
        #[cfg(feature = "wgpu")]
        LedSamplingStrategy::SparkleFlingerZones(_) => Some(true),
        LedSamplingStrategy::SparkleFlinger(_) => Some(false),
        LedSamplingStrategy::RetainedPreSampled { .. } | LedSamplingStrategy::ReusePublished(_) => {
            None
        }
    };
    let transition_active = scene_snapshot.scene_runtime.active_transition.is_some();
    // A transition out of a multi-zone scene must not sample the crossfade,
    // whose base is a composited preview with overlap baked in: a single-zone
    // incoming scene samples its own frame and blends in LED space instead.
    #[cfg(feature = "wgpu")]
    let gpu_sample_target = match gpu_sample_target {
        Some(GpuSampleTarget::Output {
            engine,
            prepared_zones,
            fallback,
        }) if transition_active && sampling.zone_transition_planner.last_published_per_zone => {
            match render_stage
                .led_transition_source
                .as_ref()
                .and_then(|frame| one_zone_transition_set(&engine, &prepared_zones, frame))
            {
                Some(set) => Some(GpuSampleTarget::Zones {
                    set: Arc::new(set),
                    fallback: None,
                }),
                None => Some(GpuSampleTarget::Output {
                    engine,
                    prepared_zones,
                    fallback,
                }),
            }
        }
        other => other,
    };
    #[cfg(feature = "wgpu")]
    let gpu_sample_target = with_unassigned_fallback(
        gpu_sample_target,
        sampling
            .output_artifacts
            .unassigned_output_cache
            .gpu_fallback(),
        &render_stage.zone_canvases,
    );
    #[cfg(feature = "wgpu")]
    let samples_zone_set = matches!(gpu_sample_target, Some(GpuSampleTarget::Zones { .. }));
    #[cfg(not(feature = "wgpu"))]
    let samples_zone_set = false;
    #[cfg(feature = "wgpu")]
    let zone_sources = match gpu_sample_target.as_ref() {
        Some(GpuSampleTarget::Zones { set, .. }) => set.source_counts(),
        _ => ZoneSourceCounts::default(),
    };
    #[cfg(not(feature = "wgpu"))]
    let zone_sources = ZoneSourceCounts::default();
    let mut gpu_zone_sampling = false;
    let mut gpu_sample_deferred = false;
    let gpu_sample_stale = stale_deferred_sampling.is_some();
    let mut gpu_sample_retry_hit = false;
    let mut gpu_sample_queue_saturated = false;
    let mut gpu_sample_wait_blocked = false;
    let mut gpu_sample_cpu_fallback = false;
    let mut refresh_reused_frame_metadata = false;
    let mut zone_shape_signature = None::<u64>;
    let mut pending_gpu_zone_sampling = None;
    let mut can_reuse_published_frame = false;
    let mut can_hold_published_frame = false;
    let uses_scene_sampled_zones = matches!(
        render_stage.led_sampling_strategy,
        LedSamplingStrategy::PreSampled(_) | LedSamplingStrategy::RetainedPreSampled { .. }
    );
    let mut retained_scene_zones = None::<Arc<[ZoneColors]>>;
    let mut layout = if let LedSamplingStrategy::PreSampled(layout)
    | LedSamplingStrategy::ReusePublished(layout) =
        &render_stage.led_sampling_strategy
    {
        Arc::clone(layout)
    } else if let LedSamplingStrategy::RetainedPreSampled { layout, zones } =
        &render_stage.led_sampling_strategy
    {
        retained_scene_zones = Some(Arc::clone(zones));
        Arc::clone(layout)
    } else {
        let target = gpu_sample_target
            .as_ref()
            .expect("SparkleFlinger-owned LED sampling requires a sampling target");
        render_stage.sampled_us = 0;
        let layout = target.layout();
        zone_shape_signature = Some(target.output_shape_signature());
        (can_reuse_published_frame, can_hold_published_frame) = {
            let published_frame = state.event_bus.frame_lane().borrow();
            (
                render_stage.screen_retained && target.output_shape_matches(&published_frame.zones),
                target.output_shape_matches(&published_frame.zones),
            )
        };
        let completed_sampling_matches_current =
            completed_deferred_sampling
                .as_ref()
                .is_some_and(|completed| {
                    target.gpu_shape_matches(sampling.deferred_sampling.scratch())
                        && sampling
                            .sparkleflinger
                            .completed_zone_sampling_used_request_plans(completed, target.request())
                });
        let mut should_queue_followup_sampling = false;
        if completed_sampling_matches_current {
            sampling
                .deferred_sampling
                .clone_scratch_into(sampling.output_artifacts.zones_mut());
            gpu_zone_sampling = true;
            gpu_sample_retry_hit = true;
            should_queue_followup_sampling = true;
        }
        let mut stale_sampling_matches_current = false;
        if !gpu_zone_sampling && let Some(mut pending) = stale_deferred_sampling.take() {
            if sampling
                .sparkleflinger
                .pending_zone_sampling_matches_request(&pending, target.request())
            {
                stale_sampling_matches_current = true;
                let stale_sample_finish = Instant::now();
                match sampling.sparkleflinger.try_finish_pending_zone_sampling(
                    &mut pending,
                    sampling.output_artifacts.zones_mut(),
                ) {
                    Ok(true) => {
                        gpu_zone_sampling = true;
                        gpu_sample_retry_hit = true;
                        should_queue_followup_sampling = true;
                        gpu_sample_wait_blocked = sampling
                            .sparkleflinger
                            .take_last_sample_readback_wait_blocked();
                        render_stage.sampled_us = render_stage
                            .sampled_us
                            .saturating_add(micros_between(stale_sample_finish, Instant::now()));
                    }
                    Ok(false) => {
                        render_stage.sampled_us = render_stage
                            .sampled_us
                            .saturating_add(micros_between(stale_sample_finish, Instant::now()));
                        stale_deferred_sampling = Some(pending);
                    }
                    Err(error) => {
                        warn!(%error, "Deferred GPU spatial sampling retry failed; resampling current frame");
                    }
                }
            } else {
                stale_deferred_sampling = Some(pending);
            }
        }
        if !gpu_zone_sampling && stale_sampling_matches_current && can_hold_published_frame {
            if let Some(pending) = stale_deferred_sampling.take() {
                sampling.deferred_sampling.store_pending(pending);
            }
            gpu_sample_deferred = true;
            render_stage.led_sampling_strategy =
                LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
            refresh_reused_frame_metadata = render_stage.screen_retained;
        } else if let Some(pending) = stale_deferred_sampling.take()
            && let Some(pending) = try_retire_stale_zone_sampling(sampling, pending)
        {
            stale_deferred_sampling = Some(pending);
            gpu_sample_queue_saturated = true;
        }
        if !gpu_zone_sampling && gpu_sample_queue_saturated {
            if let Some(pending) = stale_deferred_sampling.take() {
                sampling.deferred_sampling.store_pending(pending);
            }
            if can_hold_published_frame {
                gpu_sample_deferred = true;
                render_stage.led_sampling_strategy =
                    LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
                refresh_reused_frame_metadata = render_stage.screen_retained;
            }
        } else if !gpu_zone_sampling {
            gpu_zone_sampling = if matches!(
                render_stage.composed_frame.backend,
                crate::performance::CompositorBackendKind::Gpu
            ) {
                let gpu_sample_start = Instant::now();
                match sampling.sparkleflinger.begin_sample_zones_into(
                    target.request(),
                    sampling.output_artifacts.zones_mut(),
                ) {
                    Ok(ZoneSamplingDispatch::Unsupported) => false,
                    Ok(ZoneSamplingDispatch::Ready) => {
                        render_stage.sampled_us = render_stage
                            .sampled_us
                            .saturating_add(micros_between(gpu_sample_start, Instant::now()));
                        true
                    }
                    Ok(ZoneSamplingDispatch::Saturated) => {
                        render_stage.sampled_us = render_stage
                            .sampled_us
                            .saturating_add(micros_between(gpu_sample_start, Instant::now()));
                        gpu_sample_queue_saturated = true;
                        if can_hold_published_frame {
                            false
                        } else {
                            discard_zone_sampling_backlog(sampling);
                            match sampling.sparkleflinger.begin_sample_zones_into(
                                target.request(),
                                sampling.output_artifacts.zones_mut(),
                            ) {
                                Ok(ZoneSamplingDispatch::Ready) => true,
                                Ok(ZoneSamplingDispatch::Pending(pending)) => {
                                    pending_gpu_zone_sampling = Some(pending);
                                    true
                                }
                                Ok(ZoneSamplingDispatch::Unsupported)
                                | Ok(ZoneSamplingDispatch::Saturated) => false,
                                Err(error) => {
                                    warn!(%error, "GPU spatial sampling retry after saturation failed; trying resident CPU sampling");
                                    false
                                }
                            }
                        }
                    }
                    Ok(ZoneSamplingDispatch::Pending(pending)) => {
                        render_stage.sampled_us = render_stage
                            .sampled_us
                            .saturating_add(micros_between(gpu_sample_start, Instant::now()));
                        pending_gpu_zone_sampling = Some(pending);
                        true
                    }
                    Err(error) => {
                        warn!(%error, "GPU spatial sampling failed; trying resident CPU sampling");
                        false
                    }
                }
            } else {
                false
            };
        }
        if should_queue_followup_sampling
            && matches!(
                render_stage.composed_frame.backend,
                crate::performance::CompositorBackendKind::Gpu
            )
        {
            let gpu_sample_start = Instant::now();
            match sampling
                .sparkleflinger
                .begin_sample_zones_into(target.request(), sampling.deferred_sampling.scratch_mut())
            {
                Ok(ZoneSamplingDispatch::Ready) => {
                    sampling
                        .deferred_sampling
                        .clone_scratch_into(sampling.output_artifacts.zones_mut());
                    gpu_sample_retry_hit = false;
                }
                Ok(ZoneSamplingDispatch::Pending(pending)) => {
                    sampling.deferred_sampling.store_pending(pending);
                    gpu_sample_deferred = true;
                }
                Ok(ZoneSamplingDispatch::Saturated) => {
                    gpu_sample_queue_saturated = true;
                }
                Ok(ZoneSamplingDispatch::Unsupported) => {}
                Err(error) => {
                    warn!(%error, "Follow-up GPU spatial sampling dispatch failed");
                }
            }
            render_stage.sampled_us = render_stage
                .sampled_us
                .saturating_add(micros_between(gpu_sample_start, Instant::now()));
        }
        layout
    };

    if let Some(pending) = pending_gpu_zone_sampling.take() {
        let gpu_sample_finish = Instant::now();
        if can_reuse_published_frame {
            let mut pending = pending;
            match sampling.sparkleflinger.try_finish_pending_zone_sampling(
                &mut pending,
                sampling.deferred_sampling.scratch_mut(),
            ) {
                Ok(true) => {
                    sampling
                        .deferred_sampling
                        .clone_scratch_into(sampling.output_artifacts.zones_mut());
                    gpu_sample_retry_hit = true;
                    gpu_sample_wait_blocked = sampling
                        .sparkleflinger
                        .take_last_sample_readback_wait_blocked();
                }
                Ok(false) => {
                    sampling.deferred_sampling.store_pending(pending);
                    gpu_zone_sampling = false;
                    gpu_sample_deferred = true;
                    render_stage.led_sampling_strategy =
                        LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
                    refresh_reused_frame_metadata = true;
                }
                Err(error) => {
                    warn!(%error, "Deferred GPU spatial sampling finalize failed; reusing retained frame zones");
                    gpu_zone_sampling = false;
                    render_stage.led_sampling_strategy =
                        LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
                    refresh_reused_frame_metadata = true;
                }
            }
        } else if can_hold_published_frame {
            sampling.deferred_sampling.store_pending(pending);
            gpu_zone_sampling = false;
            gpu_sample_deferred = true;
            render_stage.led_sampling_strategy =
                LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
        } else {
            let mut pending = pending;
            match sampling.sparkleflinger.try_finish_pending_zone_sampling(
                &mut pending,
                sampling.output_artifacts.zones_mut(),
            ) {
                Ok(true) => {
                    gpu_sample_wait_blocked = sampling
                        .sparkleflinger
                        .take_last_sample_readback_wait_blocked();
                }
                Ok(false) => {
                    sampling.deferred_sampling.store_pending(pending);
                    gpu_zone_sampling = false;
                    gpu_sample_deferred = true;
                }
                Err(error) => {
                    warn!(%error, "GPU spatial sampling finalize failed; trying resident CPU sampling");
                    gpu_zone_sampling = false;
                }
            }
        }
        render_stage.sampled_us = render_stage
            .sampled_us
            .saturating_add(micros_between(gpu_sample_finish, Instant::now()));
    }
    if !gpu_zone_sampling
        && !uses_scene_sampled_zones
        && !matches!(
            render_stage.led_sampling_strategy,
            LedSamplingStrategy::ReusePublished(_)
        )
    {
        gpu_sample_cpu_fallback = matches!(
            render_stage.composed_frame.backend,
            crate::performance::CompositorBackendKind::Gpu
        );
        let cpu_sample_start = Instant::now();
        let sampling_engine = match gpu_sample_target
            .as_ref()
            .expect("CPU spatial sampling requires a SparkleFlinger-owned sampling target")
        {
            GpuSampleTarget::Output { engine, .. } => Some(engine),
            // Zone snapshots never reach the CPU (Spec 48 §4.1): without a GPU
            // sample, a zone set holds its published frame or goes black.
            #[cfg(feature = "wgpu")]
            GpuSampleTarget::Zones { set, .. } => {
                if can_hold_published_frame {
                    render_stage.led_sampling_strategy =
                        LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
                    refresh_reused_frame_metadata = true;
                } else {
                    set.black_into(sampling.output_artifacts.zones_mut());
                }
                None
            }
        };
        if let Some(sampling_engine) = sampling_engine {
            let canvas_status = ensure_cpu_sampling_canvas(state, sampling, render_stage);
            match canvas_status {
                CpuSamplingCanvasStatus::Ready => {
                    if let Some(canvas) = render_stage.composed_frame.sampling_canvas.as_ref()
                        && let Err(error) = sampling_engine
                            .try_sample_into(canvas, sampling.output_artifacts.zones_mut())
                    {
                        warn!(%error, "CPU spatial sampling fallback failed; preserving published output");
                        if can_hold_published_frame {
                            render_stage.led_sampling_strategy =
                                LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
                            refresh_reused_frame_metadata = render_stage.screen_retained;
                        }
                    }
                }
                CpuSamplingCanvasStatus::Unavailable if can_hold_published_frame => {
                    warn_cpu_sampling_canvas_unavailable(
                        "CPU spatial sampling fallback has no resident canvas; reusing published frame",
                    );
                    render_stage.led_sampling_strategy =
                        LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
                    refresh_reused_frame_metadata = true;
                }
                CpuSamplingCanvasStatus::Unavailable => {
                    warn_cpu_sampling_canvas_unavailable(
                        "CPU spatial sampling fallback has no resident canvas; sampling a black fallback frame",
                    );
                    let sampling_succeeded = match Canvas::try_new(
                        state.canvas_dims.width(),
                        state.canvas_dims.height(),
                    ) {
                        Ok(black_canvas) => match sampling_engine
                            .try_sample_into(&black_canvas, sampling.output_artifacts.zones_mut())
                        {
                            Ok(()) => true,
                            Err(error) => {
                                warn!(%error, "Black CPU spatial sampling fallback failed; preserving published output");
                                false
                            }
                        },
                        Err(error) => {
                            warn!(%error, "Black CPU fallback canvas allocation failed; preserving published output");
                            false
                        }
                    };
                    if !sampling_succeeded && can_hold_published_frame {
                        render_stage.led_sampling_strategy =
                            LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
                        refresh_reused_frame_metadata = render_stage.screen_retained;
                    }
                }
            }
        }
        render_stage.sampled_us = micros_between(cpu_sample_start, Instant::now());
    }

    // A fresh GPU sample ends with any unassigned fallback outputs: keep
    // their colors for the output stage. A zone set's sample covers only its
    // `Gpu` zones; splice the CPU-sampled and held zones back into zone
    // order.
    #[cfg(feature = "wgpu")]
    if gpu_zone_sampling
        && let Some(target) = gpu_sample_target.as_ref()
        && !matches!(
            render_stage.led_sampling_strategy,
            LedSamplingStrategy::ReusePublished(_)
        )
    {
        if let Some(pass) = target.fallback() {
            let zones = sampling.output_artifacts.zones_mut();
            let fallback_colors = zones.split_off(zones.len().saturating_sub(pass.ranges));
            sampling
                .output_artifacts
                .unassigned_output_cache
                .store_gpu_fallback_colors(pass.zone_id, fallback_colors);
        }
        if let GpuSampleTarget::Zones { set, .. } = target {
            let published_frame = state.event_bus.frame_lane().borrow();
            set.assemble_into(
                sampling.output_artifacts.zones_mut(),
                &published_frame.zones,
            );
        }
    }

    let led_space_transition_zones = if !transition_active {
        None
    } else if uses_scene_sampled_zones {
        match current_scene_sampled_zones(
            &render_stage.led_sampling_strategy,
            sampling.output_artifacts.zones(),
        ) {
            // A retained zone set keeps no colors: they resolve on the GPU
            // after the scene renders (Spec 86 §4.2). The retained frame
            // repeats the set's content, so the blend reads the set's latest
            // fresh sample, or holds the published frame until one lands,
            // instead of blending every LED toward black.
            Some([]) if retained_scene_zones.is_some() && !layout.zones.is_empty() => {
                retained_zone_set_transition_zones(sampling, &layout)
            }
            zones => zones.map(<[ZoneColors]>::to_vec),
        }
    } else if samples_zone_set {
        zone_set_transition_zones(
            sampling,
            gpu_sample_target.as_ref(),
            gpu_zone_sampling
                && !matches!(
                    render_stage.led_sampling_strategy,
                    LedSamplingStrategy::ReusePublished(_)
                ),
        )
    } else {
        None
    };
    let mut blended_in_led_space = false;
    if let Some(transition) = scene_snapshot.scene_runtime.active_transition.as_ref()
        && let Some(transition_key) = scene_transition_key(transition)
        && let Some(current_zones) = led_space_transition_zones
    {
        let transition_blend_start = Instant::now();
        if sampling.zone_transition_planner.active_transition != Some(transition_key) {
            sampling.zone_transition_planner.active_transition = Some(transition_key);
            // record_published runs before each frame publishes, so the
            // published frame still holds the previous frame's zones here:
            // capturing them lazily costs one deep copy per transition instead
            // of one per frame.
            sampling.zone_transition_planner.transition_base = Some(
                match sampling
                    .zone_transition_planner
                    .last_published_layout
                    .as_ref()
                {
                    Some(published_layout) => RetainedZoneFrame {
                        layout: Arc::clone(published_layout),
                        zones: state.event_bus.frame_lane().borrow().zones.clone(),
                    },
                    None => RetainedZoneFrame {
                        layout: Arc::clone(&layout),
                        zones: current_zones.clone(),
                    },
                },
            );
        }

        let planner = &mut *sampling.zone_transition_planner;
        if let Some(base) = planner.transition_base.as_ref() {
            let transition_layout = cached_transition_layout(
                &mut planner.transition_layout,
                &base.layout,
                &layout,
                transition_key,
            );
            blend_scene_zone_frames(
                &base.zones,
                &current_zones,
                transition_layout.as_ref(),
                transition.eased_progress.clamp(0.0, 1.0),
                &transition.color_interpolation,
                sampling.output_artifacts.zones_mut(),
            );
            layout = Arc::clone(&transition_layout);
            render_stage.led_sampling_strategy = LedSamplingStrategy::PreSampled(transition_layout);
            blended_in_led_space = true;
            render_stage.sampled_us = render_stage
                .sampled_us
                .saturating_add(micros_between(transition_blend_start, Instant::now()));
        }
    } else if retained_scene_zones.is_some() {
        render_stage.led_sampling_strategy =
            LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
    }

    let reuses_published_frame = matches!(
        render_stage.led_sampling_strategy,
        LedSamplingStrategy::ReusePublished(_)
    );
    let zone_shape_signature = zone_shape_signature.unwrap_or_else(|| {
        if reuses_published_frame {
            let published_frame = state.event_bus.frame_lane().borrow();
            zone_shape_signature_for_zone_colors(&published_frame.zones)
        } else {
            zone_shape_signature_for_zone_colors(sampling.output_artifacts.zones())
        }
    });
    if scene_snapshot.scene_runtime.active_transition.is_none() {
        sampling.zone_transition_planner.record_stable();
    }
    sampling.zone_transition_planner.record_published(
        Arc::clone(&layout),
        if blended_in_led_space {
            Some(true)
        } else {
            multi_zone_frame
        },
    );

    LedSamplingOutcome {
        layout,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_stale,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback,
        refresh_reused_frame_metadata,
        reuses_published_frame,
        zone_shape_signature,
        zone_sources,
    }
}

/// The incoming scene's LED colors for a transition blend on a frame that
/// reuses a retained zone set: the set's latest fresh sample, when it was
/// taken for this layout.
fn retained_zone_set_transition_zones(
    sampling: &SamplingRuntime<'_>,
    layout: &SpatialLayout,
) -> Option<Vec<ZoneColors>> {
    let latest = &sampling.zone_transition_planner.latest_zone_set_sample;
    let sampled_this_layout = latest.len() == layout.zones.len()
        && latest
            .iter()
            .zip(&layout.zones)
            .all(|(zone, output)| zone.zone_id == output.id);
    sampled_this_layout.then(|| latest.clone())
}

/// The incoming scene's LED colors for a transition blend when it samples a
/// zone set: this frame's fresh sample, remembered for deferred frames, or
/// the latest one when this frame's sample is still in flight.
#[cfg(feature = "wgpu")]
fn zone_set_transition_zones(
    sampling: &mut SamplingRuntime<'_>,
    target: Option<&GpuSampleTarget>,
    fresh: bool,
) -> Option<Vec<ZoneColors>> {
    let target = target?;
    if fresh {
        let zones = sampling.output_artifacts.zones();
        let latest = &mut sampling.zone_transition_planner.latest_zone_set_sample;
        latest.clear();
        latest.extend_from_slice(zones);
        return Some(zones.to_vec());
    }
    let latest = &sampling.zone_transition_planner.latest_zone_set_sample;
    (!latest.is_empty() && target.output_shape_matches(latest)).then(|| latest.clone())
}

#[cfg(not(feature = "wgpu"))]
fn zone_set_transition_zones(
    _sampling: &mut SamplingRuntime<'_>,
    _target: Option<&GpuSampleTarget>,
    _fresh: bool,
) -> Option<Vec<ZoneColors>> {
    None
}

/// Add the unassigned fallback pass the output stage registered (Spec 86
/// §4.3). A zone set samples it beside its own textures; a single-zone scene
/// samples it beside the compositor output, transitions included.
#[cfg(feature = "wgpu")]
fn with_unassigned_fallback(
    target: Option<GpuSampleTarget>,
    fallback: Option<&GpuFallbackSampling>,
    zone_canvases: &[(ZoneId, ProducerFrame)],
) -> Option<GpuSampleTarget> {
    let Some(fallback) = fallback else {
        return target;
    };
    let Some(frame) = zone_canvases
        .iter()
        .find(|(zone_id, _)| *zone_id == fallback.zone_id)
        .map(|(_, frame)| frame)
    else {
        return target;
    };
    match target {
        Some(GpuSampleTarget::Zones {
            set,
            fallback: None,
        }) => {
            let pass = FallbackPass::new(&set.gpu_textures, frame, fallback);
            Some(GpuSampleTarget::Zones {
                set,
                fallback: pass,
            })
        }
        Some(GpuSampleTarget::Output {
            engine,
            prepared_zones,
            fallback: None,
        }) => {
            let pass = FallbackPass::new(&[], frame, fallback);
            Some(GpuSampleTarget::Output {
                engine,
                prepared_zones,
                fallback: pass,
            })
        }
        other => other,
    }
}

/// A single-zone scene's own frame as a one-zone set, for a transition out of
/// a multi-zone scene: GPU frames sample on the GPU, CPU-backed frames on the
/// CPU where their pixels already are. A GPU copy drops the snapshot lease,
/// because the dispatch that reads it is queued this frame, before the
/// snapshot can be reused.
#[cfg(feature = "wgpu")]
fn one_zone_transition_set(
    engine: &SpatialEngine,
    prepared_zones: &Arc<[PreparedZonePlan]>,
    frame: &ProducerFrame,
) -> Option<ZoneSamplingSet> {
    let (source, gpu_textures) = match frame {
        ProducerFrame::GpuTexture(texture)
            if SparkleFlinger::gpu_samples_plan_modes(prepared_zones) =>
        {
            let texture = ZoneTextureSample {
                frame: GpuTextureFrame {
                    immutable_lease: None,
                    ..texture.clone()
                },
                prepared_zones: Arc::clone(prepared_zones),
            };
            (ZoneSampleSource::Gpu, vec![texture])
        }
        ProducerFrame::GpuTexture(_) => return None,
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => return None,
        cpu_frame => {
            let canvas = match cpu_frame {
                ProducerFrame::Canvas(canvas) => std::borrow::Cow::Borrowed(canvas),
                ProducerFrame::Surface(surface) => {
                    std::borrow::Cow::Owned(Canvas::from_published_surface(surface))
                }
                other => std::borrow::Cow::Owned(other.clone().into_cpu_render_frame()?.0),
            };
            let mut colors = Vec::new();
            engine.try_sample_into(&canvas, &mut colors).ok()?;
            (ZoneSampleSource::Cpu(colors), Vec::new())
        }
    };
    Some(ZoneSamplingSet {
        layout: engine.layout(),
        zones: vec![ZoneSampling {
            prepared_zones: Arc::clone(prepared_zones),
            source,
        }],
        gpu_textures,
    })
}

#[cfg(test)]
mod tests {
    use hypercolor_types::event::ZoneColors;
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
        StripDirection,
    };

    use super::can_hold_zone_colors_for_deferred_sampling;

    fn test_zone(id: &str) -> Output {
        Output {
            id: id.to_owned(),
            name: id.to_owned(),
            device_id: format!("device:{id}"),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 2,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }
    }

    fn test_layout(mut zones: Vec<Output>) -> SpatialLayout {
        if let Some(display) = zones.iter_mut().find(|zone| zone.id == "display") {
            display.zone_name = Some("Display".to_owned());
        }
        SpatialLayout {
            id: "layout".to_owned(),
            name: "Layout".to_owned(),
            description: None,
            canvas_width: 4,
            canvas_height: 4,
            zones,
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        }
    }

    fn zone_colors(id: &str) -> ZoneColors {
        zone_colors_with_count(id, 2)
    }

    fn zone_colors_with_count(id: &str, count: usize) -> ZoneColors {
        ZoneColors {
            zone_id: id.to_owned(),
            colors: vec![[1, 2, 3]; count],
        }
    }

    #[test]
    fn frame_sampling_accepts_matching_deferred_zone_colors() {
        let layout = test_layout(vec![test_zone("left"), test_zone("right")]);
        let zones = vec![zone_colors("left"), zone_colors("right")];

        assert!(can_hold_zone_colors_for_deferred_sampling(&layout, &zones));
    }

    #[test]
    fn frame_sampling_rejects_stale_deferred_zone_colors() {
        let layout = test_layout(vec![test_zone("left"), test_zone("right")]);
        let zones = vec![zone_colors("right"), zone_colors("left")];

        assert!(!can_hold_zone_colors_for_deferred_sampling(&layout, &zones));
    }

    #[test]
    fn frame_sampling_rejects_same_zone_ids_with_different_sample_counts() {
        let layout = test_layout(vec![test_zone("left"), test_zone("right")]);
        let zones = vec![
            zone_colors_with_count("left", 2),
            zone_colors_with_count("right", 3),
        ];

        assert!(!can_hold_zone_colors_for_deferred_sampling(&layout, &zones));
    }

    #[test]
    fn frame_sampling_rejects_missing_extra_and_resized_zones() {
        let layout = test_layout(vec![test_zone("left"), test_zone("right")]);
        let missing = vec![zone_colors("left")];
        let extra = vec![
            zone_colors("left"),
            zone_colors("right"),
            zone_colors("third"),
        ];
        let resized = vec![zone_colors_with_count("left", 1), zone_colors("right")];

        assert!(!can_hold_zone_colors_for_deferred_sampling(
            &layout, &missing
        ));
        assert!(!can_hold_zone_colors_for_deferred_sampling(&layout, &extra));
        assert!(!can_hold_zone_colors_for_deferred_sampling(
            &layout, &resized
        ));
    }

    #[test]
    fn frame_sampling_ignores_display_zones_for_deferred_match() {
        let layout = test_layout(vec![test_zone("left"), test_zone("display")]);
        let zones = vec![zone_colors("left")];

        assert!(can_hold_zone_colors_for_deferred_sampling(&layout, &zones));
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn zone_sampling_set_counts_each_zone_by_its_source() {
        use std::sync::Arc;

        use super::{ZoneSampleSource, ZoneSampling, ZoneSamplingSet, ZoneSourceCounts};

        let zone = |source| ZoneSampling {
            prepared_zones: Arc::from(Vec::new()),
            source,
        };
        let set = ZoneSamplingSet {
            layout: Arc::new(test_layout(Vec::new())),
            zones: vec![
                zone(ZoneSampleSource::Gpu),
                zone(ZoneSampleSource::Cpu(vec![zone_colors("cpu")])),
                zone(ZoneSampleSource::Held),
                zone(ZoneSampleSource::Gpu),
            ],
            gpu_textures: Vec::new(),
        };

        assert_eq!(
            set.source_counts(),
            ZoneSourceCounts {
                gpu: 2,
                cpu: 1,
                held: 1,
            }
        );
    }
}
