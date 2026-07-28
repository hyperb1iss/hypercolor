//! Contract tests for the backend-neutral capture frame envelope.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureCursor, CaptureDamage, CaptureEpoch, CaptureFrame, CaptureFrameError,
    CaptureFrameMetadata, CaptureGeometry, CapturePixelFormat, CapturePlanePool, CaptureRotation,
    CaptureSourceId, CaptureStageKind, CaptureStorage, CaptureTransferFunction, CpuCaptureStorage,
    MoveRegion, PhysicalOrigin, PixelExtent, PixelRect, PlatformGpuApi, PlatformGpuSurface,
    RawCaptureSurface, SourceScale,
};

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn source_id(value: &str) -> CaptureSourceId {
    CaptureSourceId::new(Arc::<str>::from(value)).expect("test source id is non-empty")
}

fn metadata(rotation: CaptureRotation) -> CaptureFrameMetadata {
    let captured_at = Instant::now();
    CaptureFrameMetadata {
        source_id: source_id("test:monitor:stable"),
        topology_generation: 3,
        session_generation: 7,
        sequence: 11,
        captured_at,
        fresh_until: captured_at + Duration::from_millis(50),
        geometry: CaptureGeometry::new(
            PhysicalOrigin { x: -1920, y: -120 },
            extent(4, 3),
            extent(4, 3),
            rotation,
            None,
            SourceScale::ONE,
        )
        .expect("test geometry is valid"),
        color_space: CaptureColorSpace::Srgb,
        transfer_function: CaptureTransferFunction::Srgb,
        cursor: CaptureCursor::default(),
    }
}

fn cpu_storage(width: u32, height: u32) -> CaptureStorage {
    let len = usize::try_from(width * height * 4).expect("test storage length fits usize");
    CaptureStorage::Cpu(CpuCaptureStorage::new(
        vec![0; len].into(),
        CapturePixelFormat::Rgba8,
        i64::from(width * 4),
        0,
    ))
}

#[test]
fn every_rotation_preserves_raw_scanout_and_reports_logical_extent() {
    for (rotation, expected) in [
        (CaptureRotation::Identity, extent(4, 3)),
        (CaptureRotation::Clockwise90, extent(3, 4)),
        (CaptureRotation::Clockwise180, extent(4, 3)),
        (CaptureRotation::Clockwise270, extent(3, 4)),
    ] {
        let frame = CaptureFrame::<RawCaptureSurface>::new(
            metadata(rotation),
            cpu_storage(4, 3),
            CaptureDamage::default(),
        )
        .expect("raw frame accepts every pending rotation");

        assert_eq!(frame.stage(), CaptureStageKind::Raw);
        assert_eq!(
            frame
                .metadata()
                .geometry
                .rotation()
                .apply_to_extent(frame.metadata().geometry.native_extent()),
            expected
        );
    }
}

#[test]
fn negative_physical_origins_survive_without_unsigned_coercion() {
    let frame = CaptureFrame::<RawCaptureSurface>::new(
        metadata(CaptureRotation::Identity),
        cpu_storage(4, 3),
        CaptureDamage::default(),
    )
    .expect("negative desktop origin is valid");

    assert_eq!(
        frame.metadata().geometry.origin(),
        PhysicalOrigin { x: -1920, y: -120 }
    );
}

#[test]
fn crop_must_fit_the_native_scanout_extent() {
    let valid = PixelRect::new(1, 1, 3, 2).expect("test crop is non-empty");
    assert!(
        CaptureGeometry::new(
            PhysicalOrigin::default(),
            extent(4, 3),
            extent(4, 3),
            CaptureRotation::Identity,
            Some(valid),
            SourceScale::ONE,
        )
        .is_ok()
    );

    let invalid = PixelRect::new(2, 1, 3, 2).expect("test crop is non-empty");
    assert!(matches!(
        CaptureGeometry::new(
            PhysicalOrigin::default(),
            extent(4, 3),
            extent(4, 3),
            CaptureRotation::Identity,
            Some(invalid),
            SourceScale::ONE,
        ),
        Err(CaptureFrameError::CropOutOfBounds { .. })
    ));
}

#[test]
fn cpu_stride_accepts_padding_and_bottom_up_rows_but_rejects_short_rows() {
    let padded = CaptureStorage::Cpu(CpuCaptureStorage::new(
        vec![0; 60].into(),
        CapturePixelFormat::Rgba8,
        20,
        0,
    ));
    assert!(
        CaptureFrame::<RawCaptureSurface>::new(
            metadata(CaptureRotation::Identity),
            padded,
            CaptureDamage::default(),
        )
        .is_ok()
    );

    let bottom_up = CaptureStorage::Cpu(CpuCaptureStorage::new(
        vec![0; 48].into(),
        CapturePixelFormat::Rgba8,
        -16,
        32,
    ));
    assert!(
        CaptureFrame::<RawCaptureSurface>::new(
            metadata(CaptureRotation::Identity),
            bottom_up,
            CaptureDamage::default(),
        )
        .is_ok()
    );

    let short = CaptureStorage::Cpu(CpuCaptureStorage::new(
        vec![0; 48].into(),
        CapturePixelFormat::Rgba8,
        15,
        0,
    ));
    assert!(matches!(
        CaptureFrame::<RawCaptureSurface>::new(
            metadata(CaptureRotation::Identity),
            short,
            CaptureDamage::default(),
        ),
        Err(CaptureFrameError::InvalidCpuStride { .. })
    ));
}

struct GpuLifetimeProbe(Arc<AtomicBool>);

impl Drop for GpuLifetimeProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[test]
fn gpu_surface_erases_platform_type_but_retains_owner_lifetime() {
    let dropped = Arc::new(AtomicBool::new(false));
    let owner = Arc::new(GpuLifetimeProbe(Arc::clone(&dropped)));
    let weak: Weak<GpuLifetimeProbe> = Arc::downgrade(&owner);
    let surface = PlatformGpuSurface::new(
        PlatformGpuApi::Direct3d11,
        42,
        extent(4, 3),
        CapturePixelFormat::Bgra8,
        owner,
    )
    .expect("non-zero opaque handle is valid");
    let recovered = surface
        .owner::<GpuLifetimeProbe>()
        .expect("platform adapter can recover its erased owner");
    assert!(Arc::ptr_eq(
        &recovered,
        &weak.upgrade().expect("owner remains live")
    ));
    drop(recovered);
    let frame = CaptureFrame::<RawCaptureSurface>::new(
        metadata(CaptureRotation::Identity),
        CaptureStorage::Gpu(surface),
        CaptureDamage::default(),
    )
    .expect("GPU frame metadata is consistent");

    assert!(weak.upgrade().is_some());
    assert!(!dropped.load(Ordering::Acquire));
    drop(frame);
    assert!(weak.upgrade().is_none());
    assert!(dropped.load(Ordering::Acquire));
}

#[test]
fn storage_extent_and_addressing_must_match_geometry() {
    let short_buffer = CaptureStorage::Cpu(CpuCaptureStorage::new(
        vec![0; 47].into(),
        CapturePixelFormat::Rgba8,
        16,
        0,
    ));
    assert!(matches!(
        CaptureFrame::<RawCaptureSurface>::new(
            metadata(CaptureRotation::Identity),
            short_buffer,
            CaptureDamage::default(),
        ),
        Err(CaptureFrameError::CpuBufferOutOfBounds { .. })
    ));

    let gpu = PlatformGpuSurface::new(
        PlatformGpuApi::Vulkan,
        9,
        extent(5, 3),
        CapturePixelFormat::Rgba8,
        Arc::new(()),
    )
    .expect("test GPU descriptor is valid");
    assert!(matches!(
        CaptureFrame::<RawCaptureSurface>::new(
            metadata(CaptureRotation::Identity),
            CaptureStorage::Gpu(gpu),
            CaptureDamage::default(),
        ),
        Err(CaptureFrameError::GpuExtentMismatch { .. })
    ));
}

#[test]
fn identity_and_generation_invariants_fail_closed() {
    assert!(matches!(
        CaptureSourceId::new(Arc::<str>::from("  ")),
        Err(CaptureFrameError::EmptySourceId)
    ));
    assert!(matches!(
        PixelExtent::new(0, 3),
        Err(CaptureFrameError::EmptyExtent { .. })
    ));
    assert!(matches!(
        SourceScale::new(1, 0),
        Err(CaptureFrameError::InvalidSourceScale { .. })
    ));

    for (topology_generation, session_generation, sequence, expected) in [
        (0, 7, 11, "topology"),
        (3, 0, 11, "session"),
        (3, 7, 0, "sequence"),
    ] {
        let mut invalid = metadata(CaptureRotation::Identity);
        invalid.topology_generation = topology_generation;
        invalid.session_generation = session_generation;
        invalid.sequence = sequence;
        let error = CaptureFrame::<RawCaptureSurface>::new(
            invalid,
            cpu_storage(4, 3),
            CaptureDamage::default(),
        )
        .expect_err("zero identity metadata is rejected");
        match expected {
            "topology" => assert_eq!(error, CaptureFrameError::ZeroGeneration("topology")),
            "session" => assert_eq!(error, CaptureFrameError::ZeroGeneration("session")),
            "sequence" => assert_eq!(error, CaptureFrameError::ZeroSequence),
            _ => unreachable!("test cases only contain known invariants"),
        }
    }
}

#[test]
fn stale_topology_and_session_generations_are_rejected_precisely() {
    let frame = CaptureFrame::<RawCaptureSurface>::new(
        metadata(CaptureRotation::Identity),
        cpu_storage(4, 3),
        CaptureDamage::default(),
    )
    .expect("test frame is valid");

    let stale_topology = CaptureEpoch {
        source_id: source_id("test:monitor:stable"),
        topology_generation: 4,
        session_generation: 7,
    };
    assert!(matches!(
        frame.validate_epoch(&stale_topology),
        Err(CaptureFrameError::StaleTopology { .. })
    ));

    let stale_session = CaptureEpoch {
        source_id: source_id("test:monitor:stable"),
        topology_generation: 3,
        session_generation: 8,
    };
    assert!(matches!(
        frame.validate_epoch(&stale_session),
        Err(CaptureFrameError::StaleSession { .. })
    ));
}

#[test]
fn invalid_freshness_damage_and_move_metadata_fail_at_construction() {
    let mut stale = metadata(CaptureRotation::Identity);
    stale.fresh_until = stale
        .captured_at
        .checked_sub(Duration::from_millis(1))
        .expect("current monotonic time exceeds one millisecond");
    assert!(matches!(
        CaptureFrame::<RawCaptureSurface>::new(stale, cpu_storage(4, 3), CaptureDamage::default(),),
        Err(CaptureFrameError::InvalidFreshness)
    ));

    let dirty = PixelRect::new(3, 0, 2, 1).expect("test damage is non-empty");
    assert!(matches!(
        CaptureFrame::<RawCaptureSurface>::new(
            metadata(CaptureRotation::Identity),
            cpu_storage(4, 3),
            CaptureDamage::new(vec![dirty], Vec::new()),
        ),
        Err(CaptureFrameError::DamageOutOfBounds(_))
    ));

    let moved = MoveRegion {
        source: PixelRect::new(0, 0, 2, 2).expect("test move is non-empty"),
        destination: (3, 2),
    };
    assert!(matches!(
        CaptureFrame::<RawCaptureSurface>::new(
            metadata(CaptureRotation::Identity),
            cpu_storage(4, 3),
            CaptureDamage::new(Vec::new(), vec![moved]),
        ),
        Err(CaptureFrameError::MoveOutOfBounds(_))
    ));
}

#[test]
fn processed_transition_rejects_any_pending_rotation_or_crop() {
    let raw = CaptureFrame::<RawCaptureSurface>::new(
        metadata(CaptureRotation::Identity),
        cpu_storage(4, 3),
        CaptureDamage::default(),
    )
    .expect("raw input is valid");
    assert!(matches!(
        raw.into_geometry_normalized(
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent(4, 3),
                extent(4, 3),
                CaptureRotation::Clockwise90,
                None,
                SourceScale::ONE,
            )
            .expect("pending rotation is valid raw geometry"),
            cpu_storage(4, 3),
            CaptureDamage::default(),
        ),
        Err(CaptureFrameError::ProcessedRotationPending(
            CaptureRotation::Clockwise90
        ))
    ));

    let raw = CaptureFrame::<RawCaptureSurface>::new(
        metadata(CaptureRotation::Identity),
        cpu_storage(4, 3),
        CaptureDamage::default(),
    )
    .expect("raw input is valid");
    let pending_crop = PixelRect::new(1, 1, 2, 2).expect("test crop is valid");
    assert!(matches!(
        raw.into_geometry_normalized(
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent(4, 3),
                extent(2, 2),
                CaptureRotation::Identity,
                Some(pending_crop),
                SourceScale::ONE,
            )
            .expect("pending crop is valid raw geometry"),
            cpu_storage(2, 2),
            CaptureDamage::default(),
        ),
        Err(CaptureFrameError::ProcessedCropPending(crop)) if crop == pending_crop
    ));

    let raw = CaptureFrame::<RawCaptureSurface>::new(
        metadata(CaptureRotation::Identity),
        cpu_storage(4, 3),
        CaptureDamage::default(),
    )
    .expect("raw input is valid");
    let processed = raw
        .into_geometry_normalized(
            CaptureGeometry::new(
                PhysicalOrigin::default(),
                extent(4, 3),
                extent(4, 3),
                CaptureRotation::Identity,
                None,
                SourceScale::ONE,
            )
            .expect("canonical processed geometry is valid"),
            cpu_storage(4, 3),
            CaptureDamage::default(),
        )
        .expect("canonical geometry is a legal processed stage");
    assert_eq!(processed.stage(), CaptureStageKind::GeometryNormalized);
}

#[test]
fn native_and_stored_extents_are_distinct_contracts() {
    let mut downsampled = metadata(CaptureRotation::Identity);
    downsampled.geometry = CaptureGeometry::new(
        PhysicalOrigin { x: -2560, y: 40 },
        extent(2560, 1440),
        extent(1280, 720),
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("downsampled geometry is valid");
    let frame = CaptureFrame::<RawCaptureSurface>::new(
        downsampled,
        cpu_storage(1280, 720),
        CaptureDamage::default(),
    )
    .expect("storage validates against the retained plane extent");

    assert_eq!(
        frame.metadata().geometry.native_extent(),
        extent(2560, 1440)
    );
    assert_eq!(
        frame.metadata().geometry.storage_extent(),
        extent(1280, 720)
    );
    assert_eq!(
        frame.metadata().geometry.origin(),
        PhysicalOrigin { x: -2560, y: 40 }
    );
}

#[test]
fn pooled_cpu_planes_transfer_ownership_without_copying_and_reuse_capacity() {
    let pool = CapturePlanePool::default();
    let mut lease = pool
        .try_acquire(48)
        .expect("test plane allocation succeeds");
    lease.resize(48, 7);
    let pointer = lease.as_ptr();
    let plane = lease.freeze();
    let storage = CpuCaptureStorage::from_owner(plane, CapturePixelFormat::Rgba8, 16, 0);

    assert_eq!(storage.bytes().as_ptr(), pointer);
    assert_eq!(pool.allocation_count(), 1);
    assert_eq!(pool.available_count(), 0);
    drop(storage);
    assert_eq!(pool.available_count(), 1);

    let reused = pool.try_acquire(48).expect("test plane reuse succeeds");
    assert_eq!(reused.as_ptr(), pointer);
    assert_eq!(pool.allocation_count(), 1);
}

#[test]
fn failed_plane_growth_preserves_the_reusable_allocation() {
    let pool = CapturePlanePool::default();
    let mut lease = pool.try_acquire(48).expect("test allocation succeeds");
    lease.resize(48, 7);
    let pointer = lease.as_ptr();
    drop(lease);

    assert!(matches!(
        pool.try_acquire(usize::MAX),
        Err(CaptureFrameError::PlaneAllocationFailed {
            byte_len: usize::MAX
        })
    ));
    assert_eq!(pool.allocation_count(), 1);
    assert_eq!(pool.available_count(), 1);

    let reused = pool
        .try_acquire(48)
        .expect("last-good plane remains reusable");
    assert_eq!(reused.as_ptr(), pointer);
}

#[test]
fn core_frame_contract_contains_no_platform_api_types() {
    let source = include_str!("../src/input/screen/frame.rs");
    for forbidden in [
        "windows::",
        "ID3D11",
        "IDXGI",
        "pipewire::",
        "spa::",
        "OwnedFd",
    ] {
        assert!(
            !source.contains(forbidden),
            "backend-neutral frame contract leaked {forbidden}"
        );
    }
}
