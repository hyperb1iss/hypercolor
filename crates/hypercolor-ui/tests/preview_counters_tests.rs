use hypercolor_ui::ws::{
    PREVIEW_GAP_BUCKET_BOUNDS_MS, PreviewCounterSnapshot, PreviewCounters, PreviewTag,
};

/// Receive one frame and take its tag, as a preview surface does on arrival.
fn arrive(counters: &mut PreviewCounters, frame_number: u32) -> PreviewTag {
    counters.record_received(frame_number);
    counters
        .tag(frame_number)
        .expect("a frame that just arrived has a tag")
}

fn arrive_all(
    counters: &mut PreviewCounters,
    frames: impl IntoIterator<Item = u32>,
) -> Vec<PreviewTag> {
    frames
        .into_iter()
        .map(|frame| arrive(counters, frame))
        .collect()
}

fn accounted(snapshot: &PreviewCounterSnapshot) -> u64 {
    snapshot.displayed + snapshot.dropped + snapshot.unobserved
}

#[test]
fn superseded_frames_count_as_dropped_once_a_newer_frame_shows() {
    let mut counters = PreviewCounters::default();
    let surface = counters.register_surface();
    let tags = arrive_all(&mut counters, [10, 11, 12]);
    assert!(counters.record_displayed(surface, tags[0], 0.0));
    assert!(counters.record_displayed(surface, tags[2], 33.0));

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.received, 3);
    assert_eq!(snapshot.displayed, 2);
    assert_eq!(snapshot.dropped, 1);
    assert_eq!(snapshot.unobserved, 0);
}

#[test]
fn one_surface_measures_while_others_show_the_same_stream() {
    let mut counters = PreviewCounters::default();
    let first = counters.register_surface();
    let second = counters.register_surface();
    let tags = arrive_all(&mut counters, [10, 11]);

    assert!(counters.record_displayed(first, tags[1], 0.0));
    assert!(!counters.record_displayed(second, tags[0], 1.0));
    assert!(!counters.record_displayed(second, tags[1], 2.0));
    assert!(!counters.record_displayed(first, tags[1], 3.0));

    let snapshot = counters.snapshot();
    assert_eq!((snapshot.displayed, snapshot.dropped), (1, 0));
    assert_eq!(
        snapshot.unobserved, 1,
        "frame 10 arrived before anyone measured"
    );
    assert!(snapshot.gap_counts.iter().all(|count| *count == 0));
}

#[test]
fn a_late_completion_of_an_older_frame_changes_nothing() {
    let mut counters = PreviewCounters::default();
    let surface = counters.register_surface();
    let tags = arrive_all(&mut counters, [1, 2, 3]);
    assert!(counters.record_displayed(surface, tags[0], 0.0));
    assert!(counters.record_displayed(surface, tags[2], 40.0));
    assert!(!counters.record_displayed(surface, tags[1], 50.0));

    let snapshot = counters.snapshot();
    assert_eq!((snapshot.displayed, snapshot.dropped), (2, 1));
}

#[test]
fn a_frame_in_flight_through_a_long_stall_still_counts_when_it_lands() {
    let mut counters = PreviewCounters::default();
    let surface = counters.register_surface();
    let first = arrive(&mut counters, 0);
    assert!(counters.record_displayed(surface, first, 0.0));
    let in_flight = arrive(&mut counters, 1);
    let tags = arrive_all(&mut counters, 2..=300);

    assert!(counters.record_displayed(surface, in_flight, 4_000.0));
    assert!(counters.record_displayed(surface, tags[tags.len() - 1], 4_016.0));

    let snapshot = counters.snapshot();
    assert_eq!((snapshot.displayed, snapshot.dropped), (3, 298));
    assert_eq!(
        snapshot.gap_counts[0], 1,
        "the queued frame followed quickly"
    );
    assert_eq!(
        snapshot.gap_counts[PREVIEW_GAP_BUCKET_BOUNDS_MS.len()],
        1,
        "the stall itself"
    );
}

#[test]
fn displayed_gaps_land_in_inclusive_buckets() {
    let mut counters = PreviewCounters::default();
    let surface = counters.register_surface();
    for (frame, at_ms) in [(1, 0.0), (2, 16.0), (3, 50.0), (4, 200.0), (5, 2_200.0)] {
        let tag = arrive(&mut counters, frame);
        assert!(counters.record_displayed(surface, tag, at_ms));
    }

    let snapshot = counters.snapshot();
    assert_eq!(
        snapshot.gap_bounds_ms,
        PREVIEW_GAP_BUCKET_BOUNDS_MS.to_vec()
    );
    assert_eq!(
        snapshot.gap_counts.len(),
        PREVIEW_GAP_BUCKET_BOUNDS_MS.len() + 1
    );
    assert_eq!(snapshot.gap_counts[0], 1, "16 ms fits the 17 ms bucket");
    assert_eq!(snapshot.gap_counts[1], 1, "34 ms fits the 34 ms bucket");
    assert_eq!(snapshot.gap_counts[7], 1, "150 ms fits the 200 ms bucket");
    assert_eq!(
        snapshot.gap_counts[PREVIEW_GAP_BUCKET_BOUNDS_MS.len()],
        1,
        "two seconds overflows"
    );
    assert_eq!(snapshot.gap_counts.iter().sum::<u64>(), 4);
}

#[test]
fn a_new_stream_ignores_completions_from_the_old_one() {
    let mut counters = PreviewCounters::default();
    let surface = counters.register_surface();
    let old = arrive_all(&mut counters, [7, 8, 9]);
    assert!(counters.record_displayed(surface, old[0], 0.0));
    counters.end_stream();
    assert_eq!(
        counters.tag(9),
        None,
        "a new stream starts with no arrivals"
    );

    let renewed = arrive(&mut counters, 9);
    assert_ne!(renewed, old[2]);
    assert!(!counters.record_displayed(surface, old[2], 10.0));
    assert!(counters.record_displayed(surface, renewed, 60_000.0));

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.received, 4);
    assert_eq!((snapshot.displayed, snapshot.dropped), (2, 2));
    assert_eq!(
        snapshot.gap_counts.iter().sum::<u64>(),
        0,
        "a reconnect gap is not an inter-frame gap"
    );
}

#[test]
fn frames_nobody_measures_are_unobserved() {
    let mut counters = PreviewCounters::default();
    let first = counters.register_surface();
    let second = counters.register_surface();
    let early = arrive_all(&mut counters, [1, 2]);
    assert!(counters.record_displayed(first, early[1], 0.0));
    counters.release_surface(first);

    let later = arrive_all(&mut counters, [3, 4, 5]);
    assert!(counters.record_displayed(second, later[2], 100.0));
    arrive(&mut counters, 6);
    counters.end_stream();

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.received, 6);
    assert_eq!(snapshot.displayed, 2);
    assert_eq!(snapshot.unobserved, 3, "frames 1, 3 and 4");
    assert_eq!(snapshot.dropped, 1, "frame 6 was never shown");
    assert_eq!(snapshot.received, accounted(&snapshot));
    assert!(snapshot.gap_counts.iter().all(|count| *count == 0));
}

#[test]
fn snapshot_serializes_the_shape_browser_harnesses_read() {
    let mut counters = PreviewCounters::default();
    let surface = counters.register_surface();
    let tag = arrive(&mut counters, 1);
    assert!(counters.record_displayed(surface, tag, 0.0));
    let json = serde_json::to_value(counters.snapshot()).expect("snapshot serializes");

    assert_eq!(json["received"], 1);
    assert_eq!(json["displayed"], 1);
    assert_eq!(json["dropped"], 0);
    assert_eq!(json["unobserved"], 0);
    assert_eq!(
        json["gap_bounds_ms"].as_array().map(Vec::len),
        Some(PREVIEW_GAP_BUCKET_BOUNDS_MS.len())
    );
    assert_eq!(
        json["gap_counts"].as_array().map(Vec::len),
        Some(PREVIEW_GAP_BUCKET_BOUNDS_MS.len() + 1)
    );
}
