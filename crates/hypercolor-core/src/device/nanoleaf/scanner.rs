//! Nanoleaf discovery scanner.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinSet;
use tracing::warn;

use crate::device::discovery::{DiscoveryConnectBehavior, TransportScanner};
use crate::device::net::{CredentialStore, MdnsBrowser};

use super::fetch_device_info;
use super::fetch_panel_layout;
use super::streaming::DEFAULT_NANOLEAF_API_PORT;
use super::types::{
    NanoleafDiscoveredDevice, NanoleafPanelLayout, build_device_info, panel_ids_from_layout,
};

const NANOLEAF_SERVICE_TYPE: &str = "_nanoleafapi._tcp.local.";
const DEFAULT_SCAN_TIMEOUT: Duration = Duration::from_secs(2);

/// Persistable hint for a known Nanoleaf device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NanoleafKnownDevice {
    pub device_id: String,
    pub ip: IpAddr,
    pub port: u16,
    pub name: String,
    pub model: String,
    pub firmware: String,
}

impl NanoleafKnownDevice {
    /// Create a minimal probe target from an IP address.
    #[must_use]
    pub fn from_ip(ip: IpAddr) -> Self {
        Self {
            device_id: String::new(),
            ip,
            port: DEFAULT_NANOLEAF_API_PORT,
            name: String::new(),
            model: String::new(),
            firmware: String::new(),
        }
    }

    fn merge_from(&mut self, other: &Self) {
        if self.device_id.is_empty() {
            self.device_id.clone_from(&other.device_id);
        }
        if self.port == 0 {
            self.port = other.port;
        }
        if self.name.is_empty() {
            self.name.clone_from(&other.name);
        }
        if self.model.is_empty() {
            self.model.clone_from(&other.model);
        }
        if self.firmware.is_empty() {
            self.firmware.clone_from(&other.firmware);
        }
    }
}

/// Nanoleaf `mDNS` + HTTP discovery scanner.
pub struct NanoleafScanner {
    scan_timeout: Duration,
    known_devices: Vec<NanoleafKnownDevice>,
    mdns_enabled: bool,
    credential_store: Arc<CredentialStore>,
}

impl NanoleafScanner {
    /// Create a scanner with the default timeout and no known devices.
    #[must_use]
    pub fn new(credential_store: Arc<CredentialStore>) -> Self {
        Self {
            scan_timeout: DEFAULT_SCAN_TIMEOUT,
            known_devices: Vec::new(),
            mdns_enabled: true,
            credential_store,
        }
    }

    /// Create a scanner seeded with persisted/known-device hints.
    #[must_use]
    pub fn with_options(
        known_devices: Vec<NanoleafKnownDevice>,
        credential_store: Arc<CredentialStore>,
        timeout: Duration,
        mdns_enabled: bool,
    ) -> Self {
        Self {
            scan_timeout: timeout,
            known_devices,
            mdns_enabled,
            credential_store,
        }
    }

    /// Create a scanner seeded with persisted/known-device hints.
    #[must_use]
    pub fn with_known_devices(
        known_devices: Vec<NanoleafKnownDevice>,
        credential_store: Arc<CredentialStore>,
        timeout: Duration,
    ) -> Self {
        Self::with_options(known_devices, credential_store, timeout, true)
    }

    /// Create a scanner from a list of manual IPs.
    #[must_use]
    pub fn with_known_ips(
        known_ips: Vec<IpAddr>,
        credential_store: Arc<CredentialStore>,
        timeout: Duration,
    ) -> Self {
        Self::with_options(
            known_ips
                .into_iter()
                .map(NanoleafKnownDevice::from_ip)
                .collect(),
            credential_store,
            timeout,
            true,
        )
    }

    /// Run discovery and return rich Nanoleaf device details.
    ///
    /// # Errors
    ///
    /// Returns an error when the shared `mDNS` browse helper cannot execute.
    pub async fn scan_devices(&mut self) -> Result<Vec<NanoleafDiscoveredDevice>> {
        let mut candidates: HashMap<IpAddr, NanoleafKnownDevice> = self
            .known_devices
            .iter()
            .cloned()
            .map(|device| (device.ip, device))
            .collect();

        if self.mdns_enabled {
            let browser = MdnsBrowser::new()?;
            let services = browser
                .browse(NANOLEAF_SERVICE_TYPE, self.scan_timeout)
                .await?;
            for service in services {
                let discovered = NanoleafKnownDevice {
                    device_id: service.txt.get("id").cloned().unwrap_or_default(),
                    ip: service.host,
                    port: service.port,
                    name: service
                        .txt
                        .get("name")
                        .or_else(|| service.txt.get("nm"))
                        .cloned()
                        .unwrap_or(service.name),
                    model: service
                        .txt
                        .get("model")
                        .or_else(|| service.txt.get("md"))
                        .cloned()
                        .unwrap_or_default(),
                    firmware: service
                        .txt
                        .get("firmware")
                        .or_else(|| service.txt.get("fw"))
                        .cloned()
                        .unwrap_or_default(),
                };

                candidates
                    .entry(discovered.ip)
                    .and_modify(|existing| existing.merge_from(&discovered))
                    .or_insert(discovered);
            }
        }

        let mut tasks = JoinSet::new();
        for candidate in candidates.into_values() {
            let credential_store = Arc::clone(&self.credential_store);
            tasks.spawn(async move { build_discovered_device(candidate, credential_store).await });
        }

        let mut devices = Vec::new();
        while let Some(task) = tasks.join_next().await {
            match task {
                Ok(Ok(device)) => devices.push(device),
                Ok(Err(error)) => {
                    warn!(error = %error, "Nanoleaf device probe failed");
                }
                Err(error) => {
                    warn!(error = %error, "Nanoleaf scanner task failed");
                }
            }
        }

        devices.sort_by(|left, right| left.info.name.cmp(&right.info.name));
        Ok(devices)
    }
}

impl Default for NanoleafScanner {
    fn default() -> Self {
        let store_dir = crate::config::ConfigManager::data_dir();
        let credential_store = CredentialStore::open_blocking(&store_dir)
            .expect("default Nanoleaf scanner should open credential store");
        Self::new(Arc::new(credential_store))
    }
}

#[async_trait::async_trait]
impl TransportScanner for NanoleafScanner {
    fn name(&self) -> &'static str {
        "Nanoleaf"
    }

    async fn scan(&mut self) -> Result<Vec<crate::device::discovery::DiscoveredDevice>> {
        Ok(self
            .scan_devices()
            .await?
            .into_iter()
            .map(NanoleafDiscoveredDevice::into_discovered)
            .collect())
    }
}

async fn build_discovered_device(
    candidate: NanoleafKnownDevice,
    credential_store: Arc<CredentialStore>,
) -> Result<NanoleafDiscoveredDevice> {
    let provisional_device_key = normalized_device_key(&candidate, None);
    let auth_token =
        load_auth_token(&credential_store, &provisional_device_key, candidate.ip).await;
    let mut name = candidate.name.clone();
    let mut model = candidate.model.clone();
    let mut firmware = candidate.firmware.clone();
    let mut serial_no = String::new();
    let mut panels: Vec<NanoleafPanelLayout> = Vec::new();
    let connect_behavior = if let Some(auth_token) = auth_token {
        match fetch_device_info(candidate.ip, candidate.port, &auth_token).await {
            Ok(device_info) => {
                if name.is_empty() {
                    name = device_info.name;
                }
                if model.is_empty() {
                    model = device_info.model;
                }
                if firmware.is_empty() {
                    firmware = device_info.firmware_version;
                }
                if serial_no.is_empty() {
                    serial_no = device_info.serial_no;
                }

                match fetch_panel_layout(candidate.ip, candidate.port, &auth_token).await {
                    Ok(layout) => {
                        panels = layout.position_data;
                        DiscoveryConnectBehavior::AutoConnect
                    }
                    Err(error) => {
                        warn!(
                            ip = %candidate.ip,
                            error = %error,
                            "failed to load Nanoleaf panel layout during discovery"
                        );
                        DiscoveryConnectBehavior::Deferred
                    }
                }
            }
            Err(error) => {
                warn!(
                    ip = %candidate.ip,
                    error = %error,
                    "failed to load Nanoleaf device info during discovery"
                );
                DiscoveryConnectBehavior::Deferred
            }
        }
    } else {
        DiscoveryConnectBehavior::Deferred
    };
    let device_key = normalized_device_key(
        &candidate,
        (!serial_no.is_empty()).then_some(serial_no.as_str()),
    );

    let info = build_device_info(
        &device_key,
        &name,
        (!model.is_empty()).then_some(model.as_str()),
        (!firmware.is_empty()).then_some(firmware.as_str()),
        panels.as_slice(),
    );
    let panel_ids = panel_ids_from_layout(panels.as_slice());

    let mut metadata = HashMap::new();
    metadata.insert("backend_id".to_owned(), "nanoleaf".to_owned());
    metadata.insert("ip".to_owned(), candidate.ip.to_string());
    metadata.insert("api_port".to_owned(), candidate.port.to_string());
    metadata.insert("device_key".to_owned(), device_key.clone());
    if !serial_no.is_empty() {
        metadata.insert("serial_no".to_owned(), serial_no);
    }
    if !model.is_empty() {
        metadata.insert("model".to_owned(), model);
    }
    if !firmware.is_empty() {
        metadata.insert("firmware".to_owned(), firmware.clone());
    }

    Ok(NanoleafDiscoveredDevice {
        device_key,
        ip: candidate.ip,
        api_port: candidate.port,
        info,
        panel_ids,
        connect_behavior,
        metadata,
    })
}

pub(super) async fn load_auth_token(
    credential_store: &CredentialStore,
    device_key: &str,
    ip: IpAddr,
) -> Option<String> {
    let lookup_keys = [
        format!("nanoleaf:{device_key}"),
        format!("nanoleaf:ip:{ip}"),
    ];
    for key in lookup_keys {
        let Some(credentials) = credential_store.get_json(&key).await else {
            continue;
        };
        let Some(auth_token) = credentials.get("auth_token").and_then(Value::as_str) else {
            continue;
        };
        return Some(auth_token.to_owned());
    }
    None
}

fn normalized_device_key(candidate: &NanoleafKnownDevice, serial_no: Option<&str>) -> String {
    if !candidate.device_id.trim().is_empty() {
        return candidate.device_id.trim().to_ascii_lowercase();
    }
    if let Some(serial_no) = serial_no
        && !serial_no.trim().is_empty()
    {
        return serial_no.trim().to_ascii_lowercase();
    }
    if !candidate.name.trim().is_empty() {
        return candidate.name.trim().to_ascii_lowercase().replace(' ', "-");
    }
    format!("ip:{}", candidate.ip)
}
