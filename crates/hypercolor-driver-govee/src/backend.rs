use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use hypercolor_driver_api::{
    BackendInfo, CredentialStore, DeviceBackend, DeviceFrameSink, HealthStatus, TransportScanner,
};
use hypercolor_types::config::GoveeConfig;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

use crate::capabilities::{GoveeCapabilities, SkuProfile, fallback_profile, profile_for_sku};
use crate::cloud::{CloudClient, V1Command, V1Device};
use crate::lan::discovery::{GoveeKnownDevice, GoveeLanDevice, GoveeLanScanner, build_device_info};
use crate::lan::protocol::{DEVICE_PORT, LanCommand, encode_command};
use crate::lan::razer::{encode_razer_frame_base64, encode_razer_mode_base64};

pub struct GoveeBackend {
    config: GoveeConfig,
    devices: HashMap<DeviceId, Arc<Mutex<GoveeDeviceState>>>,
    shared_socket: SharedLanSocket,
    credential_store: Option<Arc<CredentialStore>>,
    cloud_base_url: Option<String>,
    cloud_client: Option<CloudClient>,
}

type SharedLanSocket = Arc<Mutex<Option<Arc<UdpSocket>>>>;

#[derive(Clone)]
struct GoveeDeviceState {
    info: DeviceInfo,
    profile: SkuProfile,
    address: Option<SocketAddr>,
    cloud_id: Option<String>,
    last_sent: Option<Vec<[u8; 3]>>,
    last_write_at: Option<Instant>,
    razer_enabled: bool,
}

impl GoveeBackend {
    #[must_use]
    pub fn new(config: GoveeConfig) -> Self {
        Self {
            config,
            devices: HashMap::new(),
            shared_socket: Arc::new(Mutex::new(None)),
            credential_store: None,
            cloud_base_url: None,
            cloud_client: None,
        }
    }

    #[must_use]
    pub fn with_credential_store(mut self, credential_store: Arc<CredentialStore>) -> Self {
        self.credential_store = Some(credential_store);
        self
    }

    #[must_use]
    pub fn with_cloud_client(mut self, cloud_client: CloudClient) -> Self {
        self.cloud_client = Some(cloud_client);
        self
    }

    #[must_use]
    pub fn with_cloud_base_url(mut self, cloud_base_url: impl Into<String>) -> Self {
        self.cloud_base_url = Some(cloud_base_url.into());
        self
    }

    pub fn remember_device(&mut self, device: GoveeLanDevice) {
        self.remember_device_at(device.clone(), SocketAddr::new(device.ip, DEVICE_PORT));
    }

    pub fn remember_device_at(&mut self, device: GoveeLanDevice, address: SocketAddr) {
        let info = build_device_info(&device);
        let profile = profile_for_sku(&device.sku).unwrap_or_else(|| fallback_profile(&device.sku));
        self.devices
            .entry(info.id)
            .and_modify(|state| {
                if let Ok(mut state) = state.try_lock() {
                    state.info = info.clone();
                    state.profile = profile.clone();
                    state.address = Some(address);
                }
            })
            .or_insert_with(|| {
                Arc::new(Mutex::new(GoveeDeviceState {
                    info,
                    profile,
                    address: Some(address),
                    cloud_id: None,
                    last_sent: None,
                    last_write_at: None,
                    razer_enabled: false,
                }))
            });
    }

    pub fn remember_cloud_device(&mut self, device: V1Device) {
        let discovered = crate::build_cloud_discovered_device(device.clone());
        let profile =
            profile_for_sku(&device.model).unwrap_or_else(|| fallback_profile(&device.model));
        self.devices
            .entry(discovered.info.id)
            .and_modify(|state| {
                if let Ok(mut state) = state.try_lock() {
                    state.cloud_id = Some(device.device.clone());
                }
            })
            .or_insert_with(|| {
                Arc::new(Mutex::new(GoveeDeviceState {
                    info: discovered.info,
                    profile,
                    address: None,
                    cloud_id: Some(device.device),
                    last_sent: None,
                    last_write_at: None,
                    razer_enabled: false,
                }))
            });
    }

    async fn ensure_socket(shared_socket: &SharedLanSocket) -> Result<Arc<UdpSocket>> {
        let mut shared_socket = shared_socket.lock().await;
        if let Some(socket) = shared_socket.as_ref() {
            return Ok(Arc::clone(socket));
        }

        let socket = Arc::new(
            UdpSocket::bind(("0.0.0.0", 0))
                .await
                .context("failed to bind Govee LAN command socket")?,
        );
        *shared_socket = Some(Arc::clone(&socket));
        Ok(socket)
    }

    async fn send_lan_command(
        shared_socket: &SharedLanSocket,
        address: SocketAddr,
        command: LanCommand,
    ) -> Result<()> {
        let payload = encode_command(&command)?;
        let socket = Self::ensure_socket(shared_socket).await?;
        socket
            .send_to(&payload, address)
            .await
            .with_context(|| format!("failed to send Govee LAN command to {address}"))?;
        Ok(())
    }

    async fn send_command(&self, id: &DeviceId, command: LanCommand) -> Result<()> {
        let device = self
            .devices
            .get(id)
            .with_context(|| format!("Govee device {id} is not known"))?
            .lock()
            .await;
        let address = device
            .address
            .with_context(|| format!("Govee device {id} has no LAN address"))?;
        drop(device);
        Self::send_lan_command(&self.shared_socket, address, command).await
    }

    fn frame_interval_for(config: &GoveeConfig, device: &GoveeDeviceState) -> Duration {
        if device.address.is_none() {
            return Duration::from_secs(6);
        }
        let fps = if device
            .profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
        {
            config.razer_fps
        } else {
            config.lan_state_fps
        }
        .max(1);
        Duration::from_millis(1000 / u64::from(fps))
    }

    async fn cloud_client_from(
        credential_store: Option<&Arc<CredentialStore>>,
        cloud_client: Option<&CloudClient>,
        cloud_base_url: Option<&str>,
    ) -> Result<Option<CloudClient>> {
        if let Some(client) = cloud_client {
            return Ok(Some(client.clone()));
        }

        let Some(store) = credential_store else {
            return Ok(None);
        };
        let Some(api_key) = store
            .get_json("govee:account")
            .await
            .and_then(|value| {
                value
                    .get("api_key")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .map(ToOwned::to_owned)
            })
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };

        match cloud_base_url {
            Some(base_url) => CloudClient::with_base_url(api_key, base_url).map(Some),
            None => CloudClient::new(api_key).map(Some),
        }
    }

    async fn cloud_client(&self) -> Result<Option<CloudClient>> {
        Self::cloud_client_from(
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
        )
        .await
    }

    async fn send_cloud_command_to(
        credential_store: Option<&Arc<CredentialStore>>,
        cloud_client: Option<&CloudClient>,
        cloud_base_url: Option<&str>,
        model: &str,
        cloud_id: &str,
        command: V1Command,
    ) -> Result<()> {
        let client = Self::cloud_client_from(credential_store, cloud_client, cloud_base_url)
            .await?
            .context("Govee cloud credentials are not configured")?;
        client.v1_control(model, cloud_id, command).await
    }

    async fn send_cloud_command(&self, id: &DeviceId, command: V1Command) -> Result<()> {
        let device = self
            .devices
            .get(id)
            .with_context(|| format!("Govee device {id} is not known"))?
            .lock()
            .await;
        let cloud_id = device
            .cloud_id
            .clone()
            .with_context(|| format!("Govee device {id} is not cloud-backed"))?;
        let model = device.info.model.clone().unwrap_or_default();
        drop(device);
        Self::send_cloud_command_to(
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
            &model,
            &cloud_id,
            command,
        )
        .await
    }

    async fn write_device_colors(
        id: &DeviceId,
        device: &Arc<Mutex<GoveeDeviceState>>,
        colors: &[[u8; 3]],
        config: &GoveeConfig,
        shared_socket: &SharedLanSocket,
        credential_store: Option<&Arc<CredentialStore>>,
        cloud_client: Option<&CloudClient>,
        cloud_base_url: Option<&str>,
    ) -> Result<()> {
        if colors.is_empty() {
            return Ok(());
        }

        let mut device = device.lock().await;
        let (command, sent_frame) = if device
            .profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
            && device
                .profile
                .razer_led_count
                .is_some_and(|count| usize::from(count) == colors.len())
            && let Some(pt) = encode_razer_frame_base64(colors)
        {
            (LanCommand::Razer { pt }, colors.to_vec())
        } else {
            let [red, green, blue] = mean_color(colors);
            (
                LanCommand::ColorWc { red, green, blue },
                vec![[red, green, blue]],
            )
        };

        if device.last_sent.as_deref() == Some(sent_frame.as_slice()) {
            return Ok(());
        }
        if device.last_write_at.is_some_and(|last_write| {
            last_write.elapsed() < Self::frame_interval_for(config, &device)
        }) {
            return Ok(());
        }

        match command {
            LanCommand::Razer { pt } => {
                let address = device
                    .address
                    .with_context(|| format!("Govee device {id} has no LAN address"))?;
                Self::send_lan_command(shared_socket, address, LanCommand::Razer { pt }).await?;
            }
            LanCommand::ColorWc { red, green, blue } => {
                if let Some(address) = device.address {
                    Self::send_lan_command(
                        shared_socket,
                        address,
                        LanCommand::ColorWc { red, green, blue },
                    )
                    .await?;
                } else {
                    let cloud_id = device
                        .cloud_id
                        .clone()
                        .with_context(|| format!("Govee device {id} is not cloud-backed"))?;
                    let model = device.info.model.clone().unwrap_or_default();
                    Self::send_cloud_command_to(
                        credential_store,
                        cloud_client,
                        cloud_base_url,
                        &model,
                        &cloud_id,
                        V1Command::Color {
                            r: red,
                            g: green,
                            b: blue,
                        },
                    )
                    .await?;
                }
            }
            _ => unreachable!("Govee write_colors only emits color frame commands"),
        }

        device.last_sent = Some(sent_frame);
        device.last_write_at = Some(Instant::now());
        Ok(())
    }
}

struct GoveeFrameSink {
    device_id: DeviceId,
    device: Arc<Mutex<GoveeDeviceState>>,
    config: GoveeConfig,
    shared_socket: SharedLanSocket,
    credential_store: Option<Arc<CredentialStore>>,
    cloud_client: Option<CloudClient>,
    cloud_base_url: Option<String>,
}

#[async_trait]
impl DeviceFrameSink for GoveeFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<()> {
        GoveeBackend::write_device_colors(
            &self.device_id,
            &self.device,
            colors.as_slice(),
            &self.config,
            &self.shared_socket,
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
        )
        .await
    }
}

#[async_trait]
impl DeviceBackend for GoveeBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "govee".to_owned(),
            name: "Govee LAN".to_owned(),
            description: "Govee Wi-Fi lights over LAN UDP".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        let known_devices = self
            .config
            .known_ips
            .iter()
            .copied()
            .map(GoveeKnownDevice::from_ip)
            .collect();
        let mut scanner = GoveeLanScanner::new(known_devices, std::time::Duration::from_secs(2));
        let discovered = scanner.scan().await?;

        self.devices.clear();
        let mut infos = Vec::with_capacity(discovered.len());
        for device in discovered {
            let ip = device
                .metadata
                .get("ip")
                .and_then(|value| value.parse::<IpAddr>().ok());
            let sku = device.metadata.get("sku").cloned();
            let mac = device.metadata.get("mac").cloned();
            if let (Some(ip), Some(sku), Some(mac)) = (ip, sku, mac) {
                self.remember_device(GoveeLanDevice {
                    ip,
                    sku,
                    mac,
                    name: device.info.name.clone(),
                    firmware_version: device.info.firmware_version.clone(),
                });
            }
            infos.push(device.info);
        }

        if let Some(client) = self.cloud_client().await? {
            for device in client.list_v1_devices().await? {
                self.remember_cloud_device(device);
            }
            infos.extend(
                self.devices
                    .values()
                    .filter_map(|device| device.try_lock().ok().map(|device| device.info.clone())),
            );
            infos.sort_by_key(|info| info.id.to_string());
            infos.dedup_by_key(|info| info.id);
        }

        Ok(infos)
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        let Some(device) = self.devices.get(id) else {
            return Ok(None);
        };
        Ok(Some(device.lock().await.info.clone()))
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if !self.devices.contains_key(id) {
            bail!("Govee device {id} is not known");
        }

        if self.devices.get(id).is_some_and(|device| {
            device
                .try_lock()
                .is_ok_and(|device| device.address.is_some())
        }) {
            self.send_command(id, LanCommand::Turn { on: true }).await?;
        } else {
            self.send_cloud_command(id, V1Command::Turn(true)).await?;
            return Ok(());
        }
        let should_enable_razer = self.devices.get(id).is_some_and(|device| {
            device.try_lock().is_ok_and(|device| {
                device
                    .profile
                    .capabilities
                    .contains(GoveeCapabilities::RAZER_STREAMING)
                    && device.profile.razer_led_count.is_some()
            })
        });
        if should_enable_razer {
            self.send_command(
                id,
                LanCommand::Razer {
                    pt: encode_razer_mode_base64(true),
                },
            )
            .await?;
            if let Some(device) = self.devices.get(id) {
                let mut device = device.lock().await;
                device.razer_enabled = true;
            }
        }

        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        let Some(device) = self.devices.get(id) else {
            return Ok(());
        };
        let device = device.lock().await;
        let razer_enabled = device.razer_enabled;
        let has_lan_address = device.address.is_some();
        drop(device);
        if razer_enabled {
            self.send_command(
                id,
                LanCommand::Razer {
                    pt: encode_razer_mode_base64(false),
                },
            )
            .await?;
        }
        if self.config.power_off_on_disconnect {
            if has_lan_address {
                self.send_command(id, LanCommand::Turn { on: false })
                    .await?;
            } else {
                self.send_cloud_command(id, V1Command::Turn(false)).await?;
            }
        }
        if let Some(device) = self.devices.get(id) {
            let mut device = device.lock().await;
            device.razer_enabled = false;
            device.last_sent = None;
        }
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let Some(device) = self.devices.get(id) else {
            bail!("Govee device {id} is not connected");
        };
        Self::write_device_colors(
            id,
            device,
            colors,
            &self.config,
            &self.shared_socket,
            self.credential_store.as_ref(),
            self.cloud_client.as_ref(),
            self.cloud_base_url.as_deref(),
        )
        .await
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        if self.devices.get(id).is_none_or(|device| {
            device
                .try_lock()
                .map_or(true, |device| device.address.is_none())
        }) {
            return self
                .send_cloud_command(id, V1Command::Brightness(brightness))
                .await;
        }

        self.send_command(
            id,
            LanCommand::Brightness {
                value: brightness.clamp(1, 100),
            },
        )
        .await
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.devices.get(id).and_then(|device| {
            let device = device.try_lock().ok()?;
            if device.address.is_none() {
                return Some(1);
            }
            Some(
                if device
                    .profile
                    .capabilities
                    .contains(GoveeCapabilities::RAZER_STREAMING)
                {
                    self.config.razer_fps
                } else {
                    self.config.lan_state_fps
                },
            )
        })
    }

    async fn health_check(&self, id: &DeviceId) -> Result<HealthStatus> {
        if self.devices.contains_key(id) {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Unreachable)
        }
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.devices.get(id).map(|device| {
            Arc::new(GoveeFrameSink {
                device_id: *id,
                device: Arc::clone(device),
                config: self.config.clone(),
                shared_socket: Arc::clone(&self.shared_socket),
                credential_store: self.credential_store.clone(),
                cloud_client: self.cloud_client.clone(),
                cloud_base_url: self.cloud_base_url.clone(),
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}

fn mean_color(colors: &[[u8; 3]]) -> [u8; 3] {
    let mut red = 0_u32;
    let mut green = 0_u32;
    let mut blue = 0_u32;
    for [r, g, b] in colors {
        red += u32::from(*r);
        green += u32::from(*g);
        blue += u32::from(*b);
    }
    let count = u32::try_from(colors.len()).unwrap_or(u32::MAX).max(1);
    [
        u8::try_from(red / count).unwrap_or(u8::MAX),
        u8::try_from(green / count).unwrap_or(u8::MAX),
        u8::try_from(blue / count).unwrap_or(u8::MAX),
    ]
}
