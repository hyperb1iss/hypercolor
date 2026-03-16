use std::time::Duration;

use hypercolor_core::device::DiscoveryConnectBehavior;
use hypercolor_driver_api::{
    DeviceAuthState, DiscoveryRequest, DriverDescriptor, DriverDiscoveredDevice, DriverTransport,
    PairDeviceRequest, PairDeviceStatus, PairingDescriptor, PairingFieldDescriptor,
    PairingFlowKind,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceId, DeviceInfo, DeviceTopologyHint, ZoneInfo,
};

#[test]
fn driver_descriptor_constructor_sets_expected_flags() {
    let descriptor =
        DriverDescriptor::new("hue", "Philips Hue", DriverTransport::Network, true, true);

    assert_eq!(descriptor.id, "hue");
    assert_eq!(descriptor.display_name, "Philips Hue");
    assert_eq!(descriptor.transport, DriverTransport::Network);
    assert!(descriptor.supports_discovery);
    assert!(descriptor.supports_pairing);
}

#[test]
fn pair_device_request_defaults_to_activation() {
    let request: PairDeviceRequest =
        serde_json::from_str(r#"{"values":{"token":"abc123"}}"#).expect("request should parse");

    assert!(request.activate_after_pair);
    assert_eq!(request.values.get("token"), Some(&"abc123".to_owned()));
}

#[test]
fn pairing_descriptor_round_trips_with_optional_fields() {
    let descriptor = PairingDescriptor {
        kind: PairingFlowKind::CredentialsForm,
        title: "Connect WLED".to_owned(),
        instructions: vec!["Enter the device credentials.".to_owned()],
        action_label: "Save Credentials".to_owned(),
        fields: vec![PairingFieldDescriptor {
            key: "password".to_owned(),
            label: "Password".to_owned(),
            secret: true,
            optional: false,
            placeholder: Some("Required".to_owned()),
        }],
    };

    let json = serde_json::to_value(&descriptor).expect("descriptor should serialize");
    let decoded: PairingDescriptor =
        serde_json::from_value(json).expect("descriptor should deserialize");

    assert_eq!(decoded.kind, PairingFlowKind::CredentialsForm);
    assert_eq!(decoded.fields.len(), 1);
    assert_eq!(decoded.fields[0].key, "password");
}

#[test]
fn discovered_device_payload_keeps_connect_behavior() {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Desk Strip".to_owned(),
        vendor: "WLED".to_owned(),
        family: DeviceFamily::Wled,
        model: None,
        connection_type: ConnectionType::Network,
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 60,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let discovered = DriverDiscoveredDevice {
        info,
        fingerprint: DeviceFingerprint("wled:desk-strip".to_owned()),
        metadata: std::collections::HashMap::from([("ip".to_owned(), "10.0.0.50".to_owned())]),
        connect_behavior: DiscoveryConnectBehavior::Deferred,
    };

    assert_eq!(discovered.metadata.get("ip"), Some(&"10.0.0.50".to_owned()));
    assert_eq!(
        discovered.connect_behavior,
        DiscoveryConnectBehavior::Deferred
    );
}

#[test]
fn discovery_request_keeps_timeout_and_mdns_flag() {
    let request = DiscoveryRequest {
        timeout: Duration::from_secs(5),
        mdns_enabled: true,
    };

    assert_eq!(request.timeout, Duration::from_secs(5));
    assert!(request.mdns_enabled);
}

#[test]
fn pair_device_status_serde_uses_snake_case() {
    let value =
        serde_json::to_value(PairDeviceStatus::AlreadyPaired).expect("status should serialize");
    assert_eq!(value, serde_json::json!("already_paired"));

    let auth_state =
        serde_json::to_value(DeviceAuthState::Configured).expect("state should serialize");
    assert_eq!(auth_state, serde_json::json!("configured"));
}
