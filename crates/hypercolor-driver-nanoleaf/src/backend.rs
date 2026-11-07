//! Nanoleaf backend — External Control streaming over `UDP`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use hypercolor_color::Rgb;
use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceFrameSink, DeviceWriteOutcome, DiscoveredDevice,
};
use hypercolor_driver_support::{
    CredentialStore, DeviceSession, FrameDelivery, Session, SessionTable,
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
    sessions: SessionTable<NanoleafSession>,
}

/// One open External Control stream.
struct NanoleafSession {
    device_id: DeviceId,
    stream: NanoleafStreamSession,
    led_count: usize,
    transition_time: u16,
    brightness: u8,
    scaled_colors: Vec<[u8; 3]>,
    last_size_mismatch_warn_at: Option<Instant>,
}

impl DeviceSession for NanoleafSession {
    type Facts = DeviceInfo;

    async fn write_frame(
        &mut self,
        colors: &[[u8; 3]],
        delivery: Option<&FrameDelivery>,
    ) -> Result<DeviceWriteOutcome, DeviceError> {
        if colors.len() != self.led_count {
            let should_warn = self
                .last_size_mismatch_warn_at
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);
            if should_warn {
                warn!(
                    device_id = %self.device_id,
                    expected_led_count = self.led_count,
                    actual_led_count = colors.len(),
                    "Nanoleaf frame size mismatch; truncating or padding to match panel count"
                );
                self.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let frame = if self.brightness == u8::MAX {
            colors
        } else {
            let brightness = f32::from(self.brightness) / f32::from(u8::MAX);
            self.scaled_colors.clear();
            self.scaled_colors.reserve(colors.len());
            for [r, g, b] in colors.iter().copied() {
                let scaled = Rgb::new(r, g, b).scale(brightness);
                self.scaled_colors.push([scaled.r, scaled.g, scaled.b]);
            }
            self.scaled_colors.as_slice()
        };
        if let Some(delivery) = delivery {
            delivery.transport_started();
        }
        self.stream
            .send_frame(frame, self.transition_time)
            .await
            .map_err(|error| DeviceError::write(self.device_id, error))?;
        Ok(DeviceWriteOutcome::Sent)
    }
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
            sessions: SessionTable::new(),
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

    async fn connected_device_info(
        &self,
        id: &DeviceId,
    ) -> std::result::Result<Option<DeviceInfo>, DeviceError> {
        Ok(self.sessions.facts(id).map(|info| info.as_ref().clone()))
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    #[expect(
        clippy::too_many_lines,
        reason = "connect performs credential lookup, metadata refresh, and stream bootstrap in one linear flow"
    )]
    async fn connect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let Some(ticket) = self.sessions.begin_open(id).await? else {
            return Ok(());
        };

        let discovered = self
            .discovered
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .ok_or(DeviceError::NotAdopted { device_id: *id })?;

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
        })
        .map_err(|error| DeviceError::connection(id, error))?;

        let device_info = fetch_device_info(discovered.ip, discovered.api_port, &auth_token)
            .await
            .with_context(|| {
                format!(
                    "failed to fetch Nanoleaf device info for {} ({})",
                    discovered.info.name, discovered.ip
                )
            })
            .map_err(|error| DeviceError::connection(id, error))?;
        let layout = fetch_panel_layout(discovered.ip, discovered.api_port, &auth_token)
            .await
            .with_context(|| {
                format!(
                    "failed to fetch Nanoleaf panel layout for {} ({})",
                    discovered.info.name, discovered.ip
                )
            })
            .map_err(|error| DeviceError::connection(id, error))?;
        let panel_ids = panel_ids_from_layout(layout.position_data.as_slice());
        if panel_ids.is_empty() {
            return Err(DeviceError::connection(
                id,
                format!(
                    "Nanoleaf device {} exposes no addressable panels",
                    discovered.info.name
                ),
            ));
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
        })
        .map_err(|error| DeviceError::connection(id, error))?;

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

        let led_count = usize::try_from(info.total_led_count()).unwrap_or(usize::MAX);
        let panels = info.total_led_count();
        ticket
            .install(Session::new(
                *id,
                info,
                NanoleafSession {
                    device_id: *id,
                    stream,
                    led_count,
                    transition_time: self.config.transition_time,
                    brightness: u8::MAX,
                    scaled_colors: Vec::new(),
                    last_size_mismatch_warn_at: None,
                },
            ))
            .await?;

        info!(
            device_id = %id,
            ip = %discovered.ip,
            panels,
            "Connected to Nanoleaf device"
        );
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        self.sessions.close(id).await?;
        info!(device_id = %id, "Disconnected from Nanoleaf device");
        Ok(())
    }

    async fn retire(&self) -> std::result::Result<(), DeviceError> {
        self.sessions.retire().await
    }

    async fn write_colors(
        &self,
        id: &DeviceId,
        colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        self.sessions.require(id)?.write(colors).await.map(|_| ())
    }

    async fn set_brightness(
        &self,
        id: &DeviceId,
        brightness: u8,
    ) -> std::result::Result<(), DeviceError> {
        self.sessions.require(id)?.admit().await?.brightness = brightness;
        Ok(())
    }

    fn target_fps(&self, _id: &DeviceId) -> Option<u32> {
        Some(10)
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.sessions.frame_sink(id)
    }
}
