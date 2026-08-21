//! Nanoleaf backend — External Control streaming over `UDP`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::CredentialStore;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceWriteOutcome, DiscoveredDevice,
};
use hypercolor_types::device::{DeviceError, DeviceId, DeviceInfo};

use super::scanner::load_auth_token;
use super::streaming::{DEFAULT_NANOLEAF_STREAM_PORT, NanoleafStreamSession};
use super::types::{NanoleafDiscoveredDevice, build_device_info, panel_ids_from_layout};
use super::{fetch_device_info, fetch_panel_layout};

const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);

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
    stream_port: u16,
    discovered: StdRwLock<HashMap<DeviceId, NanoleafDiscoveredDevice>>,
    devices: StdRwLock<HashMap<DeviceId, Arc<Mutex<NanoleafDeviceState>>>>,
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
        Self {
            config,
            credential_store,
            stream_port: DEFAULT_NANOLEAF_STREAM_PORT,
            discovered: StdRwLock::new(HashMap::new()),
            devices: StdRwLock::new(HashMap::new()),
        }
    }

    /// Override the `UDP` port this backend streams external control to.
    ///
    /// Hardware always listens on [`DEFAULT_NANOLEAF_STREAM_PORT`]. Tests
    /// override it so their receiver can take an ephemeral port: binding the
    /// fixed one fails with `WSAEACCES` on Windows whenever it lands inside a
    /// range the OS has reserved for dynamic allocation.
    #[must_use]
    pub fn with_stream_port(mut self, stream_port: u16) -> Self {
        self.stream_port = stream_port;
        self
    }

    async fn write_device_colors(
        id: &DeviceId,
        device: &Arc<Mutex<NanoleafDeviceState>>,
        colors: &[[u8; 3]],
        transition_time: u16,
        delivery: Option<(DeviceDeliveryId, &dyn DeviceDeliveryObserver)>,
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
            if let Some((id, observer)) = delivery {
                observer.transport_started(id);
            }
            device.stream.send_frame(colors, transition_time).await?;
            return Ok(());
        }

        let brightness = f32::from(device.brightness) / f32::from(u8::MAX);
        device.scaled_colors.clear();
        device.scaled_colors.reserve(colors.len());
        for [r, g, b] in colors.iter().copied() {
            let scaled = Rgb::new(r, g, b).scale(brightness);
            device.scaled_colors.push([scaled.r, scaled.g, scaled.b]);
        }

        let scaled_colors = std::mem::take(&mut device.scaled_colors);
        if let Some((id, observer)) = delivery {
            observer.transport_started(id);
        }
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
            None,
        )
        .await
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = NanoleafBackend::write_device_colors(
            &self.device_id,
            &self.device,
            colors.as_slice(),
            self.transition_time,
            Some((id, observer.as_ref())),
        )
        .await
        .map(|()| DeviceWriteOutcome::Sent);
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
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

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let ip = discovered
            .metadata
            .get("ip")
            .and_then(|value| value.parse::<IpAddr>().ok())
            .ok_or(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        let api_port = discovered
            .metadata
            .get("api_port")
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            })?;
        let device_key =
            discovered
                .metadata
                .get("device_key")
                .cloned()
                .ok_or(DeviceError::NotAdopted {
                    device_id: discovered.info.id,
                })?;
        self.discovered
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                discovered.info.id,
                NanoleafDiscoveredDevice {
                    device_key,
                    ip,
                    api_port,
                    info: discovered.info.clone(),
                    panel_ids: Vec::new(),
                    connect_behavior: discovered.connect_behavior,
                    metadata: discovered.metadata.clone(),
                    claim: discovered.claim.clone(),
                },
            );
        Ok(())
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(device) = device else {
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
    async fn connect(&self, id: &DeviceId) -> Result<()> {
        if self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(id)
        {
            return Ok(());
        }

        let discovered = {
            let discovered = self
                .discovered
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            let known_ids = discovered
                .keys()
                .take(4)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            discovered.get(id).cloned().with_context(|| {
                format!(
                    "Nanoleaf device {id} is not known; cache_size={}, sample_ids=[{}]. discover() likely returned different IDs",
                    discovered.len(), known_ids
                )
            })?
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

        let stream = NanoleafStreamSession::connect_with_udp_port(
            discovered.ip,
            discovered.api_port,
            self.stream_port,
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

        self.discovered
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                *id,
                NanoleafDiscoveredDevice {
                    device_key: discovered.device_key.clone(),
                    ip: discovered.ip,
                    api_port: discovered.api_port,
                    info: info.clone(),
                    panel_ids,
                    connect_behavior: discovered.connect_behavior,
                    metadata: discovered.metadata,
                    claim: discovered.claim,
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
        let device = Arc::new(Mutex::new(device));
        let panels = device
            .try_lock()
            .ok()
            .map_or(0, |device| device.info.total_led_count());
        self.devices
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, device);

        info!(
            device_id = %id,
            ip = %discovered.ip,
            panels,
            "Connected to Nanoleaf device"
        );
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<()> {
        let device = self
            .devices
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        if let Some(device) = device {
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

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .with_context(|| format!("Nanoleaf device {id} is not connected"))?;
        Self::write_device_colors(id, &device, colors, self.config.transition_time, None).await
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<()> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .with_context(|| format!("Nanoleaf device {id} is not connected"))?;
        device.lock().await.brightness = brightness;
        Ok(())
    }

    fn target_fps(&self, _id: &DeviceId) -> Option<u32> {
        Some(10)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .map(|device| {
                Arc::new(NanoleafFrameSink {
                    device_id: *id,
                    device: Arc::clone(device),
                    transition_time: self.config.transition_time,
                }) as Arc<dyn DeviceFrameSink>
            })
    }
}
