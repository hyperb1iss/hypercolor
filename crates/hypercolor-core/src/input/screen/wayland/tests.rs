use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{
    CapturedScreenSnapshot, SharedSettings, WaylandCaptureUserData, WaylandSourceMetadata,
    WaylandTopologySignature,
};
use crate::input::screen::{
    CaptureConfig, CaptureSourceId, LegacyScreenSnapshot, PhysicalOrigin, PixelExtent,
    analyze_legacy_screen_frame,
};

fn settings(session_generation: u64) -> Arc<SharedSettings> {
    Arc::new(SharedSettings {
        config: Mutex::new(CaptureConfig::default()),
        generation: 0.into(),
        frame_generation: 0.into(),
        topology_generation: 0.into(),
        topology: Mutex::new(None),
        session_generation: session_generation.into(),
        expected_epoch: Mutex::new(None),
    })
}

fn source_id(value: &str) -> CaptureSourceId {
    CaptureSourceId::new(Arc::<str>::from(value)).expect("test source id is valid")
}

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is valid")
}

fn source(
    session_generation: u64,
    origin: PhysicalOrigin,
    logical_extent: PixelExtent,
) -> WaylandSourceMetadata {
    WaylandSourceMetadata {
        signature: WaylandTopologySignature {
            source_id: source_id("wayland:portal:stable"),
            origin,
            logical_extent: Some(logical_extent),
        },
        session_generation,
        topology: None,
    }
}

fn capture_legacy(
    user_data: &mut WaylandCaptureUserData,
    width: u32,
    height: u32,
    fill: u8,
) -> LegacyScreenSnapshot {
    let plane_len = usize::try_from(width)
        .expect("test width fits usize")
        .checked_mul(usize::try_from(height).expect("test height fits usize"))
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("test plane length fits usize");
    let mut plane = user_data.plane_pool.acquire(plane_len);
    plane.resize(plane_len, fill);
    let frame = user_data
        .capture_frame(Instant::now(), width, height, plane.freeze())
        .expect("test frame is valid");
    analyze_legacy_screen_frame(&mut user_data.analyzer, frame)
        .expect("legacy analysis accepts canonical test geometry")
}

#[test]
fn physical_topology_persists_across_storage_resize_and_session_restart() {
    let settings = settings(7);
    let latest = Arc::new(Mutex::new(None::<CapturedScreenSnapshot>));
    let physical_origin = PhysicalOrigin { x: -1920, y: 0 };
    let mut first_worker = WaylandCaptureUserData::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(7, physical_origin, extent(1920, 1080)),
    );

    let first = capture_legacy(&mut first_worker, 4, 2, 1);
    let resized = capture_legacy(&mut first_worker, 2, 1, 2);
    assert_eq!(first.frame().metadata().topology_generation, 1);
    assert_eq!(resized.frame().metadata().topology_generation, 1);
    assert_eq!(
        resized.frame().metadata().geometry.native_extent(),
        extent(4, 2)
    );
    assert_eq!(
        resized.frame().metadata().geometry.storage_extent(),
        extent(2, 1)
    );

    let next_session = settings.begin_session();
    let mut successor = WaylandCaptureUserData::new(
        Arc::clone(&settings),
        latest,
        source(next_session, physical_origin, extent(1920, 1080)),
    );
    let restarted = capture_legacy(&mut successor, 1, 1, 3);
    assert_eq!(restarted.frame().metadata().topology_generation, 1);
    assert_eq!(
        restarted.frame().metadata().geometry.native_extent(),
        extent(4, 2)
    );

    let mut moved_source = WaylandCaptureUserData::new(
        Arc::clone(&settings),
        Arc::new(Mutex::new(None)),
        source(
            next_session,
            PhysicalOrigin { x: 0, y: -1080 },
            extent(1920, 1080),
        ),
    );
    let moved = capture_legacy(&mut moved_source, 2, 1, 4);
    assert_eq!(moved.frame().metadata().topology_generation, 2);
    assert_eq!(
        moved.frame().metadata().geometry.native_extent(),
        extent(2, 1)
    );
}

#[test]
fn stale_worker_cannot_overwrite_the_successor_snapshot() {
    let settings = settings(9);
    let latest = Arc::new(Mutex::new(None::<CapturedScreenSnapshot>));
    let physical_origin = PhysicalOrigin::default();
    let logical_extent = extent(1920, 1080);
    let mut retiring = WaylandCaptureUserData::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(9, physical_origin, logical_extent),
    );
    let stale = capture_legacy(&mut retiring, 4, 2, 1);

    let active_session = settings.begin_session();
    let mut active = WaylandCaptureUserData::new(
        Arc::clone(&settings),
        Arc::clone(&latest),
        source(active_session, physical_origin, logical_extent),
    );
    let current = capture_legacy(&mut active, 2, 1, 2);
    assert!(settings.publish_snapshot(&latest, current));
    assert!(!settings.publish_snapshot(&latest, stale));

    let published = latest
        .lock()
        .expect("latest snapshot mutex is healthy")
        .clone()
        .expect("successor snapshot remains published");
    assert_eq!(
        published.legacy.frame().metadata().session_generation,
        active_session
    );
    assert_eq!(published.generation, 1);
}
