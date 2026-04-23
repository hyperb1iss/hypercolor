use super::frame_io::PublishFrameStats;
use super::frame_policy::FrameAdmissionSample;
use super::pipeline_runtime::RenderSurfaceSnapshot;
use super::scene_snapshot::FrameSceneSnapshot;
use crate::performance::{CompositorBackendKind, FrameTimeline, LatestFrameMetrics};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActiveFrameSummary {
    pub(crate) metrics: LatestFrameMetrics,
    pub(crate) admission: FrameAdmissionSample,
}

pub(crate) struct ActiveFrameMetricsInput<'a> {
    pub(crate) scene_snapshot: &'a FrameSceneSnapshot,
    pub(crate) render_surfaces: &'a RenderSurfaceSnapshot,
    pub(crate) publish_stats: &'a PublishFrameStats,
    pub(crate) input_us: u32,
    pub(crate) producer_us: u32,
    pub(crate) producer_render_us: u32,
    pub(crate) producer_scene_compose_us: u32,
    pub(crate) composition_us: u32,
    pub(crate) producer_done_us: u32,
    pub(crate) composition_done_us: u32,
    pub(crate) render_us: u32,
    pub(crate) sample_us: u32,
    pub(crate) push_us: u32,
    pub(crate) postprocess_us: u32,
    pub(crate) total_us: u32,
    pub(crate) wake_late_us: u32,
    pub(crate) jitter_us: u32,
    pub(crate) overhead_us: u32,
    pub(crate) reused_inputs: bool,
    pub(crate) reused_canvas: bool,
    pub(crate) gpu_zone_sampling: bool,
    pub(crate) gpu_sample_deferred: bool,
    pub(crate) gpu_sample_retry_hit: bool,
    pub(crate) gpu_sample_queue_saturated: bool,
    pub(crate) gpu_sample_wait_blocked: bool,
    pub(crate) cpu_sampling_late_readback: bool,
    pub(crate) cpu_readback_skipped: bool,
    pub(crate) compositor_backend: CompositorBackendKind,
    pub(crate) output_errors: u32,
    pub(crate) logical_layer_count: u32,
    pub(crate) render_group_count: u32,
    pub(crate) scene_active: bool,
    pub(crate) scene_transition_active: bool,
    pub(crate) effect_retained: bool,
    pub(crate) screen_retained: bool,
    pub(crate) composition_bypassed: bool,
    pub(crate) scene_snapshot_done_us: u32,
    pub(crate) input_done_us: u32,
    pub(crate) sample_done_us: u32,
    pub(crate) output_done_us: u32,
    pub(crate) publish_done_us: u32,
}

pub(crate) struct ThrottleFrameMetricsInput<'a> {
    pub(crate) scene_snapshot: &'a FrameSceneSnapshot,
    pub(crate) render_surfaces: &'a RenderSurfaceSnapshot,
    pub(crate) publish_stats: &'a PublishFrameStats,
    pub(crate) sample_us: u32,
    pub(crate) push_us: u32,
    pub(crate) total_us: u32,
    pub(crate) overhead_us: u32,
    pub(crate) output_errors: u32,
    pub(crate) scene_snapshot_done_us: u32,
    pub(crate) sample_done_us: u32,
    pub(crate) output_done_us: u32,
    pub(crate) publish_done_us: u32,
}

pub(crate) fn build_active_frame_metrics(input: ActiveFrameMetricsInput<'_>) -> LatestFrameMetrics {
    let ActiveFrameMetricsInput {
        scene_snapshot,
        render_surfaces,
        publish_stats,
        input_us,
        producer_us,
        producer_render_us,
        producer_scene_compose_us,
        composition_us,
        producer_done_us,
        composition_done_us,
        render_us,
        sample_us,
        push_us,
        postprocess_us,
        total_us,
        wake_late_us,
        jitter_us,
        overhead_us,
        reused_inputs,
        reused_canvas,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        cpu_sampling_late_readback,
        cpu_readback_skipped,
        compositor_backend,
        output_errors,
        logical_layer_count,
        render_group_count,
        scene_active,
        scene_transition_active,
        effect_retained,
        screen_retained,
        composition_bypassed,
        scene_snapshot_done_us,
        input_done_us,
        sample_done_us,
        output_done_us,
        publish_done_us,
    } = input;
    LatestFrameMetrics {
        timestamp_ms: scene_snapshot.elapsed_ms,
        input_us,
        producer_us,
        producer_render_us,
        producer_scene_compose_us,
        composition_us,
        render_us,
        sample_us,
        push_us,
        postprocess_us,
        publish_us: publish_stats.elapsed_us,
        publish_frame_data_us: publish_stats.frame_data_us,
        publish_group_canvas_us: publish_stats.group_canvas_us,
        publish_preview_us: publish_stats.preview_us,
        publish_events_us: publish_stats.events_us,
        overhead_us,
        total_us,
        wake_late_us,
        jitter_us,
        reused_inputs,
        reused_canvas,
        retained_effect: effect_retained,
        retained_screen: screen_retained,
        composition_bypassed,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        cpu_sampling_late_readback,
        cpu_readback_skipped,
        compositor_backend,
        logical_layer_count,
        render_group_count,
        scene_active,
        scene_transition_active,
        render_surface_slot_count: render_surfaces.slot_count,
        render_surface_free_slots: render_surfaces.free_slots,
        render_surface_published_slots: render_surfaces.published_slots,
        render_surface_dequeued_slots: render_surfaces.dequeued_slots,
        scene_pool_saturation_reallocs: render_surfaces.scene_pool_saturation_reallocs,
        direct_pool_saturation_reallocs: render_surfaces.direct_pool_saturation_reallocs,
        scene_pool_grown_slots: render_surfaces.scene_pool_grown_slots,
        direct_pool_grown_slots: render_surfaces.direct_pool_grown_slots,
        canvas_receiver_count: render_surfaces.canvas_receivers,
        full_frame_copy_count: publish_stats.full_frame_copy_count,
        full_frame_copy_bytes: publish_stats.full_frame_copy_bytes,
        output_errors,
        timeline: build_frame_timeline(
            scene_snapshot,
            scene_snapshot_done_us,
            input_done_us,
            input_done_us.saturating_add(producer_done_us),
            input_done_us.saturating_add(composition_done_us),
            sample_done_us,
            output_done_us,
            publish_done_us,
            total_us,
        ),
    }
}

pub(crate) fn summarize_active_frame(input: ActiveFrameMetricsInput<'_>) -> ActiveFrameSummary {
    let metrics = build_active_frame_metrics(input);
    let admission = build_frame_admission_sample(metrics);

    ActiveFrameSummary { metrics, admission }
}

pub(crate) fn build_throttle_frame_metrics(
    input: ThrottleFrameMetricsInput<'_>,
) -> LatestFrameMetrics {
    let ThrottleFrameMetricsInput {
        scene_snapshot,
        render_surfaces,
        publish_stats,
        sample_us,
        push_us,
        total_us,
        overhead_us,
        output_errors,
        scene_snapshot_done_us,
        sample_done_us,
        output_done_us,
        publish_done_us,
    } = input;
    LatestFrameMetrics {
        timestamp_ms: scene_snapshot.elapsed_ms,
        input_us: 0,
        producer_us: 0,
        producer_render_us: 0,
        producer_scene_compose_us: 0,
        composition_us: 0,
        render_us: 0,
        sample_us,
        push_us,
        postprocess_us: 0,
        publish_us: publish_stats.elapsed_us,
        publish_frame_data_us: publish_stats.frame_data_us,
        publish_group_canvas_us: publish_stats.group_canvas_us,
        publish_preview_us: publish_stats.preview_us,
        publish_events_us: publish_stats.events_us,
        overhead_us,
        total_us,
        wake_late_us: 0,
        jitter_us: 0,
        reused_inputs: false,
        reused_canvas: false,
        retained_effect: false,
        retained_screen: false,
        composition_bypassed: false,
        gpu_zone_sampling: false,
        gpu_sample_deferred: false,
        gpu_sample_retry_hit: false,
        gpu_sample_queue_saturated: false,
        gpu_sample_wait_blocked: false,
        cpu_sampling_late_readback: false,
        cpu_readback_skipped: false,
        compositor_backend: CompositorBackendKind::Cpu,
        logical_layer_count: 0,
        render_group_count: scene_snapshot.scene_runtime.active_render_group_count(),
        scene_active: scene_snapshot.scene_runtime.active_scene_id.is_some(),
        scene_transition_active: scene_snapshot.scene_runtime.active_transition.is_some(),
        render_surface_slot_count: render_surfaces.slot_count,
        render_surface_free_slots: render_surfaces.free_slots,
        render_surface_published_slots: render_surfaces.published_slots,
        render_surface_dequeued_slots: render_surfaces.dequeued_slots,
        scene_pool_saturation_reallocs: render_surfaces.scene_pool_saturation_reallocs,
        direct_pool_saturation_reallocs: render_surfaces.direct_pool_saturation_reallocs,
        scene_pool_grown_slots: render_surfaces.scene_pool_grown_slots,
        direct_pool_grown_slots: render_surfaces.direct_pool_grown_slots,
        canvas_receiver_count: render_surfaces.canvas_receivers,
        full_frame_copy_count: publish_stats.full_frame_copy_count,
        full_frame_copy_bytes: publish_stats.full_frame_copy_bytes,
        output_errors,
        timeline: build_frame_timeline(
            scene_snapshot,
            scene_snapshot_done_us,
            scene_snapshot_done_us,
            scene_snapshot_done_us,
            scene_snapshot_done_us,
            sample_done_us,
            output_done_us,
            publish_done_us,
            total_us,
        ),
    }
}

fn build_frame_admission_sample(metrics: LatestFrameMetrics) -> FrameAdmissionSample {
    FrameAdmissionSample {
        total_us: metrics.total_us,
        producer_us: metrics.producer_us,
        composition_us: metrics.composition_us,
        push_us: metrics.push_us,
        publish_us: metrics.publish_us,
        wake_late_us: metrics.wake_late_us,
        jitter_us: metrics.jitter_us,
        full_frame_copy_count: metrics.full_frame_copy_count,
        cpu_sampling_late_readback: metrics.cpu_sampling_late_readback,
        output_errors: metrics.output_errors,
    }
}

fn build_frame_timeline(
    scene_snapshot: &FrameSceneSnapshot,
    scene_snapshot_done_us: u32,
    input_done_us: u32,
    producer_done_us: u32,
    composition_done_us: u32,
    sample_done_us: u32,
    output_done_us: u32,
    publish_done_us: u32,
    frame_done_us: u32,
) -> FrameTimeline {
    FrameTimeline {
        frame_token: scene_snapshot.frame_token,
        budget_us: scene_snapshot.budget_us,
        scene_snapshot_done_us,
        input_done_us,
        producer_done_us,
        composition_done_us,
        sample_done_us,
        output_done_us,
        publish_done_us,
        frame_done_us,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::scene::{ColorInterpolation, RenderGroup};
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

    use super::{
        ActiveFrameMetricsInput, PublishFrameStats, RenderSurfaceSnapshot,
        ThrottleFrameMetricsInput, build_throttle_frame_metrics, summarize_active_frame,
    };
    use crate::performance::CompositorBackendKind;
    use crate::render_thread::scene_dependency::SceneDependencyKey;
    use crate::render_thread::scene_snapshot::{
        EffectDemand, FrameSceneSnapshot, SceneRuntimeSnapshot, SceneTransitionSnapshot,
    };
    use crate::session::OutputPowerState;

    fn scene_snapshot() -> FrameSceneSnapshot {
        FrameSceneSnapshot {
            frame_token: 42,
            elapsed_ms: 1_234,
            budget_us: 16_666,
            output_power: OutputPowerState::default(),
            effect_demand: EffectDemand {
                effect_running: true,
                audio_capture_active: false,
                screen_capture_active: true,
            },
            effect_dependency_key: SceneDependencyKey::new(3, 7),
            scene_runtime: SceneRuntimeSnapshot {
                active_scene_id: None,
                active_transition: Some(SceneTransitionSnapshot {
                    from_scene: None,
                    to_scene: None,
                    progress: 0.25,
                    eased_progress: 0.5,
                    color_interpolation: ColorInterpolation::Srgb,
                }),
                active_render_groups: Arc::<[RenderGroup]>::from(Vec::<RenderGroup>::new()),
                active_render_groups_revision: 3,
                active_render_group_count: 2,
                active_display_group_target_fps: HashMap::new(),
            },
            spatial_engine: SpatialEngine::new(SpatialLayout {
                id: "layout".to_owned(),
                name: "layout".to_owned(),
                description: None,
                canvas_width: 1,
                canvas_height: 1,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Nearest,
                default_edge_behavior: EdgeBehavior::Clamp,
                spaces: None,
                version: 1,
            }),
        }
    }

    fn render_surfaces() -> RenderSurfaceSnapshot {
        RenderSurfaceSnapshot {
            slot_count: 8,
            free_slots: 4,
            published_slots: 2,
            dequeued_slots: 1,
            canvas_receivers: 3,
            scene_pool_saturation_reallocs: 9,
            direct_pool_saturation_reallocs: 5,
            scene_pool_grown_slots: 2,
            direct_pool_grown_slots: 1,
        }
    }

    fn publish_stats() -> PublishFrameStats {
        PublishFrameStats {
            elapsed_us: 310,
            full_frame_copy_count: 2,
            full_frame_copy_bytes: 8_192,
            frame_data_us: 50,
            group_canvas_us: 60,
            preview_us: 70,
            events_us: 80,
        }
    }

    #[test]
    fn active_frame_summary_derives_admission_from_metrics() {
        let summary = summarize_active_frame(ActiveFrameMetricsInput {
            scene_snapshot: &scene_snapshot(),
            render_surfaces: &render_surfaces(),
            publish_stats: &publish_stats(),
            input_us: 120,
            producer_us: 220,
            producer_render_us: 140,
            producer_scene_compose_us: 80,
            composition_us: 90,
            producer_done_us: 45,
            composition_done_us: 55,
            render_us: 300,
            sample_us: 40,
            push_us: 25,
            postprocess_us: 15,
            total_us: 640,
            wake_late_us: 11,
            jitter_us: 7,
            overhead_us: 40,
            reused_inputs: true,
            reused_canvas: false,
            gpu_zone_sampling: true,
            gpu_sample_deferred: false,
            gpu_sample_retry_hit: true,
            gpu_sample_queue_saturated: false,
            gpu_sample_wait_blocked: false,
            cpu_sampling_late_readback: true,
            cpu_readback_skipped: true,
            compositor_backend: CompositorBackendKind::Gpu,
            output_errors: 3,
            logical_layer_count: 4,
            render_group_count: 2,
            scene_active: true,
            scene_transition_active: true,
            effect_retained: true,
            screen_retained: false,
            composition_bypassed: false,
            scene_snapshot_done_us: 30,
            input_done_us: 60,
            sample_done_us: 500,
            output_done_us: 560,
            publish_done_us: 640,
        });

        assert_eq!(summary.metrics.publish_us, 310);
        assert_eq!(summary.metrics.full_frame_copy_count, 2);
        assert_eq!(summary.metrics.output_errors, 3);
        assert_eq!(summary.admission.total_us, summary.metrics.total_us);
        assert_eq!(summary.admission.producer_us, summary.metrics.producer_us);
        assert_eq!(
            summary.admission.composition_us,
            summary.metrics.composition_us
        );
        assert_eq!(summary.admission.push_us, summary.metrics.push_us);
        assert_eq!(summary.admission.publish_us, summary.metrics.publish_us);
        assert_eq!(summary.admission.wake_late_us, summary.metrics.wake_late_us);
        assert_eq!(summary.admission.jitter_us, summary.metrics.jitter_us);
        assert_eq!(
            summary.admission.full_frame_copy_count,
            summary.metrics.full_frame_copy_count
        );
        assert_eq!(
            summary.admission.output_errors,
            summary.metrics.output_errors
        );
    }

    #[test]
    fn throttle_frame_metrics_preserve_sleep_defaults() {
        let metrics = build_throttle_frame_metrics(ThrottleFrameMetricsInput {
            scene_snapshot: &scene_snapshot(),
            render_surfaces: &render_surfaces(),
            publish_stats: &publish_stats(),
            sample_us: 22,
            push_us: 33,
            total_us: 111,
            overhead_us: 44,
            output_errors: 2,
            scene_snapshot_done_us: 10,
            sample_done_us: 70,
            output_done_us: 90,
            publish_done_us: 111,
        });

        assert_eq!(metrics.input_us, 0);
        assert_eq!(metrics.producer_us, 0);
        assert_eq!(metrics.composition_us, 0);
        assert_eq!(metrics.render_us, 0);
        assert_eq!(metrics.postprocess_us, 0);
        assert_eq!(metrics.compositor_backend, CompositorBackendKind::Cpu);
        assert!(!metrics.reused_inputs);
        assert!(!metrics.reused_canvas);
        assert!(!metrics.retained_effect);
        assert!(!metrics.retained_screen);
        assert!(!metrics.composition_bypassed);
        assert_eq!(metrics.output_errors, 2);
        assert_eq!(metrics.timeline.input_done_us, 10);
        assert_eq!(metrics.timeline.producer_done_us, 10);
        assert_eq!(metrics.timeline.composition_done_us, 10);
        assert_eq!(metrics.timeline.sample_done_us, 70);
        assert_eq!(metrics.timeline.output_done_us, 90);
        assert_eq!(metrics.timeline.publish_done_us, 111);
        assert_eq!(metrics.timeline.frame_done_us, 111);
    }
}
