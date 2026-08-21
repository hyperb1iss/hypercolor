use hypercolor_core::input::screen::{
    CapturePixelFormat, ResolvedScreenColorTransform, ResolvedScreenPublicationDescriptor,
};
use hypercolor_macos_gpu_interop::MacosNativeTargetFormat;

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
#[error("unsupported macOS native reduction target format: {0:?}")]
pub(in crate::render_thread::sparkleflinger::gpu) struct UnsupportedMacosNativeTargetFormat(
    pub(in crate::render_thread::sparkleflinger::gpu) CapturePixelFormat,
);

pub(in crate::render_thread::sparkleflinger::gpu) fn macos_native_target_format(
    format: CapturePixelFormat,
) -> std::result::Result<MacosNativeTargetFormat, UnsupportedMacosNativeTargetFormat> {
    match format {
        CapturePixelFormat::Rgba8 => Ok(MacosNativeTargetFormat::Rgba8),
        CapturePixelFormat::Bgra8 => Ok(MacosNativeTargetFormat::Bgra8),
        unsupported => Err(UnsupportedMacosNativeTargetFormat(unsupported)),
    }
}

pub(super) fn requires_native_work(descriptor: &ResolvedScreenPublicationDescriptor) -> bool {
    let source = descriptor.source();
    descriptor.source_pixel_format() != CapturePixelFormat::Bgra8
        || source.geometry().crop().is_some()
        || descriptor.geometry().output_extent() != source.geometry().storage_extent()
        || descriptor.physical().reduction_extent() != source.geometry().storage_extent()
        || descriptor.physical().target_pixel_format() != descriptor.source_pixel_format()
        || !matches!(
            descriptor.physical().color_pipeline().transform(),
            ResolvedScreenColorTransform::PreserveEncodedSamples
        )
}
