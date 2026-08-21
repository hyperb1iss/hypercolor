//! WLED backend — implements [`DeviceBackend`] for WLED LED controllers over UDP.
//!
//! This module is split across a few focused submodules:
//!
//! - [`protocol`] holds the wire-format send state machine ([`WledDevice`])
//!   plus per-protocol dedup, encoding, and `/json/cfg` realtime-receiver
//!   validation helpers.
//! - [`cache`] holds metadata parsed from `/json/info` and `/json/state`,
//!   fingerprinting, and translation into the generic [`DeviceInfo`].
//! - [`health`] holds the realtime-mode HTTP lifecycle (enter/exit/prime/
//!   clear/validate) and a cheap reachability probe.
//!
//! [`WledBackend`] stitches the three together and implements
//! [`DeviceBackend`].

mod cache;
mod health;
mod protocol;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, OnceCell};
use tracing::{debug, info, warn};

use hypercolor_driver_api::{
    BackendInfo, DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceFrameSink, DeviceWriteOutcome, DiscoveredDevice, OutputCadence,
};
use hypercolor_types::device::{DeviceColorFormat, DeviceError, DeviceId, DeviceInfo};

use health::{
    clear_device, enter_realtime_mode, exit_realtime_mode, prime_device,
    validate_wled_receiver_config,
};
use protocol::{KEEPALIVE_INTERVAL, encode_colors};

use super::ddp::{DDP_PORT, DdpSequence};
use super::e131::{E131_CHANNELS_PER_UNIVERSE, E131_PORT, E131SequenceTracker};

// ── Re-exports: preserve `backend::Foo` public paths ────────────────────

pub use cache::{WledDeviceInfo, WledSegmentInfo, parse_wled_info, parse_wled_segments};
pub use protocol::{
    WledColorFormat, WledDevice, WledLiveReceiverConfig, WledProtocol,
    parse_wled_live_receiver_config, wled_receiver_config_mismatches,
};

const DEFAULT_DEDUP_THRESHOLD: u8 = 2;
const SIZE_MISMATCH_WARN_INTERVAL: Duration = Duration::from_mins(1);

// ── WledBackend ─────────────────────────────────────────────────────────

/// WLED device backend implementing [`DeviceBackend`].
///
/// Manages adopted devices and per-device UDP streaming over DDP or E1.31.
pub struct WledBackend {
    /// Connected devices, keyed by `DeviceId`.
    devices: StdRwLock<HashMap<DeviceId, Arc<Mutex<WledDevice>>>>,

    /// Maps `DeviceId` to IP for lookup during connect.
    device_ips: StdRwLock<HashMap<DeviceId, IpAddr>>,

    /// Maps `DeviceId` to parsed info for lazy connect.
    device_infos: StdRwLock<HashMap<DeviceId, WledDeviceInfo>>,

    /// Default protocol for new connections.
    default_protocol: WledProtocol,

    /// Shared UDP socket used by all connected WLED devices.
    shared_socket: OnceCell<Arc<UdpSocket>>,

    /// Whether connect/disconnect should manage WLED realtime mode over HTTP.
    realtime_http_enabled: bool,

    /// Global fuzzy deduplication threshold for connected devices.
    dedup_threshold: u8,

    /// E1.31 sender CID (stable UUID per backend instance).
    e131_cid: uuid::Uuid,

    /// Next available E1.31 universe number for auto-allocation.
    next_e131_universe: StdMutex<u16>,
}

impl WledBackend {
    /// Create a new WLED backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            devices: StdRwLock::new(HashMap::new()),
            device_ips: StdRwLock::new(HashMap::new()),
            device_infos: StdRwLock::new(HashMap::new()),
            default_protocol: WledProtocol::default(),
            shared_socket: OnceCell::new(),
            realtime_http_enabled: true,
            dedup_threshold: DEFAULT_DEDUP_THRESHOLD,
            e131_cid: uuid::Uuid::now_v7(),
            next_e131_universe: StdMutex::new(1),
        }
    }

    /// Set the default protocol for new connections.
    pub fn set_protocol(&mut self, protocol: WledProtocol) {
        self.default_protocol = protocol;
    }

    /// Enable or disable HTTP realtime-mode lifecycle calls.
    pub fn set_realtime_http_enabled(&mut self, enabled: bool) {
        self.realtime_http_enabled = enabled;
    }

    /// Set the global fuzzy dedup threshold for newly connected devices.
    pub fn set_dedup_threshold(&mut self, threshold: u8) {
        self.dedup_threshold = threshold;
    }

    /// The local address of the shared UDP socket, if initialized.
    #[must_use]
    pub fn shared_socket_local_addr(&self) -> Option<SocketAddr> {
        self.shared_socket
            .get()
            .and_then(|socket| socket.local_addr().ok())
    }

    /// The local UDP socket used by a connected device, if available.
    #[must_use]
    pub fn connected_socket_local_addr(&self, id: &DeviceId) -> Option<SocketAddr> {
        self.devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .and_then(|device| device.try_lock().ok())
            .and_then(|device| device.socket.local_addr().ok())
    }

    /// The starting E1.31 universe assigned to a connected device.
    #[must_use]
    pub fn connected_e131_start_universe(&self, id: &DeviceId) -> Option<u16> {
        self.devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .and_then(|device| device.try_lock().ok())
            .map(|device| device.e131_start_universe)
    }

    async fn ensure_shared_socket(&self) -> Result<Arc<UdpSocket>> {
        self.shared_socket
            .get_or_try_init(|| async {
                UdpSocket::bind("0.0.0.0:0")
                    .await
                    .map(Arc::new)
                    .context("Failed to bind shared WLED UDP socket")
            })
            .await
            .map(Arc::clone)
    }

    async fn ensure_device_ready_for_output(
        id: &DeviceId,
        device: &mut WledDevice,
        realtime_http_enabled: bool,
    ) -> Result<()> {
        if device.stream_initialized {
            return Ok(());
        }

        if realtime_http_enabled {
            enter_realtime_mode(device.ip)
                .await
                .with_context(|| format!("Failed to enter realtime mode for WLED device {id}"))?;
            device.realtime_mode_active = true;
            validate_wled_receiver_config(
                device.ip,
                device.protocol,
                device.color_format,
                device.e131_start_universe,
            )
            .await;
            if let Err(error) = prime_device(device).await {
                if let Err(exit_error) = exit_realtime_mode(device.ip).await {
                    debug!(
                        device_id = %id,
                        ip = %device.ip,
                        error = %exit_error,
                        "best-effort exit from WLED realtime mode failed after priming error"
                    );
                }
                device.realtime_mode_active = false;
                return Err(error).with_context(|| format!("Failed to prime WLED device {id}"));
            }
        }

        device.stream_initialized = true;
        Ok(())
    }

    async fn write_device_colors(
        id: &DeviceId,
        device: &Arc<Mutex<WledDevice>>,
        colors: &[[u8; 3]],
        realtime_http_enabled: bool,
        delivery: Option<(DeviceDeliveryId, &dyn DeviceDeliveryObserver)>,
    ) -> Result<DeviceWriteOutcome> {
        let mut device = device.lock().await;
        Self::ensure_device_ready_for_output(id, &mut device, realtime_http_enabled).await?;
        let expected_led_count = usize::from(device.led_count);

        if colors.len() != expected_led_count {
            let should_warn = device
                .last_size_mismatch_warn_at
                .is_none_or(|last_warn_at| last_warn_at.elapsed() >= SIZE_MISMATCH_WARN_INTERVAL);

            if should_warn {
                warn!(
                    device_id = %id,
                    expected_led_count,
                    actual_led_count = colors.len(),
                    "WLED frame size mismatch; truncating or padding to match device"
                );
                device.last_size_mismatch_warn_at = Some(Instant::now());
            }
        }

        let wire_format = match device.protocol {
            WledProtocol::Ddp => device.ddp_wire_format(),
            WledProtocol::E131 => device.color_format,
        };
        let pixel_data = encode_colors(colors, wire_format, expected_led_count);

        device
            .send_frame_outcome_observed(&pixel_data, delivery)
            .await
    }

    fn allocate_e131_start_universe(
        &self,
        color_format: WledColorFormat,
        led_count: u16,
        protocol: WledProtocol,
    ) -> u16 {
        if protocol != WledProtocol::E131 {
            return 1;
        }

        let pixels_per_universe = E131_CHANNELS_PER_UNIVERSE / color_format.bytes_per_pixel();
        let universes_needed = usize::from(led_count)
            .div_ceil(pixels_per_universe)
            .clamp(1, usize::from(u16::MAX));
        let universes_needed = u16::try_from(universes_needed).unwrap_or(u16::MAX);
        let mut next_universe = self
            .next_e131_universe
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let start_universe = *next_universe;
        *next_universe = next_universe.saturating_add(universes_needed.max(1));
        start_universe
    }
}

impl Default for WledBackend {
    fn default() -> Self {
        Self::new()
    }
}

struct WledFrameSink {
    device_id: DeviceId,
    device: Arc<Mutex<WledDevice>>,
    realtime_http_enabled: bool,
}

#[async_trait::async_trait]
impl DeviceFrameSink for WledFrameSink {
    async fn write_colors_shared(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> std::result::Result<(), DeviceError> {
        self.write_colors_shared_outcome(colors).await.map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> std::result::Result<DeviceWriteOutcome, DeviceError> {
        WledBackend::write_device_colors(
            &self.device_id,
            &self.device,
            colors.as_slice(),
            self.realtime_http_enabled,
            None,
        )
        .await
        .map_err(|error| DeviceError::write(self.device_id, error))
    }

    async fn deliver_colors_shared_observed(
        &self,
        id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let result = WledBackend::write_device_colors(
            &self.device_id,
            &self.device,
            colors.as_slice(),
            self.realtime_http_enabled,
            Some((id, observer.as_ref())),
        )
        .await
        .map_err(|error| DeviceError::write(self.device_id, error));
        DeviceDeliveryAck::from_write_result(id, payload_bytes, started_at.elapsed(), result)
    }
}

#[async_trait::async_trait]
impl DeviceBackend for WledBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "wled".to_owned(),
            name: "WLED (DDP/E1.31)".to_owned(),
            description: "Network-attached WLED LED controllers over UDP".to_owned(),
        }
    }

    fn adopt_device(&self, discovered: &DiscoveredDevice) -> Result<(), DeviceError> {
        let Some(ip) = discovered
            .metadata
            .get("ip")
            .and_then(|ip| ip.parse::<IpAddr>().ok())
        else {
            return Err(DeviceError::NotAdopted {
                device_id: discovered.info.id,
            });
        };
        let rgbw = discovered
            .info
            .segments
            .first()
            .is_some_and(|segment| matches!(segment.color_format, DeviceColorFormat::Rgbw));
        let mac = discovered.metadata.get("mac").cloned().unwrap_or_default();
        let info = WledDeviceInfo {
            firmware_version: discovered
                .info
                .firmware_version
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            build_id: 0,
            mac,
            name: discovered.info.name.clone(),
            led_count: u16::try_from(discovered.info.total_led_count()).unwrap_or(u16::MAX),
            rgbw,
            max_segments: 1,
            fps: u8::try_from(discovered.info.capabilities.max_fps).unwrap_or(u8::MAX),
            power_draw_ma: 0,
            max_power_ma: 0,
            free_heap: 0,
            uptime_secs: 0,
            arch: discovered
                .metadata
                .get("arch")
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            is_wifi: true,
            effect_count: 0,
            palette_count: 0,
        };
        self.device_ips
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, ip);
        self.device_infos
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(discovered.info.id, info);
        Ok(())
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    async fn connect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let ip = {
            let device_ips = self
                .device_ips
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            device_ips
                .get(id)
                .copied()
                .ok_or(DeviceError::NotAdopted { device_id: *id })?
        };
        let wled_info = {
            let device_infos = self
                .device_infos
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            device_infos
                .get(id)
                .cloned()
                .ok_or(DeviceError::NotAdopted { device_id: *id })?
        };
        let socket = self
            .ensure_shared_socket()
            .await
            .map_err(|error| DeviceError::connection(id, error))?;

        let port = match self.default_protocol {
            WledProtocol::Ddp => DDP_PORT,
            WledProtocol::E131 => E131_PORT,
        };
        let pixel_addr = SocketAddr::new(ip, port);

        let color_format = if wled_info.rgbw {
            WledColorFormat::Rgbw
        } else {
            WledColorFormat::Rgb
        };
        let protocol = self.default_protocol;
        let e131_start_universe =
            self.allocate_e131_start_universe(color_format, wled_info.led_count, protocol);

        let device = WledDevice {
            device_id: *id,
            ip,
            protocol,
            pixel_addr,
            color_format,
            led_count: wled_info.led_count,
            info: wled_info,
            socket,
            ddp_sequence: DdpSequence::default(),
            e131_sequences: E131SequenceTracker::default(),
            e131_cid: self.e131_cid,
            e131_start_universe,
            dedup_threshold: self.dedup_threshold,
            last_sent_pixels: None,
            consecutive_failures: 0,
            last_success_at: None,
            last_size_mismatch_warn_at: None,
            realtime_mode_active: false,
            stream_initialized: false,
            frames_sent: 0,
            last_frame_at: None,
        };

        info!(
            device_id = %id,
            ip = %ip,
            protocol = ?protocol,
            leds = device.led_count,
            start_universe = device.e131_start_universe,
            "Connected to WLED device"
        );

        self.devices
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, Arc::new(Mutex::new(device)));
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        let device = self
            .devices
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        if let Some(device) = device {
            let mut device = device.lock().await;
            if device.last_sent_pixels.is_some()
                && let Err(error) = clear_device(&mut device).await
            {
                debug!(
                    device_id = %id,
                    ip = %device.ip,
                    error = %error,
                    "best-effort WLED clear frame failed during disconnect"
                );
            }
            if device.realtime_mode_active
                && let Err(error) = exit_realtime_mode(device.ip).await
            {
                debug!(
                    device_id = %id,
                    ip = %device.ip,
                    error = %error,
                    "best-effort exit from WLED realtime mode failed"
                );
            }
            info!(device_id = %id, "Disconnected from WLED device");
            Ok(())
        } else {
            Err(DeviceError::Disconnected {
                device: id.to_string(),
            })
        }
    }

    async fn write_colors(
        &self,
        id: &DeviceId,
        colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .ok_or_else(|| DeviceError::Disconnected {
                device: id.to_string(),
            })?;
        Self::write_device_colors(id, &device, colors, self.realtime_http_enabled, None)
            .await
            .map_err(|error| DeviceError::write(id, error))
            .map(|_| ())
    }

    async fn write_colors_shared(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> std::result::Result<(), DeviceError> {
        self.write_colors_shared_outcome(id, colors)
            .await
            .map(|_| ())
    }

    async fn write_colors_shared_outcome(
        &self,
        id: &DeviceId,
        colors: Arc<Vec<[u8; 3]>>,
    ) -> std::result::Result<DeviceWriteOutcome, DeviceError> {
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
            .ok_or_else(|| DeviceError::Disconnected {
                device: id.to_string(),
            })?;
        Self::write_device_colors(
            id,
            &device,
            colors.as_slice(),
            self.realtime_http_enabled,
            None,
        )
        .await
        .map_err(|error| DeviceError::write(id, error))
    }

    async fn deliver_colors_shared_observed(
        &self,
        device_id: &DeviceId,
        delivery_id: DeviceDeliveryId,
        colors: Arc<Vec<[u8; 3]>>,
        observer: Arc<dyn DeviceDeliveryObserver>,
    ) -> DeviceDeliveryAck {
        let payload_bytes = colors.len().saturating_mul(3);
        let started_at = Instant::now();
        let device = self
            .devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(device_id)
            .cloned();
        let Some(device) = device else {
            return DeviceDeliveryAck::rejected(
                delivery_id,
                DeviceError::Disconnected {
                    device: device_id.to_string(),
                },
            );
        };
        let result = Self::write_device_colors(
            device_id,
            &device,
            colors.as_slice(),
            self.realtime_http_enabled,
            Some((delivery_id, observer.as_ref())),
        )
        .await
        .map_err(|error| DeviceError::write(device_id, error));
        DeviceDeliveryAck::from_write_result(
            delivery_id,
            payload_bytes,
            started_at.elapsed(),
            result,
        )
    }

    fn target_fps(&self, id: &DeviceId) -> Option<u32> {
        self.devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .and_then(|device| device.try_lock().ok())
            .map(|device| device.info.negotiated_target_fps())
            .or_else(|| {
                self.device_infos
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(id)
                    .map(WledDeviceInfo::negotiated_target_fps)
            })
    }

    fn output_cadence(&self, id: &DeviceId) -> Option<OutputCadence> {
        self.target_fps(id).map(|target_fps| {
            OutputCadence::from_fps(target_fps).with_max_frame_silence(KEEPALIVE_INTERVAL)
        })
    }

    fn frame_sink(&self, id: &DeviceId) -> Option<Arc<dyn DeviceFrameSink>> {
        self.devices
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .map(|device| {
                Arc::new(WledFrameSink {
                    device_id: *id,
                    device: Arc::clone(device),
                    realtime_http_enabled: self.realtime_http_enabled,
                }) as Arc<dyn DeviceFrameSink>
            })
    }
}
