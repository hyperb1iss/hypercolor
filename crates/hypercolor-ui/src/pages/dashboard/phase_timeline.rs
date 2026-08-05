//! Frame-phase attribution for the dashboard's timeline waterfall.
//!
//! Kept leptos-free so the arithmetic is directly unit-testable.

use crate::components::perf_charts::PhaseFrame;
use crate::ws::messages::MetricsTimeline;

/// Split the daemon's cumulative frame milestones into waterfall phase
/// durations.
///
/// Two stages run outside every phase the milestones can express, so the
/// daemon reports each as a standalone duration: the previous frame's
/// deferred zone readback, which runs after input sampling and before
/// composition starts, and the GPU preview submit, which runs after
/// composition and only while a preview consumer is attached. The producer
/// and composition milestones are derived from the composition stage's own
/// clock while `sampling_done_ms` is absolute, so both stages fall inside the
/// `sampling_done_ms - composition_done_ms` difference. Differencing
/// milestones alone therefore draws them as sampler time, which is what made
/// an attached preview look like a sampler spike. Each is carved back out
/// into its own segment at its true position in the stack.
///
/// A daemon that predates those fields reports neither, both default to zero,
/// and every bar collapses to the plain milestone difference it has always
/// been.
pub fn phase_frame_from_timeline(timeline: &MetricsTimeline) -> PhaseFrame {
    // Any given milestone may briefly regress by a hair under load, so clamp
    // to zero to keep the waterfall bars well-formed.
    let diff = |later: f64, earlier: f64| (later - earlier).max(0.0);

    // Payload rounding can leave the carve-outs marginally wider than the
    // span holding them; capping keeps the column total equal to the frame's
    // wall time instead of stretching it.
    let sample_span = diff(timeline.sampling_done_ms, timeline.composition_done_ms);
    let deferred_sample = timeline.deferred_sample_ms.max(0.0).min(sample_span);
    let preview_advance = timeline
        .preview_advance_ms
        .max(0.0)
        .min(sample_span - deferred_sample);
    let sample = sample_span - deferred_sample - preview_advance;

    PhaseFrame {
        input: diff(timeline.input_done_ms, timeline.scene_snapshot_done_ms) as f32,
        deferred_sample: deferred_sample as f32,
        producer: diff(timeline.producer_done_ms, timeline.input_done_ms) as f32,
        compose: diff(timeline.composition_done_ms, timeline.producer_done_ms) as f32,
        preview_advance: preview_advance as f32,
        sample: sample as f32,
        output: diff(timeline.output_done_ms, timeline.sampling_done_ms) as f32,
        publish: diff(timeline.publish_done_ms, timeline.output_done_ms) as f32,
        overhead: diff(timeline.frame_done_ms, timeline.publish_done_ms) as f32,
    }
}
