use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use hypercolor_driver_api::DriverTrackedDevice;
use hypercolor_driver_support::network::{
    ValidationError, metadata_value, network_ip_from_metadata, push_lookup_key,
    tracked_fingerprints_by_ip, validate_ip, validate_port,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceFamily, DeviceFingerprint, DeviceId, DeviceInfo,
    DeviceOrigin, DeviceState,
};

#[test]
fn validate_port_rejects_zero_and_privileged_ports() {
    assert!(matches!(validate_port(0), Err(ValidationError::PortZero)));
    assert!(matches!(
        validate_port(22),
        Err(ValidationError::PrivilegedPort(22))
    ));
    assert!(matches!(
        validate_port(1023),
        Err(ValidationError::PrivilegedPort(1023))
    ));
}

#[test]
fn validate_port_accepts_registered_and_dynamic_ports() {
    assert_eq!(validate_port(1024).expect("1024 should be allowed"), 1024);
    assert_eq!(validate_port(4048).expect("4048 should be allowed"), 4048);
    assert_eq!(
        validate_port(u16::MAX).expect("65535 should be allowed"),
        u16::MAX
    );
}

#[test]
fn validate_ip_rejects_non_routable_addresses() {
    for ip in [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V4(Ipv4Addr::BROADCAST),
        IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(169, 254, 1, 2)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ] {
        assert!(
            matches!(validate_ip(ip), Err(ValidationError::InvalidIp(_))),
            "{ip} should be rejected"
        );
    }
}

#[test]
fn validate_ip_accepts_routable_addresses() {
    let lan = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 50));
    let public = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9));
    let ipv6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

    assert_eq!(validate_ip(lan).expect("LAN IP should be accepted"), lan);
    assert_eq!(
        validate_ip(public).expect("public IP should be accepted"),
        public
    );
    assert_eq!(validate_ip(ipv6).expect("IPv6 should be accepted"), ipv6);
}

#[test]
fn validation_error_display_is_human_readable() {
    let message = ValidationError::PrivilegedPort(80).to_string();
    assert!(message.contains("80"));
    assert!(message.contains("privileged"));

    let ip_message = ValidationError::InvalidIp(IpAddr::V4(Ipv4Addr::LOCALHOST)).to_string();
    assert!(ip_message.contains("127.0.0.1"));
}

#[test]
fn network_metadata_helpers_parse_and_dedupe() {
    let metadata = HashMap::from([
        ("ip".to_owned(), "10.0.0.42".to_owned()),
        ("name".to_owned(), " Desk Strip ".to_owned()),
    ]);
    let mut keys = vec!["fixture:ip:10.0.0.42".to_owned()];

    assert_eq!(
        network_ip_from_metadata(Some(&metadata))
            .expect("ip should parse")
            .to_string(),
        "10.0.0.42"
    );
    assert_eq!(metadata_value(Some(&metadata), "name"), Some("Desk Strip"));

    push_lookup_key(&mut keys, "fixture:ip:10.0.0.42".to_owned());
    push_lookup_key(&mut keys, "fixture:desk".to_owned());
    assert_eq!(
        keys,
        vec!["fixture:ip:10.0.0.42".to_owned(), "fixture:desk".to_owned()]
    );
}

fn tracked(ip: Option<&str>, fingerprint: Option<&str>) -> DriverTrackedDevice {
    DriverTrackedDevice {
        info: DeviceInfo {
            id: DeviceId::new(),
            name: "Fixture".to_owned(),
            vendor: "Fixture".to_owned(),
            family: DeviceFamily::new_static("fixture", "Fixture"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("fixture", "fixture", ConnectionType::Network),
            segments: Vec::new(),
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        },
        metadata: ip
            .map(|ip| HashMap::from([("ip".to_owned(), ip.to_owned())]))
            .unwrap_or_default(),
        fingerprint: fingerprint.map(|raw| DeviceFingerprint::from_persisted(raw.to_owned())),
        current_state: DeviceState::Known,
    }
}

#[test]
fn tracked_fingerprints_need_a_routable_address_and_a_fingerprint() {
    let tracked_devices = [
        tracked(Some("192.0.2.5"), Some("net:fixture:lamp")),
        tracked(Some("127.0.0.1"), Some("net:fixture:loopback")),
        tracked(Some("192.0.2.6"), None),
        tracked(None, Some("net:fixture:nowhere")),
    ];

    let fingerprints = tracked_fingerprints_by_ip(&tracked_devices);

    assert_eq!(fingerprints.len(), 1);
    assert_eq!(
        fingerprints[&IpAddr::V4(Ipv4Addr::new(192, 0, 2, 5))].as_str(),
        "net:fixture:lamp"
    );
}
