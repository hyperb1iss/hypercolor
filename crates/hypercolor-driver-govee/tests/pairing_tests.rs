use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use hypercolor_driver_api::{
    DeviceAuthState, DriverCredentialStore, DriverDiscoveryState, DriverHost, DriverModule,
    DriverRuntimeActions, PairDeviceRequest, PairDeviceStatus, PairingFlowKind, TrackedDeviceCtx,
};
use hypercolor_driver_govee::{GoveeDriverModule, GoveeLanDevice, build_device_info};
use hypercolor_types::config::GoveeConfig;
use hypercolor_types::device::{DeviceId, DeviceInfo, DeviceState};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[tokio::test]
async fn pair_validates_and_stores_account_api_key() {
    let body = r#"{"code":200,"message":"Success","data":{"devices":[]}}"#;
    let (base_url, request) = serve_once(200, body).await;
    let factory = GoveeDriverModule::with_cloud_base_url(GoveeConfig::default(), base_url);
    let host = TestHost::with_activation(true);
    let info = test_device_info();
    let state = DeviceState::Known;
    let context = tracked_context(&info, &state);
    let request_payload = PairDeviceRequest {
        values: HashMap::from([("api_key".to_owned(), " test-key ".to_owned())]),
        activate_after_pair: true,
    };

    let outcome = factory
        .pairing()
        .expect("Govee factory should expose pairing")
        .pair(&host, &context, &request_payload)
        .await
        .expect("valid key should pair");

    assert_eq!(outcome.status, PairDeviceStatus::Paired);
    assert_eq!(outcome.auth_state, DeviceAuthState::Configured);
    assert!(outcome.activated);
    assert!(host.runtime.activated.load(Ordering::SeqCst));
    assert_eq!(
        host.credentials
            .get_json("govee", "account")
            .await
            .expect("stored credentials should read"),
        Some(serde_json::json!({ "api_key": "test-key" }))
    );

    let request = request.await.expect("server task should join");
    assert_header_present(&request, "govee-api-key", "test-key");
}

#[tokio::test]
async fn pair_rejects_missing_api_key_without_network_call() {
    let factory = GoveeDriverModule::new(GoveeConfig::default());
    let host = TestHost::default();
    let info = test_device_info();
    let state = DeviceState::Known;
    let context = tracked_context(&info, &state);

    let outcome = factory
        .pairing()
        .expect("Govee factory should expose pairing")
        .pair(&host, &context, &PairDeviceRequest::default())
        .await
        .expect("missing key should be handled as invalid input");

    assert_eq!(outcome.status, PairDeviceStatus::InvalidInput);
    assert_eq!(outcome.auth_state, DeviceAuthState::Open);
    assert_eq!(
        host.credentials
            .get_json("govee", "account")
            .await
            .expect("credentials should read"),
        None
    );
}

#[tokio::test]
async fn auth_summary_and_clear_credentials_use_account_key() {
    let factory = GoveeDriverModule::new(GoveeConfig::default());
    let host = TestHost::default();
    let info = test_device_info();
    let state = DeviceState::Known;
    let context = tracked_context(&info, &state);
    let pairing = factory
        .pairing()
        .expect("Govee factory should expose pairing");

    let open = pairing
        .auth_summary(&host, &context)
        .await
        .expect("Govee should report auth summary");
    assert_eq!(open.state, DeviceAuthState::Open);
    assert!(open.can_pair);
    assert_eq!(
        open.descriptor.expect("descriptor should be present").kind,
        PairingFlowKind::CredentialsForm
    );

    host.credentials
        .set_json(
            "govee",
            "account",
            serde_json::json!({ "api_key": "stored-key" }),
        )
        .await
        .expect("credentials should store");

    let configured = pairing
        .auth_summary(&host, &context)
        .await
        .expect("Govee should report auth summary");
    assert_eq!(configured.state, DeviceAuthState::Configured);
    assert!(!configured.can_pair);
    assert!(configured.descriptor.is_none());

    let cleared = pairing
        .clear_credentials(&host, &context)
        .await
        .expect("credentials should clear");

    assert_eq!(cleared.auth_state, DeviceAuthState::Open);
    assert!(cleared.disconnected);
    assert!(host.runtime.disconnected.load(Ordering::SeqCst));
    assert_eq!(
        host.credentials
            .get_json("govee", "account")
            .await
            .expect("credentials should read"),
        None
    );
}

#[tokio::test]
async fn auth_summary_requires_pairing_for_cloud_only_inventory() {
    let factory = GoveeDriverModule::new(GoveeConfig::default());
    let host = TestHost::default();
    let info = test_device_info();
    let state = DeviceState::Known;
    let metadata = HashMap::from([
        ("cloud_device_id".to_owned(), "AA:BB:CC:DD:EE:FF".to_owned()),
        ("sku".to_owned(), "H6163".to_owned()),
    ]);
    let context = tracked_context_with_metadata(&info, &state, &metadata);

    let summary = factory
        .pairing()
        .expect("Govee factory should expose pairing")
        .auth_summary(&host, &context)
        .await
        .expect("Govee should report auth summary");

    assert_eq!(summary.state, DeviceAuthState::Required);
    assert!(summary.can_pair);
    assert!(summary.descriptor.is_some());
}

#[tokio::test]
async fn auth_summary_does_not_offer_pairing_for_lan_only_sku() {
    let factory = GoveeDriverModule::new(GoveeConfig::default());
    let host = TestHost::default();
    let info = build_device_info(&GoveeLanDevice {
        ip: "127.0.0.1".parse().expect("valid test IP"),
        sku: "H70B6".to_owned(),
        mac: "001122334455".to_owned(),
        name: "LAN-only Govee".to_owned(),
        firmware_version: None,
    });
    let state = DeviceState::Known;
    let metadata = HashMap::from([
        ("ip".to_owned(), "127.0.0.1".to_owned()),
        ("sku".to_owned(), "H70B6".to_owned()),
    ]);
    let context = tracked_context_with_metadata(&info, &state, &metadata);

    let summary = factory
        .pairing()
        .expect("Govee factory should expose pairing")
        .auth_summary(&host, &context)
        .await
        .expect("Govee should report auth summary");

    assert_eq!(summary.state, DeviceAuthState::Open);
    assert!(!summary.can_pair);
    assert!(summary.descriptor.is_none());
}

fn tracked_context<'a>(
    info: &'a DeviceInfo,
    current_state: &'a DeviceState,
) -> TrackedDeviceCtx<'a> {
    TrackedDeviceCtx {
        device_id: info.id,
        info,
        metadata: None,
        current_state,
    }
}

fn tracked_context_with_metadata<'a>(
    info: &'a DeviceInfo,
    current_state: &'a DeviceState,
    metadata: &'a HashMap<String, String>,
) -> TrackedDeviceCtx<'a> {
    TrackedDeviceCtx {
        device_id: info.id,
        info,
        metadata: Some(metadata),
        current_state,
    }
}

fn test_device_info() -> DeviceInfo {
    build_device_info(&GoveeLanDevice {
        ip: "127.0.0.1".parse().expect("valid test IP"),
        sku: "H6163".to_owned(),
        mac: "aabbccddeeff".to_owned(),
        name: "Test Govee".to_owned(),
        firmware_version: None,
    })
}

#[derive(Default)]
struct TestCredentialStore {
    values: Mutex<HashMap<String, Value>>,
}

#[async_trait]
impl DriverCredentialStore for TestCredentialStore {
    async fn get_json(&self, driver_id: &str, key: &str) -> Result<Option<Value>> {
        Ok(self
            .values
            .lock()
            .await
            .get(&format!("{driver_id}:{key}"))
            .cloned())
    }

    async fn set_json(&self, driver_id: &str, key: &str, value: Value) -> Result<()> {
        self.values
            .lock()
            .await
            .insert(format!("{driver_id}:{key}"), value);
        Ok(())
    }

    async fn remove(&self, driver_id: &str, key: &str) -> Result<()> {
        self.values
            .lock()
            .await
            .remove(&format!("{driver_id}:{key}"));
        Ok(())
    }
}

struct TestRuntimeActions {
    activate_return: bool,
    activated: AtomicBool,
    disconnected: AtomicBool,
}

impl Default for TestRuntimeActions {
    fn default() -> Self {
        Self {
            activate_return: false,
            activated: AtomicBool::new(false),
            disconnected: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl DriverRuntimeActions for TestRuntimeActions {
    async fn activate_device(&self, device_id: DeviceId, backend_id: &str) -> Result<bool> {
        let _ = (device_id, backend_id);
        self.activated.store(true, Ordering::SeqCst);
        Ok(self.activate_return)
    }

    async fn disconnect_device(
        &self,
        device_id: DeviceId,
        backend_id: &str,
        will_retry: bool,
    ) -> Result<bool> {
        let _ = (device_id, backend_id, will_retry);
        self.disconnected.store(true, Ordering::SeqCst);
        Ok(true)
    }
}

#[derive(Default)]
struct TestDiscoveryState;

#[async_trait]
impl DriverDiscoveryState for TestDiscoveryState {
    async fn tracked_devices(
        &self,
        backend_id: &str,
    ) -> Vec<hypercolor_driver_api::DriverTrackedDevice> {
        let _ = backend_id;
        Vec::new()
    }

    fn load_cached_json(&self, driver_id: &str, key: &str) -> Result<Option<Value>> {
        let _ = (driver_id, key);
        Ok(None)
    }
}

#[derive(Default)]
struct TestHost {
    credentials: TestCredentialStore,
    runtime: TestRuntimeActions,
    discovery: TestDiscoveryState,
}

impl TestHost {
    fn with_activation(activate_return: bool) -> Self {
        Self {
            runtime: TestRuntimeActions {
                activate_return,
                ..TestRuntimeActions::default()
            },
            ..Self::default()
        }
    }
}

impl DriverHost for TestHost {
    fn credentials(&self) -> &dyn DriverCredentialStore {
        &self.credentials
    }

    fn runtime(&self) -> &dyn DriverRuntimeActions {
        &self.runtime
    }

    fn discovery_state(&self) -> &dyn DriverDiscoveryState {
        &self.discovery
    }
}

async fn serve_once(status: u16, body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test HTTP listener should bind");
    let address = listener
        .local_addr()
        .expect("test listener should have local address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .expect("test HTTP connection should arrive");
        let mut buf = [0_u8; 4096];
        let len = stream
            .read(&mut buf)
            .await
            .expect("test HTTP request should read");
        let request = String::from_utf8(buf[..len].to_vec()).expect("request should be UTF-8");
        let response = http_response(status, body);
        stream
            .write_all(response.as_bytes())
            .await
            .expect("test HTTP response should write");
        request
    });

    (format!("http://{address}/v1"), task)
}

fn http_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        _ => "Test",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn assert_header_present(request: &str, name: &str, value: &str) {
    let expected = format!("{name}: {value}");
    assert!(
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&expected)),
        "missing expected header {expected:?} in request:\n{request}"
    );
}
