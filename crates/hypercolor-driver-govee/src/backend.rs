use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use hypercolor_core::device::{BackendInfo, DeviceBackend, HealthStatus, TransportScanner};
use hypercolor_types::config::GoveeConfig;
use hypercolor_types::device::{DeviceId, DeviceInfo};
use tokio::net::UdpSocket;

use crate::capabilities::{GoveeCapabilities, SkuProfile, fallback_profile, profile_for_sku};
use crate::lan::discovery::{GoveeKnownDevice, GoveeLanDevice, GoveeLanScanner, build_device_info};
use crate::lan::protocol::{DEVICE_PORT, LanCommand, encode_command};
use crate::lan::razer::{encode_razer_frame_base64, encode_razer_mode_base64};

pub struct GoveeBackend {
    config: GoveeConfig,
    devices: HashMap<DeviceId, GoveeDeviceState>,
    shared_socket: Option<Arc<UdpSocket>>,
}

#[derive(Clone)]
struct GoveeDeviceState {
    info: DeviceInfo,
    profile: SkuProfile,
    address: SocketAddr,
    last_sent: Option<Vec<[u8; 3]>>,
    razer_enabled: bool,
}

impl GoveeBackend {
    #[must_use]
    pub fn new(config: GoveeConfig) -> Self {
        Self {
            config,
            devices: HashMap::new(),
            shared_socket: None,
        }
    }

    pub fn remember_device(&mut self, device: GoveeLanDevice) {
        let info = build_device_info(&device);
        let profile = profile_for_sku(&device.sku)
            .cloned()
            .unwrap_or_else(|| fallback_profile(&device.sku));
        self.devices.insert(
            info.id,
            GoveeDeviceState {
                info,
                profile,
                address: SocketAddr::new(device.ip, DEVICE_PORT),
                last_sent: None,
                razer_enabled: false,
            },
        );
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
            .map(|device| device.address)
            .with_context(|| format!("Govee device {id} is not known"))?;
        let payload = encode_command(&command)?;
        let socket = self.ensure_socket().await?;
        socket
            .send_to(&payload, address)
            .await
            .with_context(|| format!("failed to send Govee LAN command to {address}"))?;
        Ok(())
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

        Ok(infos)
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        Ok(self.devices.get(id).map(|device| device.info.clone()))
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if !self.devices.contains_key(id) {
            bail!("Govee device {id} is not known");
        }

        self.send_command(id, LanCommand::Turn { on: true }).await?;
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
            self.send_command(id, LanCommand::Turn { on: false })
                .await?;
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
            self.send_command(id, LanCommand::Razer { pt }).await?;
            if let Some(device) = self.devices.get_mut(id) {
                device.last_sent = Some(colors.to_vec());
            }
            return Ok(());
        }

        let [red, green, blue] = mean_color(colors);
        self.send_command(id, LanCommand::ColorWc { red, green, blue })
            .await?;
        if let Some(device) = self.devices.get_mut(id) {
            device.last_sent = Some(vec![[red, green, blue]]);
        }
        Ok(())
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
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
