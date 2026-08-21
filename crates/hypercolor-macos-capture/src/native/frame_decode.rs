use super::{
    AnyObject, CFArray, CFDictionary, CFGetTypeID, CFNumber, CFRetained, CFString, CFType, CGRect,
    CGRectMakeWithDictionaryRepresentation, CMSampleBuffer, CVBuffer, CVPixelBuffer,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetDataSize,
    CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth,
    CVPixelBufferGetWidthOfPlane, DecodedSample, MacosAttachment, MacosCaptureColorimetry,
    MacosCaptureError, MacosCapturePixelFormat, MacosCaptureSurface, MacosChromaLocation,
    MacosColorPrimaries, MacosColorRange, MacosDeliveredFrameMetadata, MacosFrameDecoder,
    MacosFrameEvent, MacosPixelExtent, MacosPixelRect, MacosPointRect, MacosRawCapturePlane,
    MacosRawCaptureSample, MacosRawCompleteFrame, MacosRawFrameAttachments,
    MacosStreamDeliveryRejection, MacosStreamDeliveryState, MacosStreamDeliveryValidator,
    MacosTransferFunction, MacosYuvMatrix, NSString, NSValue, PoolBackingLifetime,
    RetainedNativeSample, SCStreamFrameInfoBoundingRect, SCStreamFrameInfoContentRect,
    SCStreamFrameInfoContentScale, SCStreamFrameInfoDirtyRects, SCStreamFrameInfoDisplayTime,
    SCStreamFrameInfoScaleFactor, SCStreamFrameInfoScreenRect, SCStreamFrameInfoStatus, c_void,
    kCVImageBufferChromaLocation_Center, kCVImageBufferChromaLocation_Left,
    kCVImageBufferChromaLocation_TopLeft, kCVImageBufferChromaLocationTopFieldKey,
    kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferColorPrimaries_ITU_R_2020,
    kCVImageBufferColorPrimaries_P3_D65, kCVImageBufferColorPrimariesKey,
    kCVImageBufferContentLightLevelInfoKey, kCVImageBufferTransferFunction_ITU_R_709_2,
    kCVImageBufferTransferFunction_ITU_R_2020, kCVImageBufferTransferFunction_ITU_R_2100_HLG,
    kCVImageBufferTransferFunction_Linear, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ,
    kCVImageBufferTransferFunction_sRGB, kCVImageBufferTransferFunctionKey,
    kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
    kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrixKey, kIOSurfaceContentHeadroom,
    ptr,
};

pub(super) fn decode_sample(
    decoder: &mut MacosFrameDecoder,
    delivery_validator: &mut MacosStreamDeliveryValidator,
    sample: RetainedNativeSample,
) -> Result<DecodedSample, MacosCaptureError> {
    let awaiting_first_delivery = matches!(
        delivery_validator.state(),
        MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_)
    );
    let frame = decode_complete_frame(
        sample.pixel_buffer,
        Some(sample.admission_lifetime),
        sample.cursor_composed,
    )
    .map_err(|error| classify_delivery_error(delivery_validator, error))?;
    let event = decoder
        .decode(MacosRawCaptureSample {
            frame: Some(frame),
            attachments: sample.attachments,
        })
        .map_err(|error| classify_delivery_error(delivery_validator, error))?;
    let confirmed_delivery = if awaiting_first_delivery {
        let MacosFrameEvent::Frame(frame) = &event else {
            return Err(classify_delivery_error(
                delivery_validator,
                MacosCaptureError::MissingFramePayload,
            ));
        };
        Some(
            delivery_validator
                .observe_first_complete(frame.surface.delivery_metadata())
                .map_err(MacosCaptureError::StreamDeliveryRejected)?,
        )
    } else {
        None
    };
    Ok(DecodedSample {
        event,
        confirmed_delivery,
    })
}

pub(super) fn classify_delivery_error(
    validator: &mut MacosStreamDeliveryValidator,
    error: MacosCaptureError,
) -> MacosCaptureError {
    if matches!(
        validator.state(),
        MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_)
    ) {
        return reject_first_delivery(validator, error);
    }
    match error {
        MacosCaptureError::StreamDeliveryRejected(rejection) => {
            MacosCaptureError::FrameDeliveryDropped(rejection)
        }
        error => error,
    }
}

pub(super) fn reject_first_delivery(
    validator: &mut MacosStreamDeliveryValidator,
    error: MacosCaptureError,
) -> MacosCaptureError {
    if !matches!(
        validator.state(),
        MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_)
    ) {
        return error;
    }
    let rejection = match &error {
        MacosCaptureError::MissingFramePayload => {
            Some(MacosStreamDeliveryRejection::MissingFirstCompleteFrame)
        }
        MacosCaptureError::UnsupportedPixelFormat(_) => {
            Some(MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("pixel_format"))
        }
        MacosCaptureError::MissingColorAttachment(field)
        | MacosCaptureError::UnsupportedColorAttachment(field)
        | MacosCaptureError::MalformedLuminanceAttachment(field) => {
            Some(MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata(field))
        }
        MacosCaptureError::ColorMetadataMismatch | MacosCaptureError::MissingYuvColorMetadata => {
            Some(MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("colorimetry"))
        }
        MacosCaptureError::StreamDeliveryRejected(rejection) => Some(*rejection),
        _ => None,
    };
    rejection.map_or(error, |rejection| {
        validator.reject_delivery(rejection);
        MacosCaptureError::StreamDeliveryRejected(rejection)
    })
}

pub(super) fn decode_complete_frame(
    pixel_buffer: CFRetained<CVPixelBuffer>,
    admission_lifetime: Option<PoolBackingLifetime>,
    cursor_composed: bool,
) -> Result<MacosRawCompleteFrame, MacosCaptureError> {
    let storage_extent = extent(
        CVPixelBufferGetWidth(&pixel_buffer),
        CVPixelBufferGetHeight(&pixel_buffer),
    )?;
    let pixel_format_fourcc = CVPixelBufferGetPixelFormatType(&pixel_buffer);
    let pixel_format = MacosCapturePixelFormat::from_fourcc(pixel_format_fourcc)?;
    let planes = planes(&pixel_buffer, storage_extent)?;
    let color = colorimetry(&pixel_buffer, pixel_format_fourcc, pixel_format)?;
    let (source_reference_white_nits, content_headroom) = hdr_luminance_metadata(&pixel_buffer)?;
    let delivery_metadata = MacosDeliveredFrameMetadata::new(
        pixel_format,
        color,
        source_reference_white_nits,
        content_headroom,
    )?;
    let surface = MacosCaptureSurface::from_pixel_buffer_with_delivery_metadata(
        pixel_buffer,
        admission_lifetime,
        Some(delivery_metadata),
    )?;

    Ok(MacosRawCompleteFrame {
        storage_extent,
        planes,
        pixel_format_fourcc,
        color,
        cursor_composed,
        surface,
    })
}

pub(super) fn planes(
    pixel_buffer: &CVPixelBuffer,
    storage_extent: MacosPixelExtent,
) -> Result<Vec<MacosRawCapturePlane>, MacosCaptureError> {
    let plane_count = CVPixelBufferGetPlaneCount(pixel_buffer);
    if plane_count == 0 {
        return Ok(vec![MacosRawCapturePlane {
            index: 0,
            extent: storage_extent,
            bytes_per_row: CVPixelBufferGetBytesPerRow(pixel_buffer),
            length_bytes: u64::try_from(CVPixelBufferGetDataSize(pixel_buffer))
                .map_err(|_| MacosCaptureError::ArithmeticOverflow)?,
        }]);
    }

    (0..plane_count)
        .map(|index| {
            let extent = extent(
                CVPixelBufferGetWidthOfPlane(pixel_buffer, index),
                CVPixelBufferGetHeightOfPlane(pixel_buffer, index),
            )?;
            let bytes_per_row = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, index);
            let length_bytes = u64::try_from(bytes_per_row)
                .ok()
                .and_then(|stride| stride.checked_mul(u64::from(extent.height)))
                .ok_or(MacosCaptureError::ArithmeticOverflow)?;
            Ok(MacosRawCapturePlane {
                index: u32::try_from(index).map_err(|_| MacosCaptureError::ArithmeticOverflow)?,
                extent,
                bytes_per_row,
                length_bytes,
            })
        })
        .collect()
}

pub(super) fn extent(width: usize, height: usize) -> Result<MacosPixelExtent, MacosCaptureError> {
    let width = u32::try_from(width).map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    let height = u32::try_from(height).map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    Ok(MacosPixelExtent::new(width, height)?)
}

pub(super) fn colorimetry(
    pixel_buffer: &CVBuffer,
    fourcc: u32,
    format: MacosCapturePixelFormat,
) -> Result<MacosCaptureColorimetry, MacosCaptureError> {
    // SAFETY: These Core Video constants are process-lifetime immutable CFString
    // references supplied by the linked framework.
    let (primaries_key, rec709, display_p3, rec2020) = unsafe {
        (
            kCVImageBufferColorPrimariesKey,
            kCVImageBufferColorPrimaries_ITU_R_709_2,
            kCVImageBufferColorPrimaries_P3_D65,
            kCVImageBufferColorPrimaries_ITU_R_2020,
        )
    };
    let primaries_value = color_attachment(pixel_buffer, primaries_key, "color_primaries")?;
    let primaries = match &*primaries_value {
        value if value == rec709 => MacosColorPrimaries::Srgb,
        value if value == display_p3 => MacosColorPrimaries::DisplayP3,
        value if value == rec2020 => MacosColorPrimaries::Rec2020,
        _ => {
            return Err(MacosCaptureError::UnsupportedColorAttachment(
                "color_primaries",
            ));
        }
    };

    // SAFETY: These Core Video constants are process-lifetime immutable CFString
    // references supplied by the linked framework.
    let (transfer_key, srgb, rec709, rec2020, linear, pq, hlg) = unsafe {
        (
            kCVImageBufferTransferFunctionKey,
            kCVImageBufferTransferFunction_sRGB,
            kCVImageBufferTransferFunction_ITU_R_709_2,
            kCVImageBufferTransferFunction_ITU_R_2020,
            kCVImageBufferTransferFunction_Linear,
            kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ,
            kCVImageBufferTransferFunction_ITU_R_2100_HLG,
        )
    };
    let transfer_value = color_attachment(pixel_buffer, transfer_key, "transfer_function")?;
    let transfer = match &*transfer_value {
        value if value == srgb => MacosTransferFunction::Srgb,
        value if value == rec709 => MacosTransferFunction::Rec709,
        value if value == rec2020 => MacosTransferFunction::Rec2020,
        value if value == linear => MacosTransferFunction::Linear,
        value if value == pq => MacosTransferFunction::Pq,
        value if value == hlg => MacosTransferFunction::Hlg,
        _ => {
            return Err(MacosCaptureError::UnsupportedColorAttachment(
                "transfer_function",
            ));
        }
    };

    let range = match fourcc {
        0x3432_3076 | 0x7834_3434 => MacosColorRange::Video,
        _ => MacosColorRange::Full,
    };
    let is_rgb = matches!(
        format,
        MacosCapturePixelFormat::Bgra8
            | MacosCapturePixelFormat::Argb2101010
            | MacosCapturePixelFormat::Rgba16Float
    );
    let (matrix, chroma_location) = if is_rgb {
        (None, None)
    } else {
        (
            Some(yuv_matrix(pixel_buffer)?),
            Some(chroma_location(pixel_buffer)?),
        )
    };

    Ok(MacosCaptureColorimetry {
        primaries,
        transfer,
        matrix,
        range,
        chroma_location,
    })
}

pub(super) fn hdr_luminance_metadata(
    pixel_buffer: &CVPixelBuffer,
) -> Result<(Option<f32>, Option<f32>), MacosCaptureError> {
    let content_headroom = content_headroom(pixel_buffer)?;
    let content_peak_nits = content_peak_nits(pixel_buffer)?;
    let source_reference_white_nits = content_peak_nits
        .zip(content_headroom)
        .map(|(peak, headroom)| peak / headroom)
        .filter(|reference| reference.is_finite() && *reference > 0.0);
    Ok((source_reference_white_nits, content_headroom))
}

pub(super) fn content_headroom(
    pixel_buffer: &CVPixelBuffer,
) -> Result<Option<f32>, MacosCaptureError> {
    let surface =
        CVPixelBufferGetIOSurface(Some(pixel_buffer)).ok_or(MacosCaptureError::MissingIoSurface)?;
    // SAFETY: This is a process-lifetime IOSurface key available at the macOS
    // 15.2 deployment floor.
    let value = surface.value(unsafe { kIOSurfaceContentHeadroom });
    let Some(value) = value else {
        return Ok(None);
    };
    let headroom = value
        .downcast_ref::<CFNumber>()
        .and_then(CFNumber::as_f64)
        .map(|value| value as f32)
        .filter(|value| value.is_finite() && *value >= 1.0)
        .ok_or(MacosCaptureError::MalformedLuminanceAttachment(
            "content_headroom",
        ))?;
    Ok(Some(headroom))
}

pub(super) fn content_peak_nits(pixel_buffer: &CVBuffer) -> Result<Option<f32>, MacosCaptureError> {
    // SAFETY: This is a process-lifetime Core Video key, and a null mode
    // pointer explicitly requests no attachment-mode output.
    let value =
        unsafe { pixel_buffer.attachment(kCVImageBufferContentLightLevelInfoKey, ptr::null_mut()) };
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes = cf_data_bytes(&value).ok_or(MacosCaptureError::MalformedLuminanceAttachment(
        "content_light_level_info",
    ))?;
    if bytes.len() != 4 {
        return Err(MacosCaptureError::MalformedLuminanceAttachment(
            "content_light_level_info",
        ));
    }
    let max_content_light_level = u16::from_be_bytes([bytes[0], bytes[1]]);
    Ok((max_content_light_level != 0).then_some(f32::from(max_content_light_level)))
}

pub(super) fn cf_data_bytes(value: &CFType) -> Option<&[u8]> {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C-unwind" {
        fn CFDataGetTypeID() -> usize;
        fn CFDataGetLength(data: *const c_void) -> isize;
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    }

    // SAFETY: The CFType is live for the returned borrow, and the type ID is
    // checked before calling CFData accessors.
    unsafe {
        if CFGetTypeID(Some(value)) != CFDataGetTypeID() {
            return None;
        }
        let data = ptr::from_ref(value).cast::<c_void>();
        let length = usize::try_from(CFDataGetLength(data)).ok()?;
        let bytes = CFDataGetBytePtr(data);
        if bytes.is_null() && length != 0 {
            return None;
        }
        Some(std::slice::from_raw_parts(bytes, length))
    }
}

pub(super) fn yuv_matrix(pixel_buffer: &CVBuffer) -> Result<MacosYuvMatrix, MacosCaptureError> {
    // SAFETY: These Core Video constants are process-lifetime immutable CFString
    // references supplied by the linked framework.
    let (matrix_key, bt601, bt709, bt2020) = unsafe {
        (
            kCVImageBufferYCbCrMatrixKey,
            kCVImageBufferYCbCrMatrix_ITU_R_601_4,
            kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            kCVImageBufferYCbCrMatrix_ITU_R_2020,
        )
    };
    let value = color_attachment(pixel_buffer, matrix_key, "ycbcr_matrix")?;
    match &*value {
        value if value == bt601 => Ok(MacosYuvMatrix::Bt601),
        value if value == bt709 => Ok(MacosYuvMatrix::Bt709),
        value if value == bt2020 => Ok(MacosYuvMatrix::Bt2020),
        _ => Err(MacosCaptureError::UnsupportedColorAttachment(
            "ycbcr_matrix",
        )),
    }
}

pub(super) fn chroma_location(
    pixel_buffer: &CVBuffer,
) -> Result<MacosChromaLocation, MacosCaptureError> {
    // SAFETY: These Core Video constants are process-lifetime immutable CFString
    // references supplied by the linked framework.
    let (location_key, left, center, top_left) = unsafe {
        (
            kCVImageBufferChromaLocationTopFieldKey,
            kCVImageBufferChromaLocation_Left,
            kCVImageBufferChromaLocation_Center,
            kCVImageBufferChromaLocation_TopLeft,
        )
    };
    // SAFETY: A null mode pointer explicitly requests no attachment-mode
    // output, and the retained result survives the pixel-buffer query.
    let Some(value) = (unsafe { pixel_buffer.attachment(location_key, ptr::null_mut()) }) else {
        // ScreenCaptureKit display streams can deliver 4:2:0 buffers that
        // carry the YCbCr matrix but no chroma-location attachment
        // (observed on macOS 26). ITU-T H.273 defines left siting as the
        // default for unsignalled 4:2:0 video and AVFoundation samples
        // under the same assumption, so absence is a defaulting case, not
        // a delivery-contract violation. A present-but-unrecognized value
        // still fails below.
        return Ok(MacosChromaLocation::Left);
    };
    let value = value
        .downcast::<CFString>()
        .map_err(|_| MacosCaptureError::UnsupportedColorAttachment("chroma_location"))?;
    match &*value {
        value if value == left => Ok(MacosChromaLocation::Left),
        value if value == center => Ok(MacosChromaLocation::Center),
        value if value == top_left => Ok(MacosChromaLocation::TopLeft),
        _ => Err(MacosCaptureError::UnsupportedColorAttachment(
            "chroma_location",
        )),
    }
}

pub(super) fn color_attachment(
    pixel_buffer: &CVBuffer,
    key: &CFString,
    name: &'static str,
) -> Result<CFRetained<CFString>, MacosCaptureError> {
    // SAFETY: A null mode pointer explicitly requests no attachment-mode
    // output, and the retained result survives the pixel-buffer query.
    let value = unsafe { pixel_buffer.attachment(key, ptr::null_mut()) }
        .ok_or(MacosCaptureError::MissingColorAttachment(name))?;
    value
        .downcast::<CFString>()
        .map_err(|_| MacosCaptureError::UnsupportedColorAttachment(name))
}

pub(super) struct FrameAttachments(CFRetained<CFDictionary<CFString, CFType>>);

impl FrameAttachments {
    pub(super) fn from_sample(sample: &CMSampleBuffer) -> Result<Self, MacosCaptureError> {
        // SAFETY: The sample reference is valid for this callback. Passing
        // false prevents Core Media from mutating it to create attachments.
        let attachments = unsafe { sample.sample_attachments_array(false) }
            .ok_or(MacosCaptureError::MissingFrameAttachments)?;
        if attachments.len() != 1 {
            return Err(MacosCaptureError::MalformedAttachment("frame_info"));
        }
        // SAFETY: Core Media documents this as an array of CF attachment
        // dictionaries. The element is still type-checked before use.
        let attachments = unsafe { attachments.cast_unchecked::<CFType>() };
        let dictionary = attachments
            .get(0)
            .and_then(|value| value.downcast::<CFDictionary>().ok())
            .ok_or(MacosCaptureError::MalformedAttachment("frame_info"))?;
        // SAFETY: ScreenCaptureKit frame dictionaries use NSString keys and
        // Core Foundation object values. Both are toll-free bridge types.
        let dictionary =
            unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(dictionary) };
        Ok(Self(dictionary))
    }

    pub(super) fn decode(&self) -> MacosRawFrameAttachments {
        // SAFETY: ScreenCaptureKit exports process-lifetime immutable NSString
        // constants for every frame-info dictionary key.
        let (status, display_time, scale, content_scale, content, dirty, screen, bounding) = unsafe {
            (
                SCStreamFrameInfoStatus,
                SCStreamFrameInfoDisplayTime,
                SCStreamFrameInfoScaleFactor,
                SCStreamFrameInfoContentScale,
                SCStreamFrameInfoContentRect,
                SCStreamFrameInfoDirtyRects,
                SCStreamFrameInfoScreenRect,
                SCStreamFrameInfoBoundingRect,
            )
        };
        MacosRawFrameAttachments {
            status: self.number_i64(status),
            display_time: self.number_u64(display_time),
            display_scale_factor: self.number_f64(scale),
            content_scale: self.number_f64(content_scale),
            content_rect: self.point_rect(content),
            dirty_rects: self.pixel_rects(dirty),
            screen_rect: self.point_rect(screen),
            bounding_rect: self.point_rect(bounding),
        }
    }

    fn value(&self, key: &NSString) -> Option<CFRetained<CFType>> {
        self.0.get(cf_string(key))
    }

    fn number_i64(&self, key: &NSString) -> MacosAttachment<i64> {
        self.convert(key, |value| value.downcast_ref::<CFNumber>()?.as_i64())
    }

    fn number_u64(&self, key: &NSString) -> MacosAttachment<u64> {
        self.convert(key, |value| {
            value
                .downcast_ref::<CFNumber>()?
                .as_i64()
                .and_then(|number| u64::try_from(number).ok())
        })
    }

    fn number_f64(&self, key: &NSString) -> MacosAttachment<f64> {
        self.convert(key, |value| value.downcast_ref::<CFNumber>()?.as_f64())
    }

    fn point_rect(&self, key: &NSString) -> MacosAttachment<MacosPointRect> {
        self.convert(key, point_rect)
    }

    fn pixel_rects(&self, key: &NSString) -> MacosAttachment<Vec<MacosPixelRect>> {
        self.convert(key, |value| {
            let array = value.downcast_ref::<CFArray>()?;
            // SAFETY: ScreenCaptureKit documents dirtyRects as an NSArray of
            // NSValue objects. Every element is checked before conversion.
            let array = unsafe { array.cast_unchecked::<CFType>() };
            array.iter().map(|rect| pixel_rect(&rect)).collect()
        })
    }

    fn convert<T>(
        &self,
        key: &NSString,
        convert: impl FnOnce(&CFType) -> Option<T>,
    ) -> MacosAttachment<T> {
        match self.value(key) {
            None => MacosAttachment::Missing,
            Some(value) => {
                convert(&value).map_or(MacosAttachment::Malformed, MacosAttachment::Value)
            }
        }
    }
}

pub(super) fn cf_string(value: &NSString) -> &CFString {
    // SAFETY: NSString and CFString are toll-free bridged immutable string
    // representations on macOS.
    unsafe { &*(ptr::from_ref(value).cast::<CFString>()) }
}

pub(super) fn point_rect(value: &CFType) -> Option<MacosPointRect> {
    let dictionary = value.downcast_ref::<CFDictionary>()?;
    let mut rect = CGRect::ZERO;
    // SAFETY: The output points to initialized CGRect storage, and the input
    // was type-checked as a CFDictionary.
    if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(dictionary), &mut rect) } {
        return None;
    }
    MacosPointRect::new(
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
    .ok()
}

pub(super) fn pixel_rect(value: &CFType) -> Option<MacosPixelRect> {
    // ScreenCaptureKit has shipped dirty rects both as NSValue-wrapped
    // CGRects and as CGRect dictionary representations; accept either.
    let rect = ns_value_rect(value).or_else(|| dictionary_rect(value))?;
    pixel_rect_from_cg(rect)
}

pub(super) fn ns_value_rect(value: &CFType) -> Option<CGRect> {
    let object = <CFType as AsRef<AnyObject>>::as_ref(value);
    object.downcast_ref::<NSValue>()?.get_rect()
}

pub(super) fn dictionary_rect(value: &CFType) -> Option<CGRect> {
    let dictionary = value.downcast_ref::<CFDictionary>()?;
    let mut rect = CGRect::ZERO;
    // SAFETY: The output points to initialized CGRect storage, and the input
    // was type-checked as a CFDictionary.
    unsafe { CGRectMakeWithDictionaryRepresentation(Some(dictionary), &mut rect) }.then_some(rect)
}

pub(super) fn pixel_rect_from_cg(rect: CGRect) -> Option<MacosPixelRect> {
    // Dirty rects are a damage hint. Scaled displays deliver fractional
    // coordinates, so round outward to the containing integer rect: the
    // damaged area must always be covered, never trimmed.
    let left = rect.origin.x.floor();
    let top = rect.origin.y.floor();
    let right = (rect.origin.x + rect.size.width).ceil();
    let bottom = (rect.origin.y + rect.size.height).ceil();
    let x = exact_i64(left)?;
    let y = exact_i64(top)?;
    let width = exact_u32(right - left)?;
    let height = exact_u32(bottom - top)?;
    MacosPixelRect::new(x, y, width, height).ok()
}

pub(super) fn exact_i64(value: f64) -> Option<i64> {
    if !value.is_finite()
        || value.fract() != 0.0
        || value < i64::MIN as f64
        || value > i64::MAX as f64
    {
        return None;
    }
    Some(value as i64)
}

pub(super) fn exact_u32(value: f64) -> Option<u32> {
    if !value.is_finite() || value.fract() != 0.0 || value <= 0.0 || value > f64::from(u32::MAX) {
        return None;
    }
    Some(value as u32)
}
