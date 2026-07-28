use std::time::{Duration, Instant};

use super::{
    GpuReducer, GpuReductionError, InjectedPollFailure, PointerState, SubmitOutcome, admit_vec_len,
    checked_rgba_len, checked_rgba_row_pitch, synthetic_metadata, test_device, test_source,
};
use crate::{CaptureRegion, DisplayRotation};

#[test]
fn wide_and_portrait_8k_extents_admit_exact_bytes() {
    let wide = checked_rgba_len(7_680, 2_160, "test wide 8K").expect("wide 8K fits");
    let portrait = checked_rgba_len(2_160, 7_680, "test portrait 8K").expect("portrait 8K fits");

    assert_eq!(wide, 7_680 * 2_160 * 4);
    assert_eq!(portrait, 2_160 * 7_680 * 4);
    assert_eq!(wide, portrait);
}

#[test]
fn extreme_rgba_geometry_reports_checked_overflow() {
    assert!(matches!(
        checked_rgba_len(u32::MAX, u32::MAX, "test overflow"),
        Err(GpuReductionError::SizeOverflow { .. })
    ));
    assert!(matches!(
        checked_rgba_row_pitch(u32::MAX, 1, "test pitch overflow"),
        Err(GpuReductionError::SizeOverflow { .. })
    ));
}

#[test]
fn failed_growth_preserves_last_good_buffer() {
    let mut buffer = vec![11, 22, 33, 44];
    let previous = buffer.clone();
    let previous_capacity = buffer.capacity();

    assert!(matches!(
        admit_vec_len(&mut buffer, usize::MAX, "test failed growth"),
        Err(GpuReductionError::ResourceExhausted { .. })
    ));
    assert_eq!(buffer, previous);
    assert_eq!(buffer.capacity(), previous_capacity);
}

#[test]
fn failed_readback_growth_preserves_pending_frame_and_output() {
    let (device, context) = test_device().expect("WARP device is available");
    let source = test_source(&device, &[10, 20, 30, 0xFF], 1, 1).expect("source texture");
    let pointer = PointerState::default();
    let mut reducer = GpuReducer::new(&device, &context).expect("GPU reducer");
    let outcome = reducer
        .submit(
            Some(&source),
            1,
            synthetic_metadata(
                1,
                1,
                &pointer,
                DisplayRotation::Identity,
                CaptureRegion::full(1, 1),
                73,
            ),
        )
        .expect("submission succeeds");
    assert!(matches!(outcome, SubmitOutcome::Submitted));

    let mut output = vec![9, 8, 7, 6];
    let previous = output.clone();
    reducer.poll_failure = Some(InjectedPollFailure::Allocation);
    let deadline = Instant::now() + Duration::from_secs(2);
    let error = loop {
        match reducer.poll(&mut output) {
            Err(error) => break error,
            Ok(None) if Instant::now() < deadline => std::thread::yield_now(),
            Ok(None) => panic!("readback query did not complete"),
            Ok(Some(_)) => panic!("injected allocation failure published a frame"),
        }
    };

    assert!(matches!(error, GpuReductionError::ResourceExhausted { .. }));
    assert_eq!(output, previous);
    let resources = reducer
        .resources
        .as_ref()
        .expect("resources remain admitted");
    assert_eq!(
        resources.slots[resources.read_index]
            .pending
            .as_ref()
            .expect("pending frame remains queued")
            .metadata
            .sequence,
        73
    );
}
