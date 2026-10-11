//! A launch port override moves the port and leaves the interfaces to
//! config, end to end through `prepare`.
//!
//! One test per binary: `prepare` installs the process-wide tracing
//! subscriber, which can happen only once.

use hypercolor_daemon::daemon::{DaemonRunOptions, build_main_runtime, prepare};
use hypercolor_daemon::startup::default_config;

/// Two distinct loopback ports that were free a moment ago.
fn free_loopback_ports() -> (u16, u16) {
    let first = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("an ephemeral loopback port should be available");
    let second = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a second ephemeral loopback port should be available");
    let port = |listener: &std::net::TcpListener| {
        listener
            .local_addr()
            .expect("listener address should resolve")
            .port()
    };
    (port(&first), port(&second))
}

#[test]
fn prepare_serves_the_default_config_on_loopback_at_the_override_port() {
    let directory = tempfile::tempdir().expect("config directory should be created");
    let config_path = directory.path().join("hypercolor.toml");
    let (override_port, configured_port) = free_loopback_ports();
    let mut config = default_config();
    // Never bound: the override must replace it.
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
        .expect("the default config should bind loopback at the override port");

    let bound = prepared.advertised_bind();
    assert!(
        bound.ip().is_loopback(),
        "local_only must stay on loopback, got {bound}"
    );
    assert_eq!(bound.port(), override_port);
}
