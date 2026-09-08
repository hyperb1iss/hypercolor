//! Coverage join, conflict guard, and the inventory endpoints (Spec 81 §2.1
//! and §2.2).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::discovery::{
    BridgeOutputLock, CoverageBridgeSide, CoverageNativeSide, CoverageSource,
    CoverageUnclaimedSide, DiscoveryRuntime, GuardDecision, JoinedCoverageRow,
    enforce_native_ownership, join_coverage_sources, native_owner_reason, parse_openrgb_location,
    plan_conflict_guard,
};
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DiscoveredDevice, DiscoveryConnectBehavior, DriverHost,
};
use hypercolor_types::api::devices::{CoverageActive, CoverageIdentityKind};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    DriverTransportKind, FingerprintNamespace, SegmentInfo,
};
use hypercolor_types::event::HypercolorEvent;
use tower::ServiceExt;

static DATA_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn isolated_state() -> (AppState, tempfile::TempDir) {
    let _lock = DATA_DIR_LOCK
        .lock()
        .expect("data dir lock should not be poisoned");
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    ConfigManager::set_data_dir_override(Some(data_dir));
    let state = AppState::new();
    ConfigManager::set_data_dir_override(None);
    (state, tempdir)
}

fn native_side(name: &str, state: DeviceState) -> CoverageNativeSide {
    CoverageNativeSide {
        device_id: DeviceId::new(),
        driver_id: "nollie".to_owned(),
        name: name.to_owned(),
        state,
        serial: None,
        smbus: None,
        usb_path: None,
    }
}

fn bridge_side(name: &str, state: DeviceState) -> CoverageBridgeSide {
    CoverageBridgeSide {
        device_id: DeviceId::new(),
        name: name.to_owned(),
        state,
        advertised_output_enabled: true,
        advertised_disabled_reason: None,
        lock: None,
        serial: None,
        location: None,
    }
}

fn lock_for(native: &CoverageNativeSide) -> BridgeOutputLock {
    BridgeOutputLock {
        reason: native_owner_reason(&native.driver_id),
        native_device_id: native.device_id,
        native_driver_id: native.driver_id.clone(),
    }
}

#[test]
fn openrgb_locations_parse_into_smbus_and_usb_keys() {
    let smbus =
        parse_openrgb_location("I2C: SMBus I801 adapter at efa0 (/dev/i2c-9), address 0x71")
            .expect("i2c location should parse");
    assert_eq!(
        smbus.smbus,
        Some(("/dev/i2c-9".to_owned(), "0x71".to_owned()))
    );
    assert_eq!(smbus.usb_path, None);

    let bare = parse_openrgb_location("I2C: /dev/i2c-9, address 0x77").expect("bare bus parses");
    assert_eq!(
        bare.smbus,
        Some(("/dev/i2c-9".to_owned(), "0x77".to_owned()))
    );

    let usb = parse_openrgb_location("USB: 1-1.2").expect("host usb path parses");
    assert_eq!(usb.usb_path, Some("1-1.2".to_owned()));

    assert_eq!(parse_openrgb_location("HID: /dev/hidraw3"), None);
    assert_eq!(
        parse_openrgb_location("USB: \\\\?\\hid#vid_1532&pid_0226"),
        None,
        "platform HID paths are not host bus paths"
    );
    assert_eq!(parse_openrgb_location("I2C: , address 0x71"), None);
}

#[test]
fn coverage_joins_by_serial_case_insensitively() {
    let mut native = native_side("Nollie 32", DeviceState::Active);
    native.serial = Some("  ABC123 ".to_owned());
    let mut bridge = bridge_side("Nollie N32 (OpenRGB)", DeviceState::Known);
    bridge.serial = Some("abc123".to_owned());

    let rows = join_coverage_sources(vec![
        CoverageSource::Bridge(bridge),
        CoverageSource::Native(native),
    ]);
    assert_eq!(rows.len(), 1, "one physical device, one row");
    let row = &rows[0];
    assert_eq!(row.identity.kind, CoverageIdentityKind::Serial);
    assert_eq!(row.identity.value, "abc123");
    assert_eq!(
        row.identity.label, "Nollie 32",
        "native name labels the row"
    );
    assert!(row.native.is_some() && row.bridge.is_some());
    assert_eq!(row.active(), CoverageActive::Conflict);
}

#[test]
fn coverage_joins_by_smbus_bus_and_address() {
    let mut native = native_side("ASUS Aura DRAM", DeviceState::Connected);
    native.driver_id = "asus".to_owned();
    native.smbus = Some(("/dev/i2c-9".to_owned(), "0x71".to_owned()));
    let mut bridge = bridge_side("ENE DRAM", DeviceState::Known);
    bridge.location = Some("I2C: SMBus I801 adapter (/dev/i2c-9), address 0x71".to_owned());
    bridge.advertised_output_enabled = false;
    bridge.advertised_disabled_reason = Some("detector partition".to_owned());

    let rows = join_coverage_sources(vec![
        CoverageSource::Native(native),
        CoverageSource::Bridge(bridge),
    ]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].identity.kind, CoverageIdentityKind::Smbus);
    assert_eq!(rows[0].identity.value, "/dev/i2c-9@0x71");
    assert_eq!(
        rows[0].active(),
        CoverageActive::Native,
        "a bridge route the bridge itself disabled is no conflict"
    );

    let api_row = rows[0].clone().into_api_row();
    let bridge = api_row.bridge.expect("bridge side should project");
    assert!(!bridge.output_enabled);
    assert_eq!(
        bridge.disabled_reason.as_deref(),
        Some("detector partition")
    );
    assert_eq!(api_row.native.expect("native side").state, "connected");
}

#[test]
fn coverage_joins_unclaimed_hardware_by_usb_path() {
    let mut bridge = bridge_side("Razer Huntsman", DeviceState::Connected);
    bridge.location = Some("USB: 1-1.4".to_owned());
    let unclaimed = CoverageUnclaimedSide {
        label: "Huntsman".to_owned(),
        serial: None,
        bus_path: Some("1-1.4".to_owned()),
    };

    let rows = join_coverage_sources(vec![
        CoverageSource::Unclaimed(unclaimed),
        CoverageSource::Bridge(bridge),
    ]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].identity.kind, CoverageIdentityKind::UsbPath);
    assert!(rows[0].unclaimed.is_some());
    assert!(rows[0].native.is_none());
    assert_eq!(rows[0].active(), CoverageActive::Bridge);
    assert!(rows[0].clone().into_api_row().unclaimed);
}

#[test]
fn sources_without_keys_stay_separate_rows() {
    let native = native_side("Keyboard", DeviceState::Known);
    let bridge = bridge_side("Mystery", DeviceState::Known);
    let rows = join_coverage_sources(vec![
        CoverageSource::Native(native.clone()),
        CoverageSource::Bridge(bridge),
    ]);
    assert_eq!(rows.len(), 2);
    let keyboard = rows
        .iter()
        .find(|row| row.identity.label == "Keyboard")
        .expect("native row");
    assert_eq!(keyboard.identity.kind, CoverageIdentityKind::Device);
    assert_eq!(keyboard.identity.value, native.device_id.to_string());
    assert_eq!(keyboard.active(), CoverageActive::None);
}

#[test]
fn conflict_guard_locks_only_when_native_renders_and_bridge_is_writable() {
    let native = native_side("Nollie 32", DeviceState::Active);
    let bridge = bridge_side("Nollie (OpenRGB)", DeviceState::Known);
    let row = |native: Option<CoverageNativeSide>, bridge: Option<CoverageBridgeSide>| {
        JoinedCoverageRow {
            identity: hypercolor_types::api::devices::CoverageIdentity {
                kind: CoverageIdentityKind::Serial,
                value: "abc".to_owned(),
                label: "Nollie".to_owned(),
            },
            native,
            bridge,
            unclaimed: None,
        }
    };

    let decisions = plan_conflict_guard(&[row(Some(native.clone()), Some(bridge.clone()))]);
    assert_eq!(
        decisions,
        vec![GuardDecision::Lock {
            bridge_device_id: bridge.device_id,
            native_device_id: native.device_id,
            native_driver_id: "nollie".to_owned(),
        }]
    );

    let mut idle_native = native.clone();
    idle_native.state = DeviceState::Known;
    assert!(
        plan_conflict_guard(&[row(Some(idle_native), Some(bridge.clone()))]).is_empty(),
        "a native device that is not rendering claims nothing"
    );

    let mut bridge_off = bridge.clone();
    bridge_off.advertised_output_enabled = false;
    assert!(
        plan_conflict_guard(&[row(Some(native.clone()), Some(bridge_off))]).is_empty(),
        "a route the bridge already disabled needs no lock"
    );

    let mut locked = bridge.clone();
    locked.lock = Some(lock_for(&native));
    assert!(
        plan_conflict_guard(&[row(Some(native.clone()), Some(locked.clone()))]).is_empty(),
        "an existing lock is not re-issued"
    );

    let mut reconnecting = native.clone();
    reconnecting.state = DeviceState::Reconnecting;
    assert!(
        plan_conflict_guard(&[row(Some(reconnecting), Some(locked.clone()))]).is_empty(),
        "native keeps its claim through a reconnect blip"
    );

    let mut disabled = native.clone();
    disabled.state = DeviceState::Disabled;
    assert_eq!(
        plan_conflict_guard(&[row(Some(disabled), Some(locked.clone()))]),
        vec![GuardDecision::Unlock {
            bridge_device_id: locked.device_id,
        }],
        "the user disabling native hands the device to the bridge"
    );
    assert_eq!(
        plan_conflict_guard(&[row(None, Some(locked.clone()))]),
        vec![GuardDecision::Unlock {
            bridge_device_id: locked.device_id,
        }],
        "a native device that left releases the lock"
    );
    assert!(
        plan_conflict_guard(&[row(Some(native), None)]).is_empty(),
        "rows without a bridge side never decide anything"
    );
}

fn segment() -> SegmentInfo {
    SegmentInfo {
        name: "Strip".to_owned(),
        led_count: 8,
        topology: DeviceTopologyHint::Strip,
        color_format: DeviceColorFormat::Rgb,
        layout_hint: None,
    }
}

fn native_device(serial: &str) -> DiscoveredDevice {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Nollie 32".to_owned(),
        vendor: "Nollie".to_owned(),
        family: DeviceFamily::new_static("nollie", "Nollie"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("nollie", "usb", ConnectionType::Usb),
        segments: vec![segment()],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    DiscoveredDevice {
        fingerprint: DeviceFingerprint::mint(FingerprintNamespace::Usb, "nollie", serial),
        connect_behavior: DiscoveryConnectBehavior::Deferred,
        info,
        metadata: HashMap::from([("serial".to_owned(), serial.to_owned())]),
        claim: None,
    }
}

fn bridge_device(serial: &str) -> DiscoveredDevice {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Nollie N32 (OpenRGB)".to_owned(),
        vendor: "Nollie".to_owned(),
        family: DeviceFamily::new_static("openrgb", "OpenRGB"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::new("openrgb", "openrgb", DriverTransportKind::Bridge),
        segments: vec![segment()],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    let fingerprint = format!("bridge:openrgb:127.0.0.1:6742:serial:{serial}");
    DiscoveredDevice {
        fingerprint: DeviceFingerprint::mint(FingerprintNamespace::Bridge, "openrgb", serial),
        connect_behavior: DiscoveryConnectBehavior::Deferred,
        info,
        metadata: HashMap::from([
            ("serial".to_owned(), serial.to_lowercase()),
            ("endpoint".to_owned(), "127.0.0.1:6742".to_owned()),
            ("controller_index".to_owned(), "3".to_owned()),
            ("identity_confidence".to_owned(), "stable".to_owned()),
            ("detector_class".to_owned(), "Nollie".to_owned()),
            ("output_enabled".to_owned(), "true".to_owned()),
            ("protocol_version".to_owned(), "5".to_owned()),
            ("fingerprint".to_owned(), fingerprint),
        ]),
        claim: None,
    }
}

async fn track(
    runtime: &DiscoveryRuntime,
    discovered: DiscoveredDevice,
    state: DeviceState,
) -> DeviceId {
    let info = discovered.info.clone();
    let id = runtime.device_registry.add_discovered(discovered).await;
    let mut tracked_info = info;
    tracked_info.id = id;
    {
        let mut lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.on_discovered_with_behavior(
            id,
            &tracked_info,
            None,
            DiscoveryConnectBehavior::Deferred,
        );
        if state.is_renderable() {
            lifecycle.on_connected(id).expect("fixture should connect");
        }
    }
    runtime.device_registry.set_state(&id, state).await;
    id
}

struct NativeFixtureBackend;

#[async_trait::async_trait]
impl DeviceBackend for NativeFixtureBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "usb".to_owned(),
            name: "Fixture".to_owned(),
            description: "Native activation fixture".to_owned(),
        }
    }
    fn adopt_device(&self, _discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        Ok(())
    }
    async fn connect(&self, _id: &DeviceId) -> Result<(), DeviceError> {
        Ok(())
    }
    async fn disconnect(&self, _id: &DeviceId) -> Result<(), DeviceError> {
        Ok(())
    }
    async fn write_colors(&self, _id: &DeviceId, _colors: &[[u8; 3]]) -> Result<(), DeviceError> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn native_state_change_and_reconnect_guard_bridge_without_discovery() {
    for reconnect in [false, true] {
        let (state, _tmp) = isolated_state();
        let runtime = state.driver_host().discovery_runtime();
        runtime
            .backend_manager
            .lock()
            .await
            .register_backend(Arc::new(NativeFixtureBackend));
        let native_id = track(
            &runtime,
            native_device("late-native"),
            DeviceState::Connected,
        )
        .await;
        let bridge_id = track(&runtime, bridge_device("late-native"), DeviceState::Known).await;
        if reconnect {
            state
                .driver_host()
                .runtime()
                .request_reconnect(native_id, "usb", None)
                .await
                .expect("native reconnect");
        } else {
            hypercolor_daemon::discovery::enforce_native_ownership_for_device(&runtime, native_id)
                .await;
        }
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !runtime.bridge_output_locks.is_locked(&bridge_id) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("native connection must disable conflicting bridge without a discovery pass");
    }
}

#[tokio::test]
async fn conflict_guard_does_not_publish_a_lock_when_lifecycle_device_is_missing() {
    let (state, _tmp) = isolated_state();
    let runtime = state.driver_host().discovery_runtime();
    track(
        &runtime,
        native_device("missing-lifecycle"),
        DeviceState::Connected,
    )
    .await;
    let bridge_id = runtime
        .device_registry
        .add_discovered(bridge_device("missing-lifecycle"))
        .await;
    let report = enforce_native_ownership(&runtime).await;
    assert!(report.locked.is_empty());
    assert!(!runtime.bridge_output_locks.is_locked(&bridge_id));
}

#[tokio::test]
async fn the_guard_disables_a_shadowed_bridge_route_and_lifts_it_when_native_is_disabled() {
    let (state, _tmp) = isolated_state();
    let runtime = state.driver_host().discovery_runtime();
    let mut events = state.event_bus.subscribe_all();

    let native_id = track(
        &runtime,
        native_device("0994FA72AB3C"),
        DeviceState::Connected,
    )
    .await;
    let bridge_id = track(&runtime, bridge_device("0994FA72AB3C"), DeviceState::Known).await;

    let report = enforce_native_ownership(&runtime).await;
    assert_eq!(report.locked, vec![bridge_id]);
    assert!(report.unlocked.is_empty());

    let lock = runtime
        .bridge_output_locks
        .get(&bridge_id)
        .expect("guard should hold the bridge route");
    assert_eq!(lock.reason, "native driver owns this device (nollie)");
    assert_eq!(lock.native_device_id, native_id);
    let bridge = runtime
        .device_registry
        .get(&bridge_id)
        .await
        .expect("bridge device tracked");
    assert_eq!(bridge.state, DeviceState::Disabled);
    assert!(
        bridge.user_settings.enabled,
        "a guard lock is not a user disable"
    );

    let mut saw_lock_event = false;
    while let Ok(timestamped) = events.try_recv() {
        if let HypercolorEvent::DeviceStateChanged { device_id, changes } = timestamped.event
            && device_id == bridge_id.to_string()
        {
            assert_eq!(changes["output_enabled"], serde_json::json!(false));
            assert_eq!(
                changes["disabled_reason"],
                serde_json::json!("native driver owns this device (nollie)")
            );
            saw_lock_event = true;
        }
    }
    assert!(saw_lock_event, "the lock must publish DeviceStateChanged");

    // Idempotent: a second pass with the same rows changes nothing.
    let report = enforce_native_ownership(&runtime).await;
    assert!(report.locked.is_empty() && report.unlocked.is_empty());

    // The summary reports the effective output state with the guard's reason.
    let app = api::build_router(Arc::new(state), None);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{bridge_id}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("device request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    let bridge_summary = &body["data"]["bridge"];
    assert_eq!(bridge_summary["output_enabled"], serde_json::json!(false));
    assert_eq!(
        bridge_summary["disabled_reason"],
        serde_json::json!("native driver owns this device (nollie)")
    );
    assert_eq!(
        bridge_summary["endpoint"],
        serde_json::json!("127.0.0.1:6742")
    );
    assert_eq!(bridge_summary["controller_index"], serde_json::json!(3));
    assert_eq!(bridge_summary["protocol_version"], serde_json::json!(5));
    assert_eq!(
        bridge_summary["fingerprint"],
        serde_json::json!("bridge:openrgb:127.0.0.1:6742:serial:0994FA72AB3C")
    );
    assert_eq!(
        body["data"]["connection"]["label"],
        serde_json::json!("127.0.0.1:6742 controller 3")
    );
    assert_eq!(
        body["data"]["connection"]["endpoint"],
        serde_json::json!("127.0.0.1:6742")
    );

    hypercolor_daemon::discovery::apply_user_enabled_state(&runtime, native_id, false)
        .await
        .expect("native user disable");
    assert!(!runtime.bridge_output_locks.is_locked(&bridge_id));
    let bridge = runtime
        .device_registry
        .get(&bridge_id)
        .await
        .expect("bridge device tracked");
    assert_eq!(
        bridge.state,
        DeviceState::Known,
        "the route returns to normal connect gating"
    );
}

#[tokio::test]
async fn re_enabling_a_guard_locked_bridge_route_is_refused() {
    let (state, _tmp) = isolated_state();
    let runtime = state.driver_host().discovery_runtime();
    track(&runtime, native_device("SER1"), DeviceState::Active).await;
    let bridge_id = track(&runtime, bridge_device("SER1"), DeviceState::Known).await;
    enforce_native_ownership(&runtime).await;

    let app = api::build_router(Arc::new(state), None);
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/devices/{bridge_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .expect("request should build"),
        )
        .await
        .expect("update request should complete");
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn coverage_and_unclaimed_routes_answer_complete_lists() {
    let (state, _tmp) = isolated_state();
    let runtime = state.driver_host().discovery_runtime();
    track(&runtime, native_device("SER9"), DeviceState::Connected).await;
    track(&runtime, bridge_device("SER9"), DeviceState::Known).await;
    runtime
        .unclaimed_devices
        .replace_snapshot([hypercolor_core::device::UsbObservation {
            vendor_id: 0x1234,
            product_id: 0x0001,
            manufacturer: None,
            product: Some("Widget".to_owned()),
            serial: Some("W-1".to_owned()),
            bus_path: Some("1-1.9".to_owned()),
            interface_classes: vec![3],
            descriptor_driver_id: None,
        }]);

    let app = api::build_router(Arc::new(state), None);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/coverage")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("coverage request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    let items = body["data"]["items"].as_array().expect("items array");
    assert_eq!(body["data"]["total"], serde_json::json!(items.len()));
    assert!(body["data"].get("page").is_none(), "coverage is not paged");
    let nollie = items
        .iter()
        .find(|row| row["identity"]["value"] == "ser9")
        .expect("joined nollie row");
    assert_eq!(nollie["active"], "conflict");
    assert_eq!(nollie["native"]["driver_id"], "nollie");
    assert_eq!(nollie["bridge"]["output_enabled"], true);
    let widget = items
        .iter()
        .find(|row| row["identity"]["label"] == "Widget")
        .expect("unclaimed row");
    assert_eq!(widget["unclaimed"], true);
    assert_eq!(widget["active"], "none");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/unclaimed")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("unclaimed request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(body["data"]["total"], 1);
    assert_eq!(body["data"]["items"][0]["vendor_id"], 0x1234);
    assert_eq!(body["data"]["items"][0]["product"], "Widget");
    assert_eq!(
        body["data"]["items"][0]["claimable_by"],
        serde_json::Value::Null
    );
}
