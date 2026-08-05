use hypercolor_ui::pages::dashboard::phase_timeline::phase_frame_from_timeline;
use hypercolor_ui::ws::messages::MetricsTimeline;

/// Milestones for a frame with a 4 ms producer span and a 6 ms sampling span.
/// The daemon bills both hidden stages into the sampling span, because the
/// producer and composition milestones ride the composition stage's clock
/// while `sampling_done_ms` is absolute.
fn base_timeline() -> MetricsTimeline {
    MetricsTimeline {
        scene_snapshot_done_ms: 1.0,
        input_done_ms: 3.0,
        producer_done_ms: 7.0,
        composition_done_ms: 9.0,
        sampling_done_ms: 15.0,
        output_done_ms: 17.0,
        publish_done_ms: 18.0,
        frame_done_ms: 20.0,
        ..MetricsTimeline::default()
    }
}

#[test]
fn hidden_stages_are_carved_out_of_the_sampling_span() {
    let timeline = MetricsTimeline {
        deferred_sample_ms: 1.5,
        preview_advance_ms: 2.5,
        ..base_timeline()
    };

    let phase = phase_frame_from_timeline(&timeline);

    assert_eq!(phase.deferred_sample, 1.5);
    assert_eq!(phase.preview_advance, 2.5);
    assert_eq!(phase.sample, 2.0);
    // Phases the hidden stages never touched keep differencing consecutive
    // milestones, the producer bar included.
    assert_eq!(phase.input, 2.0);
    assert_eq!(phase.producer, 4.0);
    assert_eq!(phase.compose, 2.0);
    assert_eq!(phase.output, 2.0);
    assert_eq!(phase.publish, 1.0);
    assert_eq!(phase.overhead, 2.0);
    // Carving redistributes time, it never invents or drops any.
    assert_eq!(phase.total(), 19.0);
}

#[test]
fn legacy_payload_without_hidden_stages_renders_as_before() {
    let phase = phase_frame_from_timeline(&base_timeline());

    assert_eq!(phase.deferred_sample, 0.0);
    assert_eq!(phase.preview_advance, 0.0);
    assert_eq!(phase.producer, 4.0);
    assert_eq!(phase.sample, 6.0);
    assert_eq!(phase.total(), 19.0);
}

#[test]
fn hidden_stages_never_exceed_the_span_they_came_from() {
    // Payload rounding can report carve-outs marginally wider than their
    // span; the sampler bar floors at zero and the column total stays equal
    // to the frame's wall time.
    let timeline = MetricsTimeline {
        deferred_sample_ms: 5.0,
        preview_advance_ms: 4.0,
        ..base_timeline()
    };

    let phase = phase_frame_from_timeline(&timeline);

    assert_eq!(phase.deferred_sample, 5.0);
    assert_eq!(phase.preview_advance, 1.0);
    assert_eq!(phase.sample, 0.0);
    assert_eq!(phase.total(), 19.0);
}

#[test]
fn negative_and_regressed_values_still_produce_well_formed_bars() {
    let timeline = MetricsTimeline {
        sampling_done_ms: 8.0,
        deferred_sample_ms: -1.0,
        preview_advance_ms: 1.0,
        ..base_timeline()
    };

    let phase = phase_frame_from_timeline(&timeline);

    assert_eq!(phase.deferred_sample, 0.0);
    assert_eq!(phase.preview_advance, 0.0);
    assert_eq!(phase.sample, 0.0);
    assert!(phase.total() >= 0.0);
}
