//! Color-admission contracts for exact screen publications.

use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureColorimetry, CaptureColorimetryError, CaptureDynamicRange,
    CaptureEpoch, CaptureGeometry, CaptureLuminanceContext, CapturePixelFormat,
    CapturePositiveScalar, CaptureRotation, CaptureSourceId, CaptureTransferFunction,
    KnownCaptureColorimetry, LED_TONE_MAP_ALGORITHM_REVISION, LedToneMapCalibration,
    LedToneMapCalibrationError, PhysicalOrigin, PixelExtent, PixelRect,
    RegisteredScreenBranchDemand, ResolvedScreenColorTransform, ResolvedScreenSource,
    ResolvedScreenSourceConfig, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenCaptureBackend, ScreenColorTransformCapabilities, ScreenColorTuning,
    ScreenCursorCapabilities, ScreenExtentRequest, ScreenHdrPolicy, ScreenProcessingProfile,
    ScreenProcessingProfileConfig, ScreenPublicationError, ScreenPublicationExecutorRequest,
    ScreenPublicationKind, ScreenPublicationRequest, ScreenReductionFilter, ScreenResourceApi,
    ScreenSceneCutPolicy, ScreenSmoothingPolicy, ScreenSourceReflection, ScreenSourceSelector,
    ScreenTargetColorimetry, ScreenToneMapOperator, ScreenToneMapPolicy, ScreenUnknownColorPolicy,
    ScreenUpscalePolicy, SourceScale,
};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn scalar(value: f32) -> CapturePositiveScalar {
    CapturePositiveScalar::try_new(value).expect("test scalar is positive and finite")
}

fn luminance(reference_white: f32, peak: f32) -> CaptureLuminanceContext {
    CaptureLuminanceContext::new(scalar(reference_white), scalar(peak))
        .expect("test luminance is ordered")
}

#[test]
fn led_target_calibration_rejects_invalid_values_without_clamping() {
    let error = |values: [f32; 5]| {
        LedToneMapCalibration::try_new(values[0], values[1], values[2], values[3], values[4])
    };
    for values in [
        [0.0, 0.329, 203.0, 406.0, 0.0],
        [-0.0, 0.329, 203.0, 406.0, 0.0],
        [0.3127, 0.0, 203.0, 406.0, 0.0],
        [0.3127, -0.0, 203.0, 406.0, 0.0],
        [0.7, 0.3, 203.0, 406.0, 0.0],
        [0.8, 0.3, 203.0, 406.0, 0.0],
    ] {
        assert_eq!(
            error(values),
            Err(LedToneMapCalibrationError::WhitePointOutsideChromaticityTriangle)
        );
    }
    for values in [
        [f32::NAN, 0.329, 203.0, 406.0, 0.0],
        [0.3127, f32::INFINITY, 203.0, 406.0, 0.0],
        [0.3127, 0.329, f32::NEG_INFINITY, 406.0, 0.0],
        [0.3127, 0.329, 203.0, f32::NAN, 0.0],
        [0.3127, 0.329, 203.0, 406.0, f32::INFINITY],
    ] {
        assert_eq!(
            error(values),
            Err(LedToneMapCalibrationError::NonFiniteScalar)
        );
    }
    assert_eq!(
        error([0.3127, 0.329, 0.99, 406.0, 0.0]),
        Err(LedToneMapCalibrationError::ReferenceWhiteOutOfRange)
    );
    assert_eq!(
        error([0.3127, 0.329, -0.0, 406.0, 0.0]),
        Err(LedToneMapCalibrationError::ReferenceWhiteOutOfRange)
    );
    assert_eq!(
        error([0.3127, 0.329, 5_000.1, 10_000.0, 0.0]),
        Err(LedToneMapCalibrationError::ReferenceWhiteOutOfRange)
    );
    assert_eq!(
        error([0.3127, 0.329, 1.0, 0.99, 0.0]),
        Err(LedToneMapCalibrationError::PeakOutOfRange)
    );
    assert_eq!(
        error([0.3127, 0.329, 1.0, -0.0, 0.0]),
        Err(LedToneMapCalibrationError::PeakOutOfRange)
    );
    assert_eq!(
        error([0.3127, 0.329, 203.0, 10_000.1, 0.0]),
        Err(LedToneMapCalibrationError::PeakOutOfRange)
    );
    for values in [
        [0.3127, 0.329, 203.0, 203.0, 0.0],
        [0.3127, 0.329, 204.0, 203.0, 0.0],
        [0.3127, 0.329, 1.0, 1.0, 0.0],
    ] {
        assert_eq!(
            error(values),
            Err(LedToneMapCalibrationError::PeakNotAboveReferenceWhite)
        );
    }
    assert_eq!(
        error([0.3127, 0.329, 203.0, 406.0, -8.01]),
        Err(LedToneMapCalibrationError::ExposureOutOfRange)
    );
    assert_eq!(
        error([0.3127, 0.329, 203.0, 406.0, 8.01]),
        Err(LedToneMapCalibrationError::ExposureOutOfRange)
    );
    for values in [
        [0.3127, 0.329, 1.0, 10_000.0, -8.0],
        [0.3127, 0.329, 5_000.0, 10_000.0, 8.0],
    ] {
        assert!(error(values).is_ok());
    }
    assert_eq!(
        error([0.3127, 0.329, 203.0, 406.0, -0.0]),
        error([0.3127, 0.329, 203.0, 406.0, 0.0])
    );
}

#[test]
fn replacing_led_calibration_refreshes_an_existing_hdr_policy() {
    let calibration = LedToneMapCalibration::try_new(0.3457, 0.3585, 160.0, 480.0, 1.0)
        .expect("measured target calibration is valid");
    let profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
        hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::new(
            ScreenToneMapOperator::Bt2390Eetf,
            luminance(100.0, 100.0),
        )),
        ..ScreenProcessingProfileConfig::default()
    })
    .with_led_tone_map(calibration);
    let ScreenHdrPolicy::ToneMap(policy) = profile.hdr() else {
        panic!("HDR tone-map policy must remain enabled");
    };
    assert_eq!(policy.target_luminance(), calibration.target_luminance());
    assert_eq!(policy.operator(), ScreenToneMapOperator::Bt2390Eetf);
}

fn known_sdr(
    color_space: CaptureColorSpace,
    transfer_function: CaptureTransferFunction,
) -> KnownCaptureColorimetry {
    KnownCaptureColorimetry::try_new(
        color_space,
        transfer_function,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("test SDR colorimetry is complete")
}

fn known_hdr(
    transfer_function: CaptureTransferFunction,
    context: CaptureLuminanceContext,
) -> KnownCaptureColorimetry {
    KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Rec2020,
        transfer_function,
        CaptureDynamicRange::High,
        Some(context),
    )
    .expect("test HDR colorimetry is complete")
}

fn source(colorimetry: CaptureColorimetry) -> ResolvedScreenSource {
    let source_extent = extent(1920, 1080);
    source_with_storage(
        colorimetry,
        CaptureGeometry::new(
            PhysicalOrigin::default(),
            source_extent,
            source_extent,
            CaptureRotation::Identity,
            None,
            SourceScale::ONE,
        )
        .expect("test geometry is valid"),
        source_extent,
        ScreenSourceReflection::None,
    )
}

fn source_with_storage(
    colorimetry: CaptureColorimetry,
    geometry: CaptureGeometry,
    logical_extent: PixelExtent,
    reflection: ScreenSourceReflection,
) -> ResolvedScreenSource {
    source_with_storage_format(
        colorimetry,
        geometry,
        logical_extent,
        reflection,
        CapturePixelFormat::Rgba8,
    )
}

fn source_with_storage_format(
    colorimetry: CaptureColorimetry,
    geometry: CaptureGeometry,
    logical_extent: PixelExtent,
    reflection: ScreenSourceReflection,
    pixel_format: CapturePixelFormat,
) -> ResolvedScreenSource {
    let source_id =
        CaptureSourceId::new("synthetic:color-contract").expect("test source id is non-empty");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id,
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            logical_extent,
            reflection,
            pixel_format,
            colorimetry,
            ScreenCursorCapabilities::clean_only(),
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                7,
                11,
            ),
        ),
    )
}

fn request(
    kind: ScreenPublicationKind,
    extent: ScreenExtentRequest,
    config: ScreenProcessingProfileConfig,
) -> ScreenPublicationRequest {
    request_with_profile(kind, extent, ScreenProcessingProfile::new(config))
}

fn request_with_profile(
    kind: ScreenPublicationKind,
    extent: ScreenExtentRequest,
    profile: ScreenProcessingProfile,
) -> ScreenPublicationRequest {
    ScreenPublicationRequest::new(
        ScreenSourceSelector::Configured,
        kind,
        ScreenPublicationExecutorRequest::Cpu,
        extent,
        ScreenAspectPolicy::Contain,
        Arc::new(profile),
    )
}

fn native_surface(config: ScreenProcessingProfileConfig) -> ScreenPublicationRequest {
    request(
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        config,
    )
}

fn calibrated_native_surface(
    config: ScreenProcessingProfileConfig,
    calibration: LedToneMapCalibration,
) -> ScreenPublicationRequest {
    request_with_profile(
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenProcessingProfile::new(config).with_led_tone_map(calibration),
    )
}

#[test]
fn positive_scalars_and_luminance_reject_non_physical_values() {
    assert_eq!(CaptureColorSpace::default(), CaptureColorSpace::Unknown);
    assert_eq!(
        CaptureTransferFunction::default(),
        CaptureTransferFunction::Unknown
    );
    assert_eq!(CaptureColorimetry::default(), CaptureColorimetry::unknown());
    for value in [0.0, -0.0, -1.0] {
        assert_eq!(
            CapturePositiveScalar::try_new(value),
            Err(CaptureColorimetryError::NonPositiveScalar)
        );
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            CapturePositiveScalar::try_new(value),
            Err(CaptureColorimetryError::NonFinitePositiveScalar)
        );
    }

    let low = scalar(f32::MIN_POSITIVE);
    let high = scalar(f32::MAX);
    assert_eq!(low.value(), f32::MIN_POSITIVE);
    assert_eq!(high.value(), f32::MAX);
    assert!(low < high);
    assert_eq!(
        CaptureLuminanceContext::new(scalar(200.0), scalar(100.0)),
        Err(CaptureColorimetryError::PeakBelowReferenceWhite)
    );
}

#[test]
fn known_colorimetry_rejects_unknown_and_incoherent_contracts() {
    assert_eq!(
        KnownCaptureColorimetry::try_new(
            CaptureColorSpace::Unknown,
            CaptureTransferFunction::Srgb,
            CaptureDynamicRange::Standard,
            None,
        ),
        Err(CaptureColorimetryError::UnknownColorSpace)
    );
    assert_eq!(
        KnownCaptureColorimetry::try_new(
            CaptureColorSpace::Srgb,
            CaptureTransferFunction::Unknown,
            CaptureDynamicRange::Standard,
            None,
        ),
        Err(CaptureColorimetryError::UnknownTransferFunction)
    );
    assert_eq!(
        KnownCaptureColorimetry::try_new(
            CaptureColorSpace::Rec2020,
            CaptureTransferFunction::Pq,
            CaptureDynamicRange::Standard,
            None,
        ),
        Err(CaptureColorimetryError::TransferDynamicRangeMismatch)
    );
    assert_eq!(
        KnownCaptureColorimetry::try_new(
            CaptureColorSpace::Rec2020,
            CaptureTransferFunction::Pq,
            CaptureDynamicRange::High,
            None,
        ),
        Err(CaptureColorimetryError::MissingHdrLuminance)
    );
}

#[test]
fn default_profile_rejects_unknown_before_demand_resolution() {
    let request = native_surface(ScreenProcessingProfileConfig::default());
    assert_eq!(
        request.resolve(&source(CaptureColorimetry::unknown())),
        Err(ScreenPublicationError::UnknownSourceColorimetry)
    );
    let demand = RegisteredScreenBranchDemand::new(request, std::num::NonZeroU32::MIN);
    assert_eq!(
        demand.resolve(&source(CaptureColorimetry::unknown())),
        Err(ScreenPublicationError::UnknownSourceColorimetry)
    );
}

#[test]
fn explicit_assumption_fills_missing_fields_without_overwriting_known_metadata() {
    let partial = CaptureColorimetry::new(
        CaptureColorSpace::Unknown,
        CaptureTransferFunction::Srgb,
        Some(CaptureDynamicRange::Standard),
        None,
    )
    .expect("partial source colorimetry is coherent");
    let assumption = known_sdr(CaptureColorSpace::Srgb, CaptureTransferFunction::Srgb);
    let descriptor = native_surface(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        target_colorimetry: ScreenTargetColorimetry::PreserveSource,
        unknown_color: ScreenUnknownColorPolicy::Assume(assumption),
        ..ScreenProcessingProfileConfig::default()
    })
    .resolve(&source(partial))
    .expect("explicit assumption fills missing primaries");
    assert_eq!(
        descriptor.physical().color_pipeline().effective_source(),
        Some(assumption)
    );

    let conflicting = CaptureColorimetry::new(
        CaptureColorSpace::DisplayP3,
        CaptureTransferFunction::Unknown,
        Some(CaptureDynamicRange::Standard),
        None,
    )
    .expect("partial source colorimetry is coherent");
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            target_colorimetry: ScreenTargetColorimetry::PreserveSource,
            unknown_color: ScreenUnknownColorPolicy::Assume(assumption),
            ..ScreenProcessingProfileConfig::default()
        })
        .resolve(&source(conflicting)),
        Err(ScreenPublicationError::ColorAssumptionConflict)
    );

    let source_luminance = luminance(80.0, 120.0);
    let partial_with_luminance = CaptureColorimetry::new(
        CaptureColorSpace::Srgb,
        CaptureTransferFunction::Unknown,
        Some(CaptureDynamicRange::Standard),
        Some(source_luminance),
    )
    .expect("partial SDR luminance is coherent");
    let descriptor = native_surface(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        target_colorimetry: ScreenTargetColorimetry::PreserveSource,
        unknown_color: ScreenUnknownColorPolicy::Assume(assumption),
        ..ScreenProcessingProfileConfig::default()
    })
    .resolve(&source(partial_with_luminance))
    .expect("absent assumption luminance does not overwrite source metadata");
    assert_eq!(
        descriptor
            .physical()
            .color_pipeline()
            .effective_source()
            .and_then(KnownCaptureColorimetry::luminance),
        Some(source_luminance)
    );

    let conflicting_luminance = assumption.with_luminance(luminance(100.0, 200.0));
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            target_colorimetry: ScreenTargetColorimetry::PreserveSource,
            unknown_color: ScreenUnknownColorPolicy::Assume(conflicting_luminance),
            ..ScreenProcessingProfileConfig::default()
        })
        .resolve(&source(partial_with_luminance)),
        Err(ScreenPublicationError::ColorAssumptionConflict)
    );
}

#[test]
fn unknown_preservation_requires_a_byte_identical_surface_path() {
    let preserve = ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        target_colorimetry: ScreenTargetColorimetry::PreserveSource,
        unknown_color: ScreenUnknownColorPolicy::PreserveEncodedSamples,
        ..ScreenProcessingProfileConfig::default()
    };
    let source_extent = extent(1920, 1080);
    let unknown_source = source_with_storage(
        CaptureColorimetry::unknown(),
        CaptureGeometry::new(
            PhysicalOrigin { x: -1920, y: 240 },
            source_extent,
            source_extent,
            CaptureRotation::Identity,
            None,
            SourceScale::new(3, 2).expect("test scale is valid"),
        )
        .expect("test geometry is valid"),
        source_extent,
        ScreenSourceReflection::None,
    );
    let descriptor = native_surface(preserve.clone())
        .resolve(&unknown_source)
        .expect("native nearest surface preserves unknown samples");
    assert_eq!(
        descriptor.physical().color_pipeline().output(),
        CaptureColorimetry::unknown()
    );
    assert_eq!(
        descriptor.physical().color_pipeline().transform(),
        ResolvedScreenColorTransform::PreserveEncodedSamples
    );

    let mut invalid = vec![
        native_surface(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Area,
            ..preserve.clone()
        }),
        native_surface(ScreenProcessingProfileConfig {
            tuning: ScreenColorTuning::try_new(1.1, 1.0, 1.0).expect("test tuning is finite"),
            ..preserve.clone()
        }),
        native_surface(ScreenProcessingProfileConfig {
            smoothing: ScreenSmoothingPolicy::Exponential {
                time_constant: Duration::from_millis(50),
                scene_cut: ScreenSceneCutPolicy::default(),
            },
            ..preserve.clone()
        }),
        request(
            ScreenPublicationKind::Zones {
                columns: std::num::NonZeroU32::new(8).expect("test columns are non-zero"),
                rows: std::num::NonZeroU32::new(6).expect("test rows are non-zero"),
            },
            ScreenExtentRequest::Native,
            preserve.clone(),
        ),
    ];
    invalid.push(request(
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::bounded(
            std::num::NonZeroU32::new(640),
            std::num::NonZeroU32::new(360),
            ScreenUpscalePolicy::Never,
        ),
        preserve,
    ));
    invalid.push(ScreenPublicationRequest::new(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenPublicationExecutorRequest::Cpu,
        ScreenExtentRequest::bounded(
            std::num::NonZeroU32::new(1_000),
            std::num::NonZeroU32::new(1_000),
            ScreenUpscalePolicy::Never,
        ),
        ScreenAspectPolicy::Cover,
        Arc::new(ScreenProcessingProfile::new(
            ScreenProcessingProfileConfig {
                reduction_filter: ScreenReductionFilter::Nearest,
                target_colorimetry: ScreenTargetColorimetry::PreserveSource,
                unknown_color: ScreenUnknownColorPolicy::PreserveEncodedSamples,
                ..ScreenProcessingProfileConfig::default()
            },
        )),
    ));

    for request in invalid {
        assert_eq!(
            request.resolve(&unknown_source),
            Err(ScreenPublicationError::UnknownPreservationRequiresByteIdentity)
        );
    }
}

#[test]
fn named_exact_identity_profile_executes_known_and_unknown_sdr_noops() {
    let source_extent = extent(1920, 1080);
    for pixel_format in [CapturePixelFormat::Rgba8, CapturePixelFormat::Bgra8] {
        let request = native_surface(ScreenProcessingProfileConfig::exact_encoded_identity(
            pixel_format,
        ));
        for colorimetry in [CaptureColorimetry::SRGB, CaptureColorimetry::unknown()] {
            let source = source_with_storage_format(
                colorimetry,
                CaptureGeometry::new(
                    PhysicalOrigin::default(),
                    source_extent,
                    source_extent,
                    CaptureRotation::Identity,
                    None,
                    SourceScale::ONE,
                )
                .expect("test geometry is valid"),
                source_extent,
                ScreenSourceReflection::None,
                pixel_format,
            );
            let descriptor = request
                .resolve(&source)
                .expect("exact encoded identity is executable without color capabilities");
            assert_eq!(descriptor.physical().color_pipeline().output(), colorimetry);
            assert_eq!(
                descriptor.physical().color_pipeline().transform(),
                ResolvedScreenColorTransform::PreserveEncodedSamples
            );
        }
    }
}

#[test]
fn unknown_encoded_preservation_rejects_hdr_transfer_without_range_metadata() {
    let preserve = native_surface(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        target_colorimetry: ScreenTargetColorimetry::PreserveSource,
        unknown_color: ScreenUnknownColorPolicy::PreserveEncodedSamples,
        ..ScreenProcessingProfileConfig::default()
    });

    for transfer_function in [CaptureTransferFunction::Pq, CaptureTransferFunction::Hlg] {
        let partial_hdr =
            CaptureColorimetry::new(CaptureColorSpace::Unknown, transfer_function, None, None)
                .expect("HDR transfer without range metadata remains representable");
        assert_eq!(
            preserve.resolve(&source(partial_hdr)),
            Err(ScreenPublicationError::UnsupportedTargetPixelFormat)
        );
    }
}

#[test]
fn encoded_byte_identity_rejects_noncanonical_source_storage() {
    let source_extent = extent(1920, 1080);
    let reduced_extent = extent(1280, 720);
    let unknown = CaptureColorimetry::unknown();
    let invalid_sources = [
        source_with_storage(
            unknown,
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                source_extent,
                source_extent,
                CaptureRotation::Clockwise180,
                None,
                SourceScale::ONE,
            )
            .expect("test rotation geometry is valid"),
            source_extent,
            ScreenSourceReflection::None,
        ),
        source_with_storage(
            unknown,
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                source_extent,
                source_extent,
                CaptureRotation::Identity,
                Some(PixelRect::new(0, 0, 1920, 1080).expect("test crop is valid")),
                SourceScale::ONE,
            )
            .expect("test crop geometry is valid"),
            source_extent,
            ScreenSourceReflection::None,
        ),
        source_with_storage(
            unknown,
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                source_extent,
                source_extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("test reflection geometry is valid"),
            source_extent,
            ScreenSourceReflection::Horizontal,
        ),
        source_with_storage(
            unknown,
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                source_extent,
                reduced_extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("test extent-mismatch geometry is valid"),
            reduced_extent,
            ScreenSourceReflection::None,
        ),
    ];
    let preserve = native_surface(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        target_colorimetry: ScreenTargetColorimetry::PreserveSource,
        unknown_color: ScreenUnknownColorPolicy::PreserveEncodedSamples,
        ..ScreenProcessingProfileConfig::default()
    });

    for source in invalid_sources {
        assert_eq!(
            preserve.resolve(&source),
            Err(ScreenPublicationError::UnknownPreservationRequiresByteIdentity)
        );
    }
}

#[test]
fn hdr_tone_mapping_remains_unresolved_without_reducer_capabilities() {
    let source_luminance = luminance(203.0, 1_000.0);
    let calibration = LedToneMapCalibration::DEFAULT;
    let target_luminance = calibration.target_luminance();
    let known_hdr = known_hdr(CaptureTransferFunction::Pq, source_luminance);
    let hdr_source = source(CaptureColorimetry::from_known(known_hdr));

    assert_eq!(
        native_surface(ScreenProcessingProfileConfig::default()).resolve(&hdr_source),
        Err(ScreenPublicationError::HdrRejected)
    );

    assert_eq!(
        native_surface(ScreenProcessingProfileConfig {
            hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
                ScreenToneMapOperator::Bt2390Eetf,
                calibration,
            )),
            ..ScreenProcessingProfileConfig::default()
        })
        .resolve(&hdr_source),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );

    let descriptor = native_surface(ScreenProcessingProfileConfig {
        hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
            ScreenToneMapOperator::Bt2390Eetf,
            calibration,
        )),
        ..ScreenProcessingProfileConfig::default()
    })
    .resolve_with_color_capabilities(
        &hdr_source,
        ScreenColorTransformCapabilities::new(false, false, true, std::num::NonZeroU32::MIN),
    )
    .expect("declared PQ BT.2390 support resolves the exact tone-map contract");
    let ResolvedScreenColorTransform::ToneMap(resolved) =
        descriptor.physical().color_pipeline().transform()
    else {
        panic!("PQ source should resolve the declared tone-map pipeline");
    };
    assert_eq!(resolved.operator(), ScreenToneMapOperator::Bt2390Eetf);
    assert_eq!(resolved.source_luminance(), source_luminance);
    assert_eq!(resolved.target_luminance(), target_luminance);

    let missing_luminance = CaptureColorimetry::new(
        CaptureColorSpace::Rec2020,
        CaptureTransferFunction::Pq,
        Some(CaptureDynamicRange::High),
        None,
    )
    .expect("incomplete backend HDR metadata remains representable");
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig {
            unknown_color: ScreenUnknownColorPolicy::Assume(known_hdr),
            hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::new(
                ScreenToneMapOperator::Bt2390Eetf,
                target_luminance,
            )),
            ..ScreenProcessingProfileConfig::default()
        })
        .resolve(&source(missing_luminance)),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
}

#[test]
fn extended_linear_hdr_resolves_the_reference_white_bt2390_contract() {
    let source_luminance = luminance(203.0, 1_000.0);
    let calibration = LedToneMapCalibration::DEFAULT;
    let source_color = known_hdr(CaptureTransferFunction::Linear, source_luminance);
    let descriptor = native_surface(ScreenProcessingProfileConfig {
        hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
            ScreenToneMapOperator::Bt2390Eetf,
            calibration,
        )),
        ..ScreenProcessingProfileConfig::default()
    })
    .resolve_with_color_capabilities(
        &source(CaptureColorimetry::from_known(source_color)),
        ScreenColorTransformCapabilities::new(false, false, true, LED_TONE_MAP_ALGORITHM_REVISION),
    )
    .expect("extended-linear HDR resolves through the shared BT.2390 contract");
    let ResolvedScreenColorTransform::ToneMap(resolved) =
        descriptor.physical().color_pipeline().transform()
    else {
        panic!("extended-linear HDR resolves a tone-map transform");
    };
    assert_eq!(resolved.source_luminance(), source_luminance);
    assert_eq!(resolved.calibration(), calibration);
}

#[test]
fn hdr_tone_mapping_requires_positive_source_headroom() {
    let calibration = LedToneMapCalibration::DEFAULT;
    let request = native_surface(ScreenProcessingProfileConfig {
        hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
            ScreenToneMapOperator::Bt2390Eetf,
            calibration,
        )),
        ..ScreenProcessingProfileConfig::default()
    });
    let capabilities =
        ScreenColorTransformCapabilities::new(false, false, true, LED_TONE_MAP_ALGORITHM_REVISION);

    for transfer in [CaptureTransferFunction::Pq, CaptureTransferFunction::Linear] {
        let unity = known_hdr(transfer, luminance(203.0, 203.0));
        assert_eq!(
            request.resolve_with_color_capabilities(
                &source(CaptureColorimetry::from_known(unity)),
                capabilities,
            ),
            Err(ScreenPublicationError::UnsupportedHdrConversion)
        );

        let positive = known_hdr(transfer, luminance(203.0, 203.0001));
        assert!(
            request
                .resolve_with_color_capabilities(
                    &source(CaptureColorimetry::from_known(positive)),
                    capabilities,
                )
                .is_ok()
        );
    }

    let sdr = known_sdr(CaptureColorSpace::Srgb, CaptureTransferFunction::Srgb)
        .with_luminance(luminance(203.0, 203.0));
    assert!(
        native_surface(ScreenProcessingProfileConfig::exact_encoded_identity(
            CapturePixelFormat::Rgba8,
        ))
        .resolve(&source(CaptureColorimetry::from_known(sdr)))
        .is_ok()
    );
}

#[test]
fn hdr_passthrough_and_sdr_to_hdr_conversion_remain_unavailable() {
    let hdr = known_hdr(CaptureTransferFunction::Hlg, luminance(203.0, 1_000.0));
    let preserve_hdr = native_surface(ScreenProcessingProfileConfig {
        reduction_filter: ScreenReductionFilter::Nearest,
        target_colorimetry: ScreenTargetColorimetry::PreserveSource,
        ..ScreenProcessingProfileConfig::default()
    });
    assert_eq!(
        preserve_hdr.resolve(&source(CaptureColorimetry::from_known(hdr))),
        Err(ScreenPublicationError::HdrRejected)
    );
    let partial_hdr = CaptureColorimetry::new(
        CaptureColorSpace::Unknown,
        CaptureTransferFunction::Unknown,
        Some(CaptureDynamicRange::High),
        None,
    )
    .expect("incomplete backend HDR metadata remains representable");
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            target_colorimetry: ScreenTargetColorimetry::PreserveSource,
            unknown_color: ScreenUnknownColorPolicy::PreserveEncodedSamples,
            ..ScreenProcessingProfileConfig::default()
        })
        .resolve(&source(partial_hdr)),
        Err(ScreenPublicationError::UnsupportedTargetPixelFormat)
    );

    let target_hdr = native_surface(ScreenProcessingProfileConfig {
        target_colorimetry: ScreenTargetColorimetry::ConvertTo(hdr),
        hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::new(
            ScreenToneMapOperator::Bt2390Eetf,
            luminance(203.0, 1_000.0),
        )),
        ..ScreenProcessingProfileConfig::default()
    });
    assert_eq!(
        target_hdr.resolve(&source(CaptureColorimetry::SRGB)),
        Err(ScreenPublicationError::UnsupportedHdrConversion)
    );
}

#[test]
fn hlg_tone_mapping_requires_an_explicit_system_ootf_contract() {
    let hlg = known_hdr(CaptureTransferFunction::Hlg, luminance(203.0, 1_000.0));
    let tone_map = native_surface(ScreenProcessingProfileConfig {
        hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::new(
            ScreenToneMapOperator::Bt2390Eetf,
            luminance(100.0, 100.0),
        )),
        ..ScreenProcessingProfileConfig::default()
    });

    assert_eq!(
        tone_map.resolve(&source(CaptureColorimetry::from_known(hlg))),
        Err(ScreenPublicationError::UnsupportedHdrConversion)
    );
}

#[test]
fn color_policy_and_resolved_parameters_participate_in_identity_and_ordering() {
    let srgb = known_sdr(CaptureColorSpace::Srgb, CaptureTransferFunction::Srgb);
    let bright_srgb = srgb.with_luminance(luminance(100.0, 200.0));
    let p3 = known_sdr(
        CaptureColorSpace::DisplayP3,
        CaptureTransferFunction::Linear,
    );
    let request = native_surface(ScreenProcessingProfileConfig::exact_encoded_identity(
        CapturePixelFormat::Rgba8,
    ));
    let srgb_descriptor = request
        .resolve(&source(CaptureColorimetry::from_known(srgb)))
        .expect("exact sRGB no-op resolves");
    let bright_descriptor = request
        .resolve(&source(CaptureColorimetry::from_known(bright_srgb)))
        .expect("exact sRGB no-op retains luminance context");
    let p3_descriptor = request
        .resolve(&source(CaptureColorimetry::from_known(p3)))
        .expect("fully-known SDR encodings can remain opaque under exact identity");

    assert_ne!(srgb_descriptor, bright_descriptor);
    assert_ne!(srgb_descriptor, p3_descriptor);
    assert_ne!(
        srgb_descriptor.physical().cmp(bright_descriptor.physical()),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn led_calibration_and_revision_are_stable_physical_cache_identity() {
    let d65 = LedToneMapCalibration::DEFAULT;
    let capabilities =
        ScreenColorTransformCapabilities::new(true, true, true, LED_TONE_MAP_ALGORITHM_REVISION);
    let resolve = |calibration| {
        calibrated_native_surface(
            ScreenProcessingProfileConfig {
                algorithm_revision: LED_TONE_MAP_ALGORITHM_REVISION,
                ..ScreenProcessingProfileConfig::default()
            },
            calibration,
        )
        .resolve_with_color_capabilities(&source(CaptureColorimetry::SRGB), capabilities)
        .expect("managed SDR profile resolves")
    };

    let first = resolve(d65);
    let repeated = resolve(d65);
    assert_eq!(first.physical(), repeated.physical());
    for calibration in [
        LedToneMapCalibration::try_new(0.3128, 0.329, 203.0, 406.0, 0.0),
        LedToneMapCalibration::try_new(0.3127, 0.3291, 203.0, 406.0, 0.0),
        LedToneMapCalibration::try_new(0.3127, 0.329, 202.0, 406.0, 0.0),
        LedToneMapCalibration::try_new(0.3127, 0.329, 203.0, 407.0, 0.0),
        LedToneMapCalibration::try_new(0.3127, 0.329, 203.0, 406.0, 1.0),
    ] {
        let calibration = calibration.expect("alternate calibration is valid");
        assert_ne!(first.physical(), resolve(calibration).physical());
    }
    assert_eq!(first.physical().color_pipeline().calibration(), Some(d65));
    assert_eq!(
        first.physical().algorithm_revision(),
        LED_TONE_MAP_ALGORITHM_REVISION
    );
}

#[test]
fn resolved_hdr_tone_map_carries_the_validated_target_calibration() {
    let calibration = LedToneMapCalibration::try_new(0.3457, 0.3585, 160.0, 480.0, -1.0)
        .expect("measured target calibration is valid");
    let hdr = known_hdr(CaptureTransferFunction::Pq, luminance(203.0, 1_000.0));
    let descriptor = calibrated_native_surface(
        ScreenProcessingProfileConfig {
            hdr: ScreenHdrPolicy::ToneMap(ScreenToneMapPolicy::from_calibration(
                ScreenToneMapOperator::Bt2390Eetf,
                calibration,
            )),
            algorithm_revision: LED_TONE_MAP_ALGORITHM_REVISION,
            ..ScreenProcessingProfileConfig::default()
        },
        calibration,
    )
    .resolve_with_color_capabilities(
        &source(CaptureColorimetry::from_known(hdr)),
        ScreenColorTransformCapabilities::new(true, true, true, LED_TONE_MAP_ALGORITHM_REVISION),
    )
    .expect("PQ HDR pipeline resolves");

    let ResolvedScreenColorTransform::ToneMap(tone_map) =
        descriptor.physical().color_pipeline().transform()
    else {
        panic!("resolved transform must be HDR tone mapping");
    };
    assert_eq!(tone_map.calibration(), calibration);
    assert_eq!(tone_map.target_luminance(), calibration.target_luminance());
    assert_eq!(
        descriptor.physical().color_pipeline().calibration(),
        Some(calibration)
    );
}

#[test]
fn byte_changing_color_paths_fail_closed_without_reducer_capabilities() {
    let p3 = known_sdr(CaptureColorSpace::DisplayP3, CaptureTransferFunction::Srgb);
    let linear = known_sdr(CaptureColorSpace::Srgb, CaptureTransferFunction::Linear);

    assert_eq!(
        native_surface(ScreenProcessingProfileConfig::default())
            .resolve(&source(CaptureColorimetry::SRGB)),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig::default())
            .resolve(&source(CaptureColorimetry::from_known(p3))),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig::default())
            .resolve(&source(CaptureColorimetry::from_known(linear))),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig {
            target_colorimetry: ScreenTargetColorimetry::PreserveSource,
            reduction_filter: ScreenReductionFilter::Area,
            ..ScreenProcessingProfileConfig::default()
        })
        .resolve(&source(CaptureColorimetry::SRGB)),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );

    let same_encoding = native_surface(ScreenProcessingProfileConfig::default())
        .resolve_with_color_capabilities(
            &source(CaptureColorimetry::SRGB),
            ScreenColorTransformCapabilities::new(true, false, false, std::num::NonZeroU32::MIN),
        )
        .expect("declared linear-light SDR processing admits same-encoding work");
    assert!(matches!(
        same_encoding.physical().color_pipeline().transform(),
        ResolvedScreenColorTransform::LinearLightSdr
    ));

    let sdr_with_luminance = known_sdr(CaptureColorSpace::Srgb, CaptureTransferFunction::Srgb)
        .with_luminance(luminance(80.0, 120.0));
    let same_encoding_with_luminance = native_surface(ScreenProcessingProfileConfig::default())
        .resolve_with_color_capabilities(
            &source(CaptureColorimetry::from_known(sdr_with_luminance)),
            ScreenColorTransformCapabilities::new(true, false, false, std::num::NonZeroU32::MIN),
        )
        .expect("SDR luminance metadata does not imply an encoding conversion");
    assert!(matches!(
        same_encoding_with_luminance
            .physical()
            .color_pipeline()
            .transform(),
        ResolvedScreenColorTransform::LinearLightSdr
    ));

    native_surface(ScreenProcessingProfileConfig::default())
        .resolve_with_color_capabilities(
            &source(CaptureColorimetry::from_known(p3)),
            ScreenColorTransformCapabilities::new(false, true, false, std::num::NonZeroU32::MIN),
        )
        .expect("declared relative SDR conversion admits a differing target");
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig::default()).resolve_with_color_capabilities(
            &source(CaptureColorimetry::from_known(linear)),
            ScreenColorTransformCapabilities::new(true, false, false, std::num::NonZeroU32::MIN,),
        ),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
    assert_eq!(
        native_surface(ScreenProcessingProfileConfig::default()).resolve_with_color_capabilities(
            &source(CaptureColorimetry::SRGB),
            ScreenColorTransformCapabilities::new(
                true,
                true,
                true,
                std::num::NonZeroU32::new(2).expect("test revision is non-zero"),
            ),
        ),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
}

#[test]
fn color_admission_is_independent_of_finite_resolution() {
    for (width, height) in [
        (1, 1),
        (1919, 1079),
        (5120, 720),
        (1920, 2160),
        (7680, 4320),
    ] {
        let source_extent = extent(width, height);
        let source = source_with_storage(
            CaptureColorimetry::SRGB,
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                source_extent,
                source_extent,
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("test geometry is valid"),
            source_extent,
            ScreenSourceReflection::None,
        );
        native_surface(ScreenProcessingProfileConfig {
            reduction_filter: ScreenReductionFilter::Nearest,
            target_colorimetry: ScreenTargetColorimetry::PreserveSource,
            ..ScreenProcessingProfileConfig::default()
        })
        .resolve(&source)
        .expect("exact sRGB no-op has no dimensional ceiling");
    }
}
