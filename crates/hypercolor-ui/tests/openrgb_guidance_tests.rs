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
    let mut device = hardware();
    device.bus_path = Some("private-host-path".to_owned());
    device.interface_classes = vec![0x03, 0xff];
    let url = support_issue_url(&device, "linux", true, Some("1.2.3+test&build"));
    assert!(url.starts_with(
        "https://github.com/hyperb1iss/hypercolor/issues/new?template=device-support.yml&"
    ));
    assert!(url.contains("USB%20vendor%20label%3A%20A%20%26%20B"));
    assert!(url.contains("USB%20product%20string%3A%20Strip%20%2F%202"));
    assert!(url.contains("USB%20interface%20classes%3A%200x03%2C%200xFF"));
    assert!(url.contains("vid-pid=1234%3AABCD"));
    assert!(url.contains("platform=Linux"));
    assert!(url.contains("detected-device="));
    assert!(url.contains("Lighting%20control%20has%20not%20been%20verified"));
    assert!(url.contains("version=1.2.3%2Btest%26build"));
    assert!(support_issue_url(&hardware(), "macos", false, None).contains("platform=macOS"));
    assert!(support_issue_url(&hardware(), "windows", false, None).contains("platform=Windows"));
    assert!(
        !url.contains("Device-42"),
        "Serial identifiers stay out of the public issue URL"
    );
    assert!(!url.contains("private-host-path"));
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
    device.experimental = true;
    let url = support_issue_url(&device, "Windows", true, None);
    assert!(url.contains("Matched%20disabled%20driver%3A%20nollie"));
    assert!(url.contains("experimental%3A%20true"));
    assert!(url.contains("reachable%20OpenRGB%20server"));
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
fn support_request_keeps_vid_owner_in_detected_facts_only() {
    let kraken = UnclaimedDevice {
        vendor_id: 0x1532,
        product_id: 0x0527,
        product: Some("Razer Kraken Ultimate".to_owned()),
        ..Default::default()
    };
    let url = support_issue_url(&kraken, "windows", false, None);
    assert!(url.contains("USB%20vendor%20label%3A%20Razer"));
    assert!(url.contains("USB%20product%20string%3A%20Razer%20Kraken%20Ultimate"));
    assert!(!url.contains("&vendor="));
    assert!(!url.contains("&title="));

    let screen = UnclaimedDevice {
        vendor_id: 0x1CBE,
        product_id: 0xA088,
        product: Some("Universal Screen".to_owned()),
        ..Default::default()
    };
    let url = support_issue_url(&screen, "windows", false, None);
    assert!(
        url.contains("USB%20vendor%20label%3A%20VID%201CBE"),
        "a shared VID shows the raw id, not a guessed brand"
    );
    assert!(url.contains("USB%20product%20string%3A%20Universal%20Screen"));
}

#[test]
fn generic_keyboard_and_wifi_requests_cannot_prefill_owner_answers() {
    for (vendor_id, product_id, product) in [
        (0x1a2c, 0x2c27, "USB Keyboard"),
        (0x2604, 0x0020, "AIC 8800D80"),
    ] {
        let device = UnclaimedDevice {
            vendor_id,
            product_id,
            product: Some(product.to_owned()),
            ..Default::default()
        };
        let url = support_issue_url(&device, "Windows", false, Some("0.9.0"));
        let keys: Vec<_> = url
            .split_once('?')
            .expect("query")
            .1
            .split('&')
            .map(|pair| pair.split_once('=').expect("query pair").0)
            .collect();
        assert_eq!(
            keys,
            [
                "template",
                "vid-pid",
                "detected-device",
                "platform",
                "version"
            ]
        );
        let form = include_str!("../../../.github/ISSUE_TEMPLATE/device-support.yml");
        for key in keys.iter().filter(|&&key| key != "template") {
            assert!(
                form.lines().any(|line| line.trim() == format!("id: {key}")),
                "prefilled field {key} must exist in the GitHub form"
            );
        }
        assert!(url.contains("Support%20is%20unknown"));
        assert!(url.contains("USB%20detection%20does%20not%20establish%20lighting%20capability"));
    }
}

#[test]
fn unavailable_environment_is_left_for_the_reporter_to_fill() {
    for version in [None, Some("  ")] {
        let url = support_issue_url(&UnclaimedDevice::default(), "unknown", false, version);
        assert!(!url.contains("&platform="));
        assert!(!url.contains("&version="));
        assert!(url.contains("USB%20product%20string%3A%20Not%20reported"));
        assert!(url.contains("USB%20interface%20classes%3A%20Not%20reported"));
    }
    let url = support_issue_url(&hardware(), "darwin", false, Some(" 1.2.3 "));
    assert!(url.contains("platform=macOS"));
    assert!(url.ends_with("version=1.2.3"));
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
