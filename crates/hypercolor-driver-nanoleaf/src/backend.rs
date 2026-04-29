//! Nanoleaf backend — External Control streaming over `UDP`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::{BackendInfo, DeviceBackend, DeviceFrameSink};
use hypercolor_types::device::{DeviceId, DeviceInfo};

use super::scanner::{NanoleafKnownDevice, NanoleafScanner, load_auth_token};
use super::streaming::NanoleafStreamSession;
use super::types::{NanoleafDiscoveredDevice, build_device_info, panel_ids_from_layout};
use super::{fetch_device_info, fetch_panel_layout};

const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_secs(60);

/// Nanoleaf backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NanoleafConfig {
    /// Manual device IPs for networks where mDNS discovery is unavailable.
    #[serde(default)]
    pub device_ips: Vec<IpAddr>,

    /// Transition time per frame in deciseconds (100ms units).
    #[serde(default = "default_transition_time")]
    pub transition_time: u16,
}

impl Default for NanoleafConfig {
    fn default() -> Self {
        Self {
            device_ips: Vec::new(),
            transition_time: default_transition_time(),
        }
    }
}

const fn default_transition_time() -> u16 {
    1
}

/// Nanoleaf backend implementing [`DeviceBackend`].
pub struct NanoleafBackend {
    config: NanoleafConfig,
    credential_store: Arc<CredentialStore>,
    mdns_enabled: bool,
    discovered: HashMap<DeviceId, NanoleafDiscoveredDevice>,
    devices: HashMap<DeviceId, Arc<Mutex<NanoleafDeviceState>>>,
}

struct NanoleafDeviceState {
    device_key: String,
    ip: IpAddr,
    api_port: u16,
    stream: NanoleafStreamSession,
    info: DeviceInfo,
    brightness: u8,
    scaled_colors: Vec<[u8; 3]>,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl NanoleafBackend {
    /// Create a new Nanoleaf backend using the configured manual IPs.
    #[must_use]
    pub fn new(config: NanoleafConfig, credential_store: Arc<CredentialStore>) -> Self {
        Self::with_mdns_enabled(config, credential_store, true)
    }

    /// Create a backend with explicit `mDNS` enablement.
    #[must_use]
    pub fn with_mdns_enabled(
        config: NanoleafConfig,
        credential_store: Arc<CredentialStore>,
        mdns_enabled: bool,
    ) -> Self {
        Self {
            config,
            credential_store,
            mdns_enabled,
            discovered: HashMap::new(),
            devices: HashMap::new(),
        }
    }

    /// Seed the backend with a previously discovered device.
    pub fn remember_device(&mut self, device: NanoleafDiscoveredDevice) {
        self.discovered.insert(device.info.id, device);
    }

    fn known_devices(&self) -> Vec<NanoleafKnownDevice> {
        let mut known: HashMap<IpAddr, NanoleafKnownDevice> = self
            .config
            .device_ips
            .iter()
            .copied()
            .map(NanoleafKnownDevice::from_ip)
            .map(|device| (device.ip, device))
            .collect();

        for device in self.discovered.values() {
            known
                .entry(device.ip)
                .and_modify(|existing| {
                    if existing.device_id.is_empty() {
                        existing.device_id.clone_from(&device.device_key);
                    }
                    if existing.port == 0 {
                        existing.port = device.api_port;
                    }
                    if existing.name.is_empty() {
                        existing.name.clone_from(&device.info.name);
                    }
                    if existing.model.is_empty() {
                        existing.model = device.info.model.clone().unwrap_or_default();
                    }
                    if existing.firmware.is_empty() {
                        existing.firmware =
                            device.info.firmware_version.clone().unwrap_or_default();
                    }
                })
                .or_insert_with(|| NanoleafKnownDevice {
                    device_id: device.device_key.clone(),
                    ip: device.ip,
                    port: device.api_port,
                    name: device.info.name.clone(),
                    model: device.info.model.clone().unwrap_or_default(),
                    firmware: device.info.firmware_version.clone().unwrap_or_default(),
                });
        }

        let mut resolved: Vec<_> = known.into_values().collect();
        resolved.sort_by_key(|device| device.ip);
        resolved
    }

    async fn write_device_colors(
        id: &DeviceId,
        device: &Arc<Mutex<NanoleafDeviceState>>,
        colors: &[[u8; 3]],
        transition_time: u16,
    ) -> Result<()> {
        let mut device = device.lock().await;

        let expected_led_count =
            usize::try_from(device.info.total_led_count()).unwrap_or(usize::MAX);
        if colors.len() != expected_led_count {
            let should_warn = device
                .last_size_mismatch_warn_at
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
            if should_warn {
                warn!(
                    device_id = %id,
                    expected_led_count,
                    actual_led_count = colors.len(),
                    "Nanoleaf frame size mismatch; truncating or padding to match panel count"
                );
                device.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        if device.brightness == u8::MAX {
            device.stream.send_frame(colors, transition_time).await?;
            return Ok(());
        }

        let brightness = device.brightness;
        device.scaled_colors.clear();
        device.scaled_colors.reserve(colors.len());
        for [r, g, b] in colors.iter().copied() {
            device.scaled_colors.push([
                scale_channel(r, brightness),
                scale_channel(g, brightness),
                scale_channel(b, brightness),
            ]);
        }

        let scaled_colors = std::mem::take(&mut device.scaled_colors);
        let result = device
            .stream
            .send_frame(scaled_colors.as_slice(), transition_time)
            .await;
        device.scaled_colors = scaled_colors;
        result
    }
}

struct NanoleafFrameSink {
    device_id: DeviceId,
    device: Arc<Mutex<NanoleafDeviceState>>,
    transition_time: u16,
}

#[async_trait::async_trait]
impl DeviceFrameSink for NanoleafFrameSink {
    async fn write_colors_shared(&self, colors: Arc<Vec<[u8; 3]>>) -> Result<()> {
        NanoleafBackend::write_device_colors(
            &self.device_id,
            &self.device,
            colors.as_slice(),
            self.transition_time,
        )
        .await
    }
}

#[async_trait::async_trait]
impl DeviceBackend for NanoleafBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "nanoleaf".to_owned(),
            name: "Nanoleaf".to_owned(),
            description: "Nanoleaf panels via External Control streaming".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        let mut scanner = NanoleafScanner::with_options(
            self.known_devices(),
            Arc::clone(&self.credential_store),
            Duration::from_secs(2),
            self.mdns_enabled,
        );
        let devices = scanner.scan_devices().await?;

        self.discovered = devices
            .iter()
            .cloned()
            .map(|device| (device.info.id, device))
            .collect();

        Ok(devices.into_iter().map(|device| device.info).collect())
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        let Some(device) = self.devices.get(id) else {
            return Ok(None);
        };
        Ok(Some(device.lock().await.info.clone()))
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    #[expect(
        clippy::too_many_lines,
        reason = "connect performs credential lookup, metadata refresh, and stream bootstrap in one linear flow"
    )]
    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if self.devices.contains_key(id) {
            return Ok(());
        }

        let known_ids = self
            .discovered
            .keys()
            .take(4)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let Some(discovered) = self.discovered.get(id).cloned() else {
            bail!(
                "Nanoleaf device {id} is not known; cache_size={}, sample_ids=[{}]. discover() likely returned different IDs",
                self.discovered.len(),
                known_ids
            );
        };

        let auth_token = load_auth_token(
            &self.credential_store,
            &discovered.device_key,
            discovered.ip,
        )
        .await
        .with_context(|| {
            format!(
                "Nanoleaf device {} at {} requires pairing credentials",
                discovered.info.name, discovered.ip
            )
        })?;

        let device_info = fetch_device_info(discovered.ip, discovered.api_port, &auth_token)
            .await
            .with_context(|| {
                format!(
                    "failed to fetch Nanoleaf device info for {} ({})",
                    discovered.info.name, discovered.ip
                )
            })?;
        let layout = fetch_panel_layout(discovered.ip, discovered.api_port, &auth_token)
            .await
            .with_context(|| {
                format!(
                    "failed to fetch Nanoleaf panel layout for {} ({})",
                    discovered.info.name, discovered.ip
                )
            })?;
        let panel_ids = panel_ids_from_layout(layout.position_data.as_slice());
        if panel_ids.is_empty() {
            bail!(
                "Nanoleaf device {} exposes no addressable panels",
                discovered.info.name
            );
        }

        let stream = NanoleafStreamSession::connect(
            discovered.ip,
            discovered.api_port,
            &auth_token,
            panel_ids.clone(),
        )
        .await
        .with_context(|| {
            format!(
                "failed to open Nanoleaf stream to {} ({})",
                discovered.info.name, discovered.ip
            )
        })?;

        let info = build_device_info(
            &discovered.device_key,
            &device_info.name,
            Some(device_info.model.as_str()),
            Some(device_info.firmware_version.as_str()),
            layout.position_data.as_slice(),
        );

        self.discovered.insert(
            *id,
            NanoleafDiscoveredDevice {
                device_key: discovered.device_key.clone(),
                ip: discovered.ip,
                api_port: discovered.api_port,
                info: info.clone(),
                panel_ids,
                connect_behavior: discovered.connect_behavior,
                metadata: discovered.metadata,
            },
        );

        let device = NanoleafDeviceState {
            device_key: discovered.device_key.clone(),
            ip: discovered.ip,
            api_port: discovered.api_port,
            stream,
            info,
            brightness: u8::MAX,
            scaled_colors: Vec::new(),
            last_size_mismatch_warn_at: None,
        };
        self.devices.insert(*id, Arc::new(Mutex::new(device)));

        let panels = self
            .devices
            .get(id)
            .and_then(|device| device.try_lock().ok())
            .map_or(0, |device| device.info.total_led_count());
        info!(
            device_id = %id,
            ip = %discovered.ip,
            panels,
            "Connected to Nanoleaf device"
        );
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if let Some(device) = self.devices.remove(id) {
            let device = device.lock().await;
            info!(
                device_id = %id,
                ip = %device.ip,
                api_port = device.api_port,
                device_key = %device.device_key,
                "Disconnected from Nanoleaf device"
            );
            Ok(())
        } else {
            bail!("Nanoleaf device {id} is not connected")
        }
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let device = self
            .devices
            .get(id)
            .with_context(|| format!("Nanoleaf device {id} is not connected"))?;
        Self::write_device_colors(id, device, colors, self.config.transition_time).await
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        let device = self
            .devices
            .get(id)
            .with_context(|| format!("Nanoleaf device {id} is not connected"))?;
        device.lock().await.brightness = brightness;
        Ok(())
    }

    fn target_fps(&self, _id: &DeviceId) -> Option<u32> {
        Some(10)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.devices.get(id).map(|device| {
            Arc::new(NanoleafFrameSink {
                device_id: *id,
                device: Arc::clone(device),
                transition_time: self.config.transition_time,
            }) as Arc<dyn DeviceFrameSink>
        })
    }
}

fn scale_channel(channel: u8, brightness: u8) -> u8 {
    let scaled = u16::from(channel) * u16::from(brightness);
    u8::try_from(scaled / 255).unwrap_or(u8::MAX)
}
