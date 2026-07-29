use std::num::{NonZeroU32, NonZeroU64};
use std::time::Duration;

use hypercolor_windows_capture::{
    CaptureError, CaptureExtent, CaptureRegion, DisplayRotation, GpuSurfaceAdmission,
    GpuSurfaceColorPipeline, GpuSurfaceCoordinateSpace, GpuSurfaceCursorPolicy,
    GpuSurfaceDescriptor, GpuSurfaceDescriptorConfig, GpuSurfaceDescriptorId, GpuSurfaceFilter,
    GpuSurfaceFormat, GpuSurfaceSourceColorSpace, GpuSurfaceUnsupportedReason,
};

fn descriptor_config(id: u64, output_extent: CaptureExtent) -> GpuSurfaceDescriptorConfig {
    GpuSurfaceDescriptorConfig {
        id: GpuSurfaceDescriptorId::new(NonZeroU64::new(id).expect("fixture id is non-zero")),
        source_region: CaptureRegion::new(0, 0, 8, 4).expect("fixture region is non-empty"),
        coordinate_space: GpuSurfaceCoordinateSpace::LogicalDisplay,
        source_rotation: DisplayRotation::Identity,
        source_color_space: GpuSurfaceSourceColorSpace::RgbFullG22P709,
        output_extent,
        filter: GpuSurfaceFilter::Nearest,
        format: GpuSurfaceFormat::Rgba8Unorm,
        color_pipeline: GpuSurfaceColorPipeline::PreserveEncoded,
        cursor: GpuSurfaceCursorPolicy::Exclude,
        algorithm_revision: NonZeroU32::new(1).expect("fixture revision is non-zero"),
        freshness: Duration::from_millis(250),
    }
}

fn descriptor(
    id: u64,
    output_extent: CaptureExtent,
    filter: GpuSurfaceFilter,
    format: GpuSurfaceFormat,
    color_pipeline: GpuSurfaceColorPipeline,
) -> GpuSurfaceDescriptor {
    GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
        filter,
        format,
        color_pipeline,
        ..descriptor_config(id, output_extent)
    })
}

#[test]
fn admission_accounts_only_candidate_inflight_slots() {
    let source = CaptureExtent::try_new(8, 4).expect("source extent is valid");
    let output = CaptureExtent::try_new(4, 2).expect("output extent is valid");
    let descriptor = descriptor(
        1,
        output,
        GpuSurfaceFilter::Nearest,
        GpuSurfaceFormat::Rgba8Unorm,
        GpuSurfaceColorPipeline::PreserveEncoded,
    );
    let admission =
        GpuSurfaceAdmission::new(96, NonZeroU32::new(3).expect("three slots is non-zero"));

    assert_eq!(
        admission
            .admit(source, &[descriptor])
            .expect("exact texture ledger is admitted"),
        96
    );
}

#[test]
fn admission_has_no_artificial_resolution_axis_cap() {
    let source = CaptureExtent::try_new(15_360, 8_640).expect("16K extent is non-empty");
    let descriptor = descriptor(
        1,
        source,
        GpuSurfaceFilter::Nearest,
        GpuSurfaceFormat::Rgba8Unorm,
        GpuSurfaceColorPipeline::PreserveEncoded,
    );
    let admission =
        GpuSurfaceAdmission::new(u64::MAX, NonZeroU32::new(2).expect("two slots is non-zero"));

    assert_eq!(
        admission
            .admit(source, &[descriptor])
            .expect("large descriptor remains resource-admitted"),
        1_061_683_200
    );
}

#[test]
fn admission_rejects_checked_geometry_overflow_before_backend_allocation() {
    let extent = CaptureExtent::try_new(u32::MAX, u32::MAX).expect("extent is non-empty");
    let descriptor = descriptor(
        1,
        extent,
        GpuSurfaceFilter::Nearest,
        GpuSurfaceFormat::Rgba8Unorm,
        GpuSurfaceColorPipeline::PreserveEncoded,
    );
    let admission = GpuSurfaceAdmission::new(
        u64::MAX,
        NonZeroU32::new(2).expect("two slots are non-zero"),
    );

    assert!(matches!(
        admission.admit(extent, &[descriptor]),
        Err(CaptureError::GeometryOverflow {
            operation: "account GPU Surface texture",
            width: u32::MAX,
            height: u32::MAX,
        })
    ));
}

#[test]
fn admission_rejects_budget_pressure_without_resizing_the_descriptor() {
    let source = CaptureExtent::try_new(8, 4).expect("source extent is valid");
    let output = CaptureExtent::try_new(4, 2).expect("output extent is valid");
    let descriptor = descriptor(
        7,
        output,
        GpuSurfaceFilter::Nearest,
        GpuSurfaceFormat::Rgba8Unorm,
        GpuSurfaceColorPipeline::PreserveEncoded,
    );
    let admission =
        GpuSurfaceAdmission::new(95, NonZeroU32::new(3).expect("three slots is non-zero"));

    assert!(matches!(
        admission.admit(source, &[descriptor]),
        Err(CaptureError::GpuSurfaceBudgetExceeded {
            requested_bytes: 96,
            budget_bytes: 95,
        })
    ));
}

#[test]
fn admission_requires_two_independently_owned_native_slots() {
    let source = CaptureExtent::try_new(8, 4).expect("source extent is valid");
    let descriptor = descriptor(
        8,
        CaptureExtent::try_new(4, 2).expect("output extent is valid"),
        GpuSurfaceFilter::Nearest,
        GpuSurfaceFormat::Rgba8Unorm,
        GpuSurfaceColorPipeline::PreserveEncoded,
    );
    let admission =
        GpuSurfaceAdmission::new(u64::MAX, NonZeroU32::new(1).expect("one is non-zero"));

    assert!(matches!(
        admission.admit(source, &[descriptor]),
        Err(CaptureError::GpuSurfaceInFlightDepthTooSmall {
            requested: 1,
            minimum: 2,
        })
    ));
}

#[test]
fn unsupported_exact_semantics_return_typed_reasons() {
    let source = CaptureExtent::try_new(8, 4).expect("source extent is valid");
    let output = CaptureExtent::try_new(4, 2).expect("output extent is valid");
    let admission = GpuSurfaceAdmission::new(
        u64::MAX,
        NonZeroU32::new(2).expect("two slots are non-zero"),
    );
    let cases = [
        (
            descriptor(
                1,
                output,
                GpuSurfaceFilter::Bilinear,
                GpuSurfaceFormat::Rgba8Unorm,
                GpuSurfaceColorPipeline::PreserveEncoded,
            ),
            GpuSurfaceUnsupportedReason::Filter(GpuSurfaceFilter::Bilinear),
        ),
        (
            descriptor(
                2,
                output,
                GpuSurfaceFilter::Nearest,
                GpuSurfaceFormat::Rgba16Float,
                GpuSurfaceColorPipeline::PreserveEncoded,
            ),
            GpuSurfaceUnsupportedReason::OutputFormat(GpuSurfaceFormat::Rgba16Float),
        ),
        (
            descriptor(
                3,
                output,
                GpuSurfaceFilter::Nearest,
                GpuSurfaceFormat::Rgba8Unorm,
                GpuSurfaceColorPipeline::LinearSdr,
            ),
            GpuSurfaceUnsupportedReason::ColorPipeline(GpuSurfaceColorPipeline::LinearSdr),
        ),
        (
            GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
                coordinate_space: GpuSurfaceCoordinateSpace::NativeScanout,
                ..descriptor_config(4, output)
            }),
            GpuSurfaceUnsupportedReason::CoordinateSpace(GpuSurfaceCoordinateSpace::NativeScanout),
        ),
        (
            GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
                source_color_space: GpuSurfaceSourceColorSpace::RgbFullPqP2020,
                ..descriptor_config(5, output)
            }),
            GpuSurfaceUnsupportedReason::SourceColorSpace(
                GpuSurfaceSourceColorSpace::RgbFullPqP2020,
            ),
        ),
        (
            GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
                source_color_space: GpuSurfaceSourceColorSpace::Unknown,
                ..descriptor_config(6, output)
            }),
            GpuSurfaceUnsupportedReason::SourceColorSpace(GpuSurfaceSourceColorSpace::Unknown),
        ),
    ];

    for (descriptor, expected) in cases {
        assert!(matches!(
            admission.admit(source, std::slice::from_ref(&descriptor)),
            Err(CaptureError::UnsupportedGpuSurface {
                descriptor_id,
                reason,
            }) if descriptor_id == descriptor.id() && reason == expected
        ));
    }

    let revision = NonZeroU32::new(2).expect("second revision is non-zero");
    let descriptor = GpuSurfaceDescriptor::new(GpuSurfaceDescriptorConfig {
        id: GpuSurfaceDescriptorId::new(NonZeroU64::new(4).expect("fixture id is non-zero")),
        source_region: CaptureRegion::new(0, 0, 8, 4).expect("fixture region is non-empty"),
        coordinate_space: GpuSurfaceCoordinateSpace::LogicalDisplay,
        source_rotation: DisplayRotation::Identity,
        source_color_space: GpuSurfaceSourceColorSpace::RgbFullG22P709,
        output_extent: output,
        filter: GpuSurfaceFilter::Nearest,
        format: GpuSurfaceFormat::Rgba8Unorm,
        color_pipeline: GpuSurfaceColorPipeline::PreserveEncoded,
        cursor: GpuSurfaceCursorPolicy::Exclude,
        algorithm_revision: revision,
        freshness: Duration::from_millis(250),
    });
    assert!(matches!(
        admission.admit(source, std::slice::from_ref(&descriptor)),
        Err(CaptureError::UnsupportedGpuSurface {
            descriptor_id,
            reason: GpuSurfaceUnsupportedReason::AlgorithmRevision(observed),
        }) if descriptor_id == descriptor.id() && observed == revision
    ));
}

#[test]
fn source_region_admission_is_typed_and_descriptor_specific() {
    let source = CaptureExtent::try_new(4, 4).expect("source extent is valid");
    let descriptor = descriptor(
        31,
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
        GpuSurfaceFilter::Nearest,
        GpuSurfaceFormat::Rgba8Unorm,
        GpuSurfaceColorPipeline::PreserveEncoded,
    );
    let admission = GpuSurfaceAdmission::new(
        u64::MAX,
        NonZeroU32::new(2).expect("two slots are non-zero"),
    );

    assert!(matches!(
        admission.admit(source, &[descriptor]),
        Err(CaptureError::GpuSurfaceRegionOutOfBounds {
            descriptor_id,
            source_width: 4,
            source_height: 4,
        }) if descriptor_id.get() == 31
    ));
}

#[test]
fn duplicate_descriptor_identity_is_rejected_even_when_geometry_differs() {
    let source = CaptureExtent::try_new(8, 4).expect("source extent is valid");
    let first = descriptor(
        9,
        CaptureExtent::try_new(4, 2).expect("first output is valid"),
        GpuSurfaceFilter::Nearest,
        GpuSurfaceFormat::Rgba8Unorm,
        GpuSurfaceColorPipeline::PreserveEncoded,
    );
    let second = descriptor(
        9,
        CaptureExtent::try_new(2, 4).expect("second output is valid"),
        GpuSurfaceFilter::Nearest,
        GpuSurfaceFormat::Rgba8Unorm,
        GpuSurfaceColorPipeline::PreserveEncoded,
    );
    let admission = GpuSurfaceAdmission::new(
        u64::MAX,
        NonZeroU32::new(2).expect("two slots are non-zero"),
    );

    assert!(matches!(
        admission.admit(source, &[first, second]),
        Err(CaptureError::DuplicateGpuSurfaceDescriptor { descriptor_id })
            if descriptor_id.get() == 9
    ));
}
