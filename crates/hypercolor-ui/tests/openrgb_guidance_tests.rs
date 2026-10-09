use hypercolor_types::api::devices::*;
use hypercolor_ui::components::unclaimed_hardware::{
    matching_bridge, support_issue_url, vendor_and_model,
};

fn hardware() -> UnclaimedDevice {
    UnclaimedDevice {
        vendor_id: 0x1234,
        product_id: 0xabcd,
        manufacturer: Some("A & B".to_owned()),
        product: Some("Strip / 2".to_owned()),
        serial: Some(" Device-42 ".to_owned()),
        ..Default::default()
    }
}

fn coverage(kind: CoverageIdentityKind, value: &str) -> DeviceCoverageRow {
    DeviceCoverageRow {
        identity: CoverageIdentity {
            kind,
            value: value.to_owned(),
            label: "Strip".to_owned(),
        },
        native: None,
        bridge: Some(CoverageBridgeDevice {
            device_id: "bridge-route".to_owned(),
            state: "connected".to_owned(),
            output_enabled: true,
            disabled_reason: None,
        }),
        unclaimed: true,
        active: CoverageActive::Bridge,
    }
}

#[test]
fn support_request_preserves_issue_form_fields_and_escapes_values() {
    let url = support_issue_url(&hardware(), "linux", true);
    assert!(url.starts_with(
        "https://github.com/hyperb1iss/hypercolor/issues/new?template=device-support.yml&"
    ));
    assert!(url.contains("vendor=A%20%26%20B"));
    assert!(url.contains("model=Strip%20%2F%202"));
    assert!(url.contains("vid-pid=1234%3AABCD"));
    assert!(url.contains("platform=Linux"));
    assert!(url.contains("existing-support=OpenRGB"));
    assert!(support_issue_url(&hardware(), "macos", false).contains("platform=macOS"));
    assert!(support_issue_url(&hardware(), "windows", false).contains("platform=Windows"));
    assert!(
        !url.contains("Device-42"),
        "Serial identifiers stay out of the public issue URL"
    );
}

#[test]
fn bridge_match_requires_nonempty_case_insensitive_serial() {
    assert_eq!(
        matching_bridge(
            &hardware(),
            &[coverage(CoverageIdentityKind::Serial, "device-42")]
        ),
        Some("bridge-route")
    );
    assert_eq!(
        matching_bridge(
            &hardware(),
            &[coverage(CoverageIdentityKind::UsbPath, "device-42")]
        ),
        None
    );
    assert_eq!(
        matching_bridge(
            &hardware(),
            &[coverage(CoverageIdentityKind::Serial, "different")]
        ),
        None
    );
    let mut empty = hardware();
    empty.serial = Some("  ".to_owned());
    assert_eq!(
        matching_bridge(&empty, &[coverage(CoverageIdentityKind::Serial, "")]),
        None
    );
}

#[test]
fn disabled_native_driver_is_explained_in_support_request() {
    let mut device = hardware();
    device.claimable_by = Some("nollie".to_owned());
    let url = support_issue_url(&device, "Windows", false);
    assert!(url.contains("disabled%20nollie%20driver"));
}

#[test]
fn availability_requires_the_matching_routes_endpoint_to_be_reachable() {
    use hypercolor_types::api::system::{OpenRgbEndpointStatus, OpenRgbStatus};
    use hypercolor_ui::components::unclaimed_hardware::bridge_available;
    let mut status = OpenRgbStatus {
        compiled: true,
        enabled: true,
        platform: "Linux".to_owned(),
        binary_path: None,
        binary_version: None,
        bridge_config: Default::default(),
        probes: vec![OpenRgbEndpointStatus {
            endpoint: "other:6742".to_owned(),
            reachable: true,
            protocol_version: Some(5),
            controller_count: Some(1),
            error: None,
        }],
        install_hints: vec![],
        permission_checks: vec![],
        coverage: vec![coverage(CoverageIdentityKind::Serial, "device-42")],
        output_disabled_count: 0,
    };
    let route: DeviceSummary = serde_json::from_value(serde_json::json!({
        "id": "bridge-route", "layout_device_id": "bridge-route", "name": "Strip",
        "status": "connected", "brightness": 100, "total_leds": 8,
        "origin": {"driver_id": "openrgb", "backend_id": "openrgb", "transport": "bridge"},
        "presentation": {"label": "OpenRGB"},
        "bridge": {"endpoint": "matching:6742", "output_enabled": true}
    }))
    .expect("minimal summary uses contract defaults");
    assert!(!bridge_available(
        &hardware(),
        &status,
        std::slice::from_ref(&route)
    ));
    status.probes[0].endpoint = "matching:6742".to_owned();
    assert!(bridge_available(
        &hardware(),
        &status,
        std::slice::from_ref(&route)
    ));
    status.probes[0].reachable = false;
    assert!(!bridge_available(&hardware(), &status, &[route]));
}

#[test]
fn inventory_and_config_events_reach_the_device_hint_channel() {
    use hypercolor_ui::ws::messages::DEVICE_LIFECYCLE_EVENTS;
    assert!(DEVICE_LIFECYCLE_EVENTS.contains(&"unclaimed_devices_changed"));
    assert!(DEVICE_LIFECYCLE_EVENTS.contains(&"config_changed"));
    use hypercolor_ui::api::openrgb::refresh_status_for_event;
    assert!(!refresh_status_for_event("device_discovered"));
    assert!(refresh_status_for_event("device_discovery_completed"));
    assert!(refresh_status_for_event("device_state_changed"));
    assert!(refresh_status_for_event("device_disconnected"));
}

#[test]
fn support_request_names_the_vid_owner_when_the_host_reports_no_manufacturer() {
    let kraken = UnclaimedDevice {
        vendor_id: 0x1532,
        product_id: 0x0527,
        product: Some("Razer Kraken Ultimate".to_owned()),
        ..Default::default()
    };
    let url = support_issue_url(&kraken, "windows", false);
    assert!(url.contains("title=%5Bdevice%5D%20Razer%20Kraken%20Ultimate&"));
    assert!(url.contains("vendor=Razer&"));

    let screen = UnclaimedDevice {
        vendor_id: 0x1CBE,
        product_id: 0xA088,
        product: Some("Universal Screen".to_owned()),
        ..Default::default()
    };
    let url = support_issue_url(&screen, "windows", false);
    assert!(
        url.contains("vendor=VID%201CBE&"),
        "a shared VID shows the raw id, not a guessed brand"
    );
    assert!(!url.contains("Unknown"));
    assert!(url.contains("title=%5Bdevice%5D%20VID%201CBE%20Universal%20Screen&"));
}

#[test]
fn a_title_does_not_repeat_a_vendor_the_product_string_leads_with() {
    assert_eq!(
        vendor_and_model("Razer", "Razer Kraken Ultimate"),
        "Razer Kraken Ultimate"
    );
    assert_eq!(
        vendor_and_model("razer", "RAZER Base Station"),
        "RAZER Base Station"
    );
    assert_eq!(vendor_and_model("Razer", "Razer"), "Razer");
    assert_eq!(
        vendor_and_model("Razer", "Razerblade Dock"),
        "Razer Razerblade Dock"
    );
    assert_eq!(
        vendor_and_model("Corsair", "iCUE LINK Hub"),
        "Corsair iCUE LINK Hub"
    );
    assert_eq!(
        vendor_and_model("VID 1CBE", "8.8\" Screen"),
        "VID 1CBE 8.8\" Screen"
    );
}
