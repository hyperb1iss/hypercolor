//! Tests for device identity, capabilities, and state types.

use std::time::Duration;

use hypercolor_color::DevicePixelLayout;
use hypercolor_types::device::{
    ConnectionType, DRIVER_MODULE_API_SCHEMA_VERSION, DeviceCapabilities, DeviceClassHint,
    DeviceColorFormat, DeviceColorSpace, DeviceError, DeviceFamily, DeviceFeatures,
    DeviceFingerprint, DeviceHandle, DeviceId, DeviceIdentifier, DeviceInfo, DeviceOrigin,
    DeviceState, DeviceTopologyHint, DeviceUserSettings, DriverCapabilitySet,
    DriverModuleDescriptor, DriverModuleKind, DriverPresentation, DriverTransportAvailability,
    DriverTransportDescriptor, DriverTransportKind, FingerprintNamespace, SegmentInfo,
    SegmentLayoutHint,
};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, ZoneShape};
use uuid::Uuid;

// ── DeviceId ──────────────────────────────────────────────────────────────

#[test]
fn device_id_unique_on_each_call() {
    let a = DeviceId::new();
    let b = DeviceId::new();
    assert_ne!(a, b);
}

#[test]
fn device_id_from_uuid_round_trips() {
    let uuid = Uuid::now_v7();
    let id = DeviceId::from_uuid(uuid);
    assert_eq!(id.as_uuid(), uuid);
}

#[test]
fn device_id_display_matches_uuid() {
    let uuid = Uuid::now_v7();
    let id = DeviceId::from_uuid(uuid);
    assert_eq!(id.to_string(), uuid.to_string());
}

#[test]
fn device_id_parse_from_string() {
    let id = DeviceId::new();
    let s = id.to_string();
    let parsed: DeviceId = s.parse().expect("valid uuid string");
    assert_eq!(parsed, id);
}

#[test]
fn device_id_serde_round_trip() {
    let id = DeviceId::new();
    let json = serde_json::to_string(&id).expect("serialize");
    let back: DeviceId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, id);
}

#[test]
fn device_id_default_generates_unique() {
    let a = DeviceId::default();
    let b = DeviceId::default();
    assert_ne!(a, b);
}

// ── DeviceInfo ────────────────────────────────────────────────────────────

fn sample_device_info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "Test Strip".into(),
        vendor: "Fixture Strip".into(),
        family: DeviceFamily::new_static("fixture-network", "Fixture Strip"),
        model: Some("strip".into()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native(
            "fixture-network",
            "fixture-network",
            ConnectionType::Network,
        ),
        segments: vec![
            SegmentInfo {
                name: "Main".into(),
                led_count: 60,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            SegmentInfo {
                name: "Accent".into(),
                led_count: 30,
                topology: DeviceTopologyHint::Ring { count: 30 },
                color_format: DeviceColorFormat::Rgbw,
                layout_hint: None,
            },
        ],
        firmware_version: Some("0.15.0".into()),
        capabilities: DeviceCapabilities {
            led_count: 90,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: DeviceColorSpace::Rgb,
            features: DeviceFeatures::default(),
        },
    }
}

#[test]
fn device_info_total_led_count() {
    let info = sample_device_info();
    assert_eq!(info.total_led_count(), 90);
}

#[test]
fn device_info_exposes_driver_and_output_backend_separately() {
    let mut info = sample_device_info();
    info.origin = DeviceOrigin::native("fixture-driver", "usb", ConnectionType::Usb);

    assert_eq!(info.driver_id(), "fixture-driver");
    assert_eq!(info.output_backend_id(), "usb");
}

#[test]
fn device_info_serde_round_trip() {
    let info = sample_device_info();
    let json = serde_json::to_string_pretty(&info).expect("serialize");
    let value: serde_json::Value = serde_json::from_str(&json).expect("deserialize value");
    assert!(value.get("segments").is_some());
    assert!(value.get("zones").is_none());
    let back: DeviceInfo = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, "Test Strip");
    assert_eq!(back.total_led_count(), 90);
    assert_eq!(back.firmware_version, Some("0.15.0".into()));
}

#[test]
fn device_info_empty_segments_yields_zero_leds() {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Empty".into(),
        vendor: "Test".into(),
        family: DeviceFamily::named("test"),
        model: None,
        connection_type: ConnectionType::Bridge,
        origin: DeviceOrigin::native("test", "test", ConnectionType::Bridge),
        segments: vec![],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    assert_eq!(info.total_led_count(), 0);
}

// ── DeviceCapabilities ────────────────────────────────────────────────────

#[test]
fn capabilities_default_values() {
    let caps = DeviceCapabilities::default();
    assert_eq!(caps.led_count, 0);
    assert!(caps.supports_direct);
    assert!(!caps.supports_brightness);
    assert_eq!(caps.max_fps, 60);
    assert_eq!(caps.color_space, DeviceColorSpace::Rgb);
}

#[test]
fn capabilities_serde_round_trip() {
    let caps = DeviceCapabilities {
        led_count: 144,
        supports_direct: false,
        supports_brightness: true,
        has_display: false,
        display_resolution: None,
        max_fps: 30,
        color_space: DeviceColorSpace::CieXy,
        features: DeviceFeatures::default(),
    };
    let json = serde_json::to_string(&caps).expect("serialize");
    let back: DeviceCapabilities = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, caps);
}

// ── DeviceUserSettings ────────────────────────────────────────────────────

#[test]
fn user_settings_default_values() {
    let settings = DeviceUserSettings::default();
    assert_eq!(settings.name, None);
    assert!(settings.enabled);
    assert!((settings.brightness - 1.0).abs() < f32::EPSILON);
}

#[test]
fn user_settings_serde_round_trip() {
    let settings = DeviceUserSettings {
        name: Some("Desk Strip".into()),
        enabled: false,
        brightness: 0.42,
    };
    let json = serde_json::to_string(&settings).expect("serialize");
    let back: DeviceUserSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, settings);
}

// ── DeviceState ───────────────────────────────────────────────────────────

#[test]
fn device_state_variant_names() {
    assert_eq!(DeviceState::Known.variant_name(), "Known");
    assert_eq!(DeviceState::Connected.variant_name(), "Connected");
    assert_eq!(DeviceState::Active.variant_name(), "Active");
    assert_eq!(DeviceState::Reconnecting.variant_name(), "Reconnecting");
    assert_eq!(DeviceState::Disabled.variant_name(), "Disabled");
}

#[test]
fn device_state_is_renderable() {
    assert!(!DeviceState::Known.is_renderable());
    assert!(DeviceState::Connected.is_renderable());
    assert!(DeviceState::Active.is_renderable());
    assert!(!DeviceState::Reconnecting.is_renderable());
    assert!(!DeviceState::Disabled.is_renderable());
}

#[test]
fn device_state_display() {
    assert_eq!(DeviceState::Active.to_string(), "Active");
    assert_eq!(DeviceState::Reconnecting.to_string(), "Reconnecting");
}

#[test]
fn device_state_serde_round_trip() {
    for state in [
        DeviceState::Known,
        DeviceState::Connected,
        DeviceState::Active,
        DeviceState::Reconnecting,
        DeviceState::Disabled,
    ] {
        let json = serde_json::to_string(&state).expect("serialize");
        let back: DeviceState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, state);
    }
}

// ── LedTopology ───────────────────────────────────────────────────────────

#[test]
fn led_topology_variants_exist() {
    let topologies = [
        DeviceTopologyHint::Strip,
        DeviceTopologyHint::Matrix { rows: 8, cols: 32 },
        DeviceTopologyHint::Ring { count: 24 },
        DeviceTopologyHint::Point,
        DeviceTopologyHint::Custom,
    ];
    assert_eq!(topologies.len(), 5);
}

#[test]
fn led_topology_serde_round_trip() {
    let matrix = DeviceTopologyHint::Matrix { rows: 4, cols: 16 };
    let json = serde_json::to_string(&matrix).expect("serialize");
    let back: DeviceTopologyHint = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, matrix);
}

// ── ConnectionType ────────────────────────────────────────────────────────

#[test]
fn connection_type_is_copy() {
    let ct = ConnectionType::Usb;
    let ct2 = ct; // Copy
    assert_eq!(ct, ct2);
}

#[test]
fn connection_type_serde_round_trip() {
    for ct in [
        ConnectionType::Usb,
        ConnectionType::SmBus,
        ConnectionType::Network,
        ConnectionType::Bluetooth,
        ConnectionType::Bridge,
    ] {
        let json = serde_json::to_string(&ct).expect("serialize");
        let back: ConnectionType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ct);
    }
}

// ── Driver Metadata ───────────────────────────────────────────────────────

#[test]
fn driver_capability_set_empty_has_no_capabilities() {
    let capabilities = DriverCapabilitySet::empty();
    assert_eq!(capabilities, DriverCapabilitySet::default());
    assert!(!capabilities.config);
    assert!(!capabilities.discovery);
    assert!(!capabilities.pairing);
    assert!(!capabilities.output_backend);
    assert!(!capabilities.protocol_catalog);
    assert!(!capabilities.runtime_cache);
    assert!(!capabilities.credentials);
    assert!(!capabilities.presentation);
    assert!(!capabilities.controls);
}

#[test]
fn driver_capability_set_round_trips_and_requires_controls() {
    let capabilities = DriverCapabilitySet {
        discovery: true,
        output_backend: true,
        controls: true,
        ..DriverCapabilitySet::empty()
    };
    let json = serde_json::to_value(capabilities).expect("serialize capabilities");
    let roundtrip: DriverCapabilitySet =
        serde_json::from_value(json.clone()).expect("deserialize capabilities");
    assert_eq!(roundtrip, capabilities);

    let mut missing_controls = json;
    missing_controls
        .as_object_mut()
        .expect("capabilities are an object")
        .remove("controls");
    let error = serde_json::from_value::<DriverCapabilitySet>(missing_controls)
        .expect_err("controls is required");
    assert!(
        error.to_string().contains("missing field `controls`"),
        "{error}"
    );
}

#[test]
fn driver_transport_kind_round_trips_custom_transport() {
    let transport = DriverTransportKind::Custom("openlinkhub".into());
    let json = serde_json::to_string(&transport).expect("serialize");
    let back: DriverTransportKind = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, transport);
}

#[test]
fn driver_transport_kind_exposes_stable_api_ids() {
    let cases = [
        (DriverTransportKind::Network, "network"),
        (DriverTransportKind::Usb, "usb"),
        (DriverTransportKind::Smbus, "smbus"),
        (DriverTransportKind::Midi, "midi"),
        (DriverTransportKind::Serial, "serial"),
        (DriverTransportKind::Bridge, "bridge"),
        (DriverTransportKind::Virtual, "virtual"),
        (
            DriverTransportKind::Custom("openlinkhub".into()),
            "openlinkhub",
        ),
    ];

    for (transport, expected) in cases {
        assert_eq!(transport.as_id(), expected);
    }
}

#[test]
fn driver_transport_kind_maps_to_module_kind() {
    let cases = [
        (DriverTransportKind::Network, DriverModuleKind::Network),
        (DriverTransportKind::Usb, DriverModuleKind::Hal),
        (DriverTransportKind::Smbus, DriverModuleKind::Hal),
        (DriverTransportKind::Midi, DriverModuleKind::Hal),
        (DriverTransportKind::Serial, DriverModuleKind::Hal),
        (DriverTransportKind::Bridge, DriverModuleKind::Bridge),
        (DriverTransportKind::Virtual, DriverModuleKind::Virtual),
        (
            DriverTransportKind::Custom("openlinkhub".into()),
            DriverModuleKind::Virtual,
        ),
    ];

    for (transport, expected) in cases {
        assert_eq!(transport.module_kind(), expected);
    }
}

#[test]
fn driver_transport_kind_preserves_bridge_connections() {
    let transport = DriverTransportKind::from(ConnectionType::Bridge);

    assert_eq!(transport, DriverTransportKind::Bridge);
}

#[test]
fn device_origin_separates_driver_from_backend() {
    let origin = DeviceOrigin::new("fixture-driver", "usb", DriverTransportKind::Usb)
        .with_protocol_id("fixture/protocol");

    assert_eq!(origin.driver_id, "fixture-driver");
    assert_eq!(origin.backend_id, "usb");
    assert_eq!(origin.protocol_id.as_deref(), Some("fixture/protocol"));

    let json = serde_json::to_string(&origin).expect("serialize");
    let back: DeviceOrigin = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, origin);
}

#[test]
fn device_origin_omits_absent_protocol_id() {
    let origin = DeviceOrigin::new(
        "fixture-network",
        "fixture-network",
        DriverTransportKind::Network,
    );
    let json = serde_json::to_string(&origin).expect("serialize");

    assert!(!json.contains("protocol_id"));
}

#[test]
fn driver_presentation_serializes_optional_metadata() {
    let presentation = DriverPresentation {
        label: "Fixture Light".into(),
        short_label: Some("Light".into()),
        accent_rgb: Some([0x80, 0xff, 0xea]),
        secondary_rgb: None,
        icon: Some("panel-top".into()),
        default_device_class: Some(DeviceClassHint::Light),
    };

    let json = serde_json::to_string(&presentation).expect("serialize");
    assert!(!json.contains("secondary_rgb"));

    let back: DriverPresentation = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, presentation);
}

#[test]
fn driver_module_descriptor_round_trips_capabilities_and_transports() {
    let descriptor = DriverModuleDescriptor {
        id: "fixture-hal".into(),
        display_name: "Fixture HAL".into(),
        vendor_name: Some("Fixture HAL".into()),
        module_kind: DriverModuleKind::Hal,
        transports: vec![DriverTransportDescriptor::available(
            DriverTransportKind::Usb,
        )],
        capabilities: DriverCapabilitySet {
            protocol_catalog: true,
            presentation: true,
            ..DriverCapabilitySet::empty()
        },
        api_schema_version: DRIVER_MODULE_API_SCHEMA_VERSION,
        config_version: 1,
        default_enabled: true,
    };

    let json = serde_json::to_string(&descriptor).expect("serialize");
    let back: DriverModuleDescriptor = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(back, descriptor);
}

#[test]
fn driver_transport_descriptor_serializes_platform_availability() {
    let transports = vec![
        DriverTransportDescriptor::available(DriverTransportKind::Usb),
        DriverTransportDescriptor::unsupported_platform(DriverTransportKind::Smbus, "macOS"),
    ];

    let json = serde_json::to_value(&transports).expect("transport inventory should serialize");

    assert_eq!(json[0]["kind"], "usb");
    assert_eq!(json[0]["availability"]["status"], "available");
    assert_eq!(json[1]["kind"], "smbus");
    assert_eq!(json[1]["availability"]["status"], "unsupported_platform");
    assert_eq!(json[1]["availability"]["platform"], "macOS");
    assert!(transports[0].is_available());
    assert_eq!(
        transports[1].availability,
        DriverTransportAvailability::UnsupportedPlatform {
            platform: "macOS".to_owned(),
        }
    );
}

// ── DeviceFamily ──────────────────────────────────────────────────────────

#[test]
fn device_family_display() {
    assert_eq!(
        DeviceFamily::new_static("fixture-network", "Fixture Strip").to_string(),
        "Fixture Strip"
    );
    assert_eq!(
        DeviceFamily::new("fixture-hal", "Fixture HAL").to_string(),
        "Fixture HAL"
    );
    assert_eq!(
        DeviceFamily::named("Fixture HAL").to_string(),
        "Fixture HAL"
    );
}

#[test]
fn device_family_equality() {
    assert_eq!(
        DeviceFamily::new_static("fixture-network", "Fixture Strip"),
        DeviceFamily::new_static("fixture-network", "Fixture Strip")
    );
    assert_ne!(
        DeviceFamily::new_static("fixture-network", "Fixture Strip"),
        DeviceFamily::new_static("fixture-bridge", "Fixture Bridge")
    );
    assert_ne!(
        DeviceFamily::new_static("fixture-bridge", "Fixture Bridge"),
        DeviceFamily::new_static("fixture-light", "Fixture Light")
    );
    assert_eq!(DeviceFamily::named("Foo"), DeviceFamily::named("Foo"));
    assert_ne!(DeviceFamily::named("Foo"), DeviceFamily::named("Bar"));
}

#[test]
fn device_family_serde_round_trip() {
    let families = vec![
        DeviceFamily::new_static("fixture-network", "Fixture Strip"),
        DeviceFamily::new_static("fixture-bridge", "Fixture Bridge"),
        DeviceFamily::new_static("fixture-light", "Fixture Light"),
        DeviceFamily::new_static("fixture-input", "Fixture Input"),
        DeviceFamily::new_static("fixture-display", "Fixture Display"),
        DeviceFamily::new_static("fixture-keyboard", "Fixture Keyboard"),
        DeviceFamily::new_static("fixture-accessory", "Fixture Accessory"),
        DeviceFamily::new_static("fixture-driver", "Fixture Driver"),
        DeviceFamily::new_static("fixture-hal", "Fixture HAL"),
        DeviceFamily::new_static("fixture-smbus", "Fixture SMBus"),
        DeviceFamily::named("Fixture HAL"),
    ];
    for family in families {
        let json = serde_json::to_string(&family).expect("serialize");
        let back: DeviceFamily = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, family);
    }
}

#[test]
fn device_family_rejects_non_object_payloads() {
    let error = serde_json::from_str::<DeviceFamily>(r#""Wled""#)
        .expect_err("family strings should not deserialize");
    assert!(error.is_data());

    let error = serde_json::from_str::<DeviceFamily>(r#"{"Custom":"Fixture HAL"}"#)
        .expect_err("family custom maps should not deserialize");
    assert!(error.is_data());
}

// ── Color ─────────────────────────────────────────────────────────────────

#[test]
fn color_format_display() {
    assert_eq!(DeviceColorFormat::Rgb.to_string(), "RGB");
    assert_eq!(DeviceColorFormat::Rgbw.to_string(), "RGBW");
    assert_eq!(DeviceColorFormat::Grb.to_string(), "GRB");
    assert_eq!(DeviceColorFormat::Rbg.to_string(), "RBG");
}

#[test]
fn color_format_maps_to_pixel_layouts() {
    assert_eq!(
        DeviceColorFormat::Rgb.pixel_layout(),
        Some(DevicePixelLayout::Rgb)
    );
    assert_eq!(
        DeviceColorFormat::Grb.pixel_layout(),
        Some(DevicePixelLayout::Grb)
    );
    assert_eq!(
        DeviceColorFormat::Rbg.pixel_layout(),
        Some(DevicePixelLayout::Rbg)
    );
    assert_eq!(
        DeviceColorFormat::Rgbw.pixel_layout(),
        Some(DevicePixelLayout::RgbwZeroWhite)
    );
}

#[test]
fn color_space_defaults_to_rgb() {
    assert_eq!(DeviceColorSpace::default(), DeviceColorSpace::Rgb);
}

#[test]
fn color_space_serde_round_trip() {
    for color_space in [DeviceColorSpace::Rgb, DeviceColorSpace::CieXy] {
        let json = serde_json::to_string(&color_space).expect("serialize");
        let back: DeviceColorSpace = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, color_space);
    }
}

#[test]
fn color_format_serde_round_trip() {
    for fmt in [
        DeviceColorFormat::Rgb,
        DeviceColorFormat::Rgbw,
        DeviceColorFormat::Grb,
        DeviceColorFormat::Rbg,
    ] {
        let json = serde_json::to_string(&fmt).expect("serialize");
        let back: DeviceColorFormat = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, fmt);
    }
}

// ── DeviceError ───────────────────────────────────────────────────────────

#[test]
fn device_error_display_messages() {
    let err = DeviceError::ConnectionFailed {
        device: "Fixture Kitchen".into(),
        reason: "TCP refused".into(),
    };
    assert_eq!(
        err.to_string(),
        "connection to Fixture Kitchen failed: TCP refused"
    );

    let err = DeviceError::NotFound {
        device: "Prism 8".into(),
    };
    assert_eq!(err.to_string(), "device not found: Prism 8");

    let err = DeviceError::Timeout {
        after: Duration::from_secs(2),
    };
    assert_eq!(err.to_string(), "device operation timed out after 2s");

    let err = DeviceError::WriteError {
        device: "USB Controller".into(),
        detail: "HID write returned -1".into(),
    };
    assert_eq!(
        err.to_string(),
        "write error on USB Controller: HID write returned -1"
    );

    let err = DeviceError::ProtocolError {
        device: "Fixture Strip".into(),
        detail: "unexpected packet type 0xFF".into(),
    };
    assert_eq!(
        err.to_string(),
        "protocol error for Fixture Strip: unexpected packet type 0xFF"
    );

    let err = DeviceError::Disconnected {
        device: "USB Controller".into(),
    };
    assert_eq!(err.to_string(), "device disconnected: USB Controller");

    let err = DeviceError::InvalidHandle {
        handle_id: 42,
        backend: "fixture-network".into(),
    };
    assert_eq!(
        err.to_string(),
        "invalid handle 42 for backend fixture-network"
    );

    let err = DeviceError::InvalidTransition {
        device: "Fixture Strip".into(),
        from: "Known".into(),
        to: "Active".into(),
    };
    assert_eq!(
        err.to_string(),
        "invalid device transition for Fixture Strip: Known -> Active"
    );

    let err = DeviceError::Unsupported {
        backend: "fixture-network".into(),
        operation: "display output",
    };
    assert_eq!(
        err.to_string(),
        "backend fixture-network does not support display output"
    );

    let err = DeviceError::PermissionDenied {
        device: "USB Controller".into(),
        detail: "udev policy rejected access".into(),
    };
    assert_eq!(
        err.to_string(),
        "permission denied for USB Controller: udev policy rejected access"
    );
}

#[test]
fn device_error_recoverability_is_typed() {
    use hypercolor_types::device::ErrorRecoverability;

    assert_eq!(
        DeviceError::ConnectionFailed {
            device: String::new(),
            reason: String::new()
        }
        .recoverability(),
        ErrorRecoverability::Reconnect
    );

    assert_eq!(
        DeviceError::WriteError {
            device: String::new(),
            detail: String::new()
        }
        .recoverability(),
        ErrorRecoverability::Reconnect
    );

    assert_eq!(
        DeviceError::Timeout {
            after: Duration::from_secs(1),
        }
        .recoverability(),
        ErrorRecoverability::Retry
    );

    assert_eq!(
        DeviceError::ProtocolError {
            device: String::new(),
            detail: String::new()
        }
        .recoverability(),
        ErrorRecoverability::Reconnect
    );

    assert_eq!(
        DeviceError::NotFound {
            device: String::new()
        }
        .recoverability(),
        ErrorRecoverability::Permanent
    );

    assert_eq!(
        DeviceError::Disconnected {
            device: String::new()
        }
        .recoverability(),
        ErrorRecoverability::Reconnect
    );

    assert_eq!(
        DeviceError::InvalidHandle {
            handle_id: 1,
            backend: String::new()
        }
        .recoverability(),
        ErrorRecoverability::Permanent
    );

    assert_eq!(
        DeviceError::InvalidTransition {
            device: String::new(),
            from: String::new(),
            to: String::new()
        }
        .recoverability(),
        ErrorRecoverability::Permanent
    );

    assert_eq!(
        DeviceError::NotAdopted {
            device_id: DeviceId::new()
        }
        .recoverability(),
        ErrorRecoverability::Permanent
    );

    assert_eq!(
        DeviceError::Unsupported {
            backend: "fixture-network".into(),
            operation: "display output"
        }
        .recoverability(),
        ErrorRecoverability::Permanent
    );

    assert_eq!(
        DeviceError::PermissionDenied {
            device: String::new(),
            detail: String::new(),
        }
        .recoverability(),
        ErrorRecoverability::Permanent
    );
}

// ── DeviceIdentifier ──────────────────────────────────────────────────────

#[test]
fn device_identifier_usb_display_with_serial() {
    let id = DeviceIdentifier::UsbHid {
        vendor_id: 0x16D5,
        product_id: 0x1F01,
        serial: Some("ABC123".into()),
        usb_path: None,
    };
    assert_eq!(id.display_short(), "USB 16D5:1F01 [ABC123]");
    assert_eq!(id.to_string(), "USB 16D5:1F01 [ABC123]");
}

#[test]
fn device_identifier_usb_display_without_serial() {
    let id = DeviceIdentifier::UsbHid {
        vendor_id: 0x16D5,
        product_id: 0x1F01,
        serial: None,
        usb_path: Some("usb-0000:00:14.0-2.3".into()),
    };
    assert_eq!(id.display_short(), "USB 16D5:1F01");
}

#[test]
fn device_identifier_network_display_with_hostname() {
    let id = DeviceIdentifier::Network {
        mac_address: "A4:CF:12:34:AB:CD".into(),
        last_ip: Some("192.168.1.42".parse().expect("valid ip")),
        mdns_hostname: Some("fixture-network-kitchen".into()),
    };
    assert_eq!(
        id.display_short(),
        "fixture-network-kitchen (A4:CF:12:34:AB:CD)"
    );
}

#[test]
fn device_identifier_network_display_without_hostname() {
    let id = DeviceIdentifier::Network {
        mac_address: "A4:CF:12:34:AB:CD".into(),
        last_ip: None,
        mdns_hostname: None,
    };
    assert_eq!(id.display_short(), "A4:CF:12:34:AB:CD");
}

#[test]
fn device_identifier_smbus_display() {
    let id = DeviceIdentifier::SmBus {
        bus_path: "/dev/i2c-9".into(),
        address: 0x40,
    };
    assert_eq!(id.display_short(), "SMBus /dev/i2c-9 [0x40]");
}

#[test]
fn device_identifier_bridge_display() {
    let id = DeviceIdentifier::Bridge {
        service: "openlinkhub".into(),
        device_serial: "ABC1234".into(),
    };
    assert_eq!(id.display_short(), "openlinkhub:ABC1234");
}

#[test]
fn device_identifier_fingerprint_usb_serial() {
    let id = DeviceIdentifier::UsbHid {
        vendor_id: 0x16D5,
        product_id: 0x1F01,
        serial: Some("SN001".into()),
        usb_path: Some("usb-0000:00:14.0-2".into()),
    };
    // Serial takes precedence over path
    assert_eq!(
        id.fingerprint("test-driver").as_str(),
        "usb:test-driver:16d5:1f01:SN001"
    );
}

#[test]
fn device_identifier_fingerprint_usb_path_fallback() {
    let id = DeviceIdentifier::UsbHid {
        vendor_id: 0x16D5,
        product_id: 0x1F01,
        serial: None,
        usb_path: Some("usb-0000:00:14.0-2".into()),
    };
    assert_eq!(
        id.fingerprint("test-driver").as_str(),
        "usb:test-driver:16d5:1f01:usb-0000:00:14.0-2"
    );
}

#[test]
fn device_identifier_fingerprint_smbus() {
    let id = DeviceIdentifier::SmBus {
        bus_path: "/dev/i2c-9".into(),
        address: 0x40,
    };
    assert_eq!(
        id.fingerprint("test-driver").as_str(),
        "smbus:test-driver:/dev/i2c-9:40"
    );
}

#[test]
fn device_identifier_fingerprint_network() {
    let id = DeviceIdentifier::Network {
        mac_address: "A4:CF:12:34:AB:CD".into(),
        last_ip: Some("10.0.0.5".parse().expect("valid ip")),
        mdns_hostname: None,
    };
    // IP is transient — fingerprint uses only MAC
    assert_eq!(
        id.fingerprint("test-driver").as_str(),
        "net:test-driver:a4:cf:12:34:ab:cd"
    );
}

#[test]
fn device_identifier_fingerprint_bridge() {
    let id = DeviceIdentifier::Bridge {
        service: "openlinkhub".into(),
        device_serial: "ABC1234".into(),
    };
    assert_eq!(
        id.fingerprint("test-driver").as_str(),
        "bridge:test-driver:openlinkhub:ABC1234"
    );
}

#[test]
fn device_identifier_serde_round_trip() {
    let identifiers = vec![
        DeviceIdentifier::UsbHid {
            vendor_id: 0x16D5,
            product_id: 0x1F01,
            serial: Some("SN001".into()),
            usb_path: None,
        },
        DeviceIdentifier::SmBus {
            bus_path: "/dev/i2c-9".into(),
            address: 0x40,
        },
        DeviceIdentifier::Network {
            mac_address: "AA:BB:CC:DD:EE:FF".into(),
            last_ip: None,
            mdns_hostname: Some("fixture-network-desk".into()),
        },
        DeviceIdentifier::Bridge {
            service: "openlinkhub".into(),
            device_serial: "bridge-serial".into(),
        },
    ];

    for ident in identifiers {
        let json = serde_json::to_string(&ident).expect("serialize");
        let back: DeviceIdentifier = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ident);
    }
}

// ── DeviceHandle ──────────────────────────────────────────────────────────

#[test]
fn device_handle_ids_are_unique_and_monotonic() {
    let h1 = DeviceHandle::new(
        DeviceIdentifier::Network {
            mac_address: "AA:BB:CC:DD:EE:01".into(),
            last_ip: None,
            mdns_hostname: None,
        },
        "fixture-network",
    );
    let h2 = DeviceHandle::new(
        DeviceIdentifier::Network {
            mac_address: "AA:BB:CC:DD:EE:02".into(),
            last_ip: None,
            mdns_hostname: None,
        },
        "fixture-network",
    );

    assert!(
        h2.id() > h1.id(),
        "handle IDs should increase monotonically"
    );
}

#[test]
fn device_handle_accessors_and_display() {
    let identifier = DeviceIdentifier::Network {
        mac_address: "AA:BB:CC:DD:EE:05".into(),
        last_ip: None,
        mdns_hostname: Some("desk-strip".into()),
    };
    let handle = DeviceHandle::new(identifier.clone(), "fixture-network");

    assert_eq!(handle.device_id(), &identifier);
    assert_eq!(handle.backend_id(), "fixture-network");
    assert!(handle.to_string().starts_with("fixture-network#"));
}

#[test]
fn device_handle_serde_round_trip() {
    let handle = DeviceHandle::new(
        DeviceIdentifier::Bridge {
            service: "bridge-service".into(),
            device_serial: "bridge-123:5".into(),
        },
        "bridge-service",
    );

    let json = serde_json::to_string(&handle).expect("serialize");
    let back: DeviceHandle = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, handle);
}

#[test]
fn device_fingerprint_display() {
    let fp = DeviceFingerprint::mint(FingerprintNamespace::Net, "test", "aa:bb:cc:dd:ee:ff");
    assert_eq!(fp.to_string(), "net:test:aa:bb:cc:dd:ee:ff");
}

#[test]
fn device_fingerprint_stable_device_id_is_deterministic() {
    let fp = DeviceFingerprint::mint(FingerprintNamespace::Usb, "razer", "1532:0276:7-3.2");
    let first = fp.stable_device_id();
    let second = fp.stable_device_id();
    assert_eq!(first, second);
}

#[test]
fn device_fingerprint_stable_device_id_differs_for_distinct_fingerprints() {
    let left = DeviceFingerprint::mint(FingerprintNamespace::Net, "test", "aa:bb:cc:dd:ee:ff")
        .stable_device_id();
    let right = DeviceFingerprint::mint(FingerprintNamespace::Net, "test", "11:22:33:44:55:66")
        .stable_device_id();
    assert_ne!(left, right);
}

// ── SegmentInfo ───────────────────────────────────────────────────────────

#[test]
fn segment_info_serde_round_trip() {
    let segment = SegmentInfo {
        name: "Main Strip".into(),
        led_count: 144,
        topology: DeviceTopologyHint::Strip,
        color_format: DeviceColorFormat::Rgb,
        layout_hint: None,
    };
    let json = serde_json::to_string(&segment).expect("serialize");
    let back: SegmentInfo = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, "Main Strip");
    assert_eq!(back.led_count, 144);
    assert_eq!(back.topology, DeviceTopologyHint::Strip);
    assert_eq!(back.color_format, DeviceColorFormat::Rgb);
}

#[test]
fn segment_layout_hint_custom_grid_builds_normalized_positions() {
    let hint = SegmentLayoutHint::custom_grid(3, 2, &[(0, 0), (1, 1), (2, 1)])
        .with_size(NormalizedPosition::new(0.2, 0.1))
        .with_shape(ZoneShape::Rectangle)
        .co_located();

    assert_eq!(hint.size, Some(NormalizedPosition::new(0.2, 0.1)));
    assert_eq!(hint.shape, Some(ZoneShape::Rectangle));
    assert!(hint.co_located);

    let Some(LedTopology::Custom { positions }) = hint.topology else {
        panic!("expected custom topology");
    };
    assert_eq!(positions.len(), 3);
    assert_eq!(positions[0], NormalizedPosition::new(0.0, 0.0));
    assert_eq!(positions[1], NormalizedPosition::new(0.5, 1.0));
    assert_eq!(positions[2], NormalizedPosition::new(1.0, 1.0));
}

#[test]
fn segment_layout_hint_custom_grid_preserves_coordinates_above_u16() {
    let hint = SegmentLayoutHint::custom_grid(100_001, 2, &[(0, 0), (50_000, 1), (100_000, 1)]);
    let positions = match hint.topology {
        Some(LedTopology::Custom { positions }) => positions,
        other => panic!("expected custom topology, got {other:?}"),
    };

    assert_eq!(positions[0], NormalizedPosition::new(0.0, 0.0));
    assert!((positions[1].x - 0.5).abs() < f32::EPSILON);
    assert_eq!(positions[1].y, 1.0);
    assert_eq!(positions[2], NormalizedPosition::new(1.0, 1.0));
}

#[test]
fn segment_info_matrix_topology() {
    let segment = SegmentInfo {
        name: "Panel".into(),
        led_count: 256,
        topology: DeviceTopologyHint::Matrix { rows: 16, cols: 16 },
        color_format: DeviceColorFormat::Rgbw,
        layout_hint: None,
    };
    if let DeviceTopologyHint::Matrix { rows, cols } = segment.topology {
        assert_eq!(rows, 16);
        assert_eq!(cols, 16);
    } else {
        panic!("expected Matrix topology");
    }
}

// ── TOML serialization (dev-dependency) ───────────────────────────────────

#[test]
fn device_info_toml_round_trip() {
    let info = sample_device_info();
    let toml_str = toml::to_string_pretty(&info).expect("toml serialize");
    let back: DeviceInfo = toml::from_str(&toml_str).expect("toml deserialize");
    assert_eq!(back.name, info.name);
    assert_eq!(back.total_led_count(), info.total_led_count());
}
