use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use hypercolor_driver_api::validation::{ValidationError, validate_ip, validate_port};
use hypercolor_driver_api::{DRIVER_API_SCHEMA_VERSION, DriverDescriptor, DriverTransport};

#[test]
fn schema_version_constant_is_stamped_onto_new_descriptors() {
    let descriptor = DriverDescriptor::new(
        "fixture-network",
        "Fixture Network",
        DriverTransport::Network,
        true,
        true,
    );
    assert_eq!(descriptor.schema_version, DRIVER_API_SCHEMA_VERSION);
}

#[test]
fn with_schema_version_accepts_explicit_value() {
    let descriptor = DriverDescriptor::with_schema_version(
        "legacy",
        "Legacy",
        DriverTransport::Network,
        true,
        false,
        0,
    );
    assert_eq!(descriptor.schema_version, 0);
}

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
fn validate_ip_rejects_loopback_multicast_and_unspecified() {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let unspecified = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let broadcast = IpAddr::V4(Ipv4Addr::BROADCAST);
    let multicast = IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1));
    let link_local = IpAddr::V4(Ipv4Addr::new(169, 254, 1, 2));
    let ipv6_loopback = IpAddr::V6(Ipv6Addr::LOCALHOST);

    for ip in [
        loopback,
        unspecified,
        broadcast,
        multicast,
        link_local,
        ipv6_loopback,
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
