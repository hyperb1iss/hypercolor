use hypercolor_core::bus::PreviewKind;
use hypercolor_types::api::system::{
    EffectHealthStatus, GpuCompositorProbeStatus, LatestFrameStatus, PreviewDemandStatus,
    PreviewRuntimeStatus, RenderAccelerationStatus, RenderSurfaceStatus,
};
use hypercolor_types::config::RenderAccelerationMode;

use crate::performance::{EffectHealthSummary, LatestFrameMetrics, render_health_counts};
use crate::preview_runtime::{PreviewDemandSummary, PreviewRuntime};

pub(super) fn effect_health_status(effect_health: EffectHealthSummary) -> EffectHealthStatus {
    let health = render_health_counts();
    let servo_health = health.servo;
    let pipeline_health = health.pipeline;
    EffectHealthStatus {
        errors_total: effect_health.errors_total,
        fallbacks_applied_total: effect_health.fallbacks_applied_total,
        producer_gpu_readback_failures_total: effect_health.producer_gpu_readback_failures_total,
        servo_soft_stalls_total: servo_health.soft_stalls_total,
        servo_breaker_opens_total: servo_health.breaker_opens_total,
        servo_session_creates_total: servo_health.session_creates_total,
        servo_session_create_failures_total: servo_health.session_create_failures_total,
        servo_session_create_wait_total_ms: us_to_ms_f64(servo_health.session_create_wait_total_us),
        servo_session_create_wait_max_ms: us_to_ms_f64(servo_health.session_create_wait_max_us),
        servo_page_loads_total: servo_health.page_loads_total,
        servo_page_load_failures_total: servo_health.page_load_failures_total,
        servo_page_load_wait_total_ms: us_to_ms_f64(servo_health.page_load_wait_total_us),
        servo_page_load_wait_max_ms: us_to_ms_f64(servo_health.page_load_wait_max_us),
        servo_detached_destroys_total: servo_health.detached_destroys_total,
        servo_detached_destroy_failures_total: servo_health.detached_destroy_failures_total,
        servo_render_requests_total: servo_health.render_requests_total,
        servo_render_queue_wait_total_ms: us_to_ms_f64(servo_health.render_queue_wait_total_us),
        servo_render_queue_wait_max_ms: us_to_ms_f64(servo_health.render_queue_wait_max_us),
        servo_render_scene_requests_total: servo_health.render_scene_requests_total,
        servo_render_scene_queue_wait_total_ms: us_to_ms_f64(
            servo_health.render_scene_queue_wait_total_us,
        ),
        servo_render_scene_queue_wait_max_ms: us_to_ms_f64(
            servo_health.render_scene_queue_wait_max_us,
        ),
        servo_render_display_requests_total: servo_health.render_display_requests_total,
        servo_render_display_queue_wait_total_ms: us_to_ms_f64(
            servo_health.render_display_queue_wait_total_us,
        ),
        servo_render_display_queue_wait_max_ms: us_to_ms_f64(
            servo_health.render_display_queue_wait_max_us,
        ),
        servo_render_cpu_frames_total: servo_health.render_cpu_frames_total,
        servo_render_cached_frames_total: servo_health.render_cached_frames_total,
        servo_render_gpu_frames_total: servo_health.render_gpu_frames_total,
        servo_gpu_import_failures_total: servo_health.render_gpu_import_failures_total,
        servo_gpu_import_fallbacks_total: servo_health.render_gpu_import_fallbacks_total,
        servo_gpu_import_fallback_reason: servo_health
            .render_gpu_import_fallback_reason
            .map(str::to_owned),
        servo_gpu_import_windows_sync_mode: servo_health
            .render_gpu_import_windows_sync_mode
            .map(str::to_owned),
        servo_gpu_import_stale_frame_total: servo_health.render_gpu_import_stale_frame_total,
        servo_gpu_import_adapter_mismatch_total: servo_health
            .render_gpu_import_adapter_mismatch_total,
        servo_gpu_import_slot_count: servo_health.render_gpu_import_slot_count,
        servo_gpu_import_pending_slots: servo_health.render_gpu_import_pending_slots,
        servo_gpu_import_pending_slots_max: servo_health.render_gpu_import_pending_slots_max,
        servo_gpu_import_completed_slots: servo_health.render_gpu_import_completed_slots,
        servo_gpu_import_available_slots: servo_health.render_gpu_import_available_slots,
        servo_gpu_import_available_slots_min: servo_health.render_gpu_import_available_slots_min,
        servo_gpu_import_oldest_pending_age_max_ms: us_to_ms_f64(
            servo_health.render_gpu_import_oldest_pending_age_max_us,
        ),
        servo_gpu_import_blit_total_ms: us_to_ms_f64(servo_health.render_gpu_import_blit_total_us),
        servo_gpu_import_blit_max_ms: us_to_ms_f64(servo_health.render_gpu_import_blit_max_us),
        servo_gpu_import_sync_total_ms: us_to_ms_f64(servo_health.render_gpu_import_sync_total_us),
        servo_gpu_import_sync_max_ms: us_to_ms_f64(servo_health.render_gpu_import_sync_max_us),
        servo_gpu_import_total_ms: us_to_ms_f64(servo_health.render_gpu_import_total_us),
        servo_gpu_import_max_ms: us_to_ms_f64(servo_health.render_gpu_import_max_us),
        producer_cpu_frames_total: pipeline_health.cpu_producer_frames,
        producer_gpu_frames_total: pipeline_health.gpu_producer_frames,
        producer_gpu_cpu_materialization_blocked_total: pipeline_health
            .gpu_cpu_materialization_blocked_total,
        sparkleflinger_gpu_source_upload_skipped_total: pipeline_health.skipped_gpu_source_uploads,
        sparkleflinger_media_texture_allocations_total: pipeline_health
            .media_texture_allocations_total,
        sparkleflinger_media_texture_upload_bytes_total: pipeline_health
            .media_texture_upload_bytes_total,
        sparkleflinger_display_finalize_rgba_attempts_total: pipeline_health
            .display_finalize_rgba_attempts_total,
        sparkleflinger_display_finalize_yuv_attempts_total: pipeline_health
            .display_finalize_yuv_attempts_total,
        sparkleflinger_display_finalize_successes_total: pipeline_health
            .display_finalize_successes_total,
        sparkleflinger_display_finalize_misses_total: pipeline_health.display_finalize_misses_total,
        sparkleflinger_display_finalize_latches_total: pipeline_health
            .display_finalize_latches_total,
        sparkleflinger_display_finalize_blocking_wait_total_ms: us_to_ms_f64(
            pipeline_health.display_finalize_blocking_wait_total_us,
        ),
        sparkleflinger_display_finalize_blocking_wait_max_ms: us_to_ms_f64(
            pipeline_health.display_finalize_blocking_wait_max_us,
        ),
        sparkleflinger_display_finalize_surface_reallocs_total: pipeline_health
            .display_finalize_surface_reallocs_total,
        servo_render_evaluate_scripts_total_ms: us_to_ms_f64(
            servo_health.render_evaluate_scripts_total_us,
        ),
        servo_render_evaluate_scripts_max_ms: us_to_ms_f64(
            servo_health.render_evaluate_scripts_max_us,
        ),
        servo_render_event_loop_total_ms: us_to_ms_f64(servo_health.render_event_loop_total_us),
        servo_render_event_loop_max_ms: us_to_ms_f64(servo_health.render_event_loop_max_us),
        servo_render_paint_total_ms: us_to_ms_f64(servo_health.render_paint_total_us),
        servo_render_paint_max_ms: us_to_ms_f64(servo_health.render_paint_max_us),
        servo_render_readback_total_ms: us_to_ms_f64(servo_health.render_readback_total_us),
        servo_render_readback_max_ms: us_to_ms_f64(servo_health.render_readback_max_us),
        servo_render_frame_total_ms: us_to_ms_f64(servo_health.render_frame_total_us),
        servo_render_frame_max_ms: us_to_ms_f64(servo_health.render_frame_max_us),
    }
}

pub(super) fn render_acceleration_status(
    resolution: &crate::startup::CompositorAccelerationResolution,
) -> RenderAccelerationStatus {
    RenderAccelerationStatus {
        requested_mode: render_acceleration_mode_name(resolution.requested_mode).to_owned(),
        effective_mode: render_acceleration_mode_name(resolution.effective_mode).to_owned(),
        fallback_reason: resolution.fallback_reason.map(str::to_owned),
        servo_gpu_import_mode: servo_gpu_import_mode_name().to_owned(),
        servo_gpu_import_attempting: servo_gpu_import_attempting(),
        gpu_probe: resolution
            .gpu_probe
            .as_ref()
            .map(|probe| GpuCompositorProbeStatus {
                adapter_name: probe.adapter_name.clone(),
                adapter_device_type: probe.adapter_device_type.to_owned(),
                backend: probe.backend.to_owned(),
                texture_format: probe.texture_format.to_owned(),
                max_texture_dimension_2d: probe.max_texture_dimension_2d,
                max_storage_textures_per_shader_stage: probe.max_storage_textures_per_shader_stage,
                software_adapter_reason: probe.software_adapter_reason.map(str::to_owned),
                servo_gpu_import_backend_compatible: probe.servo_gpu_import_backend_compatible,
                servo_gpu_import_backend_reason: probe
                    .servo_gpu_import_backend_reason
                    .map(str::to_owned),
                linux_servo_gpu_import_backend_compatible: probe
                    .linux_servo_gpu_import_backend_compatible,
                linux_servo_gpu_import_backend_reason: probe
                    .linux_servo_gpu_import_backend_reason
                    .map(str::to_owned),
            }),
    }
}

#[cfg(feature = "servo-gpu-import")]
fn servo_gpu_import_mode_name() -> &'static str {
    match hypercolor_core::effect::servo_gpu_import_mode() {
        hypercolor_types::config::ServoGpuImportMode::Off => "off",
        hypercolor_types::config::ServoGpuImportMode::Auto => "auto",
        hypercolor_types::config::ServoGpuImportMode::On => "on",
    }
}

#[cfg(not(feature = "servo-gpu-import"))]
const fn servo_gpu_import_mode_name() -> &'static str {
    "unavailable"
}

#[cfg(feature = "servo-gpu-import")]
fn servo_gpu_import_attempting() -> bool {
    hypercolor_core::effect::servo_gpu_import_should_attempt()
}

#[cfg(not(feature = "servo-gpu-import"))]
const fn servo_gpu_import_attempting() -> bool {
    false
}

const fn render_acceleration_mode_name(mode: RenderAccelerationMode) -> &'static str {
    match mode {
        RenderAccelerationMode::Cpu => "cpu",
        RenderAccelerationMode::Auto => "auto",
        RenderAccelerationMode::Gpu => "gpu",
    }
}

pub(super) fn latest_frame_status(
    frame: &LatestFrameMetrics,
    render_elapsed_ms: f64,
) -> LatestFrameStatus {
    let frame_age_ms = if frame.timestamp_ms > 0 {
        (render_elapsed_ms - f64::from(frame.timestamp_ms)).max(0.0)
    } else {
        0.0
    };

    LatestFrameStatus {
        frame_token: frame.timeline.frame_token,
        compositor_backend: frame.compositor_backend.as_str().to_owned(),
        output_frame_source: frame.output_frame_source.as_str().to_owned(),
        output_reuses_published_frame: frame.output_reuses_published_frame,
        output_brightness_bits: frame.output_brightness_bits,
        output_brightness_generation: frame.output_brightness_generation,
        output_routing_signature: frame.output_routing_signature,
        output_zone_shape_signature: frame.output_zone_shape_signature,
        output_unassigned_behavior_generation: frame.output_unassigned_behavior_generation,
        devices_written: frame.devices_written,
        total_leds: frame.total_leds,
        gpu_zone_sampling: frame.gpu_zone_sampling,
        gpu_sample_deferred: frame.gpu_sample_deferred,
        gpu_sample_stale: frame.gpu_sample_stale,
        gpu_sample_retry_hit: frame.gpu_sample_retry_hit,
        gpu_sample_queue_saturated: frame.gpu_sample_queue_saturated,
        gpu_sample_wait_blocked: frame.gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback: frame.gpu_sample_cpu_fallback,
        preview_surface: frame.preview_surface,
        scene_canvas_forced_surface: frame.scene_canvas_forced_surface,
        cpu_readback_skipped: frame.cpu_readback_skipped,
        gpu_readback_failed: frame.gpu_readback_failed,
        total_ms: round_2(us_to_ms(frame.total_us)),
        wake_late_ms: round_2(us_to_ms(frame.wake_late_us)),
        jitter_ms: round_2(us_to_ms(frame.jitter_us)),
        frame_age_ms: round_2(frame_age_ms),
        input_sampling_ms: round_2(us_to_ms(frame.input_us)),
        producer_ms: round_2(us_to_ms(frame.producer_us)),
        producer_render_ms: round_2(us_to_ms(frame.producer_render_us)),
        producer_scene_compose_ms: round_2(us_to_ms(frame.producer_scene_compose_us)),
        composition_ms: round_2(us_to_ms(frame.composition_us)),
        effect_rendering_ms: round_2(us_to_ms(frame.render_us)),
        spatial_sampling_ms: round_2(us_to_ms(frame.sample_us)),
        device_output_ms: round_2(us_to_ms(frame.push_us)),
        preview_postprocess_ms: round_2(us_to_ms(frame.postprocess_us)),
        event_bus_ms: round_2(us_to_ms(frame.publish_us)),
        coordination_overhead_ms: round_2(us_to_ms(frame.overhead_us)),
        publish_frame_data_ms: round_2(us_to_ms(frame.publish_frame_data_us)),
        publish_group_canvas_ms: round_2(us_to_ms(frame.publish_group_canvas_us)),
        publish_preview_ms: round_2(us_to_ms(frame.publish_preview_us)),
        publish_events_ms: round_2(us_to_ms(frame.publish_events_us)),
        logical_layer_count: frame.logical_layer_count,
        render_group_count: frame.render_group_count,
        full_frame_copy_count: frame.full_frame_copy_count,
        full_frame_copy_kb: round_2(bytes_to_kib(frame.full_frame_copy_bytes)),
        producer_full_frame_copy_count: frame.producer_full_frame_copy.count,
        producer_full_frame_copy_kb: round_2(bytes_to_kib(frame.producer_full_frame_copy.bytes)),
        producer_full_frame_copy_reason: frame.producer_full_frame_copy.reason.map(str::to_owned),
        publication_full_frame_copy_count: frame.publication_full_frame_copy.count,
        publication_full_frame_copy_kb: round_2(bytes_to_kib(
            frame.publication_full_frame_copy.bytes,
        )),
        publication_full_frame_copy_reason: frame
            .publication_full_frame_copy
            .reason
            .map(str::to_owned),
        output_errors: frame.output_errors,
        render_surfaces: RenderSurfaceStatus {
            canvas_receivers: frame.canvas_receiver_count,
            scene_pool_slot_count: frame.scene_pool_slot_count,
            scene_pool_free_slots: frame.scene_pool_free_slots,
            scene_pool_published_slots: frame.scene_pool_published_slots,
            scene_pool_dequeued_slots: frame.scene_pool_dequeued_slots,
            direct_pool_slot_count: frame.direct_pool_slot_count,
            direct_pool_free_slots: frame.direct_pool_free_slots,
            direct_pool_published_slots: frame.direct_pool_published_slots,
            direct_pool_dequeued_slots: frame.direct_pool_dequeued_slots,
            preview_pool_slot_count: frame.preview_pool_slot_count,
            preview_pool_free_slots: frame.preview_pool_free_slots,
            preview_pool_published_slots: frame.preview_pool_published_slots,
            preview_pool_dequeued_slots: frame.preview_pool_dequeued_slots,
            compositor_pool_slot_count: frame.compositor_pool_slot_count,
            compositor_pool_free_slots: frame.compositor_pool_free_slots,
            compositor_pool_published_slots: frame.compositor_pool_published_slots,
            compositor_pool_dequeued_slots: frame.compositor_pool_dequeued_slots,
        },
    }
}

pub(super) fn preview_runtime_status(runtime: &PreviewRuntime) -> PreviewRuntimeStatus {
    let snapshot = runtime.snapshot();
    let canvas = snapshot.preview(PreviewKind::Canvas);
    let scene_canvas = snapshot.preview(PreviewKind::SceneCanvas);
    let screen_canvas = snapshot.preview(PreviewKind::ScreenCanvas);
    let zone_preview = snapshot.zone_preview;
    PreviewRuntimeStatus {
        canvas_receivers: canvas.receivers,
        scene_canvas_receivers: scene_canvas.receivers,
        screen_canvas_receivers: screen_canvas.receivers,
        zone_preview_receivers: zone_preview.receivers,
        canvas_frames_published: canvas.frames_published,
        scene_canvas_frames_published: scene_canvas.frames_published,
        screen_canvas_frames_published: screen_canvas.frames_published,
        zone_preview_frames_published: zone_preview.frames_published,
        latest_canvas_frame_number: canvas.latest_frame_number,
        latest_scene_canvas_frame_number: scene_canvas.latest_frame_number,
        latest_screen_canvas_frame_number: screen_canvas.latest_frame_number,
        latest_zone_preview_frame_number: zone_preview.latest_frame_number,
        canvas_demand: preview_demand_status(runtime.canvas_demand()),
        scene_canvas_demand: preview_demand_status(runtime.scene_canvas_demand()),
        screen_canvas_demand: preview_demand_status(runtime.screen_canvas_demand()),
        zone_preview_demand: preview_demand_status(runtime.zone_preview_demand()),
    }
}

fn preview_demand_status(summary: PreviewDemandSummary) -> PreviewDemandStatus {
    PreviewDemandStatus {
        subscribers: summary.subscribers,
        max_fps: summary.max_fps,
        max_width: summary.max_width,
        max_height: summary.max_height,
        any_full_resolution: summary.any_full_resolution,
        any_rgb: summary.any_rgb,
        any_rgba: summary.any_rgba,
        any_jpeg: summary.any_jpeg,
    }
}

pub(super) fn paced_fps(avg_frame_secs: f64, target_fps: u32) -> f64 {
    if avg_frame_secs <= 0.0 {
        return f64::from(target_fps);
    }

    (1.0 / avg_frame_secs).clamp(0.0, f64::from(target_fps))
}

fn us_to_ms(value: u32) -> f64 {
    f64::from(value) / 1000.0
}

fn us_to_ms_f64(value: u64) -> f64 {
    std::time::Duration::from_micros(value).as_secs_f64() * 1000.0
}

fn bytes_to_kib(value: u32) -> f64 {
    f64::from(value) / 1024.0
}

pub(super) fn round_1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

pub(super) fn round_2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
