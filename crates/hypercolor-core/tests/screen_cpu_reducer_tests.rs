//! Arbitrary-resolution deterministic CPU reduction contracts.

use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::thread;

use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureColorimetry, CaptureDynamicRange, CaptureEpoch, CaptureGeometry,
    CapturePixelFormat, CaptureRotation, CaptureSourceId, CaptureTransferFunction,
    CpuCaptureStorage, CpuReductionError, CpuReductionExecutor, CpuReductionLayout,
    CpuReductionRequest, KnownCaptureColorimetry, PhysicalOrigin, PixelExtent,
    ResolvedScreenColorPipeline, ResolvedScreenColorTransform, ResolvedScreenSource,
    ResolvedScreenSourceConfig, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenCaptureBackend, ScreenColorTransformCapabilities, ScreenExtentRequest,
    ScreenProcessingProfile, ScreenProcessingProfileConfig, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenReductionFilter, ScreenResourceApi, ScreenSourceReflection,
    ScreenSourceSelector, ScreenTargetColorimetry, SourceScale,
};
use hypercolor_types::canvas::{linear_to_srgb_u8, srgb_u8_to_linear};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn linear_srgb_pipeline() -> ResolvedScreenColorPipeline {
    managed_pipeline(KnownCaptureColorimetry::SRGB, KnownCaptureColorimetry::SRGB)
}

fn managed_pipeline(
    source_color: KnownCaptureColorimetry,
    target_color: KnownCaptureColorimetry,
) -> ResolvedScreenColorPipeline {
    resolve_pipeline(
        source_color,
        CapturePixelFormat::Rgba8,
        ScreenProcessingProfileConfig {
            target_colorimetry: ScreenTargetColorimetry::ConvertTo(target_color),
            ..ScreenProcessingProfileConfig::default()
        },
        true,
        true,
    )
}

fn preserve_encoded_pipeline(pixel_format: CapturePixelFormat) -> ResolvedScreenColorPipeline {
    resolve_pipeline(
        KnownCaptureColorimetry::SRGB,
        pixel_format,
        ScreenProcessingProfileConfig::exact_encoded_identity(pixel_format),
        false,
        false,
    )
}

fn resolve_pipeline(
    source_color: KnownCaptureColorimetry,
    source_pixel_format: CapturePixelFormat,
    profile_config: ScreenProcessingProfileConfig,
    linear_light_sdr: bool,
    relative_color_conversion: bool,
) -> ResolvedScreenColorPipeline {
    let source_extent = extent(2, 2);
    let source_id =
        CaptureSourceId::new("synthetic:cpu-reducer").expect("test source identity is non-empty");
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        source_extent,
        source_extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    let source = ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id,
            topology_generation: 1,
            session_generation: 1,
        },
        ResolvedScreenSourceConfig::new(
            geometry,
            source_extent,
            ScreenSourceReflection::None,
            source_pixel_format,
            CaptureColorimetry::from_known(source_color),
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                1,
                1,
            ),
        ),
    );
    let profile = Arc::new(ScreenProcessingProfile::new(profile_config));
    ScreenPublicationRequest::new(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        Arc::clone(&profile),
    )
    .resolve_with_color_capabilities(
        &source,
        if linear_light_sdr || relative_color_conversion {
            ScreenColorTransformCapabilities::new(
                linear_light_sdr,
                relative_color_conversion,
                false,
                profile.algorithm_revision(),
            )
        } else {
            ScreenColorTransformCapabilities::NONE
        },
    )
    .expect("CPU reducer declares the exact color operation")
    .physical()
    .color_pipeline()
}

fn executor(worker_count: usize, tile_rows: u32) -> CpuReductionExecutor {
    CpuReductionExecutor::new(
        NonZeroUsize::new(worker_count).expect("test worker count is non-zero"),
        NonZeroU32::new(tile_rows).expect("test tile height is non-zero"),
    )
    .expect("test worker pool builds")
}

fn storage(
    pixels: Vec<u8>,
    source_extent: PixelExtent,
    format: CapturePixelFormat,
) -> CpuCaptureStorage {
    let row_stride = i64::from(source_extent.width()) * 4;
    CpuCaptureStorage::new(Arc::from(pixels), format, row_stride, 0)
}

fn patterned_pixels(extent: PixelExtent, format: CapturePixelFormat) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(
        usize::try_from(u64::from(extent.width()) * u64::from(extent.height()) * 4)
            .expect("test buffer is addressable"),
    );
    for y in 0..extent.height() {
        for x in 0..extent.width() {
            let rgba = [
                ((u64::from(x) * 37 + u64::from(y) * 11 + 19) % 256) as u8,
                ((u64::from(x) * 13 + u64::from(y) * 53 + 71) % 256) as u8,
                ((u64::from(x) * 97 + u64::from(y) * 7 + 3) % 256) as u8,
                ((u64::from(x) * 17 + u64::from(y) * 29 + 127) % 256) as u8,
            ];
            match format {
                CapturePixelFormat::Rgba8 => pixels.extend_from_slice(&rgba),
                CapturePixelFormat::Bgra8 => {
                    pixels.extend_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
                }
            }
        }
    }
    pixels
}

fn reduce(
    executor: &CpuReductionExecutor,
    source_extent: PixelExtent,
    target_extent: PixelExtent,
    source_format: CapturePixelFormat,
    target_format: CapturePixelFormat,
    filter: ScreenReductionFilter,
) -> Vec<u8> {
    let source = storage(
        patterned_pixels(source_extent, source_format),
        source_extent,
        source_format,
    );
    let layout = CpuReductionLayout::new(source_extent, target_extent)
        .expect("test reduction geometry is addressable");
    let mut output = vec![0; layout.target_byte_len_usize()];
    executor
        .reduce(
            CpuReductionRequest::new(
                &source,
                layout,
                target_format,
                filter,
                linear_srgb_pipeline(),
            ),
            &mut output,
        )
        .expect("test reduction succeeds");
    output
}

#[test]
fn exact_layout_admits_8k_without_allocating_an_8k_plane() {
    let layout = CpuReductionLayout::new(extent(1, 1), extent(7680, 4320))
        .expect("8K packed RGBA is addressable");
    assert_eq!(layout.source_packed_byte_len(), 4);
    assert_eq!(layout.target_byte_len(), 132_710_400);
    assert_eq!(layout.target_byte_len_usize(), 132_710_400);
}

#[test]
fn exact_layout_rejects_u32_edge_byte_overflow() {
    let edge = extent(u32::MAX, u32::MAX);
    assert_eq!(
        CpuReductionLayout::new(edge, extent(1, 1)),
        Err(CpuReductionError::GeometryOverflow { resource: "source" })
    );
    assert_eq!(
        CpuReductionLayout::new(extent(1, 1), edge),
        Err(CpuReductionError::GeometryOverflow { resource: "target" })
    );
}

#[test]
fn one_pixel_roundtrips_every_filter_and_channel_order() {
    let executor = executor(3, 7);
    for source_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
        for target_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
            for filter in [
                ScreenReductionFilter::Nearest,
                ScreenReductionFilter::Bilinear,
                ScreenReductionFilter::Area,
            ] {
                let output = reduce(
                    &executor,
                    extent(1, 1),
                    extent(1, 1),
                    source_format,
                    target_format,
                    filter,
                );
                let expected = match target_format {
                    CapturePixelFormat::Rgba8 => vec![19, 71, 3, 127],
                    CapturePixelFormat::Bgra8 => vec![3, 71, 19, 127],
                };
                assert_eq!(output, expected);
            }
        }
    }
}

#[test]
fn odd_portrait_upscale_and_ultrawide_downscale_have_exact_sizes() {
    let executor = executor(4, 3);
    let portrait = reduce(
        &executor,
        extent(3, 5),
        extent(7, 11),
        CapturePixelFormat::Bgra8,
        CapturePixelFormat::Rgba8,
        ScreenReductionFilter::Bilinear,
    );
    assert_eq!(portrait.len(), 7 * 11 * 4);

    let ultrawide = reduce(
        &executor,
        extent(257, 9),
        extent(31, 5),
        CapturePixelFormat::Rgba8,
        CapturePixelFormat::Bgra8,
        ScreenReductionFilter::Area,
    );
    assert_eq!(ultrawide.len(), 31 * 5 * 4);
    assert!(ultrawide.iter().any(|channel| *channel != 0));
}

#[test]
fn worker_count_and_tile_height_do_not_change_any_sample() {
    let serial = executor(1, 1);
    let parallel = executor(4, 5);
    for source_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
        for target_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
            for filter in [
                ScreenReductionFilter::Nearest,
                ScreenReductionFilter::Bilinear,
                ScreenReductionFilter::Area,
            ] {
                let expected = reduce(
                    &serial,
                    extent(13, 7),
                    extent(19, 11),
                    source_format,
                    target_format,
                    filter,
                );
                let actual = reduce(
                    &parallel,
                    extent(13, 7),
                    extent(19, 11),
                    source_format,
                    target_format,
                    filter,
                );
                assert_eq!(actual, expected);
            }
        }
    }
}

#[test]
fn all_filters_match_an_independent_scalar_reference() {
    let source_extent = extent(5, 3);
    let target_extent = extent(3, 7);
    let executor = executor(4, 2);
    for source_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
        let source = patterned_pixels(source_extent, source_format);
        for target_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
            for filter in [
                ScreenReductionFilter::Nearest,
                ScreenReductionFilter::Bilinear,
                ScreenReductionFilter::Area,
            ] {
                let expected = scalar_reference(
                    &source,
                    source_extent,
                    target_extent,
                    source_format,
                    target_format,
                    filter,
                );
                let actual = reduce(
                    &executor,
                    source_extent,
                    target_extent,
                    source_format,
                    target_format,
                    filter,
                );
                assert_eq!(
                    actual, expected,
                    "{source_format:?} -> {target_format:?} {filter:?}"
                );
            }
        }
    }
}

#[test]
fn negative_stride_and_caller_output_contract_are_supported() {
    let source_extent = extent(2, 2);
    let top_down = patterned_pixels(source_extent, CapturePixelFormat::Rgba8);
    let mut bottom_up = Vec::with_capacity(top_down.len());
    bottom_up.extend_from_slice(&top_down[8..16]);
    bottom_up.extend_from_slice(&top_down[0..8]);
    let source = CpuCaptureStorage::new(Arc::from(bottom_up), CapturePixelFormat::Rgba8, -8, 8);
    let layout = CpuReductionLayout::new(source_extent, source_extent)
        .expect("test reduction geometry is addressable");
    let request = CpuReductionRequest::new(
        &source,
        layout,
        CapturePixelFormat::Rgba8,
        ScreenReductionFilter::Nearest,
        linear_srgb_pipeline(),
    );
    let executor = executor(2, 1);
    let mut output = vec![0; layout.target_byte_len_usize()];
    executor
        .reduce(request, &mut output)
        .expect("negative stride addresses logical rows");
    assert_eq!(output, top_down);

    let mut short = vec![0; layout.target_byte_len_usize() - 1];
    assert_eq!(
        executor.reduce(request, &mut short),
        Err(CpuReductionError::OutputLengthMismatch {
            expected: layout.target_byte_len_usize(),
            actual: short.len(),
        })
    );
}

#[test]
fn encoded_sample_preservation_is_exact_and_rejects_byte_changing_work() {
    let source_extent = extent(3, 2);
    let pixels = patterned_pixels(source_extent, CapturePixelFormat::Bgra8);
    let source = storage(pixels.clone(), source_extent, CapturePixelFormat::Bgra8);
    let layout = CpuReductionLayout::new(source_extent, source_extent)
        .expect("test reduction geometry is addressable");
    let executor = executor(2, 1);
    let mut output = vec![0; layout.target_byte_len_usize()];
    executor
        .reduce(
            CpuReductionRequest::new(
                &source,
                layout,
                CapturePixelFormat::Bgra8,
                ScreenReductionFilter::Nearest,
                preserve_encoded_pipeline(CapturePixelFormat::Bgra8),
            ),
            &mut output,
        )
        .expect("exact encoded-sample preservation succeeds");
    assert_eq!(output, pixels);

    for (target_extent, target_format, filter) in [
        (
            source_extent,
            CapturePixelFormat::Rgba8,
            ScreenReductionFilter::Nearest,
        ),
        (
            extent(2, 2),
            CapturePixelFormat::Bgra8,
            ScreenReductionFilter::Nearest,
        ),
        (
            source_extent,
            CapturePixelFormat::Bgra8,
            ScreenReductionFilter::Area,
        ),
    ] {
        let changed_layout = CpuReductionLayout::new(source_extent, target_extent)
            .expect("test reduction geometry is addressable");
        let mut changed_output = vec![0; changed_layout.target_byte_len_usize()];
        assert_eq!(
            executor.reduce(
                CpuReductionRequest::new(
                    &source,
                    changed_layout,
                    target_format,
                    filter,
                    preserve_encoded_pipeline(CapturePixelFormat::Bgra8),
                ),
                &mut changed_output,
            ),
            Err(CpuReductionError::InexactEncodedSamplePreservation)
        );
    }
}

#[test]
fn padded_rows_and_hostile_source_addresses_are_validated_exactly() {
    let source_extent = extent(2, 2);
    let logical = patterned_pixels(source_extent, CapturePixelFormat::Rgba8);
    let mut padded = Vec::with_capacity(24);
    padded.extend_from_slice(&logical[..8]);
    padded.extend_from_slice(&[201, 202, 203, 204]);
    padded.extend_from_slice(&logical[8..]);
    padded.extend_from_slice(&[205, 206, 207, 208]);
    let layout = CpuReductionLayout::new(source_extent, source_extent)
        .expect("test reduction geometry is addressable");
    let executor = executor(2, 1);
    let mut output = vec![0; layout.target_byte_len_usize()];
    executor
        .reduce(
            CpuReductionRequest::new(
                &CpuCaptureStorage::new(Arc::from(padded), CapturePixelFormat::Rgba8, 12, 0),
                layout,
                CapturePixelFormat::Rgba8,
                ScreenReductionFilter::Nearest,
                linear_srgb_pipeline(),
            ),
            &mut output,
        )
        .expect("padded rows preserve their logical pixels");
    assert_eq!(output, logical);

    for (row_stride, row0_offset, expected) in [
        (
            7,
            0,
            CpuReductionError::InvalidSourceStride {
                stride: 7,
                minimum: 8,
            },
        ),
        (
            i64::MIN,
            0,
            CpuReductionError::InvalidSourceStride {
                stride: i64::MIN,
                minimum: 8,
            },
        ),
        (
            8,
            9,
            CpuReductionError::SourceBufferOutOfBounds { buffer_len: 16 },
        ),
    ] {
        let source = CpuCaptureStorage::new(
            Arc::from(vec![0_u8; 16]),
            CapturePixelFormat::Rgba8,
            row_stride,
            row0_offset,
        );
        assert_eq!(
            executor.reduce(
                CpuReductionRequest::new(
                    &source,
                    layout,
                    CapturePixelFormat::Rgba8,
                    ScreenReductionFilter::Nearest,
                    linear_srgb_pipeline(),
                ),
                &mut output,
            ),
            Err(expected)
        );
    }
}

#[test]
fn cloned_executor_handles_reduce_concurrently_without_output_drift() {
    let executor = executor(4, 3);
    let expected = reduce(
        &executor,
        extent(37, 19),
        extent(23, 31),
        CapturePixelFormat::Bgra8,
        CapturePixelFormat::Rgba8,
        ScreenReductionFilter::Area,
    );
    let handles = (0..8)
        .map(|_| {
            let executor = executor.clone();
            thread::spawn(move || {
                reduce(
                    &executor,
                    extent(37, 19),
                    extent(23, 31),
                    CapturePixelFormat::Bgra8,
                    CapturePixelFormat::Rgba8,
                    ScreenReductionFilter::Area,
                )
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        assert_eq!(
            handle.join().expect("concurrent reducer remains live"),
            expected
        );
    }
}

#[test]
fn linear_light_sdr_uses_the_resolved_transfer_function() {
    let source_extent = extent(2, 1);
    let target_extent = extent(3, 1);
    let source = storage(
        vec![0, 0, 0, 255, 255, 255, 255, 255],
        source_extent,
        CapturePixelFormat::Rgba8,
    );
    let layout = CpuReductionLayout::new(source_extent, target_extent)
        .expect("test reduction geometry is addressable");
    let linear = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Srgb,
        CaptureTransferFunction::Linear,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("linear SDR contract is complete");
    let executor = executor(2, 1);

    let mut srgb_output = vec![0; layout.target_byte_len_usize()];
    executor
        .reduce(
            CpuReductionRequest::new(
                &source,
                layout,
                CapturePixelFormat::Rgba8,
                ScreenReductionFilter::Bilinear,
                linear_srgb_pipeline(),
            ),
            &mut srgb_output,
        )
        .expect("sRGB reduction succeeds");
    let mut linear_output = vec![0; layout.target_byte_len_usize()];
    executor
        .reduce(
            CpuReductionRequest::new(
                &source,
                layout,
                CapturePixelFormat::Rgba8,
                ScreenReductionFilter::Bilinear,
                managed_pipeline(linear, linear),
            ),
            &mut linear_output,
        )
        .expect("linear reduction succeeds");

    assert_eq!(&srgb_output[4..8], &[188, 188, 188, 255]);
    assert_eq!(&linear_output[4..8], &[128, 128, 128, 255]);
}

#[test]
fn relative_color_conversion_is_a_typed_unsupported_operation() {
    let display_p3 = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::DisplayP3,
        CaptureTransferFunction::Srgb,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("Display P3 SDR contract is complete");
    let source_extent = extent(1, 1);
    let source = storage(
        vec![10, 20, 30, 255],
        source_extent,
        CapturePixelFormat::Rgba8,
    );
    let layout = CpuReductionLayout::new(source_extent, source_extent)
        .expect("test reduction geometry is addressable");
    let mut output = vec![0; layout.target_byte_len_usize()];
    let error = executor(1, 1)
        .reduce(
            CpuReductionRequest::new(
                &source,
                layout,
                CapturePixelFormat::Rgba8,
                ScreenReductionFilter::Area,
                managed_pipeline(display_p3, KnownCaptureColorimetry::SRGB),
            ),
            &mut output,
        )
        .expect_err("relative gamut conversion is not implemented by this CPU lane");
    assert!(matches!(
        error,
        CpuReductionError::UnsupportedColorTransform(
            ResolvedScreenColorTransform::LinearRelativeColorimetric { .. }
        )
    ));
}

fn scalar_reference(
    source: &[u8],
    source_extent: PixelExtent,
    target_extent: PixelExtent,
    source_format: CapturePixelFormat,
    target_format: CapturePixelFormat,
    filter: ScreenReductionFilter,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(
        usize::try_from(u64::from(target_extent.width()) * u64::from(target_extent.height()) * 4)
            .expect("test target is addressable"),
    );
    for target_y in 0..target_extent.height() {
        for target_x in 0..target_extent.width() {
            let rgba = match filter {
                ScreenReductionFilter::Nearest => scalar_nearest(
                    source,
                    source_extent,
                    target_extent,
                    source_format,
                    target_x,
                    target_y,
                ),
                ScreenReductionFilter::Bilinear => scalar_bilinear(
                    source,
                    source_extent,
                    target_extent,
                    source_format,
                    target_x,
                    target_y,
                ),
                ScreenReductionFilter::Area => scalar_area(
                    source,
                    source_extent,
                    target_extent,
                    source_format,
                    target_x,
                    target_y,
                ),
            };
            match target_format {
                CapturePixelFormat::Rgba8 => output.extend_from_slice(&rgba),
                CapturePixelFormat::Bgra8 => {
                    output.extend_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
                }
            }
        }
    }
    output
}

fn scalar_nearest(
    source: &[u8],
    source_extent: PixelExtent,
    target_extent: PixelExtent,
    format: CapturePixelFormat,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let source_x = (((f64::from(x) + 0.5) * f64::from(source_extent.width())
        / f64::from(target_extent.width()))
    .floor() as u32)
        .min(source_extent.width() - 1);
    let source_y = (((f64::from(y) + 0.5) * f64::from(source_extent.height())
        / f64::from(target_extent.height()))
    .floor() as u32)
        .min(source_extent.height() - 1);
    scalar_read(source, source_extent, format, source_x, source_y)
}

fn scalar_bilinear(
    source: &[u8],
    source_extent: PixelExtent,
    target_extent: PixelExtent,
    format: CapturePixelFormat,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let source_x = ((f64::from(x) + 0.5) * f64::from(source_extent.width())
        / f64::from(target_extent.width())
        - 0.5)
        .clamp(0.0, f64::from(source_extent.width() - 1));
    let source_y = ((f64::from(y) + 0.5) * f64::from(source_extent.height())
        / f64::from(target_extent.height())
        - 0.5)
        .clamp(0.0, f64::from(source_extent.height() - 1));
    let x0 = source_x.floor() as u32;
    let y0 = source_y.floor() as u32;
    let x1 = (x0 + 1).min(source_extent.width() - 1);
    let y1 = (y0 + 1).min(source_extent.height() - 1);
    let wx = source_x - f64::from(x0);
    let wy = source_y - f64::from(y0);
    let samples = [
        scalar_decode(scalar_read(source, source_extent, format, x0, y0)),
        scalar_decode(scalar_read(source, source_extent, format, x1, y0)),
        scalar_decode(scalar_read(source, source_extent, format, x0, y1)),
        scalar_decode(scalar_read(source, source_extent, format, x1, y1)),
    ];
    let mut result = [0.0; 4];
    for channel in 0..4 {
        let top = samples[0][channel] * (1.0 - wx) + samples[1][channel] * wx;
        let bottom = samples[2][channel] * (1.0 - wx) + samples[3][channel] * wx;
        result[channel] = top * (1.0 - wy) + bottom * wy;
    }
    scalar_encode(result)
}

fn scalar_area(
    source: &[u8],
    source_extent: PixelExtent,
    target_extent: PixelExtent,
    format: CapturePixelFormat,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let left = f64::from(x) * f64::from(source_extent.width()) / f64::from(target_extent.width());
    let right =
        f64::from(x + 1) * f64::from(source_extent.width()) / f64::from(target_extent.width());
    let top = f64::from(y) * f64::from(source_extent.height()) / f64::from(target_extent.height());
    let bottom =
        f64::from(y + 1) * f64::from(source_extent.height()) / f64::from(target_extent.height());
    let mut sums = [0.0; 4];
    let mut total = 0.0;
    for source_y in top.floor() as u32..bottom.ceil() as u32 {
        let y_weight = bottom.min(f64::from(source_y + 1)) - top.max(f64::from(source_y));
        for source_x in left.floor() as u32..right.ceil() as u32 {
            let x_weight = right.min(f64::from(source_x + 1)) - left.max(f64::from(source_x));
            let weight = x_weight * y_weight;
            let sample = scalar_decode(scalar_read(
                source,
                source_extent,
                format,
                source_x,
                source_y,
            ));
            for channel in 0..4 {
                sums[channel] += sample[channel] * weight;
            }
            total += weight;
        }
    }
    scalar_encode(sums.map(|sum| sum / total))
}

fn scalar_read(
    source: &[u8],
    extent: PixelExtent,
    format: CapturePixelFormat,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let index = usize::try_from((u64::from(y) * u64::from(extent.width()) + u64::from(x)) * 4)
        .expect("test source index is addressable");
    match format {
        CapturePixelFormat::Rgba8 => [
            source[index],
            source[index + 1],
            source[index + 2],
            source[index + 3],
        ],
        CapturePixelFormat::Bgra8 => [
            source[index + 2],
            source[index + 1],
            source[index],
            source[index + 3],
        ],
    }
}

fn scalar_decode(sample: [u8; 4]) -> [f64; 4] {
    [
        f64::from(srgb_u8_to_linear(sample[0])),
        f64::from(srgb_u8_to_linear(sample[1])),
        f64::from(srgb_u8_to_linear(sample[2])),
        f64::from(sample[3]) / 255.0,
    ]
}

fn scalar_encode(sample: [f64; 4]) -> [u8; 4] {
    [
        linear_to_srgb_u8(sample[0] as f32),
        linear_to_srgb_u8(sample[1] as f32),
        linear_to_srgb_u8(sample[2] as f32),
        (sample[3] * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}
