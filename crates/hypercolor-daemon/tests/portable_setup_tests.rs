use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::portable_setup::{PortableSetupService, SetupIdentity};
use hypercolor_types::device::*;
use hypercolor_types::portable_setup::*;
use std::sync::Arc;
use uuid::Uuid;

fn device(path: &str) -> (DeviceInfo, DeviceFingerprint) {
    let fingerprint = DeviceFingerprint::from_persisted(format!("usb:razer:1532:0099:{path}"));
    let info = DeviceInfo {
        id: fingerprint.stable_device_id(),
        name: "Controller".into(),
        vendor: "Razer".into(),
        family: DeviceFamily::new("razer", "Razer"),
        model: Some("case".into()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("razer", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "Main".into(),
            led_count: 16,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 16,
            ..Default::default()
        },
    };
    (info, fingerprint)
}

#[tokio::test]
async fn empty_installation_export_is_typed_and_repeatable() {
    let state = Arc::new(AppState::new());
    let service = PortableSetupService::new(state).expect("service");
    let identity = SetupIdentity {
        setup_id: Uuid::now_v7(),
        name: "Studio".into(),
    };
    let bindings = service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("bindings");
    let first = service
        .export(identity.clone(), &bindings)
        .await
        .expect("export");
    let next = service
        .export(identity, &bindings)
        .await
        .expect("repeat export");
    assert_eq!(first.manifest, next.manifest);
    assert_eq!(first.revision, next.revision);
    first.manifest.validate().expect("valid export");
    let value = serde_json::to_value(&first.manifest).expect("serialize");
    for forbidden in ["drivers", "credentials", "capture_device", "audio_device"] {
        assert!(value.get(forbidden).is_none());
    }
}

#[tokio::test]
async fn physical_setup_association_survives_service_restart() {
    let state = Arc::new(AppState::new());
    let (info, fingerprint) = device("linux-hidraw8");
    let local_id = info.id;
    state
        .device_registry
        .add_with_fingerprint(info, fingerprint)
        .await;
    let id = Uuid::now_v7();
    let service = PortableSetupService::new(Arc::clone(&state)).expect("service");
    let first = service
        .assign_bindings(id, None)
        .await
        .expect("initial association");
    drop(service);
    let reopened = PortableSetupService::new(state).expect("reopened service");
    let again = reopened
        .assign_bindings(id, None)
        .await
        .expect("persisted association");
    assert_eq!(first.devices.get(&local_id), again.devices.get(&local_id));
}

#[tokio::test]
async fn claimless_usb_rebind_uses_existing_unique_shape_evidence() {
    let source = Arc::new(AppState::new());
    let (linux, linux_fingerprint) = device("linux-hidraw8");
    let linux_id = linux.id;
    source
        .device_registry
        .add_with_fingerprint(linux, linux_fingerprint)
        .await;
    let source_service = PortableSetupService::new(source).expect("source service");
    let identity = SetupIdentity {
        setup_id: Uuid::now_v7(),
        name: "Dual boot".into(),
    };
    let source_bindings = source_service
        .assign_bindings(identity.setup_id, None)
        .await
        .expect("source association");
    let exported = source_service
        .export(identity.clone(), &source_bindings)
        .await
        .expect("source export");
    let destination = Arc::new(AppState::new());
    let (windows, windows_fingerprint) = device("windows-device-path");
    let windows_id = windows.id;
    destination
        .device_registry
        .add_with_fingerprint(windows, windows_fingerprint)
        .await;
    let service = PortableSetupService::new(destination).expect("destination service");
    let bindings = service
        .assign_bindings(identity.setup_id, Some(&exported.manifest))
        .await
        .expect("rebind");
    assert_eq!(
        bindings.devices.get(&windows_id),
        source_bindings.devices.get(&linux_id)
    );
}

#[test]
fn shared_rust_fixture_is_valid_for_fresh_restore() {
    let manifest: PortableSetupV1 = serde_json::from_str(include_str!(
        "../../hypercolor-types/tests/fixtures/portable_setup_v1.json"
    ))
    .expect("fixture");
    manifest
        .validate_with_catalog(&PortableReferenceCatalog::default())
        .expect("closed fixture graph");
}

#[tokio::test]
async fn incomplete_discovery_does_not_persist_new_setup_associations() {
    use std::sync::atomic::Ordering;
    let state = Arc::new(AppState::new());
    let (info, fingerprint) = device("partial-sweep");
    state
        .device_registry
        .add_with_fingerprint(info, fingerprint)
        .await;
    let id = Uuid::now_v7();
    let service = PortableSetupService::new(Arc::clone(&state)).expect("service");
    state.discovery_in_progress.store(true, Ordering::Release);
    assert!(service.assign_bindings(id, None).await.is_err());
    assert!(
        !state
            .state_dir
            .join("portable-setups")
            .join(format!("{id}.bindings.json"))
            .exists()
    );
    state.discovery_in_progress.store(false, Ordering::Release);
    let bindings = service
        .assign_bindings(id, None)
        .await
        .expect("complete sweep association");
    assert_eq!(bindings.devices.len(), 1);
}
