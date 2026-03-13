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
    BackendInfo, BackendManager, DeviceBackend, DeviceLifecycleManager, DiscoveryConnectBehavior,
    LifecycleAction,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::canvas::{linear_to_output_u8, srgb_to_linear};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
};
use tokio::sync::Mutex;

// ── LED color pipeline helpers (mirrors prepare_output_for_leds) ────────────

const LED_PERCEPTUAL_COMPENSATION_STRENGTH: f32 = 0.22;
const LED_NEUTRAL_COMPENSATION_WEIGHT: f32 = 0.25;
const LED_HEADROOM_WEIGHT_FLOOR: f32 = 0.1;

fn expected_led_color(color: [u8; 3]) -> [u8; 3] {
    let compensated = apply_led_perceptual_compensation([
        srgb_to_linear(f32::from(color[0]) / 255.0),
        srgb_to_linear(f32::from(color[1]) / 255.0),
        srgb_to_linear(f32::from(color[2]) / 255.0),
    ]);

    [
        linear_to_output_u8(compensated[0]),
        linear_to_output_u8(compensated[1]),
        linear_to_output_u8(compensated[2]),
    ]
}

#[allow(clippy::similar_names)]
fn apply_led_perceptual_compensation(mut color: [f32; 3]) -> [f32; 3] {
    let max_channel = color[0].max(color[1]).max(color[2]);
    if max_channel <= f32::EPSILON {
        return color;
    }

    let min_channel = color[0].min(color[1]).min(color[2]);
    let luma = color[0].mul_add(0.2126, color[1].mul_add(0.7152, color[2] * 0.0722));
    let headroom = 1.0 - max_channel;
    if headroom <= f32::EPSILON {
        return color;
    }

    let whiteness = min_channel / max_channel;
    let colorfulness = LED_NEUTRAL_COMPENSATION_WEIGHT
        + (1.0 - LED_NEUTRAL_COMPENSATION_WEIGHT) * (1.0 - whiteness);
    let shadow_bias = 1.0 - luma;
    let headroom_weight = LED_HEADROOM_WEIGHT_FLOOR + (1.0 - LED_HEADROOM_WEIGHT_FLOOR) * headroom;
    let gain = 1.0
        + LED_PERCEPTUAL_COMPENSATION_STRENGTH
            * shadow_bias
            * shadow_bias
            * headroom_weight
            * colorfulness;
    let gain = gain.min(1.0 / max_channel);

    if gain <= 1.0 {
        return color;
    }

    color[0] = (color[0] * gain).min(1.0);
    color[1] = (color[1] * gain).min(1.0);
    color[2] = (color[2] * gain).min(1.0);
    color
}

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
            has_display: false,
            display_resolution: None,
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
            display_order: 0,
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
    let expected = expected_led_color([255, 0, 128]);
    assert_eq!(writes[0], vec![expected; 4]);
}

#[test]
fn deferred_discovery_waits_for_readiness_upgrade_before_connecting() {
    let mut lifecycle = DeviceLifecycleManager::new();
    let device_id = DeviceId::new();
    let info = device_info(device_id, "Studio Strip");
    let fingerprint = DeviceFingerprint("net:wled:wled-studio.local".to_owned());

    let deferred_actions = lifecycle.on_discovered_with_behavior(
        device_id,
        &info,
        "wled",
        Some(&fingerprint),
        DiscoveryConnectBehavior::Deferred,
    );
    assert!(
        deferred_actions.is_empty(),
        "placeholder discovery should not trigger an eager connect"
    );
    assert_eq!(lifecycle.state(device_id), Some(DeviceState::Known));

    let upgraded_actions = lifecycle.on_discovered_with_behavior(
        device_id,
        &info,
        "wled",
        Some(&fingerprint),
        DiscoveryConnectBehavior::AutoConnect,
    );
    assert!(
        upgraded_actions
            .iter()
            .any(|action| matches!(action, LifecycleAction::Connect { .. })),
        "verified discovery should upgrade the same known device into auto-connect"
    );
}

#[test]
fn deferred_discovery_disconnects_connected_device() {
    let mut lifecycle = DeviceLifecycleManager::new();
    let device_id = DeviceId::new();
    let info = device_info(device_id, "Desk Strip");
    let fingerprint = DeviceFingerprint("mock:desk-strip".to_owned());

    let initial_actions = lifecycle.on_discovered_with_behavior(
        device_id,
        &info,
        "mock",
        Some(&fingerprint),
        DiscoveryConnectBehavior::AutoConnect,
    );
    assert!(
        initial_actions
            .iter()
            .any(|action| matches!(action, LifecycleAction::Connect { .. })),
        "initial discovery should connect an auto-connect device"
    );

    lifecycle
        .on_connected(device_id)
        .expect("connected transition should succeed");
    assert_eq!(lifecycle.state(device_id), Some(DeviceState::Connected));

    let deferred_actions = lifecycle.on_discovered_with_behavior(
        device_id,
        &info,
        "mock",
        Some(&fingerprint),
        DiscoveryConnectBehavior::Deferred,
    );
    assert!(
        deferred_actions
            .iter()
            .any(|action| matches!(action, LifecycleAction::Disconnect { .. })),
        "downgrading to deferred should disconnect a connected device"
    );
    assert!(
        deferred_actions
            .iter()
            .any(|action| matches!(action, LifecycleAction::Unmap { .. })),
        "downgrading to deferred should remove routing for the device"
    );
    assert_eq!(lifecycle.state(device_id), Some(DeviceState::Known));
}

#[test]
fn lifecycle_uses_usb_fingerprint_for_same_name_devices() {
    let mut lifecycle = DeviceLifecycleManager::new();
    let first_id = DeviceId::new();
    let second_id = DeviceId::new();
    let first = device_info(first_id, "PrismRGB Prism S");
    let second = device_info(second_id, "PrismRGB Prism S");

    let _ = lifecycle.on_discovered(
        first_id,
        &first,
        "usb",
        Some(&DeviceFingerprint("usb:16d0:1294:1-3.3".to_owned())),
    );
    let _ = lifecycle.on_discovered(
        second_id,
        &second,
        "usb",
        Some(&DeviceFingerprint("usb:16d0:1294:1-3.4".to_owned())),
    );

    assert_eq!(
        lifecycle.layout_device_id_for(first_id),
        Some("usb:16d0:1294:1-3-3")
    );
    assert_eq!(
        lifecycle.layout_device_id_for(second_id),
        Some("usb:16d0:1294:1-3-4")
    );
}

#[test]
fn lifecycle_uses_smbus_fingerprint_for_same_name_devices() {
    let mut lifecycle = DeviceLifecycleManager::new();
    let device_id = DeviceId::new();
    let info = device_info(device_id, "ASUS ENE Controller");

    let _ = lifecycle.on_discovered(
        device_id,
        &info,
        "smbus",
        Some(&DeviceFingerprint("smbus:/dev/i2c-9:40".to_owned())),
    );

    assert_eq!(
        lifecycle.layout_device_id_for(device_id),
        Some("smbus:-dev-i2c-9:40")
    );
}

#[test]
fn runtime_deactivate_disconnects_without_disabling_the_device() {
    let mut lifecycle = DeviceLifecycleManager::new();
    let device_id = DeviceId::new();
    let info = device_info(device_id, "Desk Strip");

    let actions = lifecycle.on_discovered(
        device_id,
        &info,
        "mock",
        Some(&DeviceFingerprint("mock:desk-strip".to_owned())),
    );
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, LifecycleAction::Connect { .. })),
        "discovery should still request the initial connect"
    );

    lifecycle
        .on_connected(device_id)
        .expect("connect transition should succeed");
    assert_eq!(lifecycle.state(device_id), Some(DeviceState::Connected));

    let standby_actions = lifecycle
        .on_runtime_deactivate(device_id)
        .expect("runtime standby should succeed");

    assert!(
        standby_actions
            .iter()
            .any(|action| matches!(action, LifecycleAction::Disconnect { .. })),
        "standby should disconnect an already connected device"
    );
    assert!(
        standby_actions
            .iter()
            .any(|action| matches!(action, LifecycleAction::Unmap { .. })),
        "standby should remove the layout mapping"
    );
    assert_eq!(
        lifecycle.state(device_id),
        Some(DeviceState::Known),
        "standby should keep the device discovered and ready for later reconnect"
    );
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
    let expected = expected_led_color([220, 120, 20]);
    assert_eq!(
        writes.last().expect("expected last frame"),
        &vec![expected; 4]
    );
}
