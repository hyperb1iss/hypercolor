//! A `--port` launch whose configured interface address is unavailable
//! still comes up on loopback, end to end through `prepare`.
//!
//! One test per binary: `prepare` installs the process-wide tracing
//! subscriber, which can happen only once.

use std::net::{Ipv6Addr, SocketAddr};

use hypercolor_daemon::daemon::{DaemonRunOptions, build_main_runtime, prepare};
use hypercolor_daemon::startup::default_config;
use hypercolor_types::config::NetworkAccessMode;

/// Two distinct ports that were free a moment ago on both loopback
/// families.
fn free_loopback_ports() -> (u16, u16) {
    let mut held = Vec::new();
    let mut ports = Vec::new();
    while ports.len() < 2 {
        let v4 = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("an ephemeral IPv4 loopback port should be available");
        let port = v4
            .local_addr()
            .expect("listener address should resolve")
            .port();
        if std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, port)).is_ok() {
            ports.push(port);
        }
        held.push(v4);
    }
    (ports[0], ports[1])
}

#[test]
fn prepare_serves_loopback_when_the_configured_interface_is_unavailable() {
    let directory = tempfile::tempdir().expect("config directory should be created");
    let config_path = directory.path().join("hypercolor.toml");
    let (override_port, configured_port) = free_loopback_ports();
    let mut config = default_config();
    config.network.access_mode = NetworkAccessMode::Custom;
    config.network.allow_unauthenticated_remote_access = true;
    // TEST-NET-1 (RFC 5737) is never assigned to a real interface, so this
    // fails to bind the way a stale or not-yet-assigned address does.
    config.daemon.listen_address = "192.0.2.1".to_owned();
    config.daemon.port = configured_port;
    std::fs::write(
        &config_path,
        toml::to_string(&config).expect("config should serialize"),
    )
    .expect("config should be written");

    let runtime = build_main_runtime().expect("runtime should build");
    let prepared = runtime
        .block_on(prepare(DaemonRunOptions {
            config: Some(config_path),
            port: Some(override_port),
            ..DaemonRunOptions::default()
        }))
        .expect("an unavailable configured address must not stop a --port launch");

    assert_eq!(
        prepared
            .api_listen_addresses()
            .expect("listener addresses should resolve"),
        vec![
            SocketAddr::from(([127, 0, 0, 1], override_port)),
            SocketAddr::from((Ipv6Addr::LOCALHOST, override_port)),
        ]
    );
    assert!(prepared.advertised_bind().ip().is_loopback());
}
