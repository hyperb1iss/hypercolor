//! Live Desktop Duplication smoke tests.
//!
//! These drive the real API against whatever display the test host has. CI is
//! Linux-only, and a headless or RDP session has no duplicatable output, so
//! every test degrades to a skip rather than a failure when capture is
//! unavailable. When a display *is* present they assert real pixel geometry,
//! which is the part that catches a broken readback.

#![cfg(target_os = "windows")]

use std::time::{Duration, Instant};

use hypercolor_windows_capture::{CaptureError, DesktopDuplicator, monitor_count};

/// Subsample target used across the tests, matching the daemon's default.
const MAX_WIDTH: u32 = 1280;

/// Open a duplicator, or return `None` when this host cannot capture.
fn duplicator_or_skip() -> Option<DesktopDuplicator> {
    if monitor_count() == 0 {
        eprintln!("skipping: no display outputs attached");
        return None;
    }

    match DesktopDuplicator::new(0, MAX_WIDTH) {
        Ok(duplicator) => Some(duplicator),
        Err(CaptureError::AlreadyDuplicating) => {
            eprintln!("skipping: another process holds the duplication interface");
            None
        }
        Err(error) => {
            eprintln!("skipping: duplication unavailable ({error})");
            None
        }
    }
}

/// Pump until a frame arrives or the budget expires. A static desktop
/// legitimately produces nothing, so absence is not a failure.
fn wait_for_frame(
    duplicator: &mut DesktopDuplicator,
    budget: Duration,
) -> Option<(u32, u32, usize)> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        match duplicator.next_frame(Duration::from_millis(120)) {
            Ok(Some(frame)) => return Some((frame.width, frame.height, frame.rgba.len())),
            Ok(None) => {}
            Err(error) => {
                eprintln!("capture error while waiting for a frame: {error}");
                return None;
            }
        }
    }
    None
}

#[test]
fn monitor_count_is_sane() {
    // Purely a smoke check that enumeration does not panic or explode; a
    // headless agent host legitimately reports zero.
    let count = monitor_count();
    assert!(count < 64, "implausible monitor count: {count}");
}

#[test]
fn opening_a_missing_monitor_reports_not_found() {
    let Err(error) = DesktopDuplicator::new(9_999, MAX_WIDTH) else {
        panic!("monitor index 9999 should not resolve to a real output");
    };

    match error {
        CaptureError::MonitorNotFound { requested, .. } => assert_eq!(requested, 9_999),
        // A host with no D3D11 at all fails earlier, which is still correct.
        other => eprintln!("no monitors to enumerate against ({other})"),
    }
}

#[test]
fn captures_a_frame_with_consistent_geometry() {
    let Some(mut duplicator) = duplicator_or_skip() else {
        return;
    };

    let Some((width, height, len)) = wait_for_frame(&mut duplicator, Duration::from_secs(3)) else {
        eprintln!("skipping assertions: desktop produced no frame within the budget");
        return;
    };

    assert!(width > 0 && height > 0, "frame must have real extent");
    assert!(
        width <= MAX_WIDTH,
        "subsampling should hold width at or under {MAX_WIDTH}, got {width}"
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
fn alpha_is_opaque_across_repeated_acquisitions() {
    let Some(mut duplicator) = duplicator_or_skip() else {
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
                    frame.rgba.chunks_exact(4).all(|pixel| pixel[3] == 0xFF),
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
fn monitor_listing_matches_the_count_and_carries_real_geometry() {
    use hypercolor_windows_capture::list_monitors;

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
    }
    assert!(
        monitors.iter().any(|monitor| monitor.primary),
        "one output should anchor the virtual desktop origin"
    );
    eprintln!("monitors: {monitors:?}");
}
