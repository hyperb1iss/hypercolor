use hypercolor_app::daemon_client::discovered_server_api_key_allowed;

#[test]
fn saved_api_keys_are_allowed_for_loopback_discovery_targets() {
    for host in ["localhost", "LOCALHOST", "127.0.0.1", "::1", "[::1]"] {
        assert!(
            discovered_server_api_key_allowed(host),
            "loopback discovery target should keep local daemon auth working: {host}"
        );
    }
}

#[test]
fn saved_api_keys_are_blocked_for_lan_discovery_targets() {
    for host in [
        "192.168.1.23",
        "10.0.0.42",
        "172.16.4.5",
        "fe80::1",
        "malicious-daemon.local",
    ] {
        assert!(
            !discovered_server_api_key_allowed(host),
            "unauthenticated mDNS target must not receive a saved daemon API key: {host}"
        );
    }
}
