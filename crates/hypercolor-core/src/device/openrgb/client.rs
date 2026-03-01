//! TCP client for the `OpenRGB` SDK server.
//!
//! Handles connection, handshake, controller enumeration, and LED color
//! updates over the binary wire protocol. Includes reconnection logic
//! with exponential backoff.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

use super::proto::{
    self, Command, ControllerData, HEADER_SIZE, MAX_PROTOCOL_VERSION, PacketHeader,
};

// ── Connection Configuration ─────────────────────────────────────────────

/// Configuration for an `OpenRGB` SDK client connection.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Server hostname or IP address.
    pub host: String,
    /// Server TCP port.
    pub port: u16,
    /// Client name sent during handshake.
    pub client_name: String,
    /// Maximum protocol version to negotiate.
    pub protocol_version: u32,
    /// TCP connect timeout.
    pub connect_timeout: Duration,
    /// Read/write timeout for individual operations.
    pub io_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            host: proto::DEFAULT_HOST.to_owned(),
            port: proto::DEFAULT_PORT,
            client_name: proto::CLIENT_NAME.to_owned(),
            protocol_version: MAX_PROTOCOL_VERSION,
            connect_timeout: Duration::from_secs(5),
            io_timeout: Duration::from_secs(10),
        }
    }
}

// ── Reconnection Policy ─────────────────────────────────────────────────

/// Exponential backoff configuration for reconnection attempts.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// Delay before the first reconnection attempt.
    pub initial_delay: Duration,
    /// Maximum delay between attempts.
    pub max_delay: Duration,
    /// Backoff multiplier applied after each attempt.
    pub backoff_factor: f64,
    /// Maximum number of attempts (0 = unlimited).
    pub max_attempts: u32,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            backoff_factor: 2.0,
            max_attempts: 0,
        }
    }
}

impl ReconnectPolicy {
    /// Calculate the delay for a given attempt number.
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        #[allow(clippy::as_conversions, clippy::cast_possible_wrap)]
        let base = self.initial_delay.as_secs_f64() * self.backoff_factor.powi(attempt as i32);
        let clamped = base.min(self.max_delay.as_secs_f64());
        Duration::from_secs_f64(clamped)
    }

    /// Whether the given attempt number exceeds the configured maximum.
    #[must_use]
    pub fn exhausted(&self, attempt: u32) -> bool {
        self.max_attempts > 0 && attempt >= self.max_attempts
    }
}

// ── Connection State ─────────────────────────────────────────────────────

/// Current state of the SDK client connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected to any server.
    Disconnected,
    /// Successfully connected and handshake complete.
    Connected {
        /// Negotiated protocol version with the server.
        protocol_version: u32,
    },
    /// Connection lost, attempting to reconnect.
    Reconnecting {
        /// Current reconnection attempt number.
        attempt: u32,
    },
}

// ── OpenRGB SDK Client ───────────────────────────────────────────────────

/// TCP client for communicating with an `OpenRGB` SDK server.
///
/// Manages the TCP connection, protocol handshake, controller enumeration,
/// and LED color updates. Designed to be owned by a single async task.
pub struct OpenRgbClient {
    /// Client configuration.
    config: ClientConfig,
    /// Active TCP connection (None when disconnected).
    stream: Option<TcpStream>,
    /// Current connection state.
    state: ConnectionState,
    /// Negotiated protocol version for the current session.
    protocol_version: u32,
    /// Cached controller data, keyed by controller index.
    controllers: HashMap<u32, ControllerData>,
    /// Reconnection policy.
    reconnect_policy: ReconnectPolicy,
    /// Current reconnection attempt counter.
    reconnect_attempt: u32,
}

impl OpenRgbClient {
    /// Create a new client with the given configuration.
    #[must_use]
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            stream: None,
            state: ConnectionState::Disconnected,
            protocol_version: 0,
            controllers: HashMap::new(),
            reconnect_policy: ReconnectPolicy::default(),
            reconnect_attempt: 0,
        }
    }

    /// Create a client with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(ClientConfig::default())
    }

    /// Set the reconnection policy.
    pub fn set_reconnect_policy(&mut self, policy: ReconnectPolicy) {
        self.reconnect_policy = policy;
    }

    /// Current connection state.
    #[must_use]
    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    /// Whether the client is currently connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Connected { .. })
    }

    /// Negotiated protocol version (0 if not connected).
    #[must_use]
    pub fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    /// Access cached controller data.
    #[must_use]
    pub fn controllers(&self) -> &HashMap<u32, ControllerData> {
        &self.controllers
    }

    // ── Connection ───────────────────────────────────────────────────────

    /// Connect to the `OpenRGB` SDK server and perform the handshake.
    ///
    /// Sequence: TCP connect -> `SET_CLIENT_NAME` -> `REQUEST_PROTOCOL_VERSION`.
    pub async fn connect(&mut self) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        info!(address = %addr, "Connecting to OpenRGB SDK server");

        let stream = tokio::time::timeout(self.config.connect_timeout, TcpStream::connect(&addr))
            .await
            .context("connection timed out")?
            .context("TCP connect failed")?;

        stream
            .set_nodelay(true)
            .context("failed to set TCP_NODELAY")?;

        self.stream = Some(stream);

        // Handshake: set client name
        self.send_packet_raw(&proto::build_set_client_name(&self.config.client_name))
            .await
            .context("failed to send client name")?;

        // Handshake: negotiate protocol version
        self.send_packet_raw(&proto::build_request_protocol_version(
            self.config.protocol_version,
        ))
        .await
        .context("failed to send protocol version request")?;

        let (header, payload) = self
            .recv_packet()
            .await
            .context("reading protocol version response")?;
        proto::validate_response(&header, Command::RequestProtocolVersion)?;
        self.protocol_version = proto::parse_protocol_version(&payload)?;

        info!(
            negotiated_version = self.protocol_version,
            "OpenRGB protocol version negotiated"
        );

        self.state = ConnectionState::Connected {
            protocol_version: self.protocol_version,
        };
        self.reconnect_attempt = 0;

        Ok(())
    }

    /// Disconnect from the server, cleaning up internal state.
    pub async fn disconnect(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.shutdown().await;
        }
        self.state = ConnectionState::Disconnected;
        self.controllers.clear();
        self.protocol_version = 0;
        info!("Disconnected from OpenRGB SDK server");
    }

    /// Attempt to reconnect using the configured backoff policy.
    ///
    /// Returns `Ok(())` on successful reconnection, or `Err` if all
    /// attempts are exhausted or reconnection fails.
    pub async fn reconnect(&mut self) -> Result<()> {
        if self.reconnect_policy.exhausted(self.reconnect_attempt) {
            bail!(
                "reconnection attempts exhausted after {} tries",
                self.reconnect_attempt
            );
        }

        let delay = self
            .reconnect_policy
            .delay_for_attempt(self.reconnect_attempt);
        self.state = ConnectionState::Reconnecting {
            attempt: self.reconnect_attempt,
        };

        warn!(
            attempt = self.reconnect_attempt,
            delay_ms = delay.as_millis(),
            "Attempting reconnection to OpenRGB"
        );

        tokio::time::sleep(delay).await;
        self.reconnect_attempt += 1;

        // Clean up any stale connection
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.shutdown().await;
        }

        self.connect().await
    }

    // ── Controller Operations ────────────────────────────────────────────

    /// Request the number of controllers from the server.
    pub async fn request_controller_count(&mut self) -> Result<u32> {
        self.ensure_connected()?;

        self.send_packet_raw(&proto::build_request_controller_count())
            .await?;

        let (header, payload) = self.recv_packet().await?;
        proto::validate_response(&header, Command::RequestControllerCount)?;
        proto::parse_controller_count(&payload)
    }

    /// Request full data for a specific controller.
    pub async fn request_controller_data(&mut self, index: u32) -> Result<ControllerData> {
        self.ensure_connected()?;

        self.send_packet_raw(&proto::build_request_controller_data(
            index,
            self.protocol_version,
        ))
        .await?;

        let (header, payload) = self.recv_packet().await?;
        proto::validate_response(&header, Command::RequestControllerData)?;
        proto::parse_controller_data(&payload, self.protocol_version)
    }

    /// Enumerate all controllers from the server, caching the results.
    ///
    /// Returns the number of controllers found.
    pub async fn enumerate_controllers(&mut self) -> Result<u32> {
        let count = self.request_controller_count().await?;
        info!(count, "Enumerating OpenRGB controllers");

        let mut controllers = HashMap::with_capacity(usize::try_from(count).unwrap_or(0));
        for i in 0..count {
            let data = self
                .request_controller_data(i)
                .await
                .with_context(|| format!("requesting controller {i}"))?;
            debug!(
                index = i,
                name = %data.name,
                zones = data.zones.len(),
                "Controller enumerated"
            );
            controllers.insert(i, data);
        }

        self.controllers = controllers;
        Ok(count)
    }

    /// Switch a controller to Direct/Custom mode.
    pub async fn set_custom_mode(&mut self, device_index: u32) -> Result<()> {
        self.ensure_connected()?;
        self.send_packet_raw(&proto::build_set_custom_mode(device_index))
            .await
            .context("failed to send SetCustomMode")
    }

    /// Update all LEDs on a controller.
    pub async fn update_leds(&mut self, device_index: u32, colors: &[[u8; 3]]) -> Result<()> {
        self.ensure_connected()?;
        self.send_packet_raw(&proto::build_update_leds(device_index, colors))
            .await
            .context("failed to send UpdateLEDs")
    }

    /// Update LEDs in a specific zone.
    pub async fn update_zone_leds(
        &mut self,
        device_index: u32,
        zone_index: u32,
        colors: &[[u8; 3]],
    ) -> Result<()> {
        self.ensure_connected()?;
        self.send_packet_raw(&proto::build_update_zone_leds(
            device_index,
            zone_index,
            colors,
        ))
        .await
        .context("failed to send UpdateZoneLEDs")
    }

    // ── Internal I/O ─────────────────────────────────────────────────────

    /// Ensure we have an active connection.
    fn ensure_connected(&self) -> Result<()> {
        if self.stream.is_none() {
            bail!("not connected to OpenRGB server");
        }
        Ok(())
    }

    /// Send a raw packet (header + payload bytes) over the TCP stream.
    async fn send_packet_raw(&mut self, data: &[u8]) -> Result<()> {
        let stream = self.stream.as_mut().context("no active connection")?;

        tokio::time::timeout(self.config.io_timeout, stream.write_all(data))
            .await
            .context("write timed out")?
            .context("write failed")?;

        tokio::time::timeout(self.config.io_timeout, stream.flush())
            .await
            .context("flush timed out")?
            .context("flush failed")?;

        Ok(())
    }

    /// Receive a complete packet (header + payload) from the TCP stream.
    async fn recv_packet(&mut self) -> Result<(PacketHeader, Vec<u8>)> {
        /// Maximum payload size we'll accept (16 MiB). Protects against
        /// corrupt or malicious servers that advertise absurd data lengths.
        const MAX_PAYLOAD_SIZE: u32 = 16 * 1024 * 1024;

        let stream = self.stream.as_mut().context("no active connection")?;

        // Read header
        let mut header_buf = [0u8; HEADER_SIZE];
        tokio::time::timeout(self.config.io_timeout, stream.read_exact(&mut header_buf))
            .await
            .context("header read timed out")?
            .context("failed to read header")?;

        let header = PacketHeader::from_bytes(&header_buf)?;

        // Guard against absurd payload sizes
        anyhow::ensure!(
            header.data_length <= MAX_PAYLOAD_SIZE,
            "payload size {} exceeds maximum allowed ({MAX_PAYLOAD_SIZE} bytes)",
            header.data_length
        );

        // Read payload
        let payload_len =
            usize::try_from(header.data_length).context("data_length exceeds usize")?;
        let mut payload = vec![0u8; payload_len];
        if !payload.is_empty() {
            tokio::time::timeout(self.config.io_timeout, stream.read_exact(&mut payload))
                .await
                .context("payload read timed out")?
                .context("failed to read payload")?;
        }

        Ok((header, payload))
    }
}
