use std::net::IpAddr;

use hypercolor_app::daemon_client::{StoredServerApiKey, api_key_for_server};
use hypercolor_types::server::{DiscoveredServer, ServerIdentity};

fn server(instance_id: &str, host: &str, port: u16) -> DiscoveredServer {
    DiscoveredServer {
        identity: ServerIdentity {
            instance_id: instance_id.to_owned(),
            instance_name: "desk".to_owned(),
            version: "0.1.0".to_owned(),
        },
        host: host.parse::<IpAddr>().expect("test host should parse"),
        port,
        device_count: Some(1),
        auth_required: true,
    }
}

fn stored_key(instance_id: &str, host: &str, port: u16) -> StoredServerApiKey {
    StoredServerApiKey {
        instance_id: instance_id.to_owned(),
        host: host.parse::<IpAddr>().expect("test host should parse"),
        port,
        api_key: "hc_secret".to_owned(),
    }
}

#[test]
fn stored_daemon_api_key_requires_matching_instance_host_and_port() {
    let credentials = [stored_key("daemon-a", "192.0.2.10", 9420)];
    let discovered = server("daemon-a", "192.0.2.10", 9420);

    assert_eq!(
        api_key_for_server(&credentials, &discovered),
        Some("hc_secret")
    );
}

#[test]
fn stored_daemon_api_key_is_not_selected_for_spoofed_host() {
    let credentials = [stored_key("daemon-a", "192.0.2.10", 9420)];
    let spoofed = server("daemon-a", "192.0.2.66", 9420);

    assert_eq!(api_key_for_server(&credentials, &spoofed), None);
}

#[test]
fn stored_daemon_api_key_is_not_selected_for_spoofed_port() {
    let credentials = [stored_key("daemon-a", "192.0.2.10", 9420)];
    let spoofed = server("daemon-a", "192.0.2.10", 31337);

    assert_eq!(api_key_for_server(&credentials, &spoofed), None);
}

#[test]
fn stored_daemon_api_key_prefers_the_most_recent_duplicate() {
    let host = "192.0.2.10".parse::<IpAddr>().expect("test host should parse");
    let credentials = [
        StoredServerApiKey {
            instance_id: "daemon-a".to_owned(),
            host,
            port: 9420,
            api_key: "hc_rotated_out".to_owned(),
        },
        StoredServerApiKey {
            instance_id: "daemon-a".to_owned(),
            host,
            port: 9420,
            api_key: "hc_current".to_owned(),
        },
    ];
    let discovered = server("daemon-a", "192.0.2.10", 9420);

    assert_eq!(
        api_key_for_server(&credentials, &discovered),
        Some("hc_current")
    );
}
