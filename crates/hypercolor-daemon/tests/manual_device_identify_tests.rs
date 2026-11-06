//! Identify on a manual device the config no longer names.
//!
//! Identify is the one path that reconnects a retained device on request. It
//! re-adopts the device from the registry's last discovery metadata, so these
//! tests drive the real route against the real drivers and prove that
//! metadata cannot reach a removed device. Every packet stays on 127.0.0.1,
//! and every serial path sits in a fresh temporary directory.

#![cfg(feature = "builtin-drivers")]

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_core::device::{UnclaimedDeviceStore, UsbProtocolConfigStore};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::network;
use hypercolor_driver_api::{
    DiscoveryObservation, DiscoveryRequest, DriverConfigView, DriverContextProvider, OutputBinding,
};
use hypercolor_driver_support::CredentialStore;
use hypercolor_types::config::DriverConfigEntry;
use hypercolor_types::device::DeviceId;
use serde_json::{Value, json};
use tokio::net::UdpSocket;
use tower::ServiceExt;

const QUIET_WINDOW: Duration = Duration::from_millis(300);

struct Rig {
    state: Arc<AppState>,
    manager: Arc<ConfigManager>,
    driver_id: &'static str,
    _dir: tempfile::TempDir,
}

fn driver_entry(devices: &Value) -> DriverConfigEntry {
    DriverConfigEntry::enabled(
        [("devices".to_owned(), devices.clone())]
            .into_iter()
            .collect(),
    )
}

impl Rig {
    /// A daemon state whose config names `devices` and whose registry holds
    /// the built-in drivers, with only `driver_id`'s backend registered.
    async fn new(driver_id: &'static str, devices: &Value) -> Self {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let data_dir = dir.path().join("data");
        let manager = Arc::new(
            ConfigManager::new(dir.path().join("config.toml"))
                .expect("config manager should be created"),
        );
        manager.modify(|config| {
            config
                .drivers
                .insert(driver_id.to_owned(), driver_entry(devices));
        });
        let credentials = CredentialStore::open_blocking(&data_dir).expect("credential store");
        let registry = network::build_builtin_driver_module_registry(
            &manager.get(),
            Arc::new(credentials),
            UsbProtocolConfigStore::new(),
            UnclaimedDeviceStore::new(),
        )
        .expect("builtin registry");
        let state = AppState::builder()
            .with_config_manager(Arc::clone(&manager))
            .with_driver_registry(Arc::new(registry))
            .build();
        let rig = Self {
            state: Arc::new(state),
            manager,
            driver_id,
            _dir: dir,
        };
        rig.register_backend().await;
        rig
    }

    async fn register_backend(&self) {
        let driver = self
            .state
            .driver_registry()
            .get(self.driver_id)
            .expect("built-in driver");
        let OutputBinding::Owned { factory, .. } = driver.output() else {
            panic!("manual drivers own their backend");
        };
        let context = self.state.driver_host().driver_context(self.driver_id);
        let entry = context.config.current();
        let backend = factory
            .build(
                context,
                DriverConfigView {
                    driver_id: self.driver_id,
                    entry: &entry,
                },
            )
            .expect("backend");
        self.state
            .backend_manager
            .lock()
            .await
            .register_backend(backend);
    }

    /// Run the driver's discovery, which only reads config, and track its
    /// first device the way a scan does.
    async fn track_first_device(&self) -> DeviceId {
        let driver = self
            .state
            .driver_registry()
            .get(self.driver_id)
            .expect("built-in driver");
        let entry = network::driver_config_entry(&self.manager.get(), self.driver_id);
        let mut batch = driver
            .discovery()
            .expect("discovery capability")
            .discover(
                self.state.driver_host().as_ref(),
                &DiscoveryRequest {
                    timeout: Duration::from_millis(10),
                    mdns_enabled: false,
                },
                DriverConfigView {
                    driver_id: self.driver_id,
                    entry: &entry,
                },
            )
            .await
            .expect("discovery reads config only");
        let DiscoveryObservation::Device(discovered) = batch.observations.remove(0) else {
            panic!("configured devices are always reported in full");
        };
        self.state.device_registry.add_discovered(discovered).await
    }

    fn remove_every_device(&self) {
        self.manager.modify(|config| {
            config
                .drivers
                .insert(self.driver_id.to_owned(), driver_entry(&json!([])));
        });
    }

    async fn identify(&self, id: DeviceId) -> (StatusCode, Value) {
        let response = api::build_router(Arc::clone(&self.state), None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/devices/{id}/identify"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"duration_ms":1}"#))
                    .expect("request"),
            )
            .await
            .expect("router");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).expect("JSON response"),
        )
    }
}

async fn packets_until_quiet(socket: &UdpSocket) -> usize {
    let mut buf = [0_u8; 2048];
    let mut count = 0;
    while tokio::time::timeout(QUIET_WINDOW, socket.recv_from(&mut buf))
        .await
        .is_ok()
    {
        count += 1;
    }
    count
}

#[tokio::test]
async fn identify_on_a_removed_pixel_device_sends_nothing() {
    let receiver = UdpSocket::bind("127.0.0.1:0").await.expect("receiver");
    let port = receiver.local_addr().expect("address").port();
    let rig = Rig::new(
        "pixel",
        &json!([{
            "id": "roof", "host": "127.0.0.1", "port": port, "protocol": "ddp", "pixel_count": 4
        }]),
    )
    .await;
    let id = rig.track_first_device().await;

    // Control: while the config names the device, the same request lights it.
    let (status, _) = rig.identify(id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        packets_until_quiet(&receiver).await > 0,
        "identify reaches a configured device"
    );

    rig.remove_every_device();
    assert!(
        rig.state
            .device_registry
            .metadata_for_id(&id)
            .await
            .is_some_and(|metadata| metadata.contains_key("pixel_device")),
        "the registry still holds the discovery metadata"
    );
    let (status, body) = rig.identify(id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.to_string().contains("no device in the current config"),
        "{body}"
    );
    assert_eq!(
        packets_until_quiet(&receiver).await,
        0,
        "a removed device gets no packet"
    );
}

#[tokio::test]
async fn identify_on_a_removed_adalight_device_opens_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let port = dir.path().join("no-such-port");
    let rig = Rig::new(
        "adalight",
        &json!([{ "id": "desk", "port": port.to_string_lossy(), "led_count": 4 }]),
    )
    .await;
    let id = rig.track_first_device().await;

    // Control: while the config names the device, identify tries the port.
    let (status, body) = rig.identify(id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.to_string().contains("cannot open serial port"),
        "{body}"
    );

    rig.remove_every_device();
    let (status, body) = rig.identify(id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let body = body.to_string();
    assert!(body.contains("no device in the current config"), "{body}");
    assert!(
        !body.contains("cannot open serial port"),
        "a removed device never reaches the open: {body}"
    );
}

#[tokio::test]
async fn driver_contexts_read_their_own_entry_live() {
    let rig = Rig::new("pixel", &json!([])).await;
    let context = rig.state.driver_host().driver_context("pixel");
    assert_eq!(context.config.current().settings["devices"], json!([]));

    let devices = json!([{ "id": "a", "host": "127.0.0.1", "protocol": "ddp", "pixel_count": 1 }]);
    rig.manager.modify(|config| {
        config
            .drivers
            .insert("pixel".to_owned(), driver_entry(&devices));
        config.drivers.insert(
            "adalight".to_owned(),
            DriverConfigEntry::disabled([].into()),
        );
    });
    let entry = context.config.current();
    assert!(entry.enabled, "another driver's entry never leaks in");
    assert_eq!(entry.settings["devices"], devices, "the handle reads live");
    assert_eq!(
        context.installation_id.as_str(),
        rig.state.server_identity.instance_id
    );
}
