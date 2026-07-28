use super::{
    CaptureMetadata, DesktopFrameSource, PointerShape, PointerShapeKind, PointerState,
    TopologyEntry, TopologyState, average_channel, capture_region_origin, classify_hresult,
    desktop_frame_source, logical_to_scanout, native_scanout_extent, pointer_scanout_geometry,
    reacquire_duplication, scanout_to_logical,
};
use crate::{CaptureError, CaptureRegion, DisplayRotation, ReductionPath};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL};
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
        max_width,
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
    assert_ne!(second, 0);
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
fn reacquisition_drops_old_duplication_and_staging_before_opening() {
    let drops = Rc::new(Cell::new(0));
    let mut duplication = Some(DropSignal(Rc::clone(&drops)));
    let mut staging = Some(DropSignal(Rc::clone(&drops)));

    reacquire_duplication(&mut duplication, &mut staging, || {
        assert_eq!(drops.get(), 2);
        Ok::<_, ()>(DropSignal(Rc::clone(&drops)))
    })
    .expect("reacquisition succeeds");

    assert!(duplication.is_some());
    assert!(staging.is_none());
    assert_eq!(drops.get(), 2);
}

#[test]
fn failed_reacquisition_leaves_no_stale_duplication_resources() {
    let drops = Rc::new(Cell::new(0));
    let mut duplication = Some(DropSignal(Rc::clone(&drops)));
    let mut staging = Some(DropSignal(Rc::clone(&drops)));

    let result = reacquire_duplication(&mut duplication, &mut staging, || {
        Err::<DropSignal, _>("synthetic failure")
    });

    assert_eq!(
        result.expect_err("reacquisition fails"),
        "synthetic failure"
    );
    assert!(duplication.is_none());
    assert!(staging.is_none());
    assert_eq!(drops.get(), 2);
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
        2,
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
        2,
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
        1,
        &pointer,
        DisplayRotation::Identity,
        CaptureRegion::full(1, 1),
    );

    assert_eq!(dimensions, None);
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
