use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{
    CapturedScreenSnapshot, SharedSettings, WaylandCaptureUserData, WaylandSourceMetadata,
};
use crate::input::screen::{
    CaptureConfig, CaptureEpoch, CaptureFrameError, CaptureSourceId, PhysicalOrigin,
};

fn settings(session_generation: u64) -> Arc<SharedSettings> {
    Arc::new(SharedSettings {
        config: Mutex::new(CaptureConfig::default()),
        generation: 0.into(),
        frame_generation: 0.into(),
        topology_generation: 0.into(),
        session_generation: session_generation.into(),
        expected_epoch: Mutex::new(None),
    })
}

fn source_id(value: &str) -> CaptureSourceId {
    CaptureSourceId::new(Arc::<str>::from(value)).expect("test source id is valid")
}

#[test]
fn adapter_owns_expected_epoch_and_rejects_stale_topology() {
    let settings = settings(7);
    let source = WaylandSourceMetadata {
        source_id: source_id("wayland:portal:stable"),
        origin: PhysicalOrigin { x: -1920, y: 0 },
        logical_width: Some(1920),
        session_generation: 7,
        topology_generation: 0,
        storage_extent: None,
    };
    let mut user_data = WaylandCaptureUserData::new(
        Arc::clone(&settings),
        Arc::new(Mutex::new(None::<CapturedScreenSnapshot>)),
        source,
    );

    let mut first_plane = user_data.plane_pool.acquire(32);
    first_plane.resize(32, 1);
    let first = user_data
        .capture_frame(Instant::now(), 4, 2, first_plane.freeze())
        .expect("first frame establishes topology");
    let first_epoch = settings
        .expected_epoch()
        .expect("adapter installed an independent expected epoch");
    assert_eq!(first_epoch.topology_generation, 1);
    assert_eq!(first_epoch.source_id, source_id("wayland:portal:stable"));

    let mut second_plane = user_data.plane_pool.acquire(16);
    second_plane.resize(16, 2);
    let second = user_data
        .capture_frame(Instant::now(), 2, 2, second_plane.freeze())
        .expect("changed storage extent establishes a new topology");
    let second_epoch = settings
        .expected_epoch()
        .expect("updated expected epoch remains installed");
    assert_eq!(second_epoch.topology_generation, 2);
    assert!(matches!(
        first.validate_epoch(&second_epoch),
        Err(CaptureFrameError::StaleTopology { .. })
    ));
    assert!(second.validate_epoch(&second_epoch).is_ok());
}

#[test]
fn stale_worker_cannot_replace_the_active_expected_epoch() {
    let settings = settings(9);
    let active = CaptureEpoch {
        source_id: source_id("wayland:portal:active"),
        topology_generation: 4,
        session_generation: 9,
    };
    assert!(settings.install_expected_epoch(active.clone()));

    let stale = CaptureEpoch {
        source_id: source_id("wayland:portal:stale"),
        topology_generation: 5,
        session_generation: 8,
    };
    assert!(!settings.install_expected_epoch(stale));
    assert_eq!(settings.expected_epoch(), Some(active));
    assert_eq!(settings.session_generation.load(Ordering::Acquire), 9);
}
