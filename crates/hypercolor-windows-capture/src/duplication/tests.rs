use super::{
    CaptureMetadata, DesktopFrameSource, PointerShape, PointerShapeKind, PointerState,
    TopologyEntry, TopologyState, average_channel, capture_region_origin, classify_hresult,
    desktop_frame_source, gpu_surface_acquire_timeout, logical_to_scanout, native_scanout_extent,
    pointer_scanout_geometry, prepare_duplication, scanout_to_logical, session_rebuild_error,
};
use crate::{CaptureError, CaptureExtent, CaptureRegion, DisplayRotation, ReductionPath};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL};
use windows::Win32::Graphics::Direct3D11::D3D11_ASYNC_GETDATA_DONOTFLUSH;
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_SESSION_DISCONNECTED, DXGI_ERROR_WAIT_TIMEOUT,
};

fn topology_entry(id: &str, origin_x: i32) -> TopologyEntry {
    TopologyEntry {
        id: id.to_owned(),
        origin_x,
        origin_y: 0,
        width: 1920,
        height: 1080,
        primary: origin_x == 0,
        rotation: DisplayRotation::Identity,
    }
}

#[test]
fn d3d11on12_bridges_a_keyed_surface_into_d3d12() {
    super::gpu_surface::fixture::d3d11on12_bridges_a_keyed_surface_into_d3d12()
        .expect("D3D11On12 should bridge the shared capture Surface into D3D12");
}

#[derive(Debug)]
struct DropSignal(Rc<Cell<u32>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

fn pointer(kind: PointerShapeKind, bytes: Vec<u8>) -> PointerState {
    PointerState {
        visible: true,
        position_x: 0,
        position_y: 0,
        shape: Some(PointerShape {
            kind,
            width: 1,
            height: if kind == PointerShapeKind::Monochrome {
                2
            } else {
                1
            },
            pitch: if kind == PointerShapeKind::Monochrome {
                1
            } else {
                4
            },
            hotspot_x: 0,
            hotspot_y: 0,
            bytes,
        }),
        shape_generation: 1,
    }
}

fn cpu_reduce(
    bgra: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
    pointer: &PointerState,
    rotation: DisplayRotation,
) -> Vec<u8> {
    cpu_reduce_region(
        bgra,
        width,
        height,
        max_width,
        pointer,
        rotation,
        CaptureRegion::full(width, height),
    )
}

#[allow(clippy::too_many_arguments)]
fn cpu_reduce_region(
    bgra: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
    pointer: &PointerState,
    rotation: DisplayRotation,
    region: CaptureRegion,
) -> Vec<u8> {
    let mut rgba = Vec::new();
    super::DesktopDuplicator::copy_bgra_rows(
        super::BgraRows {
            bytes: bgra,
            row_pitch: width as usize * 4,
            width,
            height,
        },
        &mut rgba,
        CaptureExtent::try_new(max_width, u32::MAX).expect("test extent"),
        pointer,
        rotation,
        region,
    )
    .expect("fixture rows are valid");
    rgba
}

fn assert_gpu_parity(
    bgra: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
    pointer: &PointerState,
    rotation: DisplayRotation,
) {
    let cpu = cpu_reduce(bgra, width, height, max_width, pointer, rotation);
    let gpu =
        super::gpu_reduction::reduce_fixture(bgra, width, height, max_width, pointer, rotation)
            .expect("WARP compute reduction succeeds");
    assert_eq!(gpu.len(), cpu.len());
    for (index, (gpu, cpu)) in gpu.iter().zip(&cpu).enumerate() {
        assert!(
            gpu.abs_diff(*cpu) <= 1,
            "channel {index} differs: GPU={gpu}, CPU={cpu}"
        );
    }
}

#[test]
fn embedded_capture_shaders_compile_with_the_system_compiler() {
    super::gpu_reduction::compile_shaders_for_test().expect("both compute entries compile");
}

#[test]
fn shader_creation_failure_selects_explicit_degraded_cpu_telemetry() {
    assert!(
        super::gpu_reduction::invalid_shader_is_rejected_for_test()
            .expect("WARP test device opens")
    );
    let telemetry = super::fallback_reduction_telemetry("synthetic shader failure".to_owned());
    assert_eq!(telemetry.path, ReductionPath::CpuFallback);
    assert_eq!(telemetry.gpu_failures, 1);
    assert_eq!(telemetry.issue.as_deref(), Some("synthetic shader failure"));
}

#[test]
fn gpu_readback_ring_coalesces_pressure_at_fixed_capacity() {
    let (pending, busy) = super::gpu_reduction::ring_pressure_is_bounded_for_test()
        .expect("WARP ring pressure fixture succeeds");

    assert_eq!(pending, 3);
    assert!(busy);
}

#[test]
fn exact_gpu_surfaces_fan_out_incompatible_descriptors_from_one_source() {
    let bgra = [
        1, 2, 3, 0xFF, 11, 12, 13, 0xFF, 21, 22, 23, 0xFF, 31, 32, 33, 0xFF, 101, 102, 103, 0xFF,
        111, 112, 113, 0xFF, 121, 122, 123, 0xFF, 131, 132, 133, 0xFF,
    ];
    let region = CaptureRegion::full(4, 2);
    let descriptors = [
        super::gpu_surface::fixture::descriptor(
            1,
            region,
            CaptureExtent::try_new(3, 1).expect("landscape extent is valid"),
        ),
        super::gpu_surface::fixture::descriptor(
            2,
            region,
            CaptureExtent::try_new(2, 3).expect("portrait extent is valid"),
        ),
    ];
    let fixture = super::gpu_surface::fixture::publish(&bgra, 4, 2, &descriptors)
        .expect("WARP exact Surface fanout succeeds");

    assert_eq!(fixture.info.source_sequence(), 41);
    assert_eq!(fixture.info.published(), 2);
    assert_eq!(fixture.outcomes.len(), 2);
    assert_eq!(fixture.plan.descriptors().count(), 2);
    assert_eq!(fixture.plan.readback_byte_len(), 0);
    assert_eq!(fixture.plan.publication_buffer_byte_len(), 0);
    let publications = fixture
        .outcomes
        .iter()
        .map(|outcome| match outcome {
            crate::GpuSurfacePublishOutcome::Published(publication) => publication.clone(),
            crate::GpuSurfacePublishOutcome::Busy(id) => {
                panic!("fresh descriptor {} unexpectedly busy", id.get())
            }
        })
        .collect::<Vec<_>>();
    assert_ne!(
        super::gpu_surface::fixture::texture_handle(&publications[0]),
        super::gpu_surface::fixture::texture_handle(&publications[1]),
        "incompatible descriptors own independent resources"
    );

    let landscape =
        super::gpu_surface::fixture::readback_and_release(&fixture.plan, &publications[0])
            .expect("landscape Surface reads back after its ready fence");
    let portrait =
        super::gpu_surface::fixture::readback_and_release(&fixture.plan, &publications[1])
            .expect("portrait Surface reads back after its ready fence");
    assert_eq!(
        landscape,
        [
            103, 102, 101, 0xFF, 123, 122, 121, 0xFF, 133, 132, 131, 0xFF,
        ]
    );
    assert_eq!(
        portrait,
        [
            13, 12, 11, 0xFF, 33, 32, 31, 0xFF, 113, 112, 111, 0xFF, 133, 132, 131, 0xFF, 113, 112,
            111, 0xFF, 133, 132, 131, 0xFF,
        ]
    );
}

#[test]
fn exact_gpu_surfaces_normalize_every_display_rotation() {
    let bgra = [
        0, 0, 1, 0xFF, 0, 0, 2, 0xFF, 0, 0, 3, 0xFF, 0, 0, 4, 0xFF, 0, 0, 5, 0xFF, 0, 0, 6, 0xFF,
    ];
    let cases = [
        (DisplayRotation::Identity, 2, 3, [1, 2, 3, 4, 5, 6]),
        (DisplayRotation::Clockwise90, 3, 2, [5, 3, 1, 6, 4, 2]),
        (DisplayRotation::Clockwise180, 2, 3, [6, 5, 4, 3, 2, 1]),
        (DisplayRotation::Clockwise270, 3, 2, [2, 4, 6, 1, 3, 5]),
    ];

    for (index, (rotation, logical_width, logical_height, expected_red)) in
        cases.into_iter().enumerate()
    {
        let descriptor = super::gpu_surface::fixture::descriptor_for_rotation(
            u64::try_from(index + 1).expect("fixture id fits u64"),
            CaptureRegion::full(logical_width, logical_height),
            CaptureExtent::try_new(logical_width, logical_height).expect("logical extent is valid"),
            rotation,
        );
        let fixture = super::gpu_surface::fixture::publish_rotated(
            &bgra,
            2,
            3,
            rotation,
            std::slice::from_ref(&descriptor),
        )
        .expect("WARP exact Surface rotation succeeds");
        let crate::GpuSurfacePublishOutcome::Published(publication) = &fixture.outcomes[0] else {
            panic!("fresh rotated descriptor unexpectedly busy");
        };
        assert_eq!(
            publication.provenance().native_source_extent,
            CaptureExtent::try_new(2, 3).expect("native extent is valid")
        );
        assert_eq!(
            publication.provenance().logical_source_extent,
            descriptor.output_extent()
        );
        assert_eq!(
            publication.provenance().pending_rotation,
            DisplayRotation::Identity
        );

        let rgba = super::gpu_surface::fixture::readback_and_release(&fixture.plan, publication)
            .expect("rotated Surface reads back after release");
        let observed_red = rgba
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>();
        assert_eq!(observed_red, expected_red);
    }
}

#[test]
fn exact_cursor_include_rejects_visible_pointer_without_shape_pixels() {
    let descriptor = super::gpu_surface::fixture::descriptor_with_cursor(
        21,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
        DisplayRotation::Identity,
        crate::GpuSurfaceCursorPolicy::Include,
        std::time::Duration::from_secs(1),
    );
    let pointer = PointerState {
        visible: true,
        ..PointerState::default()
    };

    assert!(matches!(
        super::gpu_surface::fixture::publish_with_pointer(
            &[1, 2, 3, 0xFF],
            1,
            1,
            std::slice::from_ref(&descriptor),
            pointer,
        ),
        Err(CaptureError::GpuSurfaceCursorShapeUnavailable {
            descriptor_id,
            source_sequence: 41,
        }) if descriptor_id == descriptor.id()
    ));
}

#[test]
fn exact_cursor_include_composes_available_shape_pixels() {
    let descriptor = super::gpu_surface::fixture::descriptor_with_cursor(
        22,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
        DisplayRotation::Identity,
        crate::GpuSurfaceCursorPolicy::Include,
        std::time::Duration::from_secs(1),
    );
    let pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 0xFF]);
    let fixture = super::gpu_surface::fixture::publish_with_pointer(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
        pointer,
    )
    .expect("cursor-including Surface publication succeeds");
    let crate::GpuSurfacePublishOutcome::Published(publication) = &fixture.outcomes[0] else {
        panic!("cursor-including descriptor unexpectedly busy");
    };
    assert!(publication.provenance().cursor_composed);
    assert_eq!(
        super::gpu_surface::fixture::readback_and_release(&fixture.plan, publication)
            .expect("cursor-including Surface reads back"),
        [50, 100, 200, 0xFF]
    );
}

#[test]
fn exact_gpu_surface_allows_only_one_native_claim() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        23,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("WARP exact Surface publication succeeds");
    let crate::GpuSurfacePublishOutcome::Published(publication) = &fixture.outcomes[0] else {
        panic!("fresh descriptor unexpectedly busy");
    };
    let lease = publication.claim().expect("first claim succeeds");

    assert!(matches!(
        publication.claim(),
        Err(CaptureError::GpuSurfaceUseUnavailable {
            descriptor_id,
            source_sequence: 41,
        }) if descriptor_id == descriptor.id()
    ));
    super::gpu_surface::fixture::release_lease_on_producer_device(&fixture.plan, lease)
        .expect("sole claimant releases the synchronized slot");
}

#[test]
fn release_marker_without_release_fence_cannot_unlock_a_native_slot() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        25,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let mut fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("first WARP exact Surface publication succeeds");
    let crate::GpuSurfacePublishOutcome::Published(first) = fixture.outcomes.remove(0) else {
        panic!("fresh descriptor unexpectedly busy");
    };
    let first_handle = super::gpu_surface::fixture::texture_handle(&first);
    super::gpu_surface::fixture::release_key_without_fence(&first)
        .expect("consumer key release is recorded without a fence signal");
    drop(first);

    let mut second_outcomes = super::gpu_surface::fixture::republish(&mut fixture, 42)
        .expect("independent sibling remains publishable");
    let crate::GpuSurfacePublishOutcome::Published(second) = second_outcomes.remove(0) else {
        panic!("sibling descriptor unexpectedly busy");
    };
    assert_ne!(
        first_handle,
        super::gpu_surface::fixture::texture_handle(&second)
    );

    let third_outcomes = super::gpu_surface::fixture::republish(&mut fixture, 43)
        .expect("unsignaled release fence remains a normal busy state");
    assert!(matches!(
        third_outcomes.as_slice(),
        [crate::GpuSurfacePublishOutcome::Busy(id)] if *id == descriptor.id()
    ));
}

#[test]
fn expired_exact_gpu_surface_cannot_be_claimed() {
    let descriptor = super::gpu_surface::fixture::descriptor_with_freshness(
        24,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
        DisplayRotation::Identity,
        std::time::Duration::ZERO,
    );
    let fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("WARP exact Surface publication succeeds");
    let crate::GpuSurfacePublishOutcome::Published(publication) = &fixture.outcomes[0] else {
        panic!("fresh descriptor unexpectedly busy");
    };

    assert!(matches!(
        publication.claim(),
        Err(CaptureError::GpuSurfaceUseUnavailable {
            descriptor_id,
            source_sequence: 41,
        }) if descriptor_id == descriptor.id()
    ));
}

#[test]
fn abandoned_exact_gpu_surfaces_reclaim_under_sustained_pressure() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        29,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let mut fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("first WARP exact Surface publication succeeds");
    fixture.outcomes.clear();

    for sequence in 42..74 {
        let publication = (0..64)
            .find_map(|_| {
                let outcomes = super::gpu_surface::fixture::republish(&mut fixture, sequence)
                    .expect("abandoned slots remain safely reclaimable");
                let publication = outcomes.into_iter().find_map(|outcome| match outcome {
                    crate::GpuSurfacePublishOutcome::Published(publication) => Some(publication),
                    crate::GpuSurfacePublishOutcome::Busy(_) => None,
                });
                if publication.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                publication
            })
            .unwrap_or_else(|| {
                panic!(
                    "two native slots sustain no-reader publication pressure: {:?}",
                    super::gpu_surface::fixture::slot_diagnostics(&fixture.plan)
                )
            });
        drop(publication);
    }
}

#[test]
fn pre_acquire_abandon_returns_publication_to_unclaimed() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        31,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("WARP exact Surface publication succeeds");
    let crate::GpuSurfacePublishOutcome::Published(publication) = &fixture.outcomes[0] else {
        panic!("fresh descriptor unexpectedly busy");
    };
    publication
        .claim()
        .expect("publication is claimable")
        .abandon_before_acquire()
        .expect("unacquired reservation returns without poisoning");
    publication
        .claim()
        .expect("abandoned publication remains claimable")
        .abandon_before_acquire()
        .expect("second reservation also abandons cleanly");
    drop(publication.claim().expect("third reservation is claimable"));
    publication
        .claim()
        .expect("dropping an unacquired reservation also restores the claim")
        .abandon_before_acquire()
        .expect("restored reservation abandons cleanly");
}

#[test]
fn dropped_native_owner_poisoning_is_reported_before_slot_reuse() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        32,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let mut fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("WARP exact Surface publication succeeds");
    let crate::GpuSurfacePublishOutcome::Published(publication) = fixture.outcomes.remove(0) else {
        panic!("fresh descriptor unexpectedly busy");
    };
    super::gpu_surface::fixture::acquire_without_release(&publication)
        .expect("fixture acquires the native consumer key");
    drop(publication);

    assert!(matches!(
        fixture.plan.reclaim_abandoned(),
        Err(CaptureError::GpuSurfacePlanPoisoned {
            descriptor_id,
            use_id: 1,
        }) if descriptor_id == descriptor.id()
    ));
}

#[test]
fn exact_gpu_surface_result_captures_stable_identity_and_provenance() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        77,
        CaptureRegion::full(2, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF, 4, 5, 6, 0xFF],
        2,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("WARP exact Surface publication succeeds");
    let crate::GpuSurfacePublishOutcome::Published(publication) = &fixture.outcomes[0] else {
        panic!("fresh descriptor unexpectedly busy");
    };
    let provenance = publication.provenance();

    assert_eq!(provenance.descriptor.as_ref(), &descriptor);
    assert_eq!(provenance.descriptor.id().get(), 77);
    assert_eq!(provenance.plan_generation.get(), 7);
    assert_eq!(provenance.slot_id.get(), 1);
    assert_eq!(provenance.use_id, 1);
    assert_ne!(
        (
            provenance.adapter_luid.low_part(),
            provenance.adapter_luid.high_part()
        ),
        (0, 0)
    );
    assert_eq!(provenance.source_id.as_ref(), "fixture:display");
    assert_eq!(provenance.topology_generation, 3);
    assert_eq!(provenance.duplication_generation, 5);
    assert_eq!(provenance.source_sequence, 41);
    assert_eq!(
        provenance.native_source_extent,
        CaptureExtent::try_new(2, 1).expect("source extent is valid")
    );
    assert_eq!(
        provenance.logical_source_extent,
        provenance.native_source_extent
    );
    assert_eq!(
        provenance.coordinate_space,
        crate::GpuSurfaceCoordinateSpace::LogicalDisplay
    );
    assert_eq!(provenance.output_extent, descriptor.output_extent());
    assert_eq!(
        provenance.source_format,
        crate::GpuSurfaceFormat::Bgra8Unorm
    );
    assert_eq!(
        provenance.output_format,
        crate::GpuSurfaceFormat::Rgba8Unorm
    );
    assert_eq!(
        provenance.color_pipeline,
        crate::GpuSurfaceColorPipeline::PreserveEncoded
    );
    assert!(provenance.published_at >= provenance.captured_at);
    assert!(provenance.freshness_deadline > provenance.captured_at);
    let lease = publication
        .claim()
        .expect("fresh publication has one claimant");
    assert_ne!(lease.texture_handle().as_raw(), 0);
    assert_ne!(lease.fence_handle().as_raw(), 0);
    let synchronization = lease.synchronization();
    assert_eq!(synchronization.producer_acquire_key, 0);
    assert_eq!(synchronization.producer_release_key, 1);
    assert_eq!(synchronization.consumer_acquire_key, 1);
    assert_eq!(synchronization.consumer_release_key, 0);
    assert_eq!(synchronization.producer_ready_value, 1);
    assert_eq!(synchronization.consumer_release_value, 2);

    super::gpu_surface::fixture::release_lease_on_producer_device(&fixture.plan, lease)
        .expect("fixture consumer releases the synchronized slot");
}

#[test]
fn exact_gpu_surface_lease_keeps_handles_alive_after_plan_retirement() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        5,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("WARP exact Surface publication succeeds");

    assert!(
        super::gpu_surface::fixture::handles_survive_plan_drop(fixture)
            .expect("lease-owned shared handles remain open")
    );
}

#[test]
fn exact_gpu_surface_routing_skips_a_busy_slot_when_a_sibling_is_released() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        19,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let mut fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("first WARP exact Surface publication succeeds");
    let crate::GpuSurfacePublishOutcome::Published(first) = fixture.outcomes.remove(0) else {
        panic!("first descriptor unexpectedly busy");
    };
    let mut second_outcomes = super::gpu_surface::fixture::republish(&mut fixture, 42)
        .expect("second exact Surface slot publishes");
    let crate::GpuSurfacePublishOutcome::Published(second) = second_outcomes.remove(0) else {
        panic!("second descriptor unexpectedly busy");
    };
    let second_handle = super::gpu_surface::fixture::texture_handle(&second);
    let second_slot_id = second.provenance().slot_id;
    let second_use_id = second.provenance().use_id;
    super::gpu_surface::fixture::readback_and_release(&fixture.plan, &second)
        .expect("second slot is released out of order");
    drop(second);

    let mut third_outcomes = super::gpu_surface::fixture::republish(&mut fixture, 43)
        .expect("released sibling slot is selected");
    let crate::GpuSurfacePublishOutcome::Published(third) = third_outcomes.remove(0) else {
        panic!("released sibling was hidden by the busy write cursor");
    };
    assert_eq!(
        second_handle,
        super::gpu_surface::fixture::texture_handle(&third)
    );
    assert_eq!(third.provenance().slot_id, second_slot_id);
    assert_eq!(third.provenance().use_id, second_use_id + 1);
    assert_ne!(
        super::gpu_surface::fixture::texture_handle(&first),
        super::gpu_surface::fixture::texture_handle(&third)
    );

    super::gpu_surface::fixture::release_on_producer_device(&fixture.plan, &first)
        .expect("first slot releases");
    super::gpu_surface::fixture::release_on_producer_device(&fixture.plan, &third)
        .expect("third slot releases");
}

#[test]
fn callback_fanout_exposes_only_submitted_publications() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        33,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let mut fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("first WARP exact Surface publication succeeds");

    assert!(
        super::gpu_surface::fixture::callback_observes_only_submitted_publications(
            &mut fixture,
            42,
        )
        .expect("callback publication succeeds")
    );
}

#[test]
fn static_retry_publishes_only_routes_that_missed_the_latest_clean_frame() {
    let descriptor_a = super::gpu_surface::fixture::descriptor(
        35,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let descriptor_b = super::gpu_surface::fixture::descriptor(
        36,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let mut fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        &[descriptor_a.clone(), descriptor_b.clone()],
    )
    .expect("first WARP exact Surface publication succeeds");
    let mut first_a = None;
    for outcome in std::mem::take(&mut fixture.outcomes) {
        let crate::GpuSurfacePublishOutcome::Published(publication) = outcome else {
            panic!("fresh route unexpectedly busy");
        };
        if publication.provenance().descriptor.id() == descriptor_a.id() {
            first_a = Some(publication);
        } else {
            super::gpu_surface::fixture::readback_and_release(&fixture.plan, &publication)
                .expect("first route B publication releases");
        }
    }
    let first_a = first_a.expect("first source publishes route A");
    let second = super::gpu_surface::fixture::republish(&mut fixture, 42)
        .expect("second source publishes both routes");
    let mut second_a = None;
    for outcome in second {
        let crate::GpuSurfacePublishOutcome::Published(publication) = outcome else {
            panic!("second route unexpectedly busy");
        };
        if publication.provenance().descriptor.id() == descriptor_a.id() {
            second_a = Some(publication);
        } else {
            super::gpu_surface::fixture::readback_and_release(&fixture.plan, &publication)
                .expect("second route B publication releases");
        }
    }
    let second_a = second_a.expect("second source publishes route A");

    let latest = super::gpu_surface::fixture::republish(&mut fixture, 43)
        .expect("latest source reports route pressure independently");
    assert!(latest.iter().any(
        |outcome| matches!(outcome, crate::GpuSurfacePublishOutcome::Busy(id) if *id == descriptor_a.id())
    ));
    assert!(latest.iter().any(|outcome| matches!(
        outcome,
        crate::GpuSurfacePublishOutcome::Published(publication)
            if publication.provenance().descriptor.id() == descriptor_b.id()
    )));
    drop(first_a);

    let retried = (0..64)
        .find_map(|_| {
            let outcomes = super::gpu_surface::fixture::retry_pending(&mut fixture)
                .expect("static clean-frame retry remains healthy");
            assert!(outcomes.iter().all(|outcome| match outcome {
                crate::GpuSurfacePublishOutcome::Busy(id) => *id == descriptor_a.id(),
                crate::GpuSurfacePublishOutcome::Published(publication) => {
                    publication.provenance().descriptor.id() == descriptor_a.id()
                }
            }));
            let publication = outcomes.into_iter().find_map(|outcome| match outcome {
                crate::GpuSurfacePublishOutcome::Published(publication) => Some(publication),
                crate::GpuSurfacePublishOutcome::Busy(_) => None,
            });
            if publication.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            publication
        })
        .expect("route A retries from retained clean content after reclaim");
    assert_eq!(retried.provenance().source_sequence, 43);
    drop(second_a);
}

#[test]
fn pending_surface_retry_never_spends_the_callers_native_wait_budget() {
    let requested = Duration::from_secs(2);

    assert_eq!(gpu_surface_acquire_timeout(requested, true), Duration::ZERO);
    assert_eq!(gpu_surface_acquire_timeout(requested, false), requested);
}

#[test]
fn duplication_epoch_change_rejects_retained_pending_surface_retry() {
    let descriptor = super::gpu_surface::fixture::descriptor(
        37,
        CaptureRegion::full(1, 1),
        CaptureExtent::try_new(1, 1).expect("output extent is valid"),
    );
    let mut fixture = super::gpu_surface::fixture::publish(
        &[1, 2, 3, 0xFF],
        1,
        1,
        std::slice::from_ref(&descriptor),
    )
    .expect("first WARP exact Surface publication succeeds");
    let first = fixture.outcomes.remove(0);
    let second = super::gpu_surface::fixture::republish(&mut fixture, 42)
        .expect("second native slot publishes")
        .remove(0);
    let latest = super::gpu_surface::fixture::republish(&mut fixture, 43)
        .expect("latest source remains pending while both slots are retained");
    assert!(matches!(
        latest.as_slice(),
        [crate::GpuSurfacePublishOutcome::Busy(id)] if *id == descriptor.id()
    ));

    assert!(matches!(
        super::gpu_surface::fixture::retry_pending_for_duplication_epoch(&mut fixture, 6),
        Err(CaptureError::GpuSurfacePlanInvalidated)
    ));
    drop((first, second));
}

#[test]
fn raw_keyed_mutex_contention_preserves_wait_timeout() {
    assert!(
        super::gpu_surface::fixture::real_keyed_mutex_contention_times_out()
            .expect("real keyed-mutex contention probe succeeds")
    );
}

#[test]
fn raw_keyed_mutex_owner_loss_preserves_wait_abandoned() {
    assert!(
        super::gpu_surface::fixture::abandoned_keyed_mutex_owner_is_reported()
            .expect("real keyed-mutex abandonment probe succeeds")
    );
}

#[test]
fn raw_keyed_mutex_classifier_rejects_negative_and_unknown_positive_statuses() {
    assert!(
        super::gpu_surface::fixture::keyed_mutex_status_classifier_rejects_errors_and_unknown_success()
    );
}

#[test]
fn every_injected_post_acquire_exit_poison_fences_reuse() {
    for (offset, fault) in [
        super::gpu_surface::InjectedSurfaceFault::AfterProducerAcquire,
        super::gpu_surface::InjectedSurfaceFault::AfterProducerRelease,
    ]
    .into_iter()
    .enumerate()
    {
        let descriptor = super::gpu_surface::fixture::descriptor(
            40 + u64::try_from(offset).expect("fixture offset fits u64"),
            CaptureRegion::full(1, 1),
            CaptureExtent::try_new(1, 1).expect("output extent is valid"),
        );
        let mut fixture = super::gpu_surface::fixture::publish(
            &[1, 2, 3, 0xFF],
            1,
            1,
            std::slice::from_ref(&descriptor),
        )
        .expect("first WARP exact Surface publication succeeds");
        fixture.outcomes.clear();

        assert!(
            super::gpu_surface::fixture::republish_with_fault(&mut fixture, 42, fault,).is_err()
        );
        assert!(matches!(
            fixture.plan.reclaim_abandoned(),
            Err(CaptureError::GpuSurfacePlanPoisoned {
                descriptor_id,
                use_id: 1,
            }) if descriptor_id == descriptor.id()
        ));
    }
}

#[test]
fn busy_ring_keeps_pending_and_latest_clean_metadata_distinct() {
    let (pending_sequence, clean_sequence, region) =
        super::gpu_reduction::ring_busy_keeps_latest_clean_metadata_for_test()
            .expect("WARP ring metadata fixture succeeds");

    assert_eq!(pending_sequence, 1);
    assert_eq!(clean_sequence, 4);
    assert_eq!(region, CaptureRegion::full(1, 1));
}

#[test]
fn production_query_polling_progresses_without_manual_flush() {
    let bgra = [10, 20, 30, 0xFF].repeat(6);
    let reduced = super::gpu_reduction::reduce_fixture(
        &bgra,
        3,
        2,
        2,
        &PointerState::default(),
        DisplayRotation::Identity,
    )
    .expect("flags-zero first poll advances WARP without a manual flush");

    assert!(!reduced.is_empty());
}

#[test]
fn second_gpu_query_retry_is_nonflushing() {
    let (first, second) = super::gpu_reduction::query_poll_flags_for_test();

    assert_eq!(first, 0);
    assert_eq!(second, D3D11_ASYNC_GETDATA_DONOTFLUSH.0.cast_unsigned());
}

#[test]
fn first_static_frame_query_failure_preserves_cpu_fallback_state() {
    let (sequence, region) = super::gpu_reduction::poll_failure_preserves_clean_metadata_for_test(
        super::gpu_reduction::InjectedPollFailure::Query,
    )
    .expect("query failure keeps the clean first frame");

    assert_eq!(sequence, 41);
    assert_eq!(
        region,
        CaptureRegion::new(1, 1, 4, 2).expect("valid region")
    );
}

#[test]
fn first_static_frame_map_failure_preserves_cpu_fallback_state() {
    let (sequence, region) = super::gpu_reduction::poll_failure_preserves_clean_metadata_for_test(
        super::gpu_reduction::InjectedPollFailure::Map,
    )
    .expect("map failure keeps the clean first frame");

    assert_eq!(sequence, 41);
    assert_eq!(
        region,
        CaptureRegion::new(1, 1, 4, 2).expect("valid region")
    );
}

#[test]
fn unsupported_duplication_format_selects_fallback_fixture() {
    assert!(
        super::gpu_reduction::unsupported_source_format_is_rejected_for_test()
            .expect("WARP format-support query succeeds")
    );
}

#[test]
fn pointer_upload_normalizes_color_and_monochrome_shapes() {
    let color = pointer(PointerShapeKind::Color, vec![10, 20, 30, 40]);
    assert_eq!(
        super::gpu_reduction::normalized_pointer_for_test(
            color.shape.as_ref().expect("shape exists")
        ),
        [30, 20, 10, 40]
    );
    let monochrome = PointerShape {
        kind: PointerShapeKind::Monochrome,
        width: 2,
        height: 2,
        pitch: 1,
        hotspot_x: 0,
        hotspot_y: 0,
        bytes: vec![0b1000_0000, 0b0100_0000],
    };
    assert_eq!(
        super::gpu_reduction::normalized_pointer_for_test(&monochrome),
        [0xFF, 0, 0, 0xFF, 0, 0xFF, 0, 0xFF]
    );
}

#[test]
fn gpu_reduction_matches_cpu_for_odd_extents_and_clipped_edge_boxes() {
    let bgra = (0..5 * 3)
        .flat_map(|index| {
            let value = u8::try_from(index * 11).unwrap_or(u8::MAX);
            [value, value.wrapping_add(17), value.wrapping_add(31), 0xFF]
        })
        .collect::<Vec<_>>();

    assert_gpu_parity(
        &bgra,
        5,
        3,
        2,
        &PointerState::default(),
        DisplayRotation::Identity,
    );
}

#[test]
fn gpu_crop_matches_cpu_at_odd_offsets_and_rotated_edges() {
    let width = 7;
    let height = 5;
    let region = CaptureRegion::new(3, 1, 4, 4).expect("edge crop is valid");
    let bgra = (0..width * height)
        .flat_map(|index| {
            let value = u8::try_from(index * 7).unwrap_or(u8::MAX);
            [value, value.wrapping_add(23), value.wrapping_add(61), 0xFF]
        })
        .collect::<Vec<_>>();
    let mut pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 192]);
    pointer.position_x = 3;
    pointer.position_y = 2;

    for rotation in [
        DisplayRotation::Identity,
        DisplayRotation::Clockwise90,
        DisplayRotation::Clockwise180,
        DisplayRotation::Clockwise270,
    ] {
        let cpu = cpu_reduce_region(&bgra, width, height, 3, &pointer, rotation, region);
        let gpu = super::gpu_reduction::reduce_region_fixture(
            &bgra, width, height, 3, &pointer, rotation, region,
        )
        .expect("cropped WARP reduction succeeds");
        assert_eq!(gpu.len(), cpu.len());
        for (index, (gpu, cpu)) in gpu.iter().zip(&cpu).enumerate() {
            assert!(
                gpu.abs_diff(*cpu) <= 1,
                "rotation {rotation:?} channel {index} differs: GPU={gpu}, CPU={cpu}"
            );
        }
    }
}

#[test]
fn capture_region_is_part_of_gpu_resource_identity() {
    assert!(
        super::gpu_reduction::region_changes_resource_identity_for_test()
            .expect("resource keys are valid")
    );
}

#[test]
fn cropped_frame_origin_tracks_scanout_region_across_rotations() {
    let region = CaptureRegion::new(3, 1, 4, 4).expect("edge crop is valid");
    for (rotation, expected) in [
        (DisplayRotation::Identity, (-7, 21)),
        (DisplayRotation::Clockwise90, (-10, 23)),
        (DisplayRotation::Clockwise180, (-10, 20)),
        (DisplayRotation::Clockwise270, (-9, 20)),
    ] {
        let pointer = Arc::new(PointerState::default());
        let metadata = CaptureMetadata {
            source_id: Arc::from("synthetic"),
            topology_generation: 1,
            sequence: 1,
            captured_at: Instant::now(),
            cursor: pointer.cursor_info(7, 5, rotation),
            pointer,
            source_width: 7,
            source_height: 5,
            origin_x: -10,
            origin_y: 20,
            rotation,
            source_color_space: crate::GpuSurfaceSourceColorSpace::RgbFullG22P709,
            region,
        };

        assert_eq!(capture_region_origin(&metadata), expected);
    }
}

#[test]
fn gpu_reduction_matches_cpu_for_every_cursor_shape_and_rotation() {
    let bgra = [25, 50, 75, 0xFF].repeat(24);
    let fixtures = [
        pointer(PointerShapeKind::Color, vec![200, 100, 50, 128]),
        pointer(PointerShapeKind::MaskedColor, vec![0x0F, 0x33, 0x55, 0xFF]),
        pointer(PointerShapeKind::Monochrome, vec![0x00, 0x80]),
    ];
    for rotation in [
        DisplayRotation::Identity,
        DisplayRotation::Clockwise90,
        DisplayRotation::Clockwise180,
        DisplayRotation::Clockwise270,
    ] {
        for mut pointer in fixtures.clone() {
            pointer.position_x = 2;
            pointer.position_y = 1;
            assert_gpu_parity(&bgra, 6, 4, 3, &pointer, rotation);
        }
    }
}

#[test]
fn gpu_cursor_only_update_recomposes_from_clean_desktop_without_residue() {
    let bgra = [10, 20, 30, 0xFF, 40, 50, 60, 0xFF];
    let first = pointer(PointerShapeKind::Color, vec![200, 100, 50, 0xFF]);
    let mut second = first.clone();
    second.position_x = 1;
    let (first_gpu, second_gpu) =
        super::gpu_reduction::reduce_pointer_sequence(&bgra, 2, 1, &first, &second)
            .expect("pointer-only WARP sequence succeeds");

    assert_eq!(
        first_gpu,
        cpu_reduce(&bgra, 2, 1, 2, &first, DisplayRotation::Identity)
    );
    assert_eq!(
        second_gpu,
        cpu_reduce(&bgra, 2, 1, 2, &second, DisplayRotation::Identity)
    );
    assert_eq!(&second_gpu[..4], [30, 20, 10, 0xFF]);
}

#[test]
fn cursor_only_frame_seeds_staging_once_then_reuses_it() {
    assert_eq!(
        desktop_frame_source(false, false),
        DesktopFrameSource::AcquiredResource
    );
    assert_eq!(
        desktop_frame_source(false, true),
        DesktopFrameSource::RetainedStaging
    );
    assert_eq!(
        desktop_frame_source(true, true),
        DesktopFrameSource::AcquiredResource
    );
}

#[test]
fn rotated_modes_keep_logical_and_native_scanout_extents_distinct() {
    for (rotation, expected) in [
        (DisplayRotation::Identity, (2160, 3840)),
        (DisplayRotation::Clockwise90, (3840, 2160)),
        (DisplayRotation::Clockwise180, (2160, 3840)),
        (DisplayRotation::Clockwise270, (3840, 2160)),
    ] {
        assert_eq!(native_scanout_extent(2160, 3840, rotation), expected);
    }
}

#[test]
fn successful_reacquisition_releases_old_slot_before_opening() {
    let drops = Rc::new(Cell::new(0));
    let mut duplication = Some(DropSignal(Rc::clone(&drops)));
    let mut staging = Some(DropSignal(Rc::clone(&drops)));

    let (candidate, ()) = prepare_duplication(
        &mut duplication,
        &mut staging,
        || {
            assert_eq!(drops.get(), 2);
            Ok::<_, ()>(DropSignal(Rc::clone(&drops)))
        },
        |_| {
            assert_eq!(drops.get(), 2);
            Ok(())
        },
    )
    .expect("reacquisition succeeds");
    assert!(duplication.is_none());
    assert!(staging.is_none());
    duplication = Some(candidate);

    assert!(duplication.is_some());
    assert!(staging.is_none());
    assert_eq!(drops.get(), 2);
}

#[test]
fn failed_reacquisition_admission_never_installs_a_partial_candidate() {
    let drops = Rc::new(Cell::new(0));
    let mut duplication = Some(DropSignal(Rc::clone(&drops)));
    let mut staging = Some(DropSignal(Rc::clone(&drops)));

    let result = prepare_duplication(
        &mut duplication,
        &mut staging,
        || Ok(DropSignal(Rc::clone(&drops))),
        |_| Err::<(), _>("synthetic admission failure"),
    );

    assert_eq!(
        result.expect_err("reacquisition fails"),
        "synthetic admission failure"
    );
    assert!(duplication.is_none());
    assert!(staging.is_none());
    assert_eq!(drops.get(), 3);
}

#[test]
fn rebuild_resource_admission_preserves_requested_byte_context() {
    let error = session_rebuild_error(CaptureError::ResourceExhausted {
        operation: "initialize GPU reduction",
        requested_bytes: 987_654,
    });

    assert!(matches!(
        error,
        CaptureError::SessionResourceExhausted {
            operation: "initialize GPU reduction",
            requested_bytes: 987_654,
        }
    ));
}

#[test]
fn retained_clean_staging_recomposes_a_moving_pointer_without_residue() {
    let desktop = [10_u8, 20, 30, 255, 40, 50, 60, 255];
    let mut pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 255]);
    let mut rgba = Vec::new();

    super::DesktopDuplicator::copy_bgra_rows(
        super::BgraRows {
            bytes: &desktop,
            row_pitch: 8,
            width: 2,
            height: 1,
        },
        &mut rgba,
        CaptureExtent::try_new(2, u32::MAX).expect("test extent"),
        &pointer,
        DisplayRotation::Identity,
        CaptureRegion::full(2, 1),
    )
    .expect("valid desktop rows are reduced");
    assert_eq!(rgba, [50, 100, 200, 255, 60, 50, 40, 255]);

    pointer.position_x = 1;
    super::DesktopDuplicator::copy_bgra_rows(
        super::BgraRows {
            bytes: &desktop,
            row_pitch: 8,
            width: 2,
            height: 1,
        },
        &mut rgba,
        CaptureExtent::try_new(2, u32::MAX).expect("test extent"),
        &pointer,
        DisplayRotation::Identity,
        CaptureRegion::full(2, 1),
    )
    .expect("valid desktop rows are reduced");
    assert_eq!(rgba, [30, 20, 10, 255, 50, 100, 200, 255]);
}

#[test]
fn bgra_rows_reject_a_pitch_narrower_than_the_pixel_width() {
    let pointer = PointerState::default();
    let mut rgba = Vec::new();

    let dimensions = super::DesktopDuplicator::copy_bgra_rows(
        super::BgraRows {
            bytes: &[0; 4],
            row_pitch: 3,
            width: 1,
            height: 1,
        },
        &mut rgba,
        CaptureExtent::try_new(1, u32::MAX).expect("test extent"),
        &pointer,
        DisplayRotation::Identity,
        CaptureRegion::full(1, 1),
    );

    assert_eq!(dimensions.expect("row validation does not allocate"), None);
    assert!(rgba.is_empty());
}

#[test]
fn dxgi_hresult_classifier_preserves_recovery_classes() {
    for (code, expected) in [
        (DXGI_ERROR_WAIT_TIMEOUT, CaptureError::Timeout),
        (DXGI_ERROR_ACCESS_LOST, CaptureError::AccessLost),
        (E_ACCESSDENIED, CaptureError::AccessDenied),
        (
            DXGI_ERROR_NOT_CURRENTLY_AVAILABLE,
            CaptureError::AlreadyDuplicating,
        ),
        (
            DXGI_ERROR_SESSION_DISCONNECTED,
            CaptureError::SessionUnavailable,
        ),
        (DXGI_ERROR_DEVICE_REMOVED, CaptureError::DeviceLost),
        (DXGI_ERROR_DEVICE_RESET, CaptureError::DeviceLost),
    ] {
        let classified = classify_hresult("synthetic operation", code, "synthetic failure");
        assert_eq!(
            std::mem::discriminant(&classified),
            std::mem::discriminant(&expected)
        );
    }

    assert!(matches!(
        classify_hresult("synthetic operation", E_FAIL, "synthetic failure"),
        CaptureError::Windows {
            context: "synthetic operation",
            ..
        }
    ));
}

#[test]
fn color_pointer_alpha_blends_bgra() {
    let pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 128]);

    assert_eq!(
        pointer.composite_bgra([20, 40, 60, 255], 0, 0, 1, 1, DisplayRotation::Identity),
        [110, 70, 55, 255]
    );
}

#[test]
fn monochrome_pointer_applies_and_then_xor_masks() {
    let invert = pointer(PointerShapeKind::Monochrome, vec![0x80, 0x80]);
    let black = pointer(PointerShapeKind::Monochrome, vec![0x00, 0x00]);

    assert_eq!(
        invert.composite_bgra(
            [0x11, 0x22, 0x33, 0xFF],
            0,
            0,
            1,
            1,
            DisplayRotation::Identity
        ),
        [0xEE, 0xDD, 0xCC, 0xFF]
    );
    assert_eq!(
        black.composite_bgra(
            [0x11, 0x22, 0x33, 0xFF],
            0,
            0,
            1,
            1,
            DisplayRotation::Identity
        ),
        [0, 0, 0, 0xFF]
    );
}

#[test]
fn masked_color_pointer_selects_copy_or_xor_by_alpha_mask() {
    let copy = pointer(PointerShapeKind::MaskedColor, vec![1, 2, 3, 0]);
    let xor = pointer(PointerShapeKind::MaskedColor, vec![0xFF, 0x0F, 0xF0, 0xFF]);

    assert_eq!(
        copy.composite_bgra(
            [0x10, 0x20, 0x30, 0xFF],
            0,
            0,
            1,
            1,
            DisplayRotation::Identity
        ),
        [1, 2, 3, 0xFF]
    );
    assert_eq!(
        xor.composite_bgra(
            [0x10, 0x20, 0x30, 0xFF],
            0,
            0,
            1,
            1,
            DisplayRotation::Identity
        ),
        [0xEF, 0x2F, 0xC0, 0xFF]
    );
}

#[test]
fn hidden_or_already_composed_pointer_does_not_modify_desktop() {
    let mut pointer = pointer(PointerShapeKind::Color, vec![255, 255, 255, 255]);
    pointer.visible = false;
    let desktop = [1, 2, 3, 255];

    assert_eq!(
        pointer.composite_bgra(desktop, 0, 0, 1, 1, DisplayRotation::Identity),
        desktop
    );
    assert!(
        pointer
            .cursor_info(1, 1, DisplayRotation::Identity)
            .composed
    );
}

#[test]
fn pointer_coordinates_round_trip_for_every_rotation() {
    for rotation in [
        DisplayRotation::Identity,
        DisplayRotation::Clockwise90,
        DisplayRotation::Clockwise180,
        DisplayRotation::Clockwise270,
    ] {
        for (x, y) in [(0, 0), (1, 2), (3, 1)] {
            let logical = scanout_to_logical(x, y, 4, 3, rotation);
            assert_eq!(
                logical_to_scanout(logical.0, logical.1, 4, 3, rotation),
                (i64::from(x), i64::from(y))
            );
        }
    }
}

#[test]
fn rotated_pointer_geometry_stays_in_native_scanout_space() {
    let geometry = pointer_scanout_geometry(2, 1, 3, 2, 1, 1, 8, 6, DisplayRotation::Clockwise90);

    assert_eq!(geometry, (1, 1, 2, 3, 1, 1));
}

#[test]
fn moving_pointer_recomposes_from_clean_desktop_without_residue() {
    let desktop = [10, 20, 30, 255];
    let mut pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 255]);
    assert_ne!(
        pointer.composite_bgra(desktop, 0, 0, 2, 1, DisplayRotation::Identity),
        desktop
    );

    pointer.position_x = 1;
    assert_eq!(
        pointer.composite_bgra(desktop, 0, 0, 2, 1, DisplayRotation::Identity),
        desktop
    );
    assert_ne!(
        pointer.composite_bgra(desktop, 1, 0, 2, 1, DisplayRotation::Identity),
        desktop
    );
}

#[test]
fn visible_pointer_composites_at_the_rotated_scanout_pixel() {
    let mut pointer = pointer(PointerShapeKind::Color, vec![200, 100, 50, 255]);
    pointer.position_x = 2;
    pointer.position_y = 1;
    let scanout = logical_to_scanout(2, 1, 8, 6, DisplayRotation::Clockwise90);

    assert_eq!(
        pointer.composite_bgra(
            [10, 20, 30, 255],
            scanout.0 as u32,
            scanout.1 as u32,
            8,
            6,
            DisplayRotation::Clockwise90,
        ),
        [200, 100, 50, 255]
    );
}

#[test]
fn truncated_pointer_shapes_are_rejected_before_composition() {
    let color = PointerShape {
        kind: PointerShapeKind::Color,
        width: 2,
        height: 1,
        pitch: 8,
        hotspot_x: 0,
        hotspot_y: 0,
        bytes: vec![0; 4],
    };
    let monochrome = PointerShape {
        kind: PointerShapeKind::Monochrome,
        width: 9,
        height: 2,
        pitch: 1,
        hotspot_x: 0,
        hotspot_y: 0,
        bytes: vec![0; 2],
    };

    assert!(color.validate().is_err());
    assert!(monochrome.validate().is_err());
}

#[test]
fn topology_generation_changes_only_when_descriptor_content_changes() {
    let mut state = TopologyState::default();
    let mut initial = vec![topology_entry("left", -1920), topology_entry("main", 0)];
    initial.sort_unstable_by(|left, right| left.id.cmp(&right.id));

    assert_eq!(state.observe(initial.clone()), 1);
    assert_eq!(state.observe(initial.clone()), 1);

    let mut reordered = initial.clone();
    reordered.reverse();
    reordered.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    assert_eq!(state.observe(reordered), 1);

    assert_eq!(state.observe(vec![topology_entry("main", 0)]), 2);
}

#[test]
fn channel_average_handles_a_single_full_8k_reduction_box() {
    let samples = 7_680_u64 * 4_320;
    let sum = samples * u64::from(u8::MAX);

    assert!(sum > u64::from(u32::MAX));
    assert_eq!(average_channel(sum, samples), u8::MAX);
}

#[test]
fn channel_average_handles_the_maximum_d3d11_surface() {
    let samples = 16_384_u64 * 16_384;
    let sum = samples * u64::from(u8::MAX);

    assert_eq!(average_channel(sum, samples), u8::MAX);
}

#[test]
fn channel_average_defends_against_an_empty_box() {
    assert_eq!(average_channel(0, 0), 0);
}
