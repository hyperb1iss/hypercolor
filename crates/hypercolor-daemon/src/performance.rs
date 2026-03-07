//! Lightweight render-performance tracking for daemon metrics and UI diagnostics.

use std::collections::VecDeque;

const FRAME_HISTORY_CAPACITY: usize = 120;

/// Most recent per-frame timings captured from the render thread.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LatestFrameMetrics {
    pub input_us: u32,
    pub render_us: u32,
    pub sample_us: u32,
    pub push_us: u32,
    pub publish_us: u32,
    pub total_us: u32,
    pub output_errors: u32,
}

/// Aggregate frame-time summary over the recent render window.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FrameTimeSummary {
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

/// Snapshot exported to API/WebSocket consumers.
#[derive(Debug, Clone, Default)]
pub(crate) struct PerformanceSnapshot {
    pub latest_frame: Option<LatestFrameMetrics>,
    pub frame_time: FrameTimeSummary,
}

/// Rolling performance tracker updated by the render thread.
#[derive(Debug, Default)]
pub struct PerformanceTracker {
    latest_frame: Option<LatestFrameMetrics>,
    frame_times_us: VecDeque<u32>,
}

impl PerformanceTracker {
    /// Record one completed frame.
    pub(crate) fn record_frame(&mut self, metrics: LatestFrameMetrics) {
        self.latest_frame = Some(metrics);
        self.frame_times_us.push_back(metrics.total_us);

        if self.frame_times_us.len() > FRAME_HISTORY_CAPACITY {
            let _ = self.frame_times_us.pop_front();
        }
    }

    /// Snapshot the latest timings and rolling frame-time summary.
    #[must_use]
    pub(crate) fn snapshot(&self) -> PerformanceSnapshot {
        PerformanceSnapshot {
            latest_frame: self.latest_frame,
            frame_time: summarize_frame_times(&self.frame_times_us),
        }
    }
}

fn summarize_frame_times(samples: &VecDeque<u32>) -> FrameTimeSummary {
    if samples.is_empty() {
        return FrameTimeSummary::default();
    }

    let mut sorted: Vec<u32> = samples.iter().copied().collect();
    sorted.sort_unstable();

    let total_us: u64 = samples.iter().map(|value| u64::from(*value)).sum();
    let len_f64 = samples.len() as f64;

    FrameTimeSummary {
        avg_ms: (total_us as f64 / len_f64) / 1000.0,
        p95_ms: percentile_ms(&sorted, 0.95),
        p99_ms: percentile_ms(&sorted, 0.99),
        max_ms: sorted
            .last()
            .map_or(0.0, |value| f64::from(*value) / 1000.0),
    }
}

fn percentile_ms(sorted: &[u32], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }

    let clamped = percentile.clamp(0.0, 1.0);
    let rank = ((sorted.len() as f64) * clamped).ceil() as usize;
    let index = rank.saturating_sub(1).min(sorted.len().saturating_sub(1));
    f64::from(sorted[index]) / 1000.0
}
