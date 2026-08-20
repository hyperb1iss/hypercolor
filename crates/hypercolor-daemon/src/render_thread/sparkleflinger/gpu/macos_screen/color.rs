use anyhow::{Context, Result};
use hypercolor_core::input::screen::{
    CaptureTransferFunction, PreparedLedToneMap, ResolvedScreenColorTransform,
    ResolvedScreenPublicationDescriptor, ScreenLetterboxFill,
};
use hypercolor_macos_gpu_interop::{
    MacosNativeColorTransform, MacosNativeLetterboxFill, MacosNativeOutputTransfer,
};

pub(super) fn native_color_transform(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<Option<(MacosNativeOutputTransfer, MacosNativeColorTransform)>> {
    let pipeline = descriptor.physical().color_pipeline();
    if pipeline.transform() == ResolvedScreenColorTransform::PreserveEncodedSamples {
        return Ok(None);
    }
    let source = pipeline
        .effective_source()
        .context("managed macOS native reduction has no effective source colorimetry")?;
    let output = pipeline
        .output()
        .try_known()
        .context("managed macOS native reduction has no known output colorimetry")?;
    let calibration = pipeline
        .calibration()
        .context("managed macOS native reduction has no calibration")?;
    let prepared = PreparedLedToneMap::prepare(source, output, calibration)
        .context("failed to prepare shared macOS native color constants")?;
    let output_transfer = match output.transfer_function() {
        CaptureTransferFunction::Srgb => MacosNativeOutputTransfer::Srgb,
        CaptureTransferFunction::Linear => MacosNativeOutputTransfer::Linear,
        CaptureTransferFunction::Rec709 => MacosNativeOutputTransfer::Rec709,
        CaptureTransferFunction::Rec2020 => MacosNativeOutputTransfer::Rec2020,
        unsupported => {
            anyhow::bail!("unsupported macOS native output transfer function: {unsupported:?}")
        }
    };
    let constants = prepared.constants();
    Ok(Some((
        output_transfer,
        MacosNativeColorTransform::new(
            constants.source_to_target,
            constants.source_luminance_and_exposure,
            constants.curve,
        ),
    )))
}

pub(super) fn native_letterbox_fill(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> Result<MacosNativeLetterboxFill> {
    match descriptor.processing_profile().letterbox_fill() {
        ScreenLetterboxFill::Transparent => Ok(MacosNativeLetterboxFill::Transparent),
        ScreenLetterboxFill::Solid(color) => Ok(MacosNativeLetterboxFill::Solid(
            color.map(|channel| f32::from(channel) / f32::from(u8::MAX)),
        )),
        ScreenLetterboxFill::EdgeExtend => {
            anyhow::bail!("macOS native reduction does not support edge-extended letterbox fill")
        }
    }
}
