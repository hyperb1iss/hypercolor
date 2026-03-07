//! End-to-end lifecycle orchestration tests.
//!
//! These tests exercise discovery -> lifecycle actions -> backend manager
//! routing using a deterministic mock backend.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use hypercolor_core::device::{
    BackendInfo, BackendManager, DeviceBackend, DeviceLifecycleManager, LifecycleAction,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};
use tokio::sync::Mutex;

struct RecordingBackend {
    expected_device_id: DeviceId,
    connected: bool,
    writes: Arc<Mutex<Vec<Vec<[u8; 3]>>>>,
    fail_connect_attempts: Arc<AtomicUsize>,
}

impl RecordingBackend {
    fn new(
        expected_device_id: DeviceId,
        writes: Arc<Mutex<Vec<Vec<[u8; 3]>>>>,
        fail_connect_attempts: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            expected_device_id,
            connected: false,
            writes,
            fail_connect_attempts,
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for RecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "mock".to_owned(),
            name: "Lifecycle Recording Backend".to_owned(),
            description: "Tracks connect/write/disconnect for lifecycle tests".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(vec![device_info(
            self.expected_device_id,
            "Lifecycle Device",
        )])
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        let remaining = self.fail_connect_attempts.load(Ordering::Relaxed);
        if remaining > 0 {
            self.fail_connect_attempts.fetch_sub(1, Ordering::Relaxed);
            bail!("simulated connect failure");
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("disconnect called while not connected");
        }
        self.connected = false;
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("write while disconnected");
        }
        self.writes.lock().await.push(colors.to_vec());
        Ok(())
    }
}

fn device_info(id: DeviceId, name: &str) -> DeviceInfo {
    DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "TestVendor".to_owned(),
        family: DeviceFamily::Custom("mock".to_owned()),
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 4,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 4,
            supports_direct: true,
            supports_brightness: false,
            max_fps: 60,
        },
    }
}

fn make_layout(layout_device_id: &str) -> SpatialLayout {
    SpatialLayout {
        id: "lifecycle-layout".into(),
        name: "Lifecycle Layout".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![DeviceZone {
            id: "zone_main".into(),
            name: "Main".into(),
            device_id: layout_device_id.to_owned(),
            zone_name: None,
            group_id: None,
            position: NormalizedPosition { x: 0.5, y: 0.5 },
            size: NormalizedPosition { x: 1.0, y: 1.0 },
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 4,
                direction: hypercolor_types::spatial::StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
        }],
        groups: vec![],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

async fn apply_lifecycle_actions(
    manager: &mut BackendManager,
    lifecycle: &mut DeviceLifecycleManager,
    actions: Vec<LifecycleAction>,
) {
    let mut pending: VecDeque<LifecycleAction> = actions.into();

    while let Some(action) = pending.pop_front() {
        match action {
            LifecycleAction::Connect {
                device_id,
                backend_id,
                layout_device_id,
            } => {
                let connect_result = manager
                    .connect_device(&backend_id, device_id, &layout_device_id)
                    .await;
                let follow_up = if connect_result.is_ok() {
                    lifecycle
                        .on_connected(device_id)
                        .expect("connect transition")
                } else if lifecycle.state(device_id) == Some(DeviceState::Reconnecting) {
                    lifecycle
                        .on_reconnect_failed(device_id)
                        .expect("reconnect failure transition")
                } else {
                    lifecycle
                        .on_connect_failed(device_id)
                        .expect("connect failure transition")
                };
                pending.extend(follow_up);
            }
            LifecycleAction::Disconnect {
                device_id,
                backend_id,
            } => {
                let layout_id = lifecycle
                    .layout_device_id_for(device_id)
                    .expect("layout id should exist for disconnect")
                    .to_owned();
                let _ = manager
                    .disconnect_device(&backend_id, device_id, &layout_id)
                    .await;
            }
            LifecycleAction::Map {
                layout_device_id,
                backend_id,
                device_id,
            } => manager.map_device(layout_device_id, backend_id, device_id),
            LifecycleAction::Unmap { layout_device_id } => {
                manager.unmap_device(&layout_device_id);
            }
            LifecycleAction::SpawnReconnect { device_id, .. } => {
                if let Some(next_connect) = lifecycle.on_reconnect_attempt(device_id) {
                    pending.push_back(next_connect);
                }
            }
            LifecycleAction::CancelReconnect { .. } => {}
        }
    }
}

#[tokio::test]
async fn lifecycle_discovery_connect_and_frame_write() {
    let device_id = DeviceId::new();
    let info = device_info(device_id, "Desk Strip");
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let fail_connect_attempts = Arc::new(AtomicUsize::new(0));

    let backend = RecordingBackend::new(device_id, Arc::clone(&writes), fail_connect_attempts);
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    let mut lifecycle = DeviceLifecycleManager::new();
    let actions = lifecycle.on_discovered(
        device_id,
        &info,
        "mock",
        Some(&DeviceFingerprint("mock:desk-strip".to_owned())),
    );
    apply_lifecycle_actions(&mut manager, &mut lifecycle, actions).await;

    let layout_id = lifecycle
        .layout_device_id_for(device_id)
        .expect("layout id should be derived");
    let layout = make_layout(layout_id);

    let frame = vec![ZoneColors {
        zone_id: "zone_main".into(),
        colors: vec![[255, 0, 128]; 4],
    }];
    let stats = manager.write_frame(&frame, &layout).await;
    assert_eq!(stats.devices_written, 1);
    assert_eq!(stats.total_leds, 4);
    assert!(stats.errors.is_empty());

    tokio::time::sleep(Duration::from_millis(40)).await;
    let writes = writes.lock().await.clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], vec![[255, 0, 128]; 4]);
}

#[tokio::test]
async fn lifecycle_comm_error_reconnects_and_resumes_frames() {
    let device_id = DeviceId::new();
    let info = device_info(device_id, "Case Fan");
    let writes = Arc::new(Mutex::new(Vec::<Vec<[u8; 3]>>::new()));
    let fail_connect_attempts = Arc::new(AtomicUsize::new(0));

    let backend = RecordingBackend::new(
        device_id,
        Arc::clone(&writes),
        Arc::clone(&fail_connect_attempts),
    );
    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));

    let mut lifecycle = DeviceLifecycleManager::new();
    let actions = lifecycle.on_discovered(
        device_id,
        &info,
        "mock",
        Some(&DeviceFingerprint("mock:case-fan".to_owned())),
    );
    apply_lifecycle_actions(&mut manager, &mut lifecycle, actions).await;

    let layout_id = lifecycle
        .layout_device_id_for(device_id)
        .expect("layout id should be available")
        .to_owned();
    let layout = make_layout(&layout_id);

    let first_frame = vec![ZoneColors {
        zone_id: "zone_main".into(),
        colors: vec![[10, 20, 30]; 4],
    }];
    manager.write_frame(&first_frame, &layout).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    // Simulate one reconnect failure before eventual recovery.
    fail_connect_attempts.store(1, Ordering::Relaxed);
    let reconnect_actions = lifecycle
        .on_comm_error(device_id)
        .expect("comm error should produce reconnect actions");
    apply_lifecycle_actions(&mut manager, &mut lifecycle, reconnect_actions).await;
    assert_eq!(lifecycle.state(device_id), Some(DeviceState::Connected));

    let second_frame = vec![ZoneColors {
        zone_id: "zone_main".into(),
        colors: vec![[220, 120, 20]; 4],
    }];
    manager.write_frame(&second_frame, &layout).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    let writes = writes.lock().await.clone();
    assert!(
        writes.len() >= 2,
        "expected writes before and after reconnect, got {}",
        writes.len()
    );
    assert_eq!(
        writes.last().expect("expected last frame"),
        &vec![[220, 120, 20]; 4]
    );
}
