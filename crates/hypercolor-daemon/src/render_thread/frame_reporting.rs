use hypercolor_core::device::manager::FrameWriteStats;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tracing::{trace, warn};

use crate::performance::LatestFrameMetrics;

const SLOW_FRAME_WARNING_MIN_INTERVAL: Duration = Duration::from_secs(2);
const FRAME_STAGE_SPIKE_US: u32 = 2_000;
const FRAME_PACING_SPIKE_US: u32 = 2_500;

static LAST_SLOW_FRAME_WARNING: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(Mutex::default);

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameCompletionReport {
    pub(crate) frame_interval_us: u32,
    pub(crate) metrics: LatestFrameMetrics,
    pub(crate) devices_written: usize,
    pub(crate) total_leds: usize,
}

impl FrameCompletionReport {
    pub(crate) fn new(
        frame_interval_us: u32,
        metrics: &LatestFrameMetrics,
        write_stats: &FrameWriteStats,
    ) -> Self {
        Self {
            frame_interval_us,
            metrics: *metrics,
            devices_written: write_stats.devices_written,
            total_leds: write_stats.total_leds,
        }
    }
}

pub(crate) fn report_active_frame_completion(
    report: &FrameCompletionReport,
    write_errors: &[String],
) {
    for err in write_errors {
        warn!(error = %err, "device write error");
    }

    let metrics = report.metrics;
    trace!(
        frame = metrics.timeline.frame_token,
        frame_interval_us = report.frame_interval_us,
        wake_late_us = metrics.wake_late_us,
        jitter_us = metrics.jitter_us,
        input_us = metrics.input_us,
        render_us = metrics.render_us,
        producer_us = metrics.producer_us,
        producer_render_us = metrics.producer_render_us,
        producer_scene_compose_us = metrics.producer_scene_compose_us,
        composition_us = metrics.composition_us,
        compositor_backend = metrics.compositor_backend.as_str(),
        output_frame_source = metrics.output_frame_source.as_str(),
        output_reuses_published_frame = metrics.output_reuses_published_frame,
        output_routing_signature = metrics.output_routing_signature,
        output_zone_shape_signature = metrics.output_zone_shape_signature,
        output_unassigned_behavior_generation = metrics.output_unassigned_behavior_generation,
        logical_layers = metrics.logical_layer_count,
        render_groups = metrics.render_group_count,
        scene_active = metrics.scene_active,
        scene_transition_active = metrics.scene_transition_active,
        gpu_sample_stale = metrics.gpu_sample_stale,
        gpu_sample_wait_blocked = metrics.gpu_sample_wait_blocked,
        gpu_sample_queue_saturated = metrics.gpu_sample_queue_saturated,
        gpu_sample_cpu_fallback = metrics.gpu_sample_cpu_fallback,
        cpu_sampling_late_readback = metrics.cpu_sampling_late_readback,
        led_sampling_readback = metrics.led_sampling_readback,
        preview_surface = metrics.preview_surface,
        scene_canvas_forced_surface = metrics.scene_canvas_forced_surface,
        sample_us = metrics.sample_us,
        push_us = metrics.push_us,
        postprocess_us = metrics.postprocess_us,
        publish_us = metrics.publish_us,
        publish_frame_data_us = metrics.publish_frame_data_us,
        publish_group_canvas_us = metrics.publish_group_canvas_us,
        publish_preview_us = metrics.publish_preview_us,
        publish_events_us = metrics.publish_events_us,
        overhead_us = metrics.overhead_us,
        total_us = metrics.total_us,
        reused_inputs = metrics.reused_inputs,
        reused_canvas = metrics.reused_canvas,
        full_frame_copy_count = metrics.full_frame_copy_count,
        full_frame_copy_bytes = metrics.full_frame_copy_bytes,
        producer_full_frame_copy_count = metrics.producer_full_frame_copy.count,
        producer_full_frame_copy_bytes = metrics.producer_full_frame_copy.bytes,
        publication_full_frame_copy_count = metrics.publication_full_frame_copy.count,
        publication_full_frame_copy_bytes = metrics.publication_full_frame_copy.bytes,
        devices = report.devices_written,
        leds = report.total_leds,
        "frame complete"
    );

    let Some(reason) = frame_completion_warning_reason(report) else {
        return;
    };
    if !slow_frame_warning_due(Instant::now()) {
        return;
    }

    warn!(
        reason,
        frame = metrics.timeline.frame_token,
        frame_interval_us = report.frame_interval_us,
        budget_us = metrics.timeline.budget_us,
        wake_late_us = metrics.wake_late_us,
        jitter_us = metrics.jitter_us,
        input_us = metrics.input_us,
        render_us = metrics.render_us,
        producer_us = metrics.producer_us,
        producer_render_us = metrics.producer_render_us,
        producer_scene_compose_us = metrics.producer_scene_compose_us,
        composition_us = metrics.composition_us,
        sample_us = metrics.sample_us,
        push_us = metrics.push_us,
        postprocess_us = metrics.postprocess_us,
        publish_us = metrics.publish_us,
        overhead_us = metrics.overhead_us,
        total_us = metrics.total_us,
        compositor_backend = metrics.compositor_backend.as_str(),
        output_frame_source = metrics.output_frame_source.as_str(),
        output_reuses_published_frame = metrics.output_reuses_published_frame,
        output_brightness_generation = metrics.output_brightness_generation,
        output_routing_signature = metrics.output_routing_signature,
        output_zone_shape_signature = metrics.output_zone_shape_signature,
        output_unassigned_behavior_generation = metrics.output_unassigned_behavior_generation,
        gpu_zone_sampling = metrics.gpu_zone_sampling,
        gpu_sample_deferred = metrics.gpu_sample_deferred,
        gpu_sample_stale = metrics.gpu_sample_stale,
        gpu_sample_retry_hit = metrics.gpu_sample_retry_hit,
        gpu_sample_wait_blocked = metrics.gpu_sample_wait_blocked,
        gpu_sample_queue_saturated = metrics.gpu_sample_queue_saturated,
        gpu_sample_cpu_fallback = metrics.gpu_sample_cpu_fallback,
        cpu_sampling_late_readback = metrics.cpu_sampling_late_readback,
        cpu_readback_skipped = metrics.cpu_readback_skipped,
        gpu_readback_failed = metrics.gpu_readback_failed,
        led_sampling_readback = metrics.led_sampling_readback,
        output_errors = metrics.output_errors,
        devices = report.devices_written,
        leds = report.total_leds,
        "slow LED pipeline frame"
    );
}

fn frame_completion_warning_reason(report: &FrameCompletionReport) -> Option<&'static str> {
    let metrics = report.metrics;
    if report.frame_interval_us > 0 && metrics.total_us > report.frame_interval_us {
        return Some("over_budget");
    }
    if metrics.wake_late_us > FRAME_PACING_SPIKE_US {
        return Some("wake_late");
    }
    if metrics.jitter_us > FRAME_PACING_SPIKE_US {
        return Some("jitter");
    }
    if metrics.sample_us > FRAME_STAGE_SPIKE_US {
        return Some("sampling_spike");
    }
    if metrics.push_us > FRAME_STAGE_SPIKE_US {
        return Some("device_output_spike");
    }
    if metrics.gpu_sample_wait_blocked {
        return Some("gpu_sample_wait_blocked");
    }
    if metrics.gpu_sample_queue_saturated {
        return Some("gpu_sample_queue_saturated");
    }
    if metrics.gpu_sample_cpu_fallback {
        return Some("gpu_sample_cpu_fallback");
    }
    if metrics.gpu_readback_failed {
        return Some("gpu_readback_failed");
    }
    if metrics.cpu_sampling_late_readback {
        return Some("cpu_sampling_late_readback");
    }
    if metrics.led_sampling_readback {
        return Some("led_sampling_readback");
    }
    if metrics.output_errors > 0 {
        return Some("device_output_error");
    }
    None
}

fn slow_frame_warning_due(now: Instant) -> bool {
    let mut last_warning = match LAST_SLOW_FRAME_WARNING.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if last_warning
        .is_some_and(|last| now.saturating_duration_since(last) < SLOW_FRAME_WARNING_MIN_INTERVAL)
    {
        return false;
    }
    *last_warning = Some(now);
    true
}

#[cfg(test)]
mod tests {
    use hypercolor_core::device::manager::FrameWriteStats;

    use super::FrameCompletionReport;
    use super::frame_completion_warning_reason;
    use crate::performance::LatestFrameMetrics;

    #[test]
    fn frame_completion_report_captures_metrics_and_output_counts() {
        let mut metrics = LatestFrameMetrics::default();
        metrics.timeline.frame_token = 42;
        metrics.total_us = 512;
        let write_stats = FrameWriteStats {
            devices_written: 3,
            total_leds: 144,
            errors: vec!["boom".to_owned()],
        };

        let report = FrameCompletionReport::new(16_666, &metrics, &write_stats);

        assert_eq!(report.frame_interval_us, 16_666);
        assert_eq!(report.metrics.timeline.frame_token, 42);
        assert_eq!(report.metrics.total_us, 512);
        assert_eq!(report.devices_written, 3);
        assert_eq!(report.total_leds, 144);
    }

    #[test]
    fn frame_completion_warning_reason_detects_led_pipeline_stalls() {
        let mut metrics = LatestFrameMetrics {
            total_us: 20_000,
            ..LatestFrameMetrics::default()
        };
        let write_stats = FrameWriteStats::default();

        let report = FrameCompletionReport::new(16_666, &metrics, &write_stats);
        assert_eq!(
            frame_completion_warning_reason(&report),
            Some("over_budget")
        );

        metrics.total_us = 1_000;
        metrics.led_sampling_readback = true;
        let report = FrameCompletionReport::new(16_666, &metrics, &write_stats);
        assert_eq!(
            frame_completion_warning_reason(&report),
            Some("led_sampling_readback")
        );
    }

    #[test]
    fn frame_completion_warning_reason_ignores_normal_deferred_gpu_sampling() {
        let mut metrics = LatestFrameMetrics {
            total_us: 1_000,
            gpu_zone_sampling: true,
            gpu_sample_deferred: true,
            gpu_sample_retry_hit: true,
            ..LatestFrameMetrics::default()
        };
        let write_stats = FrameWriteStats::default();
        let report = FrameCompletionReport::new(16_666, &metrics, &write_stats);

        assert_eq!(frame_completion_warning_reason(&report), None);

        metrics.gpu_zone_sampling = false;
        metrics.gpu_sample_retry_hit = false;
        metrics.gpu_sample_stale = true;
        let report = FrameCompletionReport::new(16_666, &metrics, &write_stats);

        assert_eq!(frame_completion_warning_reason(&report), None);
    }
}
