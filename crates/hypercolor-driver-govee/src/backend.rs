use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use hypercolor_core::device::net::{CredentialStore, Credentials};
use hypercolor_core::device::{BackendInfo, DeviceBackend, HealthStatus, TransportScanner};
use hypercolor_types::config::GoveeConfig;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use tokio::net::UdpSocket;

use crate::capabilities::{GoveeCapabilities, SkuProfile, fallback_profile, profile_for_sku};
use crate::cloud::{CloudClient, V1Command, V1Device};
use crate::lan::discovery::{GoveeKnownDevice, GoveeLanDevice, GoveeLanScanner, build_device_info};
use crate::lan::protocol::{DEVICE_PORT, LanCommand, encode_command};
use crate::lan::razer::{encode_razer_frame_base64, encode_razer_mode_base64};

pub struct GoveeBackend {
    config: GoveeConfig,
    devices: HashMap<DeviceId, GoveeDeviceState>,
    shared_socket: Option<Arc<UdpSocket>>,
    credential_store: Option<Arc<CredentialStore>>,
    cloud_base_url: Option<String>,
    cloud_client: Option<CloudClient>,
}

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
            shared_socket: None,
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
                state.info = info.clone();
                state.profile = profile.clone();
                state.address = Some(address);
            })
            .or_insert(GoveeDeviceState {
                info,
                profile,
                address: Some(address),
                cloud_id: None,
                last_sent: None,
                last_write_at: None,
                razer_enabled: false,
            });
    }

    pub fn remember_cloud_device(&mut self, device: V1Device) {
        let discovered = crate::build_cloud_discovered_device(device.clone());
        let profile =
            profile_for_sku(&device.model).unwrap_or_else(|| fallback_profile(&device.model));
        self.devices
            .entry(discovered.info.id)
            .and_modify(|state| {
                state.cloud_id = Some(device.device.clone());
            })
            .or_insert(GoveeDeviceState {
                info: discovered.info,
                profile,
                address: None,
                cloud_id: Some(device.device),
                last_sent: None,
                last_write_at: None,
                razer_enabled: false,
            });
    }

    async fn ensure_socket(&mut self) -> Result<Arc<UdpSocket>> {
        if let Some(socket) = &self.shared_socket {
            return Ok(Arc::clone(socket));
        }

        let socket = Arc::new(
            UdpSocket::bind(("0.0.0.0", 0))
                .await
                .context("failed to bind Govee LAN command socket")?,
        );
        self.shared_socket = Some(Arc::clone(&socket));
        Ok(socket)
    }

    async fn send_command(&mut self, id: &DeviceId, command: LanCommand) -> Result<()> {
        let address = self
            .devices
            .get(id)
            .and_then(|device| device.address)
            .with_context(|| format!("Govee device {id} has no LAN address"))?;
        let payload = encode_command(&command)?;
        let socket = self.ensure_socket().await?;
        socket
            .send_to(&payload, address)
            .await
            .with_context(|| format!("failed to send Govee LAN command to {address}"))?;
        Ok(())
    }

    fn frame_interval(&self, device: &GoveeDeviceState) -> Duration {
        if device.address.is_none() {
            return Duration::from_secs(6);
        }
        let fps = if device
            .profile
            .capabilities
            .contains(GoveeCapabilities::RAZER_STREAMING)
        {
            self.config.razer_fps
        } else {
            self.config.lan_state_fps
        }
        .max(1);
        Duration::from_millis(1000 / u64::from(fps))
    }

    async fn cloud_client(&self) -> Result<Option<CloudClient>> {
        if let Some(client) = &self.cloud_client {
            return Ok(Some(client.clone()));
        }

        let Some(store) = &self.credential_store else {
            return Ok(None);
        };
        let Some(Credentials::Govee { api_key }) = store.get("govee:account").await else {
            return Ok(None);
        };

        match &self.cloud_base_url {
            Some(base_url) => CloudClient::with_base_url(api_key, base_url).map(Some),
            None => CloudClient::new(api_key).map(Some),
        }
    }

    async fn send_cloud_command(&self, id: &DeviceId, command: V1Command) -> Result<()> {
        let device = self
            .devices
            .get(id)
            .with_context(|| format!("Govee device {id} is not known"))?;
        let cloud_id = device
            .cloud_id
            .as_deref()
            .with_context(|| format!("Govee device {id} is not cloud-backed"))?;
        let client = self
            .cloud_client()
            .await?
            .context("Govee cloud credentials are not configured")?;
        let model = device.info.model.as_deref().unwrap_or_default();
        client.v1_control(model, cloud_id, command).await
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
            infos.extend(self.devices.values().map(|device| device.info.clone()));
            infos.sort_by_key(|info| info.id.to_string());
            infos.dedup_by_key(|info| info.id);
        }

        Ok(infos)
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        Ok(self.devices.get(id).map(|device| device.info.clone()))
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if !self.devices.contains_key(id) {
            bail!("Govee device {id} is not known");
        }

        if self
            .devices
            .get(id)
            .and_then(|device| device.address)
            .is_some()
        {
            self.send_command(id, LanCommand::Turn { on: true }).await?;
        } else {
            self.send_cloud_command(id, V1Command::Turn(true)).await?;
            return Ok(());
        }
        let should_enable_razer = self.devices.get(id).is_some_and(|device| {
            device
                .profile
                .capabilities
                .contains(GoveeCapabilities::RAZER_STREAMING)
                && device.profile.razer_led_count.is_some()
        });
        if should_enable_razer {
            self.send_command(
                id,
                LanCommand::Razer {
                    pt: encode_razer_mode_base64(true),
                },
            )
            .await?;
            if let Some(device) = self.devices.get_mut(id) {
                device.razer_enabled = true;
            }
        }

        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        let Some(device) = self.devices.get(id) else {
            return Ok(());
        };
        let razer_enabled = device.razer_enabled;
        let has_lan_address = device.address.is_some();
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
        if let Some(device) = self.devices.get_mut(id) {
            device.razer_enabled = false;
            device.last_sent = None;
        }
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        if colors.is_empty() {
            return Ok(());
        }
        let (command, sent_frame) = {
            let Some(device) = self.devices.get(id) else {
                bail!("Govee device {id} is not connected");
            };
            if device
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
            }
        };

        let Some(device) = self.devices.get(id) else {
            bail!("Govee device {id} is not connected");
        };
        if device.last_sent.as_deref() == Some(sent_frame.as_slice()) {
            return Ok(());
        }
        if device
            .last_write_at
            .is_some_and(|last_write| last_write.elapsed() < self.frame_interval(device))
        {
            return Ok(());
        }

        match command {
            LanCommand::Razer { pt } => {
                self.send_command(id, LanCommand::Razer { pt }).await?;
            }
            LanCommand::ColorWc { red, green, blue } => {
                if self
                    .devices
                    .get(id)
                    .and_then(|device| device.address)
                    .is_some()
                {
                    self.send_command(id, LanCommand::ColorWc { red, green, blue })
                        .await?;
                } else {
                    self.send_cloud_command(
                        id,
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

        if let Some(device) = self.devices.get_mut(id) {
            device.last_sent = Some(sent_frame);
            device.last_write_at = Some(Instant::now());
        }
        Ok(())
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        if self
            .devices
            .get(id)
            .and_then(|device| device.address)
            .is_none()
        {
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
        self.devices.get(id).map(|device| {
            if device.address.is_none() {
                return 1;
            }
            if device
                .profile
                .capabilities
                .contains(GoveeCapabilities::RAZER_STREAMING)
            {
                self.config.razer_fps
            } else {
                self.config.lan_state_fps
            }
        })
    }

    async fn health_check(&self, id: &DeviceId) -> Result<HealthStatus> {
        if self.devices.contains_key(id) {
            Ok(HealthStatus::Healthy)
        } else {
            Ok(HealthStatus::Unreachable)
        }
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
