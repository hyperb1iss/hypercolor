use hypercolor_core::input::screen::CapturePixelFormat;
use hypercolor_macos_gpu_interop::MacosNativeTargetFormat;

use super::recovery::native_screen_copy_error_invalidates_frame;
use super::reduction::{UnsupportedMacosNativeTargetFormat, macos_native_target_format};

#[test]
fn copy_errors_invalidate_retained_output() {
    let error = anyhow::anyhow!("injected macOS copy failure");
    assert!(native_screen_copy_error_invalidates_frame(&error));
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
