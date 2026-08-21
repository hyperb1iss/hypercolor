use std::num::NonZeroU64;

use hypercolor_core::input::screen::CapturePixelFormat;
use hypercolor_core::input::screen::ScreenNativeExecutionTargetId;
use hypercolor_macos_gpu_interop::{MacosGpuInteropError, MacosNativeTargetFormat};

use super::contract::{UnsupportedMacosNativeTargetFormat, macos_native_target_format};
use super::import::{is_transient_copy_failure, validate_target_id};

fn target_id(value: u64) -> ScreenNativeExecutionTargetId {
    ScreenNativeExecutionTargetId::new(NonZeroU64::new(value).expect("target id is non-zero"))
}

#[test]
fn failed_target_generation_is_fenced_after_replacement() {
    let failed = target_id(11);
    let replacement = target_id(12);

    validate_target_id(replacement, replacement).expect("the current target remains valid");
    let error = validate_target_id(failed, replacement)
        .expect_err("a publication prepared for the failed target must be rejected");
    assert!(error.to_string().contains("fenced"));
}

#[test]
fn only_explicit_gpu_fence_pressure_defers_copy() {
    let transient = anyhow::Error::new(MacosGpuInteropError::IosurfaceFenceTimeout)
        .context("native screen import did not complete");
    assert!(is_transient_copy_failure(&transient));
    assert!(!is_transient_copy_failure(&anyhow::anyhow!(
        "structural native screen import failure"
    )));
}
#[test]
fn target_formats_reject_disguised_source_storage() {
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Rgba8)
            .expect("RGBA8 is a native reduction target"),
        MacosNativeTargetFormat::Rgba8,
    );
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Bgra8)
            .expect("BGRA8 is a native reduction target"),
        MacosNativeTargetFormat::Bgra8,
    );
    assert_eq!(
        macos_native_target_format(CapturePixelFormat::Argb2101010)
            .expect_err("source-only storage must not masquerade as a target"),
        UnsupportedMacosNativeTargetFormat(CapturePixelFormat::Argb2101010),
    );
}
