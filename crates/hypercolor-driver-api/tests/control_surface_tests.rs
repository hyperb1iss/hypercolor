use std::collections::HashMap;

use hypercolor_driver_api::control_surface;
use hypercolor_types::control::{ControlValue, IpText};
use hypercolor_types::controls::{
    ApplyImpact, ControlAccess, ControlGroupKind, ControlPersistence, ControlSurfaceScope,
    ControlValueType, ControlVisibility,
};
use hypercolor_types::device::DeviceId;

#[test]
fn driver_and_device_surfaces_use_canonical_ids() {
    let driver = control_surface::driver_surface("wled");
    assert_eq!(driver.surface_id, "driver:wled");
    assert_eq!(
        driver.scope,
        ControlSurfaceScope::Driver {
            driver_id: "wled".to_owned(),
        }
    );

    let device_id = DeviceId::new();
    let device = control_surface::device_surface("wled", device_id);
    assert_eq!(device.surface_id, format!("driver:wled:device:{device_id}"));
    assert_eq!(
        device.scope,
        ControlSurfaceScope::Device {
            device_id,
            driver_id: "wled".to_owned(),
        }
    );
}

#[test]
fn driver_field_sets_standard_driver_config_defaults() {
    let field = control_surface::driver_field(
        "govee",
        "known_ips",
        "Known IPs",
        None,
        Some("connection"),
        control_surface::ip_list_value_type(64),
        ApplyImpact::DiscoveryRescan,
        0,
    );

    assert_eq!(field.access, ControlAccess::ReadWrite);
    assert_eq!(field.persistence, ControlPersistence::DriverConfig);
    assert_eq!(field.visibility, ControlVisibility::Standard);
    assert_eq!(field.group_id.as_deref(), Some("connection"));
}

#[test]
fn push_metadata_value_skips_empty_and_invalid_metadata() {
    let mut document = control_surface::device_surface("hue", DeviceId::new());
    let mut metadata = HashMap::from([("ip".to_owned(), "192.168.1.24".to_owned())]);
    metadata.insert("empty".to_owned(), String::new());
    metadata.insert("invalid_ip".to_owned(), "not-an-address".to_owned());

    control_surface::push_metadata_value(
        &mut document,
        "hue",
        &metadata,
        "invalid_ip",
        "Invalid IP",
        "connection",
        ControlValueType::IpAddress,
        |raw| IpText::new(raw).ok().map(ControlValue::Ip),
        5,
    );
    control_surface::push_metadata_value(
        &mut document,
        "hue",
        &metadata,
        "ip",
        "IP Address",
        "connection",
        ControlValueType::IpAddress,
        |raw| IpText::new(raw).ok().map(ControlValue::Ip),
        0,
    );
    control_surface::push_metadata_value(
        &mut document,
        "hue",
        &metadata,
        "empty",
        "Empty",
        "connection",
        control_surface::string_value_type(None),
        |raw| Some(ControlValue::Text(raw)),
        10,
    );

    assert_eq!(document.fields.len(), 1);
    assert_eq!(
        document.values.get("ip"),
        Some(&ControlValue::Ip(
            IpText::new("192.168.1.24").expect("fixture IP should be valid")
        ))
    );
    assert!(!document.values.contains_key("invalid_ip"));
    assert!(!document.values.contains_key("empty"));
}

#[test]
fn availability_markers_cover_fields_and_actions() {
    let mut document = control_surface::driver_surface("nanoleaf");
    document.groups.push(control_surface::group(
        "connection",
        "Connection",
        ControlGroupKind::Connection,
        0,
    ));
    document.fields.push(control_surface::readonly_field(
        "nanoleaf",
        "state",
        "State",
        "connection",
        control_surface::string_value_type(Some(32)),
        0,
    ));

    control_surface::mark_fields_available(&mut document);

    assert_eq!(document.availability["state"], control_surface::available());
}

#[test]
fn validate_control_ip_list_rejects_non_routable_ips() {
    let valid = ControlValue::List(vec![ControlValue::Ip(
        IpText::new("192.168.1.24").expect("fixture IP should be valid"),
    )]);
    control_surface::validate_control_ip_list("known IP", &valid).expect("valid IP list");

    let invalid = ControlValue::List(vec![ControlValue::Ip(
        IpText::new("127.0.0.1").expect("fixture IP should be syntactically valid"),
    )]);
    let error = control_surface::validate_control_ip_list("known IP", &invalid)
        .expect_err("non-routable IP list should fail");
    assert!(error.to_string().contains("invalid known IP"));
}

#[test]
fn revision_metadata_is_sorted_before_hashing() {
    let mut first = HashMap::new();
    first.insert("b".to_owned(), "2".to_owned());
    first.insert("a".to_owned(), "1".to_owned());

    let mut second = HashMap::new();
    second.insert("a".to_owned(), "1".to_owned());
    second.insert("b".to_owned(), "2".to_owned());

    let mut first_payload = Vec::new();
    control_surface::extend_metadata_revision(&mut first_payload, Some(&first));

    let mut second_payload = Vec::new();
    control_surface::extend_metadata_revision(&mut second_payload, Some(&second));

    assert_eq!(first_payload, second_payload);
    assert_eq!(
        control_surface::revision_hash(&first_payload),
        control_surface::revision_hash(&second_payload)
    );
}
