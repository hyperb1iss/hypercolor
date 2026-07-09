use super::frame_io::PublishFrameStats;
use super::frame_policy::FrameAdmissionSample;
use super::pipeline_runtime::RenderSurfaceSnapshot;
use super::scene_snapshot::FrameSceneSnapshot;
use super::u64_to_u32;
use crate::performance::{
    CompositorBackendKind, FrameTimeline, FullFrameCopyMetrics, LatestFrameMetrics,
    OutputFrameSourceKind,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActiveFrameSummary {
    pub(crate) metrics: LatestFrameMetrics,
    pub(crate) admission: FrameAdmissionSample,
}

pub(crate) struct ActiveFrameMetricsInput<'a> {
    pub(crate) scene_snapshot: &'a FrameSceneSnapshot,
    pub(crate) render_surfaces: &'a RenderSurfaceSnapshot,
    pub(crate) publish_stats: &'a PublishFrameStats,
    pub(crate) producer_full_frame_copy: FullFrameCopyMetrics,
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
    pub(crate) gpu_sample_stale: bool,
    pub(crate) gpu_sample_retry_hit: bool,
    pub(crate) gpu_sample_queue_saturated: bool,
    pub(crate) gpu_sample_wait_blocked: bool,
    pub(crate) gpu_sample_cpu_fallback: bool,
    pub(crate) cpu_readback_skipped: bool,
    pub(crate) gpu_readback_failed: bool,
    pub(crate) compositor_backend: CompositorBackendKind,
    pub(crate) output_frame_source: OutputFrameSourceKind,
    pub(crate) output_reuses_published_frame: bool,
    pub(crate) output_brightness_bits: u32,
    pub(crate) output_brightness_generation: u64,
    pub(crate) output_routing_signature: u64,
    pub(crate) output_zone_shape_signature: u64,
    pub(crate) output_unassigned_behavior_generation: u64,
    pub(crate) devices_written: u32,
    pub(crate) total_leds: u32,
    pub(crate) output_errors: u32,
    pub(crate) logical_layer_count: u32,
    pub(crate) render_group_count: u32,
    pub(crate) scene_active: bool,
    pub(crate) scene_transition_active: bool,
    pub(crate) effect_retained: bool,
    pub(crate) screen_retained: bool,
    pub(crate) composition_bypassed: bool,
    pub(crate) preview_surface_pressure: bool,
    pub(crate) scene_canvas_forced_surface: bool,
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
    pub(crate) devices_written: u32,
    pub(crate) total_leds: u32,
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
        producer_full_frame_copy,
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
        gpu_sample_stale,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback,
        cpu_readback_skipped,
        gpu_readback_failed,
        compositor_backend,
        output_frame_source,
        output_reuses_published_frame,
        output_brightness_bits,
        output_brightness_generation,
        output_routing_signature,
        output_zone_shape_signature,
        output_unassigned_behavior_generation,
        devices_written,
        total_leds,
        output_errors,
        logical_layer_count,
        render_group_count,
        scene_active,
        scene_transition_active,
        effect_retained,
        screen_retained,
        composition_bypassed,
        preview_surface_pressure,
        scene_canvas_forced_surface,
        scene_snapshot_done_us,
        input_done_us,
        sample_done_us,
        output_done_us,
        publish_done_us,
    } = input;
    let publication_full_frame_copy = publish_stats.publication_full_frame_copy;
    let full_frame_copy_count = producer_full_frame_copy
        .count
        .saturating_add(publication_full_frame_copy.count);
    let full_frame_copy_bytes = producer_full_frame_copy
        .bytes
        .saturating_add(publication_full_frame_copy.bytes);

    LatestFrameMetrics {
        timestamp_ms: u64_to_u32(scene_snapshot.elapsed_ms),
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
        gpu_sample_stale,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback,
        cpu_readback_skipped,
        gpu_readback_failed,
        compositor_backend,
        output_frame_source,
        output_reuses_published_frame,
        output_brightness_bits,
        output_brightness_generation,
        output_routing_signature,
        output_zone_shape_signature,
        output_unassigned_behavior_generation,
        devices_written,
        total_leds,
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
        scene_pool_slot_count: render_surfaces.scene_pool_slot_count,
        scene_pool_max_slots: render_surfaces.scene_pool_max_slots,
        direct_pool_slot_count: render_surfaces.direct_pool_slot_count,
        direct_pool_max_slots: render_surfaces.direct_pool_max_slots,
        scene_pool_shared_published_slots: render_surfaces.scene_pool_shared_published_slots,
        scene_pool_max_ref_count: render_surfaces.scene_pool_max_ref_count,
        direct_pool_shared_published_slots: render_surfaces.direct_pool_shared_published_slots,
        direct_pool_max_ref_count: render_surfaces.direct_pool_max_ref_count,
        canvas_receiver_count: render_surfaces.canvas_receivers,
        producer_full_frame_copy,
        publication_full_frame_copy,
        full_frame_copy_count,
        full_frame_copy_bytes,
        scene_canvas_forced_surface,
        preview_surface: preview_surface_pressure,
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
    let admission = build_frame_admission_sample(&metrics);

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
        devices_written,
        total_leds,
        scene_snapshot_done_us,
        sample_done_us,
        output_done_us,
        publish_done_us,
    } = input;
    LatestFrameMetrics {
        timestamp_ms: u64_to_u32(scene_snapshot.elapsed_ms),
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
        gpu_sample_stale: false,
        gpu_sample_retry_hit: false,
        gpu_sample_queue_saturated: false,
        gpu_sample_wait_blocked: false,
        gpu_sample_cpu_fallback: false,
        cpu_readback_skipped: false,
        gpu_readback_failed: false,
        compositor_backend: CompositorBackendKind::Cpu,
        output_frame_source: OutputFrameSourceKind::CurrentFrame,
        output_reuses_published_frame: false,
        output_brightness_bits: 0,
        output_brightness_generation: 0,
        output_routing_signature: 0,
        output_zone_shape_signature: 0,
        output_unassigned_behavior_generation: 0,
        devices_written,
        total_leds,
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
        scene_pool_slot_count: render_surfaces.scene_pool_slot_count,
        scene_pool_max_slots: render_surfaces.scene_pool_max_slots,
        direct_pool_slot_count: render_surfaces.direct_pool_slot_count,
        direct_pool_max_slots: render_surfaces.direct_pool_max_slots,
        scene_pool_shared_published_slots: render_surfaces.scene_pool_shared_published_slots,
        scene_pool_max_ref_count: render_surfaces.scene_pool_max_ref_count,
        direct_pool_shared_published_slots: render_surfaces.direct_pool_shared_published_slots,
        direct_pool_max_ref_count: render_surfaces.direct_pool_max_ref_count,
        canvas_receiver_count: render_surfaces.canvas_receivers,
        producer_full_frame_copy: FullFrameCopyMetrics::default(),
        publication_full_frame_copy: publish_stats.publication_full_frame_copy,
        full_frame_copy_count: publish_stats.publication_full_frame_copy.count,
        full_frame_copy_bytes: publish_stats.publication_full_frame_copy.bytes,
        scene_canvas_forced_surface: false,
        preview_surface: false,
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

fn build_frame_admission_sample(metrics: &LatestFrameMetrics) -> FrameAdmissionSample {
    FrameAdmissionSample {
        total_us: metrics.total_us,
        producer_us: metrics.producer_us,
        composition_us: metrics.composition_us,
        push_us: metrics.push_us,
        publish_us: metrics.publish_us,
        full_frame_copy_count: metrics.full_frame_copy_count,
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
    use hypercolor_types::scene::{ColorInterpolation, UnassignedBehavior, Zone};
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

    use super::{
        ActiveFrameMetricsInput, PublishFrameStats, RenderSurfaceSnapshot,
        ThrottleFrameMetricsInput, build_throttle_frame_metrics, summarize_active_frame,
    };
    use crate::performance::{CompositorBackendKind, FullFrameCopyMetrics, OutputFrameSourceKind};
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
                active_scene_name: None,
                active_transition: Some(SceneTransitionSnapshot {
                    from_scene: None,
                    to_scene: None,
                    progress: 0.25,
                    eased_progress: 0.5,
                    color_interpolation: ColorInterpolation::Srgb,
                }),
                active_render_groups: Arc::<[Zone]>::from(Vec::<Zone>::new()),
                active_render_groups_revision: 3,
                zone_layout_preview_generation: 0,
                active_render_group_count: 2,
                active_display_group_target_fps: HashMap::new(),
                active_display_group_output_routes: HashMap::new(),
                active_display_group_descriptors: HashMap::new(),
                unassigned_behavior: UnassignedBehavior::default(),
                device_registry_generation: 0,
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
            scene_pool_slot_count: 10,
            scene_pool_max_slots: 12,
            direct_pool_slot_count: 6,
            direct_pool_max_slots: 8,
            scene_pool_shared_published_slots: 7,
            scene_pool_max_ref_count: 3,
            direct_pool_shared_published_slots: 4,
            direct_pool_max_ref_count: 2,
        }
    }

    fn publish_stats() -> PublishFrameStats {
        PublishFrameStats {
            elapsed_us: 310,
            publication_full_frame_copy: FullFrameCopyMetrics {
                count: 2,
                bytes: 8_192,
                reason: Some("publication_test"),
            },
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
            producer_full_frame_copy: FullFrameCopyMetrics {
                count: 1,
                bytes: 4_096,
                reason: Some("producer_test"),
            },
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
            gpu_sample_stale: true,
            gpu_sample_retry_hit: true,
            gpu_sample_queue_saturated: false,
            gpu_sample_wait_blocked: false,
            gpu_sample_cpu_fallback: true,
            cpu_readback_skipped: true,
            gpu_readback_failed: true,
            compositor_backend: CompositorBackendKind::Gpu,
            output_frame_source: OutputFrameSourceKind::PublishedFrame,
            output_reuses_published_frame: true,
            output_brightness_bits: 1.0_f32.to_bits(),
            output_brightness_generation: 7,
            output_routing_signature: 11,
            output_zone_shape_signature: 13,
            output_unassigned_behavior_generation: 17,
            devices_written: 5,
            total_leds: 321,
            output_errors: 3,
            logical_layer_count: 4,
            render_group_count: 2,
            scene_active: true,
            scene_transition_active: true,
            effect_retained: true,
            screen_retained: false,
            composition_bypassed: false,
            preview_surface_pressure: true,
            scene_canvas_forced_surface: true,
            scene_snapshot_done_us: 30,
            input_done_us: 60,
            sample_done_us: 500,
            output_done_us: 560,
            publish_done_us: 640,
        });

        assert_eq!(summary.metrics.publish_us, 310);
        assert_eq!(summary.metrics.producer_full_frame_copy.count, 1);
        assert_eq!(summary.metrics.publication_full_frame_copy.count, 2);
        assert_eq!(summary.metrics.full_frame_copy_count, 3);
        assert_eq!(summary.metrics.full_frame_copy_bytes, 12_288);
        assert!(summary.metrics.preview_surface);
        assert!(summary.metrics.scene_canvas_forced_surface);
        assert_eq!(
            summary.metrics.output_frame_source,
            OutputFrameSourceKind::PublishedFrame
        );
        assert!(summary.metrics.output_reuses_published_frame);
        assert_eq!(summary.metrics.output_routing_signature, 11);
        assert_eq!(summary.metrics.output_zone_shape_signature, 13);
        assert_eq!(summary.metrics.devices_written, 5);
        assert_eq!(summary.metrics.total_leds, 321);
        assert_eq!(summary.metrics.output_errors, 3);
        assert_eq!(summary.admission.total_us, summary.metrics.total_us);
        assert_eq!(summary.admission.producer_us, summary.metrics.producer_us);
        assert_eq!(
            summary.admission.composition_us,
            summary.metrics.composition_us
        );
        assert_eq!(summary.admission.push_us, summary.metrics.push_us);
        assert_eq!(summary.admission.publish_us, summary.metrics.publish_us);
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
            devices_written: 3,
            total_leds: 99,
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
        assert_eq!(
            metrics.output_frame_source,
            OutputFrameSourceKind::CurrentFrame
        );
        assert!(!metrics.output_reuses_published_frame);
        assert_eq!(metrics.devices_written, 3);
        assert_eq!(metrics.total_leds, 99);
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
