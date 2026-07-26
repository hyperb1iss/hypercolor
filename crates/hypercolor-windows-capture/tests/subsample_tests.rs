//! Subsample geometry, the part of the readback that has to be exactly right.
//!
//! An off-by-one here silently truncates the bottom or right edge of the
//! desktop, which an ambient rig shows as a dead strip of LEDs rather than as
//! an error.

use hypercolor_windows_capture::{subsample_stride, subsampled_extent};

#[test]
fn stride_is_one_when_source_already_fits() {
    assert_eq!(subsample_stride(1280, 1280), 1);
    assert_eq!(subsample_stride(800, 1280), 1);
}

#[test]
fn stride_brings_every_common_resolution_under_target() {
    for (source, target) in [
        (3840, 1280),
        (2560, 1280),
        (1920, 1280),
        (7680, 1280),
        (5120, 1280),
        (3440, 1280),
    ] {
        let stride = subsample_stride(source, target);
        assert!(
            subsampled_extent(source, stride) <= target,
            "{source} at stride {stride} should fit under {target}"
        );
    }
}

#[test]
fn stride_never_divides_by_zero() {
    assert_eq!(subsample_stride(1920, 0), 1);
    assert_eq!(subsample_stride(0, 1280), 1);
}

#[test]
fn extent_rounds_up_so_a_trailing_partial_step_still_samples() {
    // Truncating division would drop the last row of an odd-height desktop.
    assert_eq!(subsampled_extent(1080, 2), 540);
    assert_eq!(subsampled_extent(1081, 2), 541);
    assert_eq!(subsampled_extent(1920, 1), 1920);
}

#[test]
fn a_4k_desktop_lands_on_the_documented_geometry() {
    // The shape the live capture test observes on a 3840x2160 display.
    let stride = subsample_stride(3840, 1280);
    assert_eq!(stride, 3);
    assert_eq!(subsampled_extent(3840, stride), 1280);
    assert_eq!(subsampled_extent(2160, stride), 720);
}
