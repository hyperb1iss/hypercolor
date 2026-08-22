use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use hypercolor_driver_support::network::{
    ValidationError, metadata_value, network_ip_from_metadata, push_lookup_key, validate_ip,
    validate_port,
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
