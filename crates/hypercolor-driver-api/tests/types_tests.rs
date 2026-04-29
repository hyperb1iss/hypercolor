use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use hypercolor_driver_api::{
    ControlApplyTarget, DeviceAuthState, DiscoveryRequest, DriverControlProvider, DriverDescriptor,
    DriverDiscoveredDevice, DriverHost, DriverModule, DriverPresentationProvider,
    DriverProtocolCatalog, DriverTransport, PairDeviceRequest, PairDeviceStatus, PairingDescriptor,
    PairingFieldDescriptor, PairingFlowKind, ValidatedControlChanges, support,
};
use hypercolor_driver_api::{DiscoveredDevice, DiscoveryConnectBehavior};
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlActionResult, ControlActionStatus, ControlChange,
    ControlSurfaceDocument, ControlSurfaceScope, ControlValueMap,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceClassHint, DeviceColorFormat, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    DriverModuleKind, DriverPresentation, DriverProtocolDescriptor, DriverTransportKind, ZoneInfo,
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
fn driver_descriptor_converts_to_module_descriptor() {
    let descriptor =
        DriverDescriptor::new("hue", "Philips Hue", DriverTransport::Network, true, true);

    let module = descriptor.module_descriptor();

    assert_eq!(module.id, "hue");
    assert_eq!(module.display_name, "Philips Hue");
    assert_eq!(module.module_kind, DriverModuleKind::Network);
    assert_eq!(module.transports, vec![DriverTransportKind::Network]);
    assert!(module.capabilities.discovery);
    assert!(module.capabilities.pairing);
    assert!(module.capabilities.output_backend);
    assert!(!module.capabilities.runtime_cache);
    assert!(module.capabilities.credentials);
    assert!(!module.capabilities.controls);
}

#[test]
fn driver_descriptor_maps_non_network_transports() {
    let cases = [
        (
            DriverTransport::Usb,
            DriverModuleKind::Hal,
            DriverTransportKind::Usb,
        ),
        (
            DriverTransport::Smbus,
            DriverModuleKind::Hal,
            DriverTransportKind::Smbus,
        ),
        (
            DriverTransport::Midi,
            DriverModuleKind::Hal,
            DriverTransportKind::Midi,
        ),
        (
            DriverTransport::Serial,
            DriverModuleKind::Hal,
            DriverTransportKind::Serial,
        ),
        (
            DriverTransport::Bridge,
            DriverModuleKind::Bridge,
            DriverTransportKind::Bridge,
        ),
        (
            DriverTransport::Virtual,
            DriverModuleKind::Virtual,
            DriverTransportKind::Virtual,
        ),
    ];

    for (transport, module_kind, transport_kind) in cases {
        let descriptor = DriverDescriptor::new("test", "Test", transport, true, false);
        let module = descriptor.module_descriptor();

        assert_eq!(module.module_kind, module_kind);
        assert_eq!(module.transports, vec![transport_kind]);
    }
}

struct ControlOnlyProvider;

#[async_trait]
impl DriverControlProvider for ControlOnlyProvider {
    async fn driver_surface(
        &self,
        host: &dyn DriverHost,
        config: hypercolor_driver_api::DriverConfigView<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>> {
        let _ = (host, config);
        Ok(Some(ControlSurfaceDocument::empty(
            "driver:control-only",
            ControlSurfaceScope::Driver {
                driver_id: "control-only".to_owned(),
            },
        )))
    }

    async fn device_surface(
        &self,
        host: &dyn DriverHost,
        device: &hypercolor_driver_api::TrackedDeviceCtx<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>> {
        let _ = (host, device);
        Ok(None)
    }

    async fn validate_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> anyhow::Result<ValidatedControlChanges> {
        let _ = (host, target);
        Ok(ValidatedControlChanges::new(changes.to_vec()))
    }

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> anyhow::Result<ApplyControlChangesResponse> {
        let _ = (host, target, changes);
        unreachable!("apply is not exercised in descriptor tests")
    }

    async fn invoke_action(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        action_id: &str,
        input: ControlValueMap,
    ) -> anyhow::Result<ControlActionResult> {
        let _ = (host, target, input);
        Ok(ControlActionResult {
            surface_id: "driver:control-only".to_owned(),
            action_id: action_id.to_owned(),
            status: ControlActionStatus::Completed,
            result: None,
            revision: 0,
        })
    }
}

struct ControlOnlyDriver;

static CONTROL_ONLY_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "control-only",
    "Control Only",
    DriverTransport::Network,
    false,
    false,
);

impl DriverModule for ControlOnlyDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &CONTROL_ONLY_DESCRIPTOR
    }

    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        Some(&ControlOnlyProvider)
    }
}

struct ProtocolOnlyCatalog;

static PROTOCOL_ONLY_DESCRIPTORS: LazyLock<Vec<DriverProtocolDescriptor>> = LazyLock::new(|| {
    vec![DriverProtocolDescriptor {
        driver_id: "protocol-only".to_owned(),
        protocol_id: "protocol-only/example".to_owned(),
        display_name: "Protocol Only Example".to_owned(),
        vendor_id: Some(0x1234),
        product_id: Some(0x5678),
        family_id: "protocol-only".to_owned(),
        model_id: None,
        transport: DriverTransportKind::Usb,
        route_backend_id: "usb".to_owned(),
        presentation: None,
    }]
});

impl DriverProtocolCatalog for ProtocolOnlyCatalog {
    fn descriptors(&self) -> &[DriverProtocolDescriptor] {
        PROTOCOL_ONLY_DESCRIPTORS.as_slice()
    }
}

struct ProtocolOnlyDriver;

static PROTOCOL_ONLY_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "protocol-only",
    "Protocol Only",
    DriverTransport::Usb,
    false,
    false,
);

impl DriverModule for ProtocolOnlyDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &PROTOCOL_ONLY_DESCRIPTOR
    }

    fn protocol_catalog(&self) -> Option<&dyn DriverProtocolCatalog> {
        Some(&ProtocolOnlyCatalog)
    }
}

struct PresentationOnlyProvider;

impl DriverPresentationProvider for PresentationOnlyProvider {
    fn presentation(&self) -> DriverPresentation {
        DriverPresentation {
            label: "Protocol Queen".to_owned(),
            short_label: Some("PQ".to_owned()),
            accent_rgb: Some([128, 255, 234]),
            secondary_rgb: Some([225, 53, 255]),
            icon: Some("usb".to_owned()),
            default_device_class: Some(DeviceClassHint::Controller),
        }
    }
}

struct PresentationOnlyDriver;

static PRESENTATION_ONLY_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "presentation-only",
    "Presentation Only",
    DriverTransport::Usb,
    false,
    false,
);

impl DriverModule for PresentationOnlyDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &PRESENTATION_ONLY_DESCRIPTOR
    }

    fn presentation(&self) -> Option<&dyn DriverPresentationProvider> {
        Some(&PresentationOnlyProvider)
    }
}

#[test]
fn driver_module_advertises_control_provider_capability() {
    let module = ControlOnlyDriver.module_descriptor();

    assert!(module.capabilities.controls);
    assert!(!module.capabilities.discovery);
    assert!(!module.capabilities.pairing);
    assert!(!module.capabilities.credentials);
    assert!(!module.capabilities.output_backend);
    assert!(!module.capabilities.runtime_cache);
}

#[test]
fn driver_module_advertises_protocol_catalog_capability() {
    let module = ProtocolOnlyDriver.module_descriptor();
    let catalog = ProtocolOnlyDriver
        .protocol_catalog()
        .expect("protocol catalog should be present");

    assert!(module.capabilities.protocol_catalog);
    assert!(!module.capabilities.output_backend);
    assert_eq!(
        catalog.descriptors()[0].protocol_id,
        "protocol-only/example"
    );
}

#[test]
fn driver_module_advertises_presentation_capability() {
    let module = PresentationOnlyDriver.module_descriptor();
    let presentation = PresentationOnlyDriver
        .presentation()
        .expect("presentation provider should be present")
        .presentation();

    assert!(module.capabilities.presentation);
    assert!(!module.capabilities.output_backend);
    assert_eq!(presentation.label, "Protocol Queen");
    assert_eq!(
        presentation.default_device_class,
        Some(DeviceClassHint::Controller)
    );
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
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        zones: vec![ZoneInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
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
fn discovered_device_converts_from_core_payload() {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Bridge".to_owned(),
        vendor: "Philips".to_owned(),
        family: DeviceFamily::new_static("hue", "Philips Hue"),
        model: Some("bridge".to_owned()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("hue", "hue", ConnectionType::Network),
        zones: Vec::new(),
        firmware_version: Some("1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 0,
            supports_direct: false,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 0,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let discovered = DriverDiscoveredDevice::from(DiscoveredDevice {
        connection_type: ConnectionType::Network,
        origin: info.origin.clone(),
        name: "Bridge".to_owned(),
        family: DeviceFamily::new_static("hue", "Philips Hue"),
        info,
        fingerprint: DeviceFingerprint("net:hue:bridge".to_owned()),
        metadata: std::collections::HashMap::from([("ip".to_owned(), "10.0.0.8".to_owned())]),
        connect_behavior: DiscoveryConnectBehavior::Deferred,
    });

    assert_eq!(discovered.metadata.get("ip"), Some(&"10.0.0.8".to_owned()));
    assert_eq!(discovered.fingerprint.0, "net:hue:bridge");
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

#[test]
fn support_helpers_parse_metadata_and_dedupe_keys() {
    let metadata = std::collections::HashMap::from([
        ("ip".to_owned(), "10.0.0.42".to_owned()),
        ("name".to_owned(), " Desk Strip ".to_owned()),
    ]);
    let mut keys = vec!["wled:ip:10.0.0.42".to_owned()];

    assert_eq!(
        support::network_ip_from_metadata(Some(&metadata))
            .expect("ip should parse")
            .to_string(),
        "10.0.0.42"
    );
    assert_eq!(
        support::metadata_value(Some(&metadata), "name"),
        Some("Desk Strip")
    );

    support::push_lookup_key(&mut keys, "wled:ip:10.0.0.42".to_owned());
    support::push_lookup_key(&mut keys, "wled:desk".to_owned());

    assert_eq!(
        keys,
        vec!["wled:ip:10.0.0.42".to_owned(), "wled:desk".to_owned()]
    );
}
