//! Transport scanner for `OpenRGB` SDK server discovery.
//!
//! [`OpenRgbScanner`] attempts a TCP connection to the configured
//! `OpenRGB` host:port and enumerates all controllers as discovered devices.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tracing::{debug, info};

use crate::device::discovery::{DiscoveredDevice, TransportScanner};
use crate::types::device::{
    ColorFormat, ConnectionType, DeviceCapabilities, DeviceFamily, DeviceId, DeviceIdentifier,
    DeviceInfo, LedTopology, ZoneInfo,
};

use super::client::{ClientConfig, OpenRgbClient};
use super::proto::{self, ControllerData, ZoneType};

// ── Scanner Configuration ────────────────────────────────────────────────

/// Configuration for the `OpenRGB` transport scanner.
#[derive(Debug, Clone)]
pub struct ScannerConfig {
    /// `OpenRGB` SDK server host.
    pub host: String,
    /// `OpenRGB` SDK server port.
    pub port: u16,
    /// TCP probe timeout.
    pub probe_timeout: Duration,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            host: proto::DEFAULT_HOST.to_owned(),
            port: proto::DEFAULT_PORT,
            probe_timeout: Duration::from_secs(2),
        }
    }
}

// ── OpenRgbScanner ───────────────────────────────────────────────────────

/// Discovers `OpenRGB` controllers by connecting to the SDK server.
///
/// The scanner first probes the configured TCP endpoint. If reachable,
/// it performs a full handshake and controller enumeration, returning
/// each controller as a [`DiscoveredDevice`].
pub struct OpenRgbScanner {
    /// Scanner configuration.
    config: ScannerConfig,
}

impl OpenRgbScanner {
    /// Create a new scanner with the given configuration.
    #[must_use]
    pub fn new(config: ScannerConfig) -> Self {
        Self { config }
    }

    /// Create a scanner with default configuration (localhost:6742).
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(ScannerConfig::default())
    }

    /// Quick TCP probe to check if the server is reachable.
    async fn probe_server(&self) -> bool {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        match tokio::time::timeout(self.config.probe_timeout, TcpStream::connect(&addr)).await {
            Ok(Ok(_stream)) => {
                debug!(address = %addr, "OpenRGB SDK server is reachable");
                true
            }
            Ok(Err(e)) => {
                debug!(address = %addr, error = %e, "OpenRGB SDK server not reachable");
                false
            }
            Err(_) => {
                debug!(address = %addr, "OpenRGB SDK server probe timed out");
                false
            }
        }
    }

    /// Map an `OpenRGB` controller to a [`DiscoveredDevice`].
    fn map_controller_to_discovered(index: u32, controller: &ControllerData) -> DiscoveredDevice {
        let zones: Vec<ZoneInfo> = controller
            .zones
            .iter()
            .map(|zone| {
                let topology = match zone.zone_type {
                    ZoneType::Single => {
                        if zone.leds_count == 1 {
                            LedTopology::Point
                        } else {
                            LedTopology::Custom
                        }
                    }
                    ZoneType::Linear => LedTopology::Strip,
                    ZoneType::Matrix => LedTopology::Matrix {
                        rows: zone.matrix_height,
                        cols: zone.matrix_width,
                    },
                };

                ZoneInfo {
                    name: zone.name.clone(),
                    led_count: zone.leds_count,
                    topology,
                    color_format: ColorFormat::Rgb,
                }
            })
            .collect();

        let total_leds: u32 = zones.iter().map(|z| z.led_count).sum();

        let identifier = DeviceIdentifier::OpenRgb {
            controller_name: controller.name.clone(),
            location: controller.location.clone(),
        };
        let fingerprint = identifier.fingerprint();

        let info = DeviceInfo {
            id: DeviceId::new(),
            name: controller.name.clone(),
            vendor: controller.vendor.clone(),
            family: DeviceFamily::OpenRgb,
            connection_type: ConnectionType::Network,
            zones,
            firmware_version: if controller.version.is_empty() {
                None
            } else {
                Some(controller.version.clone())
            },
            capabilities: DeviceCapabilities {
                led_count: total_leds,
                supports_direct: true,
                supports_brightness: false,
                max_fps: 60,
            },
        };

        let mut metadata = HashMap::new();
        metadata.insert("openrgb_index".to_owned(), index.to_string());
        metadata.insert("openrgb_location".to_owned(), controller.location.clone());
        metadata.insert(
            "openrgb_device_type".to_owned(),
            controller.device_type.to_string(),
        );
        if !controller.serial.is_empty() {
            metadata.insert("serial".to_owned(), controller.serial.clone());
        }

        DiscoveredDevice {
            connection_type: ConnectionType::Network,
            name: controller.name.clone(),
            family: DeviceFamily::OpenRgb,
            fingerprint,
            info,
            metadata,
        }
    }
}

#[async_trait::async_trait]
impl TransportScanner for OpenRgbScanner {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "OpenRGB SDK"
    }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        // Quick probe first — avoid full handshake overhead if unreachable
        if !self.probe_server().await {
            info!(
                host = %self.config.host,
                port = self.config.port,
                "OpenRGB SDK server not available, skipping scan"
            );
            return Ok(Vec::new());
        }

        // Full connection + enumeration
        let client_config = ClientConfig {
            host: self.config.host.clone(),
            port: self.config.port,
            connect_timeout: self.config.probe_timeout,
            ..ClientConfig::default()
        };

        let mut client = OpenRgbClient::new(client_config);

        client
            .connect()
            .await
            .context("failed to connect to OpenRGB for scan")?;

        let count = client
            .enumerate_controllers()
            .await
            .context("failed to enumerate controllers during scan")?;

        let mut devices = Vec::with_capacity(usize::try_from(count).unwrap_or(0));

        for (&index, controller) in client.controllers() {
            let discovered = Self::map_controller_to_discovered(index, controller);
            info!(
                index,
                name = %controller.name,
                leds = discovered.info.total_led_count(),
                "Discovered OpenRGB controller"
            );
            devices.push(discovered);
        }

        // Clean disconnect
        client.disconnect().await;

        info!(count = devices.len(), "OpenRGB scan complete");

        Ok(devices)
    }
}
