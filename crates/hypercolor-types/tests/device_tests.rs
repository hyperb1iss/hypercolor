//! Tests for device identity, capabilities, and state types.

use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorFormat, DeviceColorSpace,
    DeviceError, DeviceFamily, DeviceFeatures, DeviceFingerprint, DeviceHandle, DeviceId,
    DeviceIdentifier, DeviceInfo, DeviceOrigin, DeviceState, DeviceTopologyHint,
    DeviceUserSettings, DriverCapabilitySet, DriverModuleDescriptor, DriverModuleKind,
    DriverPresentation, DriverTransportKind, ZoneInfo,
};
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
        vendor: "WLED".into(),
        family: DeviceFamily::Wled,
        model: Some("strip".into()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        zones: vec![
            ZoneInfo {
                name: "Main".into(),
                led_count: 60,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ZoneInfo {
                name: "Accent".into(),
                led_count: 30,
                topology: DeviceTopologyHint::Ring { count: 30 },
                color_format: DeviceColorFormat::Rgbw,
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
fn device_info_serde_round_trip() {
    let info = sample_device_info();
    let json = serde_json::to_string_pretty(&info).expect("serialize");
    let back: DeviceInfo = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, "Test Strip");
    assert_eq!(back.total_led_count(), 90);
    assert_eq!(back.firmware_version, Some("0.15.0".into()));
}

#[test]
fn device_info_empty_zones_yields_zero_leds() {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Empty".into(),
        vendor: "Test".into(),
        family: DeviceFamily::Custom("test".into()),
        model: None,
        connection_type: ConnectionType::Bridge,
        origin: DeviceOrigin::native("test", "test", ConnectionType::Bridge),
        zones: vec![],
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
    assert!(!capabilities.backend_factory);
    assert!(!capabilities.protocol_catalog);
    assert!(!capabilities.runtime_cache);
    assert!(!capabilities.credentials);
    assert!(!capabilities.presentation);
    assert!(!capabilities.controls);
}

#[test]
fn driver_capability_set_defaults_missing_controls_flag() {
    let json = r#"{
        "config": false,
        "discovery": true,
        "pairing": false,
        "backend_factory": true,
        "protocol_catalog": false,
        "runtime_cache": true,
        "credentials": false,
        "presentation": false
    }"#;

    let capabilities: DriverCapabilitySet =
        serde_json::from_str(json).expect("legacy capabilities should deserialize");

    assert!(capabilities.discovery);
    assert!(capabilities.backend_factory);
    assert!(!capabilities.controls);
}

#[test]
fn driver_transport_kind_round_trips_custom_transport() {
    let transport = DriverTransportKind::Custom("openlinkhub".into());
    let json = serde_json::to_string(&transport).expect("serialize");
    let back: DriverTransportKind = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, transport);
}

#[test]
fn device_origin_separates_driver_from_backend() {
    let origin =
        DeviceOrigin::new("nollie", "usb", DriverTransportKind::Usb).with_protocol_id("nollie32");

    assert_eq!(origin.driver_id, "nollie");
    assert_eq!(origin.backend_id, "usb");
    assert_eq!(origin.protocol_id.as_deref(), Some("nollie32"));

    let json = serde_json::to_string(&origin).expect("serialize");
    let back: DeviceOrigin = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, origin);
}

#[test]
fn device_origin_omits_absent_protocol_id() {
    let origin = DeviceOrigin::new("wled", "wled", DriverTransportKind::Network);
    let json = serde_json::to_string(&origin).expect("serialize");

    assert!(!json.contains("protocol_id"));
}

#[test]
fn driver_presentation_serializes_optional_metadata() {
    let presentation = DriverPresentation {
        label: "Nanoleaf".into(),
        short_label: Some("Leaf".into()),
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
        id: "prismrgb".into(),
        display_name: "PrismRGB".into(),
        vendor_name: Some("PrismRGB".into()),
        module_kind: DriverModuleKind::Hal,
        transports: vec![DriverTransportKind::Usb],
        capabilities: DriverCapabilitySet {
            protocol_catalog: true,
            presentation: true,
            ..DriverCapabilitySet::empty()
        },
        api_schema_version: 1,
        config_version: 1,
        default_enabled: true,
    };

    let json = serde_json::to_string(&descriptor).expect("serialize");
    let back: DriverModuleDescriptor = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(back, descriptor);
}

// ── DeviceFamily ──────────────────────────────────────────────────────────

#[test]
fn device_family_display() {
    assert_eq!(DeviceFamily::Wled.to_string(), "WLED");
    assert_eq!(DeviceFamily::Hue.to_string(), "Philips Hue");
    assert_eq!(DeviceFamily::Nanoleaf.to_string(), "Nanoleaf");
    assert_eq!(DeviceFamily::Razer.to_string(), "Razer");
    assert_eq!(DeviceFamily::Corsair.to_string(), "Corsair");
    assert_eq!(DeviceFamily::Dygma.to_string(), "Dygma");
    assert_eq!(DeviceFamily::LianLi.to_string(), "Lian Li");
    assert_eq!(DeviceFamily::Nollie.to_string(), "Nollie");
    assert_eq!(DeviceFamily::PrismRgb.to_string(), "PrismRGB");
    assert_eq!(DeviceFamily::Asus.to_string(), "ASUS");
    assert_eq!(
        DeviceFamily::Custom("PrismRGB".into()).to_string(),
        "PrismRGB"
    );
}

#[test]
fn device_family_equality() {
    assert_eq!(DeviceFamily::Wled, DeviceFamily::Wled);
    assert_ne!(DeviceFamily::Wled, DeviceFamily::Hue);
    assert_ne!(DeviceFamily::Hue, DeviceFamily::Nanoleaf);
    assert_eq!(
        DeviceFamily::Custom("Foo".into()),
        DeviceFamily::Custom("Foo".into())
    );
    assert_ne!(
        DeviceFamily::Custom("Foo".into()),
        DeviceFamily::Custom("Bar".into())
    );
}

#[test]
fn device_family_serde_round_trip() {
    let families = vec![
        DeviceFamily::Wled,
        DeviceFamily::Hue,
        DeviceFamily::Nanoleaf,
        DeviceFamily::Razer,
        DeviceFamily::Corsair,
        DeviceFamily::Dygma,
        DeviceFamily::LianLi,
        DeviceFamily::Nollie,
        DeviceFamily::PrismRgb,
        DeviceFamily::Asus,
        DeviceFamily::Custom("PrismRGB".into()),
    ];
    for family in families {
        let json = serde_json::to_string(&family).expect("serialize");
        let back: DeviceFamily = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, family);
    }
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
        device: "WLED-Kitchen".into(),
        reason: "TCP refused".into(),
    };
    assert_eq!(
        err.to_string(),
        "connection to WLED-Kitchen failed: TCP refused"
    );

    let err = DeviceError::NotFound {
        device: "Prism 8".into(),
    };
    assert_eq!(err.to_string(), "device not found: Prism 8");

    let err = DeviceError::Timeout {
        device: "LED Strip".into(),
        operation: "push_frame".into(),
    };
    assert_eq!(
        err.to_string(),
        "timeout communicating with LED Strip: push_frame"
    );

    let err = DeviceError::WriteError {
        device: "USB Controller".into(),
        detail: "HID write returned -1".into(),
    };
    assert_eq!(
        err.to_string(),
        "write error on USB Controller: HID write returned -1"
    );

    let err = DeviceError::ProtocolError {
        device: "WLED".into(),
        detail: "unexpected packet type 0xFF".into(),
    };
    assert_eq!(
        err.to_string(),
        "protocol error for WLED: unexpected packet type 0xFF"
    );

    let err = DeviceError::Disconnected {
        device: "USB Controller".into(),
    };
    assert_eq!(err.to_string(), "device disconnected: USB Controller");

    let err = DeviceError::InvalidHandle {
        handle_id: 42,
        backend: "wled".into(),
    };
    assert_eq!(err.to_string(), "invalid handle 42 for backend wled");

    let err = DeviceError::InvalidTransition {
        device: "WLED".into(),
        from: "Known".into(),
        to: "Active".into(),
    };
    assert_eq!(
        err.to_string(),
        "invalid device transition for WLED: Known -> Active"
    );
}

#[test]
fn device_error_is_recoverable() {
    assert!(
        DeviceError::ConnectionFailed {
            device: String::new(),
            reason: String::new()
        }
        .is_recoverable()
    );

    assert!(
        DeviceError::WriteError {
            device: String::new(),
            detail: String::new()
        }
        .is_recoverable()
    );

    assert!(
        DeviceError::Timeout {
            device: String::new(),
            operation: String::new()
        }
        .is_recoverable()
    );

    assert!(
        DeviceError::ProtocolError {
            device: String::new(),
            detail: String::new()
        }
        .is_recoverable()
    );

    assert!(
        !DeviceError::NotFound {
            device: String::new()
        }
        .is_recoverable()
    );

    assert!(
        DeviceError::Disconnected {
            device: String::new()
        }
        .is_recoverable()
    );

    assert!(
        !DeviceError::InvalidHandle {
            handle_id: 1,
            backend: String::new()
        }
        .is_recoverable()
    );

    assert!(
        !DeviceError::InvalidTransition {
            device: String::new(),
            from: String::new(),
            to: String::new()
        }
        .is_recoverable()
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
        mdns_hostname: Some("wled-kitchen".into()),
    };
    assert_eq!(id.display_short(), "wled-kitchen (A4:CF:12:34:AB:CD)");
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
        id.fingerprint(),
        DeviceFingerprint("usb:16d5:1f01:SN001".into())
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
        id.fingerprint(),
        DeviceFingerprint("usb:16d5:1f01:usb-0000:00:14.0-2".into())
    );
}

#[test]
fn device_identifier_fingerprint_smbus() {
    let id = DeviceIdentifier::SmBus {
        bus_path: "/dev/i2c-9".into(),
        address: 0x40,
    };
    assert_eq!(
        id.fingerprint(),
        DeviceFingerprint("smbus:/dev/i2c-9:40".into())
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
        id.fingerprint(),
        DeviceFingerprint("net:a4:cf:12:34:ab:cd".into())
    );
}

#[test]
fn device_identifier_fingerprint_bridge() {
    let id = DeviceIdentifier::Bridge {
        service: "openlinkhub".into(),
        device_serial: "ABC1234".into(),
    };
    assert_eq!(
        id.fingerprint(),
        DeviceFingerprint("bridge:openlinkhub:ABC1234".into())
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
            mdns_hostname: Some("wled-desk".into()),
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
        "wled",
    );
    let h2 = DeviceHandle::new(
        DeviceIdentifier::Network {
            mac_address: "AA:BB:CC:DD:EE:02".into(),
            last_ip: None,
            mdns_hostname: None,
        },
        "wled",
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
    let handle = DeviceHandle::new(identifier.clone(), "wled");

    assert_eq!(handle.device_id(), &identifier);
    assert_eq!(handle.backend_id(), "wled");
    assert!(handle.to_string().starts_with("wled#"));
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
    let fp = DeviceFingerprint("net:aa:bb:cc:dd:ee:ff".into());
    assert_eq!(fp.to_string(), "net:aa:bb:cc:dd:ee:ff");
}

#[test]
fn device_fingerprint_stable_device_id_is_deterministic() {
    let fp = DeviceFingerprint("usb:1532:0276:7-3.2".into());
    let first = fp.stable_device_id();
    let second = fp.stable_device_id();
    assert_eq!(first, second);
}

#[test]
fn device_fingerprint_stable_device_id_differs_for_distinct_fingerprints() {
    let left = DeviceFingerprint("net:aa:bb:cc:dd:ee:ff".into()).stable_device_id();
    let right = DeviceFingerprint("net:11:22:33:44:55:66".into()).stable_device_id();
    assert_ne!(left, right);
}

// ── ZoneInfo ──────────────────────────────────────────────────────────────

#[test]
fn zone_info_serde_round_trip() {
    let zone = ZoneInfo {
        name: "Main Strip".into(),
        led_count: 144,
        topology: DeviceTopologyHint::Strip,
        color_format: DeviceColorFormat::Rgb,
    };
    let json = serde_json::to_string(&zone).expect("serialize");
    let back: ZoneInfo = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, "Main Strip");
    assert_eq!(back.led_count, 144);
    assert_eq!(back.topology, DeviceTopologyHint::Strip);
    assert_eq!(back.color_format, DeviceColorFormat::Rgb);
}

#[test]
fn zone_info_matrix_topology() {
    let zone = ZoneInfo {
        name: "Panel".into(),
        led_count: 256,
        topology: DeviceTopologyHint::Matrix { rows: 16, cols: 16 },
        color_format: DeviceColorFormat::Rgbw,
    };
    if let DeviceTopologyHint::Matrix { rows, cols } = zone.topology {
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
