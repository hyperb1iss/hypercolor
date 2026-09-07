use super::{
    CGPoint, CGRect, CGSize, CMTime, CaptureOutputIvars, MACOS_IOSURFACE_ALLOCATION_ALIGNMENT,
    MACOS_IOSURFACE_ROW_ALIGNMENT, MACOS_STREAM_QUEUE_DEPTH, MacosCaptureDynamicRange,
    MacosCaptureError, MacosCapturePixelFormat, MacosColorRange, MacosConfiguredStream,
    MacosPixelExtent, MacosPointRect, MacosProtectedSourceState, MacosScale, MacosStreamPoolQuote,
    MacosStreamPreset, MacosStreamRequest, NSError, NSString, NativeStream, Retained,
    RetainedNativeSample, SCCaptureDynamicRange, SCCaptureResolutionType, SCContentFilter,
    SCStreamConfiguration, SCStreamConfigurationPreset, SCStreamErrorCode, SCStreamErrorDomain,
};

pub(super) fn stream_configuration(
    filter: &SCContentFilter,
    request: MacosStreamRequest,
) -> Result<
    (
        Retained<SCStreamConfiguration>,
        bool,
        MacosPixelExtent,
        MacosConfiguredStream,
    ),
    MacosCaptureError,
> {
    // SAFETY: Picker callbacks supply a live SCContentFilter for the duration
    // of configuration, and returned collection values are retained.
    let (content_rect, point_pixel_scale, display_filter) = unsafe {
        (
            filter.contentRect(),
            f64::from(filter.pointPixelScale()),
            !filter.includedDisplays().is_empty(),
        )
    };
    let point_rect = MacosPointRect::new(
        content_rect.origin.x,
        content_rect.origin.y,
        content_rect.size.width,
        content_rect.size.height,
    )?;
    let scale = MacosScale::display(point_pixel_scale)?;
    let pixel_rect = point_rect.to_pixel_rect(scale)?;
    let extent = MacosPixelExtent::new(pixel_rect.width, pixel_rect.height)?;
    let cadence_timescale = request.cadence.timescale()?;
    // SAFETY: Both constructors use a positive timescale. FramesPerSecond is
    // validated before conversion, while the native-refresh sentinel is zero
    // duration at the canonical unit timescale.
    let minimum_frame_interval = unsafe {
        cadence_timescale.map_or_else(|| CMTime::new(0, 1), |timescale| CMTime::new(1, timescale))
    };
    // SAFETY: The deployment floor includes the HDR preset API. Every setter
    // receives validated point or pixel units, and the caller retains the
    // configuration through stream creation.
    let configuration = unsafe {
        let configuration = match request.preset() {
            MacosStreamPreset::SdrDefault => SCStreamConfiguration::new(),
            MacosStreamPreset::CaptureHdrStreamCanonicalDisplay => {
                SCStreamConfiguration::streamConfigurationWithPreset(
                    SCStreamConfigurationPreset::CaptureHDRStreamCanonicalDisplay,
                )
            }
        };
        configuration.setCapturesAudio(false);
        configuration.setCaptureMicrophone(false);
        configuration.setCaptureResolution(SCCaptureResolutionType::Best);
        configuration.setWidth(extent.width as usize);
        configuration.setHeight(extent.height as usize);
        configuration.setSourceRect(content_rect);
        configuration.setDestinationRect(CGRect::new(
            CGPoint::ZERO,
            CGSize::new(f64::from(extent.width), f64::from(extent.height)),
        ));
        configuration.setPreservesAspectRatio(true);
        configuration.setScalesToFit(false);
        configuration.setMinimumFrameInterval(minimum_frame_interval);
        configuration.setShowsCursor(request.cursor_composed);
        configuration.setShowMouseClicks(false);
        configuration.setStreamName(Some(&NSString::from_str("Hypercolor")));
        configuration.setQueueDepth(MACOS_STREAM_QUEUE_DEPTH as isize);
        if request.dynamic_range == MacosCaptureDynamicRange::Sdr {
            configuration.setCaptureDynamicRange(SCCaptureDynamicRange::SDR);
            configuration.setPixelFormat(0x4247_5241);
        }
        configuration
    };
    // SAFETY: The retained configuration exposes scalar values initialized by
    // its constructor and the setters above.
    let configured_stream = unsafe {
        let pixel_format_fourcc = configuration.pixelFormat();
        MacosConfiguredStream {
            requested_dynamic_range: request.dynamic_range,
            requested_preset: request.preset(),
            configured_dynamic_range: capture_dynamic_range(configuration.captureDynamicRange())?,
            configured_pixel_format: MacosCapturePixelFormat::from_fourcc(pixel_format_fourcc)?,
            configured_color_range: color_range_from_fourcc(pixel_format_fourcc),
        }
    };
    configured_stream.validate()?;
    Ok((configuration, display_filter, extent, configured_stream))
}

pub(super) const fn color_range_from_fourcc(fourcc: u32) -> MacosColorRange {
    match fourcc {
        0x3432_3076 | 0x7834_3434 => MacosColorRange::Video,
        _ => MacosColorRange::Full,
    }
}

pub(super) fn capture_dynamic_range(
    value: SCCaptureDynamicRange,
) -> Result<MacosCaptureDynamicRange, MacosCaptureError> {
    match value {
        SCCaptureDynamicRange::SDR => Ok(MacosCaptureDynamicRange::Sdr),
        SCCaptureDynamicRange::HDRLocalDisplay | SCCaptureDynamicRange::HDRCanonicalDisplay => {
            Ok(MacosCaptureDynamicRange::Hdr)
        }
        _ => Err(MacosCaptureError::UnsupportedConfiguredDynamicRange(
            value.0,
        )),
    }
}

pub(super) fn conservative_pool_quote(
    extent: MacosPixelExtent,
    format: MacosCapturePixelFormat,
) -> Result<MacosStreamPoolQuote, MacosCaptureError> {
    let plane_bytes = match format {
        MacosCapturePixelFormat::Bgra8 | MacosCapturePixelFormat::Argb2101010 => {
            conservative_plane_bytes(extent, 4)?
        }
        MacosCapturePixelFormat::Rgba16Float => conservative_plane_bytes(extent, 8)?,
        MacosCapturePixelFormat::Yuv420VideoRange | MacosCapturePixelFormat::Yuv420FullRange => {
            let chroma = MacosPixelExtent {
                width: extent.width.div_ceil(2),
                height: extent.height.div_ceil(2),
            };
            conservative_plane_bytes(extent, 1)?
                .checked_add(conservative_plane_bytes(chroma, 2)?)
                .ok_or(MacosCaptureError::ArithmeticOverflow)?
        }
        MacosCapturePixelFormat::Yuv44410BiPlanar => conservative_plane_bytes(extent, 2)?
            .checked_add(conservative_plane_bytes(extent, 4)?)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?,
    };
    let per_surface_bytes = align_up(plane_bytes, MACOS_IOSURFACE_ALLOCATION_ALIGNMENT)?;
    let stream_metadata_bytes = [
        std::mem::size_of::<NativeStream>(),
        std::mem::size_of::<CaptureOutputIvars>(),
        std::mem::size_of::<RetainedNativeSample>() * MACOS_STREAM_QUEUE_DEPTH,
    ]
    .into_iter()
    .try_fold(0_u64, |total, bytes| {
        total.checked_add(u64::try_from(bytes).ok()?)
    })
    .ok_or(MacosCaptureError::ArithmeticOverflow)?;
    Ok(MacosStreamPoolQuote {
        per_surface_bytes,
        stream_metadata_bytes,
    })
}

pub(super) fn conservative_plane_bytes(
    extent: MacosPixelExtent,
    bytes_per_pixel: u64,
) -> Result<u64, MacosCaptureError> {
    let row_bytes = u64::from(extent.width)
        .checked_mul(bytes_per_pixel)
        .ok_or(MacosCaptureError::ArithmeticOverflow)?;
    align_up(row_bytes, MACOS_IOSURFACE_ROW_ALIGNMENT)?
        .checked_mul(u64::from(extent.height))
        .ok_or(MacosCaptureError::ArithmeticOverflow)
}

pub(super) fn align_up(value: u64, alignment: u64) -> Result<u64, MacosCaptureError> {
    value
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
        .ok_or(MacosCaptureError::ArithmeticOverflow)
}

pub(super) fn classify_stream_error(error: &NSError) -> MacosProtectedSourceState {
    // SAFETY: ScreenCaptureKit and Foundation expose retained immutable error
    // domain strings for the lifetime of this callback.
    let is_stream_error = error
        .domain()
        .isEqualToString(unsafe { SCStreamErrorDomain });
    if !is_stream_error {
        return MacosProtectedSourceState::Failed;
    }
    match SCStreamErrorCode(error.code()) {
        SCStreamErrorCode::UserDeclined => MacosProtectedSourceState::PermissionDenied,
        SCStreamErrorCode::NoCaptureSource => MacosProtectedSourceState::NeedsSelection,
        SCStreamErrorCode::FailedApplicationConnectionInterrupted
        | SCStreamErrorCode::SystemStoppedStream => MacosProtectedSourceState::Interrupted,
        SCStreamErrorCode::UserStopped => MacosProtectedSourceState::ReadyIdle,
        _ => MacosProtectedSourceState::Failed,
    }
}

pub(super) fn native_error(operation: &'static str, error: &NSError) -> MacosCaptureError {
    MacosCaptureError::NativeOperation {
        operation,
        code: error.code(),
        message: error.localizedDescription().to_string(),
    }
}
