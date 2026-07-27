use std::mem::size_of;
use std::time::Duration;

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, WAIT_FAILED,
};

use super::{
    BufferReadDisposition, INITIAL_BUFFER_QWORDS, MAX_BUFFER_QWORDS, MAX_RESIZE_BACKOFF_MS,
    MAX_RESIZE_RETRIES, PumpError, SourceKeyCanonicalizers, classify_buffer_read,
    classify_wait_result, initial_topology, next_buffer_capacity,
};
use crate::RawKeyPrefix;
use crate::decode::CanonicalKeyReport;

#[test]
fn access_denied_is_a_terminal_buffer_failure() {
    assert_eq!(
        classify_buffer_read(u32::MAX, ERROR_ACCESS_DENIED.0),
        BufferReadDisposition::Failed {
            error_code: ERROR_ACCESS_DENIED.0,
        }
    );
}

#[test]
fn invalid_handle_is_a_terminal_buffer_failure() {
    assert_eq!(
        classify_buffer_read(u32::MAX, ERROR_INVALID_HANDLE.0),
        BufferReadDisposition::Failed {
            error_code: ERROR_INVALID_HANDLE.0,
        }
    );
}

#[test]
fn only_insufficient_buffer_is_classified_as_resize() {
    assert_eq!(
        classify_buffer_read(u32::MAX, ERROR_INSUFFICIENT_BUFFER.0),
        BufferReadDisposition::Resize
    );
    assert_eq!(
        classify_buffer_read(17, ERROR_ACCESS_DENIED.0),
        BufferReadDisposition::Read(17),
        "a successful call must ignore stale last-error state"
    );
}

#[test]
fn wait_failed_preserves_the_native_error_code() {
    assert_eq!(
        classify_wait_result(WAIT_FAILED.0, ERROR_INVALID_HANDLE.0),
        Err(PumpError::WaitFailed(ERROR_INVALID_HANDLE.0))
    );
    assert_eq!(classify_wait_result(0, ERROR_ACCESS_DENIED.0), Ok(()));
}

#[test]
fn keyboard_only_capture_never_requires_monitor_topology() {
    let result = initial_topology(false, || panic!("keyboard-only path enumerated monitors"));

    assert_eq!(result, Ok(None));
}

#[test]
fn documented_required_size_that_already_fits_does_not_grow() {
    let required_bytes =
        u32::try_from(INITIAL_BUFFER_QWORDS * size_of::<u64>()).expect("test capacity fits u32");

    assert_eq!(
        next_buffer_capacity(INITIAL_BUFFER_QWORDS, required_bytes, 1),
        Ok((INITIAL_BUFFER_QWORDS, Duration::from_millis(1)))
    );
}

#[test]
fn resize_races_grow_geometrically_with_bounded_backoff() {
    let mut qwords = INITIAL_BUFFER_QWORDS;
    let mut backoffs = Vec::new();
    for attempt in 0..MAX_RESIZE_RETRIES {
        let required_qwords = (qwords + 1).min(MAX_BUFFER_QWORDS);
        let required_bytes =
            u32::try_from(required_qwords * size_of::<u64>()).expect("test capacity fits u32");
        let (next, backoff) = next_buffer_capacity(qwords, required_bytes, attempt)
            .expect("transient resize stays within the ceiling");
        assert!(next >= qwords);
        qwords = next;
        backoffs.push(backoff);
    }

    assert_eq!(qwords, MAX_BUFFER_QWORDS);
    assert_eq!(backoffs[0], Duration::ZERO);
    assert!(backoffs.windows(2).all(|pair| pair[0] <= pair[1]));
    assert_eq!(
        backoffs.last().copied(),
        Some(Duration::from_millis(MAX_RESIZE_BACKOFF_MS))
    );
    let required_bytes =
        u32::try_from(MAX_BUFFER_QWORDS * size_of::<u64>()).expect("test ceiling fits u32");
    assert_eq!(
        next_buffer_capacity(MAX_BUFFER_QWORDS, required_bytes, MAX_RESIZE_RETRIES),
        Err(PumpError::BufferResizeExhausted {
            attempts: MAX_RESIZE_RETRIES + 1,
            required_bytes,
        })
    );
}

#[test]
fn resize_beyond_the_safety_ceiling_is_terminal() {
    let ceiling_bytes = MAX_BUFFER_QWORDS * size_of::<u64>();
    let required_bytes = u32::try_from(ceiling_bytes + 1).expect("test ceiling fits u32");

    assert_eq!(
        next_buffer_capacity(MAX_BUFFER_QWORDS / 2, required_bytes, 0),
        Err(PumpError::BufferCapacityExceeded {
            required_bytes,
            capacity_bytes: ceiling_bytes,
        })
    );
}

#[test]
fn interleaved_keyboards_keep_composite_state_per_source() {
    let keyboard_a: std::sync::Arc<str> = "keyboard:a".into();
    let keyboard_b: std::sync::Arc<str> = "keyboard:b".into();
    let mut canonicalizers = SourceKeyCanonicalizers::default();

    assert_eq!(
        canonicalizers.canonicalize(&keyboard_a, 0x1D, 0x04, 0x11),
        CanonicalKeyReport::Pending
    );
    assert_eq!(
        canonicalizers.canonicalize(&keyboard_b, 0x1E, 0, 0x41),
        CanonicalKeyReport::Edge {
            make_code: 0x1E,
            prefix: RawKeyPrefix::None,
            vkey: 0x41,
            pressed: true,
        }
    );
    assert_eq!(
        canonicalizers.canonicalize(&keyboard_a, 0x45, 0, 0x13),
        CanonicalKeyReport::Edge {
            make_code: 0x45,
            prefix: RawKeyPrefix::E1,
            vkey: 0x13,
            pressed: true,
        }
    );
}

#[test]
fn source_reset_discards_an_incomplete_composite_sequence() {
    let keyboard: std::sync::Arc<str> = "keyboard:a".into();
    let mut canonicalizers = SourceKeyCanonicalizers::default();

    assert_eq!(
        canonicalizers.canonicalize(&keyboard, 0x1D, 0x04, 0x11),
        CanonicalKeyReport::Pending
    );
    canonicalizers.reset(&keyboard);
    assert_eq!(
        canonicalizers.canonicalize(&keyboard, 0x1E, 0, 0x41),
        CanonicalKeyReport::Edge {
            make_code: 0x1E,
            prefix: RawKeyPrefix::None,
            vkey: 0x41,
            pressed: true,
        }
    );
}
