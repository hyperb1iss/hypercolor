use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::error::{OpenRgbError, Result};
use crate::packet::{
    CLIENT_MAX_PROTOCOL_VERSION, Packet, PacketDecoder, PacketId, client_name_payload,
    encode_client_packet, request_controller_data_payload, request_protocol_version_payload,
    update_leds_payload, update_mode_payload, update_zone_leds_payload, validate_protocol_version,
};
use crate::parser::parse_controller_data;
use crate::types::{ControllerData, ControllerMode, RgbColor};

/// Runtime settings for an OpenRGB SDK client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRgbClientConfig {
    pub client_name: String,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub max_protocol_version: u32,
}

impl Default for OpenRgbClientConfig {
    fn default() -> Self {
        Self {
            client_name: "Hypercolor".to_owned(),
            connect_timeout: Duration::from_millis(750),
            read_timeout: Duration::from_secs(2),
            write_timeout: Duration::from_secs(2),
            max_protocol_version: CLIENT_MAX_PROTOCOL_VERSION,
        }
    }
}

/// Async OpenRGB SDK TCP client.
pub struct OpenRgbClient {
    stream: TcpStream,
    decoder: PacketDecoder,
    pending_packets: VecDeque<Packet>,
    config: OpenRgbClientConfig,
    protocol_version: u32,
}

impl OpenRgbClient {
    /// Connect, negotiate protocol version, and set the client name.
    ///
    /// # Errors
    ///
    /// Returns an error when the TCP connection, protocol negotiation, or client
    /// name write fails.
    pub async fn connect(addr: SocketAddr, config: OpenRgbClientConfig) -> Result<Self> {
        let stream = timeout(config.connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| OpenRgbError::Timeout {
                operation: "connect",
            })??;
        stream.set_nodelay(true)?;
        let mut client = Self {
            stream,
            decoder: PacketDecoder::new(),
            pending_packets: VecDeque::new(),
            config,
            protocol_version: 0,
        };
        let protocol_version = client.negotiate_protocol_version().await?;
        client.protocol_version = protocol_version;
        client.set_client_name().await?;
        Ok(client)
    }

    /// The negotiated SDK protocol version.
    #[must_use]
    pub const fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    /// Request the controller count.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or the response is malformed.
    pub async fn controller_count(&mut self) -> Result<u32> {
        self.send_packet(PacketId::RequestControllerCount, 0, Vec::new())
            .await?;
        let packet = self.expect_packet(PacketId::RequestControllerCount).await?;
        if packet.payload.len() != 4 {
            return Err(OpenRgbError::Truncated {
                needed: 4,
                remaining: packet.payload.len(),
            });
        }
        Ok(u32::from_le_bytes([
            packet.payload[0],
            packet.payload[1],
            packet.payload[2],
            packet.payload[3],
        ]))
    }

    /// Request and parse one controller data block.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or controller data is malformed.
    pub async fn controller_data(&mut self, controller_index: u32) -> Result<ControllerData> {
        self.send_packet(
            PacketId::RequestControllerData,
            controller_index,
            request_controller_data_payload(self.protocol_version),
        )
        .await?;
        let packet = self.expect_packet(PacketId::RequestControllerData).await?;
        parse_controller_data(&packet.payload, self.protocol_version)
    }

    /// Ask OpenRGB to rescan devices when the negotiated server supports it.
    ///
    /// # Errors
    ///
    /// Returns an error when the packet cannot be written.
    pub async fn request_rescan(&mut self) -> Result<()> {
        self.send_packet(PacketId::RequestRescanDevices, 0, Vec::new())
            .await
    }

    /// Put a controller into its custom/software-controlled mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the packet cannot be written.
    pub async fn set_custom_mode(&mut self, controller_index: u32) -> Result<()> {
        self.send_packet(PacketId::SetCustomMode, controller_index, Vec::new())
            .await
    }

    /// Apply a mode update to a controller.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload cannot be encoded or written.
    pub async fn update_mode(
        &mut self,
        controller_index: u32,
        mode_index: u32,
        mode: &ControllerMode,
    ) -> Result<()> {
        let payload = update_mode_payload(mode_index, mode)?;
        self.send_packet(PacketId::UpdateMode, controller_index, payload)
            .await
    }

    /// Stream per-controller LED colors.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload cannot be encoded or written.
    pub async fn update_leds(&mut self, controller_index: u32, colors: &[RgbColor]) -> Result<()> {
        let payload = update_leds_payload(colors)?;
        self.send_packet(PacketId::UpdateLeds, controller_index, payload)
            .await
    }

    /// Stream per-zone LED colors.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload cannot be encoded or written.
    pub async fn update_zone_leds(
        &mut self,
        controller_index: u32,
        zone_index: u32,
        colors: &[RgbColor],
    ) -> Result<()> {
        let payload = update_zone_leds_payload(zone_index, colors)?;
        self.send_packet(PacketId::UpdateZoneLeds, controller_index, payload)
            .await
    }

    /// Drain packets already available on the socket without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error when pending bytes contain a malformed packet or the
    /// TCP stream reports a terminal read error.
    pub fn drain_pending_packets(&mut self) -> Result<Vec<Packet>> {
        let mut packets = self.pending_packets.drain(..).collect::<Vec<_>>();
        loop {
            while let Some(packet) = self.decoder.next_packet()? {
                packets.push(packet);
            }

            let mut buf = [0_u8; 4096];
            match self.stream.try_read(&mut buf) {
                Ok(0) => return Err(OpenRgbError::ConnectionClosed),
                Ok(read) => self.decoder.push(&buf[..read]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(packets),
                Err(error) => return Err(error.into()),
            }
        }
    }

    async fn negotiate_protocol_version(&mut self) -> Result<u32> {
        self.send_packet(
            PacketId::RequestProtocolVersion,
            0,
            request_protocol_version_payload(self.config.max_protocol_version),
        )
        .await?;
        let packet = self.expect_packet(PacketId::RequestProtocolVersion).await?;
        if packet.payload.len() != 4 {
            return Err(OpenRgbError::Truncated {
                needed: 4,
                remaining: packet.payload.len(),
            });
        }
        let server_max = u32::from_le_bytes([
            packet.payload[0],
            packet.payload[1],
            packet.payload[2],
            packet.payload[3],
        ]);
        validate_protocol_version(server_max.min(self.config.max_protocol_version))
    }

    async fn set_client_name(&mut self) -> Result<()> {
        self.send_packet(
            PacketId::SetClientName,
            0,
            client_name_payload(&self.config.client_name),
        )
        .await
    }

    async fn send_packet(
        &mut self,
        packet_id: PacketId,
        device_index: u32,
        payload: Vec<u8>,
    ) -> Result<()> {
        let bytes = encode_client_packet(device_index, packet_id, payload)?;
        timeout(self.config.write_timeout, self.stream.write_all(&bytes))
            .await
            .map_err(|_| OpenRgbError::Timeout { operation: "write" })??;
        Ok(())
    }

    async fn expect_packet(&mut self, expected: PacketId) -> Result<Packet> {
        loop {
            let packet = self.read_packet().await?;
            if packet.header.packet_id == PacketId::DeviceListUpdated {
                self.pending_packets.push_back(packet);
                continue;
            }
            if packet.header.packet_id != expected {
                return Err(OpenRgbError::UnexpectedPacket {
                    expected,
                    actual: packet.header.packet_id,
                });
            }
            return Ok(packet);
        }
    }

    async fn read_packet(&mut self) -> Result<Packet> {
        loop {
            if let Some(packet) = self.decoder.next_packet()? {
                return Ok(packet);
            }

            let mut buf = [0_u8; 4096];
            let read = timeout(self.config.read_timeout, self.stream.read(&mut buf))
                .await
                .map_err(|_| OpenRgbError::Timeout { operation: "read" })??;
            if read == 0 {
                return Err(OpenRgbError::ConnectionClosed);
            }
            self.decoder.push(&buf[..read]);
        }
    }
}
