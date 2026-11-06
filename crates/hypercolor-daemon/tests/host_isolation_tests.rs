//! Daemons composed for tests stay off the host.
//!
//! `DaemonState` takes its `HostPorts` as a required argument, and
//! `HostPorts::isolated` takes a live claim on the user directories, so a
//! fixture cannot boot without choosing them. These tests pin what the
//! isolated composition holds: drivers that keep their contracts but never
//! scan, and no input source or session monitor that reads the host.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::config::paths::IsolatedUserDirs;
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::network;
use hypercolor_daemon::startup::{DaemonState, HostPorts, default_config};
use hypercolor_driver_api::{
    DiscoveryBatch, DiscoveryCapability, DiscoveryRequest, DriverConfigView, DriverDescriptor,
    DriverError, DriverHost, DriverModule,
};
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::device::DriverTransportKind;
use tower::ServiceExt;

static COUNTING_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "counting_test",
    "Counting Test",
    DriverTransportKind::Network,
    true,
    false,
);

/// Stands in for a real scanner: every run it counts is a run that would
/// have reached the host.
struct CountingDriver {
    scans: Arc<AtomicUsize>,
}

impl DriverModule for CountingDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &COUNTING_DRIVER
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DiscoveryCapability for CountingDriver {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        _config: DriverConfigView<'_>,
    ) -> Result<DiscoveryBatch, DriverError> {
        self.scans.fetch_add(1, Ordering::Relaxed);
        Ok(DiscoveryBatch::complete(Vec::new()))
    }
}

fn counting_registry(scans: &Arc<AtomicUsize>) -> DriverModuleRegistry {
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(CountingDriver {
            scans: Arc::clone(scans),
        })
        .expect("counting driver registers");
    registry
}

fn isolated_daemon(root: &std::path::Path, dirs: &IsolatedUserDirs) -> DaemonState {
    let config = default_config();
    let manager = Arc::new(ConfigManager::from_config_unchecked(
        root.join("hypercolor.toml"),
        config.clone(),
    ));
    DaemonState::initialize(
        hypercolor_core::config::BootConfig::from_config_unchecked(config),
        manager,
        HostPorts::isolated(dirs),
    )
    .expect("isolated daemon initializes")
}

/// Run a full wait-mode scan through the API and return its scanner reports.
async fn scan_every_enabled_driver(state: Arc<AppState>) -> Vec<serde_json::Value> {
    let response = api::build_router(state, None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"timeout_ms": 100, "wait": true}"#))
                .expect("discover request builds"),
        )
        .await
        .expect("discover request runs");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("discover body reads");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("discover body parses");
    json["data"]["result"]["scanners"]
        .as_array()
        .expect("scan reports its scanners")
        .clone()
}

fn assert_found_nothing(scanners: &[serde_json::Value]) {
    for scanner in scanners {
        assert_eq!(scanner["discovered"], 0, "unexpected devices: {scanner}");
        assert!(scanner["error"].is_null(), "unexpected failure: {scanner}");
    }
}

#[tokio::test]
async fn isolated_ports_compose_no_host_service() {
    let root = tempfile::tempdir().expect("fixture root");
    let dirs = IsolatedUserDirs::claim(root.path());

    // Boxed so the state is not held inline across awaits.
    let mut daemon = Box::new(isolated_daemon(root.path(), &dirs));

    // No sensor broker, media bus, network counters, native input, or
    // screen portal; the audio role stays, on a source with no device.
    assert_eq!(daemon.input_manager().source_names(), ["AudioInput(none)"]);
    // The default config keeps background discovery on, so starting also
    // runs the startup scan and worker against the offline drivers.
    daemon.start().await.expect("isolated daemon starts");
    let scanners = scan_every_enabled_driver(Arc::new(AppState::from_daemon_state(&daemon))).await;
    daemon.shutdown().await.expect("isolated daemon shuts down");
    assert_found_nothing(&scanners);
}

#[tokio::test]
async fn isolated_app_state_finds_nothing_on_the_host() {
    let scanners = scan_every_enabled_driver(Arc::new(AppState::new())).await;

    assert_found_nothing(&scanners);
}

#[tokio::test]
async fn isolated_app_state_lists_no_host_audio_device() {
    let response = api::build_router(Arc::new(AppState::new()), None)
        .oneshot(
            Request::builder()
                .uri("/api/v1/system/audio-devices")
                .body(Body::empty())
                .expect("audio device request builds"),
        )
        .await
        .expect("audio device request runs");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("audio device body reads");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("audio device body parses");

    let ids: Vec<&str> = json["data"]["devices"]
        .as_array()
        .expect("response lists devices")
        .iter()
        .filter_map(|device| device["id"].as_str())
        .collect();

    assert_eq!(ids, ["default", "microphone", "none"]);
}

#[tokio::test]
async fn an_offline_driver_keeps_its_contract_and_never_runs_its_scanner() {
    let scans = Arc::new(AtomicUsize::new(0));
    let offline = network::offline_driver_module_registry(&counting_registry(&scans))
        .expect("drivers compose offline");
    let driver = offline
        .get("counting_test")
        .expect("driver stays registered");
    assert!(driver.module_descriptor().capabilities.discovery);
    assert!(driver.discovery().is_some());
    let state = AppState::builder()
        .with_driver_registry(Arc::new(offline))
        .build();

    let scanners = scan_every_enabled_driver(Arc::new(state)).await;

    assert_eq!(scanners.len(), 1, "the target still scans: {scanners:?}");
    assert_eq!(scans.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn an_injected_registry_scans_because_the_test_chose_it() {
    let scans = Arc::new(AtomicUsize::new(0));
    let state = AppState::builder()
        .with_driver_registry(Arc::new(counting_registry(&scans)))
        .build();

    let scanners = scan_every_enabled_driver(Arc::new(state)).await;

    assert_eq!(scanners.len(), 1, "the target scans: {scanners:?}");
    assert_eq!(scans.load(Ordering::Relaxed), 1);
}
