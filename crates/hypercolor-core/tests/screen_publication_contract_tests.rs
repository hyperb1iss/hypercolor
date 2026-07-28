//! Cursor capability and output-color identity contracts for screen publications.

use std::sync::Arc;

use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureColorimetry, CaptureDynamicRange, CaptureEpoch, CaptureGeometry,
    CapturePixelFormat, CaptureRotation, CaptureSourceId, CaptureTransferFunction,
    KnownCaptureColorimetry, PhysicalOrigin, PixelExtent, RegisteredScreenBranchDemand,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenCursorCapabilities,
    ScreenCursorPolicy, ScreenExtentRequest, ScreenProcessingProfile,
    ScreenProcessingProfileConfig, ScreenPublicationError, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenResourceApi, ScreenSourceReflection, ScreenSourceSelector,
    ScreenTargetColorimetry, SourceScale,
};

fn source(capabilities: ScreenCursorCapabilities) -> ResolvedScreenSource {
    let source_id =
        CaptureSourceId::new("synthetic:cursor-contract").expect("test source id is non-empty");
    let extent = PixelExtent::new(3840, 2160).expect("test extent is non-empty");
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id,
            topology_generation: 2,
            session_generation: 3,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Bgra8,
            CaptureColorimetry::SRGB,
            capabilities,
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                4,
                5,
            ),
        ),
    )
}

fn request(profile: ScreenProcessingProfile) -> ScreenPublicationRequest {
    ScreenPublicationRequest::new(
        ScreenSourceSelector::Configured,
        ScreenPublicationKind::Surface,
        ScreenExtentRequest::Native,
        ScreenAspectPolicy::Contain,
        Arc::new(profile),
    )
}

#[test]
fn cursor_policy_rejects_impossible_storage_before_transform_admission() {
    let exclude = request(ScreenProcessingProfile::new(
        ScreenProcessingProfileConfig::exact_encoded_identity(CapturePixelFormat::Bgra8),
    ));
    assert_eq!(
        exclude.resolve(&source(ScreenCursorCapabilities::composed_only())),
        Err(ScreenPublicationError::CursorExclusionUnsupported)
    );

    let include = request(ScreenProcessingProfile::new(
        ScreenProcessingProfileConfig {
            cursor: ScreenCursorPolicy::Include,
            ..ScreenProcessingProfileConfig::default()
        },
    ));
    assert_eq!(
        include.resolve(&source(ScreenCursorCapabilities::clean_only())),
        Err(ScreenPublicationError::CursorInclusionUnsupported)
    );
    assert_eq!(
        include.resolve(&source(
            ScreenCursorCapabilities::clean_with_separate_cursor(),
        )),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
    assert_eq!(
        include.resolve(&source(ScreenCursorCapabilities::composed_only())),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
}

#[test]
fn target_transfer_function_participates_in_profile_identity_before_admission() {
    let srgb_profile = ScreenProcessingProfile::default();
    let linear = KnownCaptureColorimetry::try_new(
        CaptureColorSpace::Srgb,
        CaptureTransferFunction::Linear,
        CaptureDynamicRange::Standard,
        None,
    )
    .expect("test target is complete");
    let linear_profile = ScreenProcessingProfile::new(ScreenProcessingProfileConfig {
        target_colorimetry: ScreenTargetColorimetry::ConvertTo(linear),
        ..ScreenProcessingProfileConfig::default()
    });
    assert_ne!(srgb_profile, linear_profile);
    assert_ne!(srgb_profile.cmp(&linear_profile), std::cmp::Ordering::Equal);

    let source = source(ScreenCursorCapabilities::clean_only());
    assert_eq!(
        request(srgb_profile).resolve(&source),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
    assert_eq!(
        request(linear_profile).resolve(&source),
        Err(ScreenPublicationError::UnsupportedColorTransform)
    );
}

#[test]
fn source_cursor_capabilities_participate_in_source_identity() {
    let clean = source(ScreenCursorCapabilities::clean_only());
    let separate = source(ScreenCursorCapabilities::clean_with_separate_cursor());

    assert_ne!(clean.config(), separate.config());
    assert!(clean.config().cursor_capabilities().has_clean_surface());
    assert!(
        separate
            .config()
            .cursor_capabilities()
            .has_separate_cursor()
    );
}

#[test]
fn branch_demand_propagates_cursor_admission_failures() {
    let demand = RegisteredScreenBranchDemand::new(
        request(ScreenProcessingProfile::new(
            ScreenProcessingProfileConfig {
                cursor: ScreenCursorPolicy::Include,
                ..ScreenProcessingProfileConfig::default()
            },
        )),
        std::num::NonZeroU32::MIN,
    );

    assert_eq!(
        demand.resolve(&source(ScreenCursorCapabilities::clean_only())),
        Err(ScreenPublicationError::CursorInclusionUnsupported)
    );
}
