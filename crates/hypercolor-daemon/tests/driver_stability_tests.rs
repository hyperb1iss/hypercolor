//! Spec 89 stability labels: experimental drivers run like stable ones,
//! and the daemon reports the label on drivers and devices.

use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
use hypercolor_daemon::network;
use hypercolor_driver_api::{DriverDescriptor, DriverModule, DriverProtocolCatalog};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, DriverProtocolDescriptor, DriverStability,
    DriverTransportKind, SegmentInfo,
};
use tower::ServiceExt;

static PROVEN_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "proven-lights",
    "Proven Lights",
    DriverTransportKind::Network,
    false,
    false,
);

static UNTESTED_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "untested-lights",
    "Untested Lights",
    DriverTransportKind::Network,
    false,
    false,
)
.experimental();

static MIXED_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    "mixed-family",
    "Mixed Family",
    DriverTransportKind::Usb,
    false,
    false,
);

static MIXED_PROTOCOLS: LazyLock<Vec<DriverProtocolDescriptor>> = LazyLock::new(|| {
    vec![
        protocol("mixed-family/proven", 0x0001, DriverStability::Stable),
        protocol(
            "mixed-family/untested",
            0x0002,
            DriverStability::Experimental,
        ),
    ]
});

fn protocol(
    protocol_id: &str,
    product_id: u16,
    stability: DriverStability,
) -> DriverProtocolDescriptor {
    DriverProtocolDescriptor {
        driver_id: "mixed-family".to_owned(),
        protocol_id: protocol_id.to_owned(),
        display_name: protocol_id.to_owned(),
        vendor_id: Some(0x1209),
        product_id: Some(product_id),
        family_id: "mixed-family".to_owned(),
        model_id: None,
        transport: DriverTransportKind::Usb,
        route_backend_id: "usb".to_owned(),
        presentation: None,
        stability,
    }
}

struct LabeledDriver(&'static DriverDescriptor);

impl DriverModule for LabeledDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        self.0
    }
}

struct MixedFamilyDriver;

impl DriverProtocolCatalog for MixedFamilyDriver {
    fn descriptors(&self) -> &[DriverProtocolDescriptor] {
        MIXED_PROTOCOLS.as_slice()
    }
}

impl DriverModule for MixedFamilyDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &MIXED_DESCRIPTOR
    }

    fn protocol_catalog(&self) -> Option<&dyn DriverProtocolCatalog> {
        Some(self)
    }
}

fn registry() -> DriverModuleRegistry {
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(LabeledDriver(&PROVEN_DESCRIPTOR))
        .expect("stable fixture registers");
    registry
        .register(LabeledDriver(&UNTESTED_DESCRIPTOR))
        .expect("experimental fixture registers");
    registry
        .register(MixedFamilyDriver)
        .expect("mixed fixture registers");
    registry
}

fn device(driver_id: &str, protocol_id: Option<&str>) -> DeviceInfo {
    let mut origin = DeviceOrigin::native(driver_id, "usb", ConnectionType::Usb);
    if let Some(protocol_id) = protocol_id {
        origin = origin.with_protocol_id(protocol_id);
    }
    DeviceInfo {
        id: DeviceId::new(),
        name: format!("{driver_id} device"),
        vendor: "Fixture".to_owned(),
        family: DeviceFamily::new_static("fixture", "Fixture"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin,
        segments: vec![SegmentInfo {
            name: "Strip".to_owned(),
            led_count: 8,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 8,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

#[test]
fn experimental_modules_are_enabled_by_default_like_stable_ones() {
    let registry = registry();
    let config = HypercolorConfig::default();

    let enabled = network::enabled_driver_module_ids(&registry, &config);

    assert!(enabled.contains("proven-lights"));
    assert!(
        enabled.contains("untested-lights"),
        "the experimental label never gates a driver"
    );
    assert!(enabled.contains("mixed-family"));
}

#[test]
fn experimental_modules_follow_the_ordinary_enabled_flag() {
    let registry = registry();
    let mut config = HypercolorConfig::default();
    config.drivers.insert(
        "untested-lights".to_owned(),
        DriverConfigEntry::disabled(std::collections::BTreeMap::new()),
    );

    let enabled = network::enabled_driver_module_ids(&registry, &config);

    assert!(!enabled.contains("untested-lights"));
    assert!(enabled.contains("proven-lights"));
}

#[test]
fn device_label_follows_the_module_or_the_protocol() {
    let registry = registry();

    assert!(network::device_is_experimental(
        &registry,
        &device("untested-lights", None)
    ));
    assert!(network::device_is_experimental(
        &registry,
        &device("mixed-family", Some("mixed-family/untested"))
    ));
    assert!(!network::device_is_experimental(
        &registry,
        &device("mixed-family", Some("mixed-family/proven"))
    ));
    assert!(!network::device_is_experimental(
        &registry,
        &device("mixed-family", None)
    ));
    assert!(!network::device_is_experimental(
        &registry,
        &device("proven-lights", None)
    ));
    assert!(
        !network::device_is_experimental(&registry, &device("unregistered", None)),
        "an unknown driver carries no label"
    );
}

fn state() -> (Arc<AppState>, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    let state = AppStateBuilder::new(data_dir)
        .with_driver_registry(Arc::new(registry()))
        .build();
    (Arc::new(state), tempdir)
}

async fn get_json(state: &Arc<AppState>, uri: &str) -> serde_json::Value {
    let response = api::build_router(Arc::clone(state), None)
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("request should execute");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should collect");
    serde_json::from_slice(&bytes).expect("body should be JSON")
}

fn item<'a>(items: &'a serde_json::Value, key: &str, value: &str) -> &'a serde_json::Value {
    items
        .as_array()
        .expect("items should be an array")
        .iter()
        .find(|item| item.pointer(key).and_then(serde_json::Value::as_str) == Some(value))
        .unwrap_or_else(|| panic!("no item with {key} = {value}"))
}

#[tokio::test]
async fn drivers_route_reports_module_and_protocol_labels() {
    let (state, _tmp) = state();

    let json = get_json(&state, "/api/v1/drivers").await;
    let items = &json["data"]["items"];

    let untested = item(items, "/descriptor/id", "untested-lights");
    assert_eq!(untested["descriptor"]["stability"], "experimental");
    assert_eq!(untested["descriptor"]["default_enabled"], true);
    assert_eq!(untested["enabled"], true);

    // The stable default stays off the wire, so older payloads round-trip.
    let proven = item(items, "/descriptor/id", "proven-lights");
    assert!(proven["descriptor"].get("stability").is_none());

    let mixed = item(items, "/descriptor/id", "mixed-family");
    assert!(mixed["descriptor"].get("stability").is_none());
    let protocols = &mixed["protocols"];
    assert_eq!(
        item(protocols, "/protocol_id", "mixed-family/untested")["stability"],
        "experimental"
    );
    assert!(
        item(protocols, "/protocol_id", "mixed-family/proven")
            .get("stability")
            .is_none()
    );
}

#[tokio::test]
async fn devices_route_reports_the_experimental_label() {
    let (state, _tmp) = state();
    state
        .device_registry
        .add(device("mixed-family", Some("mixed-family/untested")))
        .await;
    state
        .device_registry
        .add(device("proven-lights", None))
        .await;

    let json = get_json(&state, "/api/v1/devices").await;
    let items = &json["data"]["items"];

    assert_eq!(
        item(items, "/origin/driver_id", "mixed-family")["experimental"],
        true
    );
    assert!(
        item(items, "/origin/driver_id", "proven-lights")
            .get("experimental")
            .is_none()
    );
}
