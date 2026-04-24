use hypercolor_core::device::manager::FrameWriteStats;
use tracing::{trace, warn};

use crate::performance::LatestFrameMetrics;

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
        metrics: LatestFrameMetrics,
        write_stats: &FrameWriteStats,
    ) -> Self {
        Self {
            frame_interval_us,
            metrics,
            devices_written: write_stats.devices_written,
            total_leds: write_stats.total_leds,
        }
    }
}

pub(crate) fn report_active_frame_completion(
    report: FrameCompletionReport,
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
        logical_layers = metrics.logical_layer_count,
        render_groups = metrics.render_group_count,
        scene_active = metrics.scene_active,
        scene_transition_active = metrics.scene_transition_active,
        gpu_sample_stale = metrics.gpu_sample_stale,
        gpu_sample_wait_blocked = metrics.gpu_sample_wait_blocked,
        gpu_sample_queue_saturated = metrics.gpu_sample_queue_saturated,
        gpu_sample_cpu_fallback = metrics.gpu_sample_cpu_fallback,
        cpu_sampling_late_readback = metrics.cpu_sampling_late_readback,
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
        devices = report.devices_written,
        leds = report.total_leds,
        "frame complete"
    );
}

#[cfg(test)]
mod tests {
    use hypercolor_core::device::manager::FrameWriteStats;

    use super::FrameCompletionReport;
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

        let report = FrameCompletionReport::new(16_666, metrics, &write_stats);

        assert_eq!(report.frame_interval_us, 16_666);
        assert_eq!(report.metrics.timeline.frame_token, 42);
        assert_eq!(report.metrics.total_us, 512);
        assert_eq!(report.devices_written, 3);
        assert_eq!(report.total_leds, 144);
    }
}
