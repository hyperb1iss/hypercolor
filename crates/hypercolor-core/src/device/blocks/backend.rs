//! `BlocksBackend` — `DeviceBackend` implementation for ROLI Blocks via blocksd.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::device::traits::{BackendInfo, DeviceBackend};
use crate::types::device::{
    BLOCKS_OUTPUT_BACKEND_ID, ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint,
    SegmentInfo,
};

use super::connection::{self, BlocksConnection};
use super::types::{BlocksDeviceResponse, RoliBlockType};

/// Device backend that bridges to blocksd for ROLI Blocks hardware.
pub struct BlocksBackend {
    /// Socket path for blocksd connection.
    socket_path: PathBuf,
    /// Active connection (None if disconnected).
    state: Mutex<BlocksBackendState>,
}

struct BlocksBackendState {
    connection: Option<BlocksConnection>,
    /// Known devices reported by blocksd.
    devices: HashMap<DeviceId, BlocksDevice>,
    /// UID-to-`DeviceId` mapping for event routing.
    uid_map: HashMap<u64, DeviceId>,
    /// Per-device brightness (applied by blocksd).
    brightness: HashMap<DeviceId, u8>,
    /// Reconnection state.
    reconnect_state: ReconnectState,
}

struct BlocksDevice {
    uid: u64,
    info: DeviceInfo,
    connected: bool,
    frames_sent: u64,
}

struct ReconnectState {
    last_attempt: Option<Instant>,
    delay: Duration,
    #[allow(dead_code)]
    consecutive_failures: u32,
}

impl BlocksBackend {
    /// Create a new backend with the given socket path.
    #[must_use]
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            state: Mutex::new(BlocksBackendState {
                connection: None,
                devices: HashMap::new(),
                uid_map: HashMap::new(),
                brightness: HashMap::new(),
                reconnect_state: ReconnectState {
                    last_attempt: None,
                    delay: Duration::from_millis(500),
                    consecutive_failures: 0,
                },
            }),
        }
    }

    /// Default socket path from environment.
    #[must_use]
    pub fn default_socket_path() -> PathBuf {
        connection::default_socket_path()
    }
}

impl BlocksBackendState {
    /// Connect to blocksd if not already connected.
    async fn ensure_connected(
        &mut self,
        socket_path: &std::path::Path,
    ) -> Result<&mut BlocksConnection> {
        if let Some(ref mut conn) = self.connection {
            return Ok(conn);
        }

        // Respect backoff timing
        if self
            .reconnect_state
            .last_attempt
            .is_some_and(|last| last.elapsed() < self.reconnect_state.delay)
        {
            bail!("reconnect backoff active");
        }

        self.reconnect_state.last_attempt = Some(Instant::now());

        let connect_attempt = async {
            let mut conn = BlocksConnection::connect(socket_path).await?;
            let pong = conn.ping().await?;
            Ok::<_, anyhow::Error>((conn, pong))
        }
        .await;

        match connect_attempt {
            Ok((conn, pong)) => {
                info!(
                    version = %pong.version,
                    devices = pong.device_count,
                    "blocksd connected"
                );
                self.reconnect_state.delay = Duration::from_millis(500);
                self.reconnect_state.consecutive_failures = 0;
                self.connection = Some(conn);
                Ok(self.connection.as_mut().expect("just set"))
            }
            Err(e) => {
                self.reconnect_state.consecutive_failures += 1;
                self.reconnect_state.delay =
                    (self.reconnect_state.delay * 2).min(Duration::from_secs(5));
                Err(e)
            }
        }
    }

    /// Mark connection as dead and trigger reconnect on next call.
    fn handle_disconnect(&mut self) {
        let device_count = self.devices.len();
        self.connection = None;

        for device in self.devices.values_mut() {
            device.connected = false;
        }

        if device_count > 0 {
            info!(device_count, "blocksd disconnected, devices lost");
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for BlocksBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: BLOCKS_OUTPUT_BACKEND_ID.to_owned(),
            name: "ROLI Blocks (blocksd)".to_owned(),
            description: "ROLI Lightpad, LUMI Keys, and Seaboard via blocksd daemon".to_owned(),
        }
    }

    async fn discover(&self) -> Result<Vec<DeviceInfo>> {
        // If socket doesn't exist, blocksd isn't running — not an error
        if !connection::socket_exists(&self.socket_path) {
            return Ok(vec![]);
        }

        let mut state = self.state.lock().await;
        let Ok(conn) = state.ensure_connected(&self.socket_path).await else {
            return Ok(vec![]);
        };

        let response = match conn.discover().await {
            Ok(r) => r,
            Err(e) => {
                debug!("blocksd discover failed: {e}");
                state.handle_disconnect();
                return Ok(vec![]);
            }
        };

        state.devices.clear();
        state.uid_map.clear();

        let mut infos = Vec::with_capacity(response.devices.len());
        for dev in &response.devices {
            let info = device_info_from_blocks(dev);
            let device_id = info.id;

            state.uid_map.insert(dev.uid, device_id);
            state.devices.insert(
                device_id,
                BlocksDevice {
                    uid: dev.uid,
                    info: info.clone(),
                    connected: false,
                    frames_sent: 0,
                },
            );

            infos.push(info);
        }

        Ok(infos)
    }

    async fn connected_device_info(&self, id: &DeviceId) -> Result<Option<DeviceInfo>> {
        Ok(self
            .state
            .lock()
            .await
            .devices
            .get(id)
            .map(|d| d.info.clone()))
    }

    async fn connect(&self, id: &DeviceId) -> Result<()> {
        let mut state = self.state.lock().await;
        let device = state
            .devices
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("unknown blocks device: {id}"))?;
        device.connected = true;
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> Result<()> {
        if let Some(device) = self.state.lock().await.devices.get_mut(id) {
            device.connected = false;
        }
        Ok(())
    }

    async fn write_colors(&self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let mut state = self.state.lock().await;
        let device = state
            .devices
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown blocks device: {id}"))?;

        if !device.connected {
            bail!("device not connected: {id}");
        }

        let uid = device.uid;

        let conn = state
            .connection
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("blocksd not connected"))?;

        match conn.write_frame_binary(uid, colors).await {
            Ok(accepted) => {
                if accepted {
                    if let Some(device) = state.devices.get_mut(id) {
                        device.frames_sent += 1;
                    }
                } else {
                    debug!(%id, uid, "blocks frame deferred by daemon");
                }

                Ok(())
            }
            Err(e) => {
                state.handle_disconnect();
                Err(e)
            }
        }
    }

    async fn set_brightness(&self, id: &DeviceId, brightness: u8) -> Result<()> {
        let mut state = self.state.lock().await;
        let device = state
            .devices
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown blocks device: {id}"))?;

        let uid = device.uid;

        let conn = state
            .connection
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("blocksd not connected"))?;

        match conn.set_brightness(uid, brightness).await {
            Ok(()) => {
                state.brightness.insert(*id, brightness);
                Ok(())
            }
            Err(e) => {
                state.handle_disconnect();
                Err(e)
            }
        }
    }

    fn target_fps(&self, _id: &DeviceId) -> Option<u32> {
        Some(25)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Construct a `DeviceInfo` from blocksd's device response.
fn device_info_from_blocks(dev: &BlocksDeviceResponse) -> DeviceInfo {
    let block_type = RoliBlockType::from_api(&dev.block_type);
    let serial_short = if dev.serial.len() >= 6 {
        &dev.serial[..6]
    } else {
        &dev.serial
    };

    let fingerprint = DeviceFingerprint(format!("bridge:blocksd:{}", dev.uid));
    let device_id = fingerprint.stable_device_id();

    let rows = dev.grid_height;
    let cols = dev.grid_width;
    let led_count = rows * cols;

    DeviceInfo {
        id: device_id,
        name: format!("{} ({serial_short})", block_type.display_name()),
        vendor: "ROLI".to_owned(),
        family: DeviceFamily::new_static("roli", "ROLI"),
        model: Some(block_type.display_name().to_owned()),
        connection_type: ConnectionType::Bridge,
        origin: DeviceOrigin::native("roli", BLOCKS_OUTPUT_BACKEND_ID, ConnectionType::Bridge),
        segments: vec![SegmentInfo {
            name: "Grid".to_owned(),
            led_count,
            topology: DeviceTopologyHint::Matrix { rows, cols },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: dev.firmware_version.clone(),
        capabilities: DeviceCapabilities {
            led_count,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 25,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}
