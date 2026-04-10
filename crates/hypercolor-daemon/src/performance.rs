//! Lightweight render-performance tracking for daemon metrics and UI diagnostics.

use std::collections::VecDeque;

const FRAME_HISTORY_CAPACITY: usize = 120;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum CompositorBackendKind {
    #[default]
    Cpu,
    #[cfg_attr(
        not(feature = "wgpu"),
        allow(dead_code, reason = "GPU compositor telemetry is only constructed on wgpu builds")
    )]
    Gpu,
    #[cfg_attr(
        not(feature = "wgpu"),
        allow(dead_code, reason = "GPU compositor telemetry is only constructed on wgpu builds")
    )]
    GpuFallback,
}

impl CompositorBackendKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::GpuFallback => "gpu_fallback",
        }
    }
}

/// Absolute checkpoints for the latest completed frame.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FrameTimeline {
    pub frame_token: u64,
    pub budget_us: u32,
    pub scene_snapshot_done_us: u32,
    pub input_done_us: u32,
    pub producer_done_us: u32,
    pub composition_done_us: u32,
    pub sample_done_us: u32,
    pub output_done_us: u32,
    pub publish_done_us: u32,
    pub frame_done_us: u32,
}

/// Most recent per-frame timings captured from the render thread.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LatestFrameMetrics {
    pub timestamp_ms: u32,
    pub input_us: u32,
    pub producer_us: u32,
    pub composition_us: u32,
    pub render_us: u32,
    pub sample_us: u32,
    pub push_us: u32,
    pub postprocess_us: u32,
    pub publish_us: u32,
    pub overhead_us: u32,
    pub total_us: u32,
    pub wake_late_us: u32,
    pub jitter_us: u32,
    pub reused_inputs: bool,
    pub reused_canvas: bool,
    pub retained_effect: bool,
    pub retained_screen: bool,
    pub composition_bypassed: bool,
    pub compositor_backend: CompositorBackendKind,
    pub logical_layer_count: u32,
    pub render_group_count: u32,
    pub scene_active: bool,
    pub scene_transition_active: bool,
    pub render_surface_slot_count: u32,
    pub render_surface_free_slots: u32,
    pub render_surface_published_slots: u32,
    pub render_surface_dequeued_slots: u32,
    pub canvas_receiver_count: u32,
    pub full_frame_copy_count: u32,
    pub full_frame_copy_bytes: u32,
    pub output_errors: u32,
    pub timeline: FrameTimeline,
}

/// Aggregate frame-time summary over the recent render window.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_field_names,
    reason = "the `_ms` suffix keeps exported latency units explicit across API and WebSocket consumers"
)]
pub(crate) struct FrameTimeSummary {
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

/// Aggregate frame-pacing summary over the recent render window.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PacingSummary {
    pub jitter_avg_ms: f64,
    pub jitter_p95_ms: f64,
    pub jitter_max_ms: f64,
    pub wake_delay_avg_ms: f64,
    pub wake_delay_p95_ms: f64,
    pub wake_delay_max_ms: f64,
    pub reused_inputs: u32,
    pub reused_canvas: u32,
    pub retained_effect: u32,
    pub retained_screen: u32,
    pub composition_bypassed: u32,
}

/// Snapshot exported to API/WebSocket consumers.
#[derive(Debug, Clone, Default)]
pub(crate) struct PerformanceSnapshot {
    pub latest_frame: Option<LatestFrameMetrics>,
    pub frame_time: FrameTimeSummary,
    pub pacing: PacingSummary,
}

/// Rolling performance tracker updated by the render thread.
#[derive(Debug, Default)]
pub struct PerformanceTracker {
    latest_frame: Option<LatestFrameMetrics>,
    frame_times_us: VecDeque<u32>,
    jitter_us: VecDeque<u32>,
    wake_delay_us: VecDeque<u32>,
    reuse_history: VecDeque<FrameReuseSample>,
}

impl PerformanceTracker {
    /// Record one completed frame.
    pub(crate) fn record_frame(&mut self, metrics: LatestFrameMetrics) {
        self.latest_frame = Some(metrics);
        self.frame_times_us.push_back(metrics.total_us);
        self.jitter_us.push_back(metrics.jitter_us);
        self.wake_delay_us.push_back(metrics.wake_late_us);
        self.reuse_history.push_back(FrameReuseSample {
            inputs: metrics.reused_inputs,
            canvas: metrics.reused_canvas,
            retained_effect: metrics.retained_effect,
            retained_screen: metrics.retained_screen,
            composition_bypassed: metrics.composition_bypassed,
        });

        if self.frame_times_us.len() > FRAME_HISTORY_CAPACITY {
            let _ = self.frame_times_us.pop_front();
        }
        if self.jitter_us.len() > FRAME_HISTORY_CAPACITY {
            let _ = self.jitter_us.pop_front();
        }
        if self.wake_delay_us.len() > FRAME_HISTORY_CAPACITY {
            let _ = self.wake_delay_us.pop_front();
        }
        if self.reuse_history.len() > FRAME_HISTORY_CAPACITY {
            let _ = self.reuse_history.pop_front();
        }
    }

    /// Snapshot the latest timings and rolling frame-time summary.
    #[must_use]
    pub(crate) fn snapshot(&self) -> PerformanceSnapshot {
        PerformanceSnapshot {
            latest_frame: self.latest_frame,
            frame_time: summarize_frame_times(&self.frame_times_us),
            pacing: summarize_pacing(&self.jitter_us, &self.wake_delay_us, &self.reuse_history),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(
    clippy::struct_field_names,
    reason = "the `_ms` suffix keeps the internal pacing summary units explicit"
)]
struct ShortSummary {
    avg_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct FrameReuseSample {
    inputs: bool,
    canvas: bool,
    retained_effect: bool,
    retained_screen: bool,
    composition_bypassed: bool,
}

fn summarize_frame_times(samples: &VecDeque<u32>) -> FrameTimeSummary {
    if samples.is_empty() {
        return FrameTimeSummary::default();
    }

    let mut sorted: Vec<u32> = samples.iter().copied().collect();
    sorted.sort_unstable();

    let total_us: u64 = samples.iter().map(|value| u64::from(*value)).sum();
    let sample_count = u64::try_from(samples.len()).unwrap_or(u64::MAX).max(1);
    let average_us = total_us.saturating_add(sample_count / 2) / sample_count;

    FrameTimeSummary {
        avg_ms: micros_to_ms(average_us),
        p95_ms: percentile_ms(&sorted, 95, 100),
        p99_ms: percentile_ms(&sorted, 99, 100),
        max_ms: sorted
            .last()
            .map_or(0.0, |value| micros_to_ms(u64::from(*value))),
    }
}

fn summarize_pacing(
    jitter_us: &VecDeque<u32>,
    wake_delay_us: &VecDeque<u32>,
    reuse_history: &VecDeque<FrameReuseSample>,
) -> PacingSummary {
    let jitter = summarize_short_samples(jitter_us);
    let wake_delay = summarize_short_samples(wake_delay_us);

    PacingSummary {
        jitter_avg_ms: jitter.avg_ms,
        jitter_p95_ms: jitter.p95_ms,
        jitter_max_ms: jitter.max_ms,
        wake_delay_avg_ms: wake_delay.avg_ms,
        wake_delay_p95_ms: wake_delay.p95_ms,
        wake_delay_max_ms: wake_delay.max_ms,
        reused_inputs: u32::try_from(reuse_history.iter().filter(|sample| sample.inputs).count())
            .unwrap_or(u32::MAX),
        reused_canvas: u32::try_from(reuse_history.iter().filter(|sample| sample.canvas).count())
            .unwrap_or(u32::MAX),
        retained_effect: u32::try_from(
            reuse_history
                .iter()
                .filter(|sample| sample.retained_effect)
                .count(),
        )
        .unwrap_or(u32::MAX),
        retained_screen: u32::try_from(
            reuse_history
                .iter()
                .filter(|sample| sample.retained_screen)
                .count(),
        )
        .unwrap_or(u32::MAX),
        composition_bypassed: u32::try_from(
            reuse_history
                .iter()
                .filter(|sample| sample.composition_bypassed)
                .count(),
        )
        .unwrap_or(u32::MAX),
    }
}

fn summarize_short_samples(samples: &VecDeque<u32>) -> ShortSummary {
    if samples.is_empty() {
        return ShortSummary::default();
    }

    let mut sorted: Vec<u32> = samples.iter().copied().collect();
    sorted.sort_unstable();

    let total_us: u64 = samples.iter().map(|value| u64::from(*value)).sum();
    let sample_count = u64::try_from(samples.len()).unwrap_or(u64::MAX).max(1);
    let average_us = total_us.saturating_add(sample_count / 2) / sample_count;

    ShortSummary {
        avg_ms: micros_to_ms(average_us),
        p95_ms: percentile_ms(&sorted, 95, 100),
        max_ms: sorted
            .last()
            .map_or(0.0, |value| micros_to_ms(u64::from(*value))),
    }
}

fn percentile_ms(sorted: &[u32], numerator: usize, denominator: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }

    let rank = sorted
        .len()
        .saturating_mul(numerator)
        .saturating_add(denominator.saturating_sub(1))
        / denominator.max(1);
    let index = rank.saturating_sub(1).min(sorted.len().saturating_sub(1));
    micros_to_ms(u64::from(sorted[index]))
}

fn micros_to_ms(micros: u64) -> f64 {
    let clamped = u32::try_from(micros).unwrap_or(u32::MAX);
    f64::from(clamped) / 1000.0
}
