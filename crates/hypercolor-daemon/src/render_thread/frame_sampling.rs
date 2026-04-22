use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use tracing::warn;

use hypercolor_core::scene::interpolate_color;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::canvas::RgbaF32;
use hypercolor_core::types::event::FrameData;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::ColorInterpolation;
use hypercolor_types::spatial::SpatialLayout;

use super::frame_composer::RenderStageStats;
use super::pipeline_runtime::{
    PendingZoneSamplingStatus, RenderCaches, RetainedZoneFrame, SceneTransitionKey,
};
use super::scene_snapshot::{FrameSceneSnapshot, SceneTransitionSnapshot};
use super::sparkleflinger::{PendingZoneSampling, ZoneSamplingDispatch};
use super::{RenderThreadState, micros_between};

#[derive(Clone)]
pub(super) enum LedSamplingStrategy {
    SparkleFlinger(SpatialEngine),
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

    pub(super) fn retain(&self, zones: &[ZoneColors]) -> RetainedLedSamplingStrategy {
        match self {
            Self::PreSampled(layout) => RetainedLedSamplingStrategy::PreSampled {
                layout: Arc::clone(layout),
                zones: zones.to_vec().into(),
            },
            Self::SparkleFlinger(spatial_engine) => {
                RetainedLedSamplingStrategy::SparkleFlinger(spatial_engine.clone())
            }
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
    pub(crate) gpu_sample_retry_hit: bool,
    pub(crate) gpu_sample_queue_saturated: bool,
    pub(crate) gpu_sample_wait_blocked: bool,
    pub(crate) refresh_reused_frame_metadata: bool,
    pub(crate) reuses_published_frame: bool,
}

pub(crate) fn can_reuse_published_frame_for_deferred_sampling(
    render_stage: &RenderStageStats,
    layout: &SpatialLayout,
    published_frame: &FrameData,
) -> bool {
    render_stage.screen_retained
        && can_hold_published_frame_for_deferred_sampling(layout, published_frame)
}

pub(crate) fn can_hold_published_frame_for_deferred_sampling(
    layout: &SpatialLayout,
    published_frame: &FrameData,
) -> bool {
    published_frame.zones.len() == layout.zones.len()
        && published_frame
            .zones
            .iter()
            .zip(&layout.zones)
            .all(|(zone_colors, layout_zone)| zone_colors.zone_id == layout_zone.id)
}

pub(crate) fn try_finish_deferred_zone_sampling(
    render: &mut RenderCaches,
    error_message: &'static str,
) {
    if let Some(PendingZoneSamplingStatus::Stale(deferred_sampling)) = render
        .deferred_sampling
        .take_pending_status(&mut render.sparkleflinger, error_message)
    {
        render.deferred_sampling.store_pending(deferred_sampling);
    }
}

pub(crate) fn try_finish_retired_zone_sampling(
    render: &mut RenderCaches,
    error_message: &'static str,
) {
    render
        .deferred_sampling
        .finish_retired(&mut render.sparkleflinger, error_message);
}

fn try_retire_stale_zone_sampling(
    render: &mut RenderCaches,
    pending: PendingZoneSampling,
) -> Option<PendingZoneSampling> {
    render
        .deferred_sampling
        .retire_or_return(&mut render.sparkleflinger, pending)
}

fn discard_zone_sampling_backlog(render: &mut RenderCaches) {
    render
        .deferred_sampling
        .discard_backlog(&mut render.sparkleflinger);
}

fn scene_transition_key(transition: &SceneTransitionSnapshot) -> Option<SceneTransitionKey> {
    Some(SceneTransitionKey {
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
        LedSamplingStrategy::SparkleFlinger(_) | LedSamplingStrategy::ReusePublished(_) => None,
    }
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
        &RgbaF32::from_srgb_u8(from[0], from[1], from[2], 255),
        &RgbaF32::from_srgb_u8(to[0], to[1], to[2], 255),
        progress,
        color_interpolation,
    )
    .to_srgb_u8();
    [mixed[0], mixed[1], mixed[2]]
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
    render: &mut RenderCaches,
    scene_snapshot: &FrameSceneSnapshot,
    render_stage: &mut RenderStageStats,
    completed_deferred_sampling: Option<PendingZoneSampling>,
    mut stale_deferred_sampling: Option<PendingZoneSampling>,
) -> LedSamplingOutcome {
    let sparkleflinger_sampling_engine = match &render_stage.led_sampling_strategy {
        LedSamplingStrategy::SparkleFlinger(spatial_engine) => Some(spatial_engine.clone()),
        LedSamplingStrategy::PreSampled(_)
        | LedSamplingStrategy::RetainedPreSampled { .. }
        | LedSamplingStrategy::ReusePublished(_) => None,
    };
    let mut gpu_zone_sampling = false;
    let mut gpu_sample_deferred = false;
    let mut gpu_sample_retry_hit = false;
    let mut gpu_sample_queue_saturated = false;
    let mut gpu_sample_wait_blocked = false;
    let mut refresh_reused_frame_metadata = false;
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
        let sampling_engine = sparkleflinger_sampling_engine
            .as_ref()
            .expect("SparkleFlinger-owned LED sampling requires a spatial engine");
        render_stage.sampled_us = 0;
        let prepared_zones = sampling_engine.sampling_plan();
        let layout = sampling_engine.layout();
        (can_reuse_published_frame, can_hold_published_frame) = {
            let published_frame = state.event_bus.frame_sender().borrow();
            (
                can_reuse_published_frame_for_deferred_sampling(
                    render_stage,
                    layout.as_ref(),
                    &published_frame,
                ),
                can_hold_published_frame_for_deferred_sampling(layout.as_ref(), &published_frame),
            )
        };
        let completed_sampling_matches_current =
            completed_deferred_sampling.as_ref().is_some_and(|pending| {
                render
                    .sparkleflinger
                    .pending_zone_sampling_matches_current_work(pending, prepared_zones.as_ref())
            });
        if completed_sampling_matches_current {
            render
                .deferred_sampling
                .clone_scratch_into(render.output_artifacts.zones_mut());
            gpu_zone_sampling = true;
            gpu_sample_retry_hit = true;
        }
        let mut stale_sampling_matches_current = false;
        if !gpu_zone_sampling && let Some(mut pending) = stale_deferred_sampling.take() {
            if render
                .sparkleflinger
                .pending_zone_sampling_matches_current_work(&pending, prepared_zones.as_ref())
            {
                stale_sampling_matches_current = true;
                let stale_sample_finish = Instant::now();
                match render.sparkleflinger.try_finish_pending_zone_sampling(
                    &mut pending,
                    render.output_artifacts.zones_mut(),
                ) {
                    Ok(true) => {
                        gpu_zone_sampling = true;
                        gpu_sample_retry_hit = true;
                        gpu_sample_wait_blocked = render
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
                render.deferred_sampling.store_pending(pending);
            }
            gpu_sample_deferred = true;
            render_stage.led_sampling_strategy =
                LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
            refresh_reused_frame_metadata = render_stage.screen_retained;
        } else if let Some(pending) = stale_deferred_sampling.take()
            && let Some(pending) = try_retire_stale_zone_sampling(render, pending)
        {
            stale_deferred_sampling = Some(pending);
            gpu_sample_queue_saturated = true;
        }
        if !gpu_zone_sampling && gpu_sample_queue_saturated {
            if let Some(pending) = stale_deferred_sampling.take() {
                render.deferred_sampling.store_pending(pending);
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
                match render.sparkleflinger.begin_sample_zone_plan_into(
                    prepared_zones.as_ref(),
                    render.output_artifacts.zones_mut(),
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
                            discard_zone_sampling_backlog(render);
                            match render.sparkleflinger.begin_sample_zone_plan_into(
                                prepared_zones.as_ref(),
                                render.output_artifacts.zones_mut(),
                            ) {
                                Ok(ZoneSamplingDispatch::Ready) => true,
                                Ok(ZoneSamplingDispatch::Pending(pending)) => {
                                    pending_gpu_zone_sampling = Some(pending);
                                    true
                                }
                                Ok(ZoneSamplingDispatch::Unsupported)
                                | Ok(ZoneSamplingDispatch::Saturated) => false,
                                Err(error) => {
                                    warn!(%error, "GPU spatial sampling retry after saturation failed; falling back to CPU");
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
                        warn!(%error, "GPU spatial sampling failed; falling back to CPU");
                        false
                    }
                }
            } else {
                false
            };
        }
        layout
    };

    if let Some(pending) = pending_gpu_zone_sampling.take() {
        let gpu_sample_finish = Instant::now();
        if can_reuse_published_frame {
            let mut pending = pending;
            match render.sparkleflinger.try_finish_pending_zone_sampling(
                &mut pending,
                render.deferred_sampling.scratch_mut(),
            ) {
                Ok(true) => {
                    gpu_sample_wait_blocked = render
                        .sparkleflinger
                        .take_last_sample_readback_wait_blocked();
                }
                Ok(false) => {
                    render.deferred_sampling.store_pending(pending);
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
            render.deferred_sampling.store_pending(pending);
            gpu_zone_sampling = false;
            gpu_sample_deferred = true;
            render_stage.led_sampling_strategy =
                LedSamplingStrategy::ReusePublished(Arc::clone(&layout));
        } else if let Err(error) = render
            .sparkleflinger
            .finish_pending_zone_sampling(pending, render.output_artifacts.zones_mut())
        {
            warn!(%error, "GPU spatial sampling finalize failed; falling back to CPU");
            gpu_zone_sampling = false;
        } else {
            gpu_sample_wait_blocked = render
                .sparkleflinger
                .take_last_sample_readback_wait_blocked();
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
        let cpu_sample_start = Instant::now();
        sparkleflinger_sampling_engine
            .as_ref()
            .expect("CPU spatial sampling requires a SparkleFlinger-owned spatial engine")
            .sample_into(
                render_stage
                    .composed_frame
                    .sampling_canvas
                    .as_ref()
                    .expect("CPU spatial sampling requires a materialized canvas"),
                render.output_artifacts.zones_mut(),
            );
        render_stage.sampled_us = micros_between(cpu_sample_start, Instant::now());
    }

    if let Some(transition) = scene_snapshot.scene_runtime.active_transition.as_ref()
        && uses_scene_sampled_zones
        && let Some(transition_key) = scene_transition_key(transition)
        && let Some(current_zones) = current_scene_sampled_zones(
            &render_stage.led_sampling_strategy,
            render.output_artifacts.zones(),
        )
        .map(<[ZoneColors]>::to_vec)
    {
        let transition_blend_start = Instant::now();
        if render.zone_transition_planner.active_transition != Some(transition_key) {
            render.zone_transition_planner.active_transition = Some(transition_key);
            render.zone_transition_planner.transition_base = Some(
                render
                    .zone_transition_planner
                    .last_stable
                    .clone()
                    .unwrap_or_else(|| RetainedZoneFrame {
                        layout: Arc::clone(&layout),
                        zones: current_zones.clone(),
                    }),
            );
        }

        if let Some(base) = render.zone_transition_planner.transition_base.as_ref() {
            let transition_layout =
                build_transition_layout(base.layout.as_ref(), layout.as_ref(), transition_key);
            blend_scene_zone_frames(
                &base.zones,
                &current_zones,
                transition_layout.as_ref(),
                transition.eased_progress.clamp(0.0, 1.0),
                &transition.color_interpolation,
                render.output_artifacts.zones_mut(),
            );
            layout = Arc::clone(&transition_layout);
            render_stage.led_sampling_strategy = LedSamplingStrategy::PreSampled(transition_layout);
            retained_scene_zones = None;
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
    if scene_snapshot.scene_runtime.active_transition.is_none() {
        if let Some(retained_zones) = retained_scene_zones.as_ref() {
            render
                .zone_transition_planner
                .record_stable(Arc::clone(&layout), retained_zones.as_ref());
        } else if reuses_published_frame {
            let published_frame = state.event_bus.frame_sender().borrow();
            render
                .zone_transition_planner
                .record_stable(Arc::clone(&layout), &published_frame.zones);
        } else {
            render
                .zone_transition_planner
                .record_stable(Arc::clone(&layout), render.output_artifacts.zones());
        }
    }

    LedSamplingOutcome {
        layout,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        refresh_reused_frame_metadata,
        reuses_published_frame,
    }
}
