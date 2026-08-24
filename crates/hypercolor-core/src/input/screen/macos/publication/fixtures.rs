use super::super::{
    Arc, CaptureColorimetry, CapturePixelFormat, CaptureSourceId, Instant, MacosCaptureFrame,
    MacosCapturePixelFormat, MacosExactPublicationShared, MacosExactRuntime,
    MacosPublicationSource, MacosScreenRuntimeTelemetry, Ordering, PixelExtent, PixelRect,
    PlatformGpuApi, PlatformGpuSurface, PreparedWorker, ScreenPublicationHealth, anyhow,
};
#[cfg(feature = "macos-capture-fixtures")]
use super::super::{
    CaptureCursor, CaptureCursorContent, CaptureDamage, CaptureFrame, CaptureFrameMetadata,
    CaptureStorage, CpuCaptureStorage, CpuPublicationFanoutError, CpuSamplingError,
    CpuScalarSource, KnownCaptureColorimetry, LEGACY_ANALYSIS_MAX_HEIGHT,
    LEGACY_ANALYSIS_MAX_WIDTH, LedToneMapCalibration, MacosCpuSourceView, PreparedLedToneMap,
    RawCaptureSurface,
};
use super::metadata::capture_pixel_format;
use super::model::bind_current_macos_exact_runtime;

#[cfg(feature = "macos-capture-fixtures")]
pub(in crate::input::screen::macos) fn legacy_analysis_decimation(extent: PixelExtent) -> u32 {
    extent
        .width()
        .div_ceil(LEGACY_ANALYSIS_MAX_WIDTH)
        .max(extent.height().div_ceil(LEGACY_ANALYSIS_MAX_HEIGHT))
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "macos-capture-fixtures")]
pub(in crate::input::screen::macos) fn legacy_cpu_capture_frame(
    prepared: &mut PreparedWorker,
    frame: &MacosCaptureFrame,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    source_id: CaptureSourceId,
    topology_generation: u64,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let extent = source.geometry.storage_extent();
    let decimation = if frame.pixel_format == MacosCapturePixelFormat::Bgra8 {
        // The Bgra8 plane also feeds the CPU-exact publication, which is
        // exact by contract, so it keeps every native pixel.
        1
    } else {
        legacy_analysis_decimation(extent)
    };
    let storage_extent = if decimation == 1 {
        extent
    } else {
        PixelExtent::new(
            extent.width().div_ceil(decimation),
            extent.height().div_ceil(decimation),
        )?
    };
    let row_stride = usize::try_from(storage_extent.width())
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| anyhow!("macOS capture row stride overflow"))?;
    let byte_len = row_stride
        .checked_mul(usize::try_from(storage_extent.height())?)
        .ok_or_else(|| anyhow!("macOS capture plane length overflow"))?;
    let mut plane = prepared.plane_pool.try_acquire(byte_len)?;
    plane.resize(byte_len, 0);
    let (pixel_format, colorimetry) = if frame.pixel_format == MacosCapturePixelFormat::Bgra8 {
        frame.copy_bgra8_to(&mut plane, row_stride)?;
        (CapturePixelFormat::Bgra8, source.colorimetry)
    } else {
        let calibration = LedToneMapCalibration::try_new(
            prepared.analyzer.config().target_led_white_x,
            prepared.analyzer.config().target_led_white_y,
            prepared.analyzer.config().target_led_reference_white_nits,
            prepared.analyzer.config().target_led_peak_nits,
            prepared.analyzer.config().exposure_ev,
        )?;
        let tone_map = PreparedLedToneMap::prepare(
            source.colorimetry.try_known()?,
            KnownCaptureColorimetry::SRGB,
            calibration,
        )?;
        frame.with_cpu_source(|samples| -> anyhow::Result<()> {
            for y in 0..storage_extent.height() {
                let source_y = y
                    .checked_mul(decimation)
                    .ok_or_else(|| anyhow!("macOS legacy sample row overflow"))?;
                let row_start = usize::try_from(y)?
                    .checked_mul(row_stride)
                    .ok_or_else(|| anyhow!("macOS legacy row offset overflow"))?;
                for x in 0..storage_extent.width() {
                    let source_x = x
                        .checked_mul(decimation)
                        .ok_or_else(|| anyhow!("macOS legacy sample column overflow"))?;
                    let pixel_start = usize::try_from(x)?
                        .checked_mul(4)
                        .and_then(|offset| row_start.checked_add(offset))
                        .ok_or_else(|| anyhow!("macOS legacy pixel offset overflow"))?;
                    let pixel_end = pixel_start
                        .checked_add(4)
                        .ok_or_else(|| anyhow!("macOS legacy pixel end overflow"))?;
                    let source_pixel = samples.sample_rgba32f(source_x, source_y)?;
                    plane[pixel_start..pixel_end].copy_from_slice(
                        &tone_map.encode(tone_map.decode_and_map_source(source_pixel)),
                    );
                }
            }
            Ok(())
        })??;
        (CapturePixelFormat::Rgba8, CaptureColorimetry::SRGB)
    };
    let geometry = if decimation == 1 {
        source.geometry
    } else {
        super::super::super::CaptureGeometry::new(
            source.geometry.origin(),
            source.geometry.native_extent(),
            storage_extent,
            source.geometry.rotation(),
            source.geometry.crop(),
            source.geometry.source_scale(),
        )?
    };
    let damage = if decimation == 1 {
        CaptureDamage::new(
            frame
                .damage
                .iter()
                .map(|rect| {
                    Ok(PixelRect::new(
                        u32::try_from(rect.x)?,
                        u32::try_from(rect.y)?,
                        rect.width,
                        rect.height,
                    )?)
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            Vec::new(),
        )
    } else {
        // Native damage rects no longer address the decimated pixels, and the
        // analyzer resamples the whole surface anyway.
        CaptureDamage::new(
            vec![PixelRect::new(
                0,
                0,
                storage_extent.width(),
                storage_extent.height(),
            )?],
            Vec::new(),
        )
    };
    let sequence = frame
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    Ok(CaptureFrame::<RawCaptureSurface>::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation,
            session_generation: frame.epoch,
            sequence,
            captured_at,
            fresh_until,
            geometry,
            colorimetry,
            cursor: CaptureCursor {
                visible: frame.cursor_composed,
                position: None,
                hotspot: None,
                shape_extent: None,
                shape_generation: None,
                content: if frame.cursor_composed {
                    CaptureCursorContent::Composed
                } else {
                    CaptureCursorContent::Hidden
                },
            },
        },
        CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            plane.freeze(),
            pixel_format,
            i64::try_from(row_stride)?,
            0,
        )),
        damage,
    )?)
}

#[cfg(feature = "macos-capture-fixtures")]
pub(in crate::input::screen::macos) fn native_cpu_capture_frame(
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    source_id: CaptureSourceId,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let sequence = frame
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    let surface = PlatformGpuSurface::new(
        PlatformGpuApi::Metal,
        u64::from(frame.surface.iosurface_id),
        source.geometry.storage_extent(),
        capture_pixel_format(frame.pixel_format),
        Arc::clone(frame),
    )?;
    Ok(CaptureFrame::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation: source.epoch.topology_generation,
            session_generation: frame.epoch,
            sequence,
            captured_at,
            fresh_until,
            geometry: source.geometry,
            colorimetry: source.colorimetry,
            cursor: CaptureCursor {
                visible: frame.cursor_composed,
                position: None,
                hotspot: None,
                shape_extent: None,
                shape_generation: None,
                content: if frame.cursor_composed {
                    CaptureCursorContent::Composed
                } else {
                    CaptureCursorContent::Hidden
                },
            },
        },
        CaptureStorage::Gpu(surface),
        CaptureDamage::new(
            frame
                .damage
                .iter()
                .map(|rect| {
                    Ok(PixelRect::new(
                        u32::try_from(rect.x)?,
                        u32::try_from(rect.y)?,
                        rect.width,
                        rect.height,
                    )?)
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            Vec::new(),
        ),
    )?)
}

#[cfg(feature = "macos-capture-fixtures")]
pub(in crate::input::screen::macos) fn publish_macos_cpu_exact(
    frame: &CaptureFrame<RawCaptureSurface>,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    telemetry: &MacosScreenRuntimeTelemetry,
) -> anyhow::Result<()> {
    let Some(hub) = exact.hub() else {
        return Ok(());
    };
    let Some(runtime) =
        bind_current_macos_exact_runtime(runtimes, source, &hub, frame.metadata().captured_at)?
    else {
        return Ok(());
    };
    if let Some(fanout) = runtime.fanout.as_mut() {
        telemetry
            .publication_plan_generation
            .store(fanout.plan_generation().get(), Ordering::Release);
        let report = fanout.publish_due(
            &hub,
            Some(frame),
            Instant::now(),
            ScreenPublicationHealth::Healthy,
        )?;
        if report.published() > 0 {
            telemetry.record_converted_publication(frame.metadata().captured_at);
        }
    }
    Ok(())
}

#[cfg(feature = "macos-capture-fixtures")]
pub(in crate::input::screen::macos) fn publish_macos_scalar_exact(
    native_frame: &MacosCaptureFrame,
    frame: &CaptureFrame<RawCaptureSurface>,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    telemetry: &MacosScreenRuntimeTelemetry,
) -> anyhow::Result<()> {
    let Some(hub) = exact.hub() else {
        return Ok(());
    };
    let Some(runtime) =
        bind_current_macos_exact_runtime(runtimes, source, &hub, frame.metadata().captured_at)?
    else {
        return Ok(());
    };
    if let Some(fanout) = runtime.fanout.as_mut() {
        let reduction_started = Instant::now();
        telemetry
            .publication_plan_generation
            .store(fanout.plan_generation().get(), Ordering::Release);
        let report = fanout.publish_due_scalar(
            &hub,
            frame,
            Instant::now(),
            ScreenPublicationHealth::Healthy,
            |execute| {
                native_frame
                    .with_cpu_source(|samples| execute(&samples))
                    .map_err(|error| {
                        CpuPublicationFanoutError::ScalarSourceAccessFailed(error.to_string())
                    })?
            },
        )?;
        if report.published() > 0 {
            telemetry.record_cpu_reduction(reduction_started.elapsed());
            telemetry.record_converted_publication(frame.metadata().captured_at);
        }
    }
    Ok(())
}

#[cfg(feature = "macos-capture-fixtures")]
impl CpuScalarSource for MacosCpuSourceView<'_> {
    fn storage_extent(&self) -> PixelExtent {
        let extent = (*self).extent();
        PixelExtent::new(extent.width, extent.height)
            .expect("validated macOS CPU source has a non-empty extent")
    }

    fn pixel_format(&self) -> CapturePixelFormat {
        capture_pixel_format((*self).pixel_format())
    }

    fn sample_rgba32f(&self, x: u32, y: u32) -> Result<[f32; 4], CpuSamplingError> {
        (*self)
            .sample_rgba32f(x, y)
            .map_err(|_| CpuSamplingError::ScalarSourceReadFailed { x, y })
    }
}
