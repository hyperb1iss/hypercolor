//! Live Desktop Duplication smoke tests.
//!
//! These drive the real API against whatever display the test host has. They
//! stay outside the default suite because a hosted, headless, or RDP session
//! has no reliable duplicatable output. Once deliberately enabled and a
//! duplicator opens, API errors are test failures. Attached outputs also have
//! to open independently and report geometry consistent with their pending
//! rotation.
//!
//! Run with `cargo test --locked -p hypercolor-windows-capture --test
//! duplication_tests -- --ignored --test-threads=1` from an interactive local
//! Windows console with attached DXGI outputs, changing desktop content, and no
//! competing Desktop Duplication client.

#![cfg(target_os = "windows")]

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use hypercolor_windows_capture::{
    CaptureError, CaptureExtent, CaptureResult, DesktopDuplicator, DisplayRotation, list_monitors,
    monitor_count,
};

/// Subsample target used across the tests, matching the daemon's default.
const MAX_WIDTH: u32 = 1280;
const MAX_HEIGHT: u32 = 720;

fn requested_extent() -> CaptureExtent {
    CaptureExtent::try_new(MAX_WIDTH, MAX_HEIGHT).expect("test extent is non-empty")
}

fn capture_test_lock() -> MutexGuard<'static, ()> {
    static CAPTURE_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    CAPTURE_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Open a duplicator, or return `None` when this host cannot capture.
fn duplicator_or_skip(monitor: usize) -> Option<DesktopDuplicator> {
    if monitor_count() == 0 {
        eprintln!("skipping: no display outputs attached");
        return None;
    }

    match DesktopDuplicator::new(monitor, requested_extent()) {
        Ok(duplicator) => Some(duplicator),
        Err(CaptureError::AlreadyDuplicating) => {
            eprintln!("skipping monitor {monitor}: another process holds the duplication slot");
            None
        }
        Err(CaptureError::AccessDenied | CaptureError::SessionUnavailable) => {
            eprintln!("skipping monitor {monitor}: interactive desktop is unavailable");
            None
        }
        Err(error) => panic!("opening attached monitor {monitor} failed: {error}"),
    }
}

/// Pump until a frame arrives or the budget expires. A static desktop
/// legitimately produces nothing, so absence is not a failure.
fn wait_for_frame(
    duplicator: &mut DesktopDuplicator,
    budget: Duration,
) -> CaptureResult<Option<(u32, u32, usize)>> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        match duplicator.next_frame(Duration::from_millis(120)) {
            Ok(Some(frame)) => return Ok(Some((frame.width, frame.height, frame.rgba().len()))),
            Ok(None) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

const fn expected_native_extent(
    logical_width: u32,
    logical_height: u32,
    rotation: DisplayRotation,
) -> (u32, u32) {
    match rotation {
        DisplayRotation::Identity | DisplayRotation::Clockwise180 => {
            (logical_width, logical_height)
        }
        DisplayRotation::Clockwise90 | DisplayRotation::Clockwise270 => {
            (logical_height, logical_width)
        }
    }
}

#[test]
#[ignore = "requires Windows with D3D11 and an attached DXGI display"]
fn monitor_count_is_sane() {
    // Purely a smoke check that enumeration does not panic or explode; a
    // headless agent host legitimately reports zero.
    let count = monitor_count();
    assert!(count < 64, "implausible monitor count: {count}");
}

#[test]
#[ignore = "requires Windows with D3D11 display enumeration"]
fn opening_a_missing_monitor_reports_not_found() {
    let Err(error) = DesktopDuplicator::new(9_999, requested_extent()) else {
        panic!("monitor index 9999 should not resolve to a real output");
    };

    match error {
        CaptureError::MonitorNotFound { requested, .. } => assert_eq!(requested, 9_999),
        // A host with no D3D11 at all fails earlier, which is still correct.
        other => eprintln!("no monitors to enumerate against ({other})"),
    }
}

#[test]
#[ignore = "requires an interactive Windows desktop with changing DXGI output"]
fn captures_a_frame_with_consistent_geometry() {
    let _capture_guard = capture_test_lock();
    let Some(mut duplicator) = duplicator_or_skip(0) else {
        return;
    };

    let Some((width, height, len)) = wait_for_frame(&mut duplicator, Duration::from_secs(3))
        .expect("capture must remain healthy after a successful open")
    else {
        eprintln!("skipping assertions: desktop produced no frame within the budget");
        return;
    };

    assert!(width > 0 && height > 0, "frame must have real extent");
    assert!(
        width <= MAX_WIDTH,
        "subsampling should hold width at or under {MAX_WIDTH}, got {width}"
    );
    assert!(
        height <= MAX_HEIGHT,
        "subsampling should hold height at or under {MAX_HEIGHT}, got {height}"
    );
    assert_eq!(
        len,
        width as usize * height as usize * 4,
        "buffer must be tightly packed RGBA"
    );

    let (native_width, native_height) = duplicator.native_extent();
    assert!(
        native_width >= width && native_height >= height,
        "subsampled frame must not exceed the native desktop"
    );
    eprintln!("captured {width}x{height} from a {native_width}x{native_height} desktop");
}

#[test]
#[ignore = "requires an interactive Windows desktop with repeated DXGI frames"]
fn alpha_is_opaque_across_repeated_acquisitions() {
    let _capture_guard = capture_test_lock();
    let Some(mut duplicator) = duplicator_or_skip(0) else {
        return;
    };

    // Repeated acquisitions exercise the release-before-acquire contract:
    // DXGI refuses a second acquire while the first frame is still held, so a
    // missing ReleaseFrame would surface here rather than in production.
    let mut checked = 0_u32;
    let deadline = Instant::now() + Duration::from_secs(5);
    while checked < 2 && Instant::now() < deadline {
        match duplicator.next_frame(Duration::from_millis(150)) {
            Ok(Some(frame)) => {
                assert!(
                    frame.rgba().chunks_exact(4).all(|pixel| pixel[3] == 0xFF),
                    "every captured pixel must be fully opaque"
                );
                checked += 1;
            }
            Ok(None) => {}
            Err(error) => panic!("repeated acquisition failed: {error}"),
        }
    }

    if checked < 2 {
        eprintln!("desktop was static; only {checked} frame(s) verified");
    }
}

#[test]
#[ignore = "requires an interactive Windows desktop with repeated DXGI frames"]
fn repeated_frames_reuse_the_owned_plane_allocation() {
    let _capture_guard = capture_test_lock();
    let Some(mut duplicator) = duplicator_or_skip(0) else {
        return;
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut first_pointer = None;
    while Instant::now() < deadline {
        match duplicator.next_frame(Duration::from_millis(150)) {
            Ok(Some(frame)) => {
                let pointer = frame.rgba().as_ptr();
                assert_eq!(frame.native_width, duplicator.native_extent().0);
                assert_eq!(frame.native_height, duplicator.native_extent().1);
                drop(frame);
                if let Some(first_pointer) = first_pointer {
                    assert_eq!(
                        pointer, first_pointer,
                        "the released frame plane should be reused at capture cadence"
                    );
                    return;
                }
                first_pointer = Some(pointer);
            }
            Ok(None) => {}
            Err(error) => panic!("repeated acquisition failed: {error}"),
        }
    }

    eprintln!("desktop was static; allocation reuse needed two frames to verify");
}

#[test]
#[ignore = "requires Windows with at least one attached DXGI display"]
fn monitor_listing_matches_the_count_and_carries_real_geometry() {
    let monitors = list_monitors();
    assert_eq!(
        monitors.len(),
        monitor_count(),
        "listing and count must agree"
    );

    if monitors.is_empty() {
        eprintln!("skipping: no display outputs attached");
        return;
    }

    for monitor in &monitors {
        assert!(
            monitor.width > 0 && monitor.height > 0,
            "monitor {} ({}) reported a zero extent",
            monitor.index,
            monitor.name
        );
        assert!(!monitor.name.is_empty(), "monitor names must not be empty");
        assert!(
            monitor.id.starts_with("display:"),
            "monitor {} lacks a persistent device identity: {}",
            monitor.index,
            monitor.id
        );
        assert!(
            monitor.topology_generation > 0,
            "monitor descriptors must carry a topology generation"
        );
    }
    assert!(
        monitors.iter().any(|monitor| monitor.primary),
        "one output should anchor the virtual desktop origin"
    );
    eprintln!("monitors: {monitors:?}");
}

#[test]
#[ignore = "requires an interactive Windows desktop with all DXGI outputs available"]
fn every_attached_monitor_opens_with_rotation_consistent_geometry() {
    let _capture_guard = capture_test_lock();
    let monitors = list_monitors();
    for monitor in monitors {
        let Some(duplicator) = duplicator_or_skip(monitor.index) else {
            continue;
        };

        assert_eq!(
            duplicator.logical_extent(),
            (monitor.width, monitor.height),
            "monitor {} logical mode must match its desktop bounds",
            monitor.index
        );
        assert_eq!(
            duplicator.native_extent(),
            expected_native_extent(monitor.width, monitor.height, monitor.rotation),
            "monitor {} native scanout must account for pending {:?} rotation",
            monitor.index,
            monitor.rotation
        );
        drop(duplicator);
    }
}

#[test]
#[ignore = "requires Windows with at least one attached DXGI display"]
fn opened_capture_exposes_adapter_identity() {
    let _capture_guard = capture_test_lock();
    let Some(duplicator) = duplicator_or_skip(0) else {
        return;
    };

    let adapter = duplicator.adapter_luid();
    assert_ne!(
        (adapter.low_part(), adapter.high_part()),
        (0, 0),
        "capture should expose a concrete DXGI adapter LUID",
    );
}
