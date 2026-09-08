use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout};

use crate::error::{OpenRgbError, Result};
use crate::packet::{
    CLIENT_MAX_PROTOCOL_VERSION, ClientPacketPolicy, Packet, PacketDecoder, PacketId,
    REQUEST_RESCAN_DEVICES_MIN_PROTOCOL_VERSION, client_name_payload,
    encode_client_packet_with_policy, request_controller_data_payload,
    request_protocol_version_payload, resize_zone_payload, update_leds_payload,
    update_mode_payload, update_zone_leds_payload, validate_protocol_version,
};
use crate::parser::parse_controller_data;
use crate::types::{ControllerData, ControllerMode, RgbColor, ZoneType};

/// Runtime settings for an OpenRGB SDK client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRgbClientConfig {
    pub client_name: String,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub max_protocol_version: u32,
    /// Permit `RESIZEZONE`. Off by default; `SAVEMODE` has no such gate.
    pub allow_zone_resize: bool,
}

impl Default for OpenRgbClientConfig {
    fn default() -> Self {
        Self {
            client_name: "Hypercolor".to_owned(),
            connect_timeout: Duration::from_millis(750),
            read_timeout: Duration::from_secs(2),
            write_timeout: Duration::from_secs(2),
            max_protocol_version: CLIENT_MAX_PROTOCOL_VERSION,
            allow_zone_resize: false,
        }
    }
}

impl OpenRgbClientConfig {
    /// The packet policy this configuration authorizes.
    #[must_use]
    pub const fn packet_policy(&self) -> ClientPacketPolicy {
        ClientPacketPolicy {
            allow_zone_resize: self.allow_zone_resize,
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
                after: config.connect_timeout,
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

    /// Whether the negotiated server protocol documents device rescan requests.
    #[must_use]
    pub const fn supports_device_rescan(&self) -> bool {
        self.protocol_version >= REQUEST_RESCAN_DEVICES_MIN_PROTOCOL_VERSION
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
        let payload = self.controller_data_payload(controller_index).await?;
        parse_controller_data(&payload, self.protocol_version)
    }

    /// Request one raw controller data payload without parsing it.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or the SDK response packet
    /// cannot be decoded.
    pub async fn controller_data_payload(&mut self, controller_index: u32) -> Result<Vec<u8>> {
        self.send_packet(
            PacketId::RequestControllerData,
            controller_index,
            request_controller_data_payload(self.protocol_version),
        )
        .await?;
        let packet = self.expect_packet(PacketId::RequestControllerData).await?;
        Ok(packet.payload)
    }

    /// Ask OpenRGB to rescan devices.
    ///
    /// Callers should check [`Self::supports_device_rescan`] before sending
    /// this request to a negotiated server.
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

    /// Resize one zone on a controller.
    ///
    /// The zone is looked up on the live controller data so the request is
    /// clamped to the server's advertised `leds_min..=leds_max` range. Returns
    /// the size actually requested. The server acknowledges with a
    /// `DEVICE_LIST_UPDATED` notification rather than a direct response, so
    /// callers re-enumerate afterwards.
    ///
    /// # Errors
    ///
    /// Returns [`OpenRgbError::ForbiddenPacket`] when the client was not
    /// configured with `allow_zone_resize`, [`OpenRgbError::ZoneIndexOutOfRange`]
    /// when the zone does not exist, [`OpenRgbError::ZoneNotResizable`] for
    /// single-LED zones or zones whose bounds admit no other size, and
    /// transport errors otherwise.
    pub async fn resize_zone(
        &mut self,
        controller_index: u32,
        zone_index: u32,
        new_size: u32,
    ) -> Result<u32> {
        if !self.config.allow_zone_resize {
            return Err(OpenRgbError::ForbiddenPacket(PacketId::ResizeZone));
        }
        let controller = self.controller_data(controller_index).await?;
        let zone = usize::try_from(zone_index)
            .ok()
            .and_then(|index| controller.zones.get(index))
            .ok_or(OpenRgbError::ZoneIndexOutOfRange {
                zone_index,
                zone_count: controller.zones.len(),
            })?;
        if zone.zone_type == ZoneType::Single || zone.leds_min > zone.leds_max {
            return Err(OpenRgbError::ZoneNotResizable { zone_index });
        }
        let size = new_size.clamp(zone.leds_min, zone.leds_max);
        self.send_packet(
            PacketId::ResizeZone,
            controller_index,
            resize_zone_payload(zone_index, size),
        )
        .await?;
        Ok(size)
    }

    /// Wait up to `wait` for the server to announce a device-list change.
    ///
    /// Notifications already harvested while answering earlier requests are
    /// consumed first. Returns `Ok(false)` when the deadline passes with no
    /// announcement.
    ///
    /// # Errors
    ///
    /// Returns an error when the stream closes, a malformed packet arrives, or
    /// the server sends an unsolicited packet other than `DEVICE_LIST_UPDATED`.
    pub async fn wait_for_device_list_update(&mut self, wait: Duration) -> Result<bool> {
        if self.take_pending_device_list_update() {
            return Ok(true);
        }
        let deadline = Instant::now() + wait;
        let Some(packet) = self.read_packet_until(deadline).await? else {
            return Ok(false);
        };
        if packet.header.packet_id == PacketId::DeviceListUpdated {
            return Ok(true);
        }
        Err(OpenRgbError::UnexpectedPacket {
            expected: PacketId::DeviceListUpdated,
            actual: packet.header.packet_id,
        })
    }

    /// Close the connection cleanly.
    ///
    /// Drains any notifications still buffered on the socket, then shuts down
    /// the write half so the server observes an orderly FIN instead of a
    /// reset when the stream is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket shutdown itself fails. A peer that
    /// already closed is not an error.
    pub async fn close(mut self) -> Result<()> {
        match self.drain_pending_packets() {
            Ok(_) | Err(OpenRgbError::ConnectionClosed) => {}
            Err(error) => return Err(error),
        }
        match timeout(self.config.write_timeout, self.stream.shutdown()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.kind() == ErrorKind::NotConnected => Ok(()),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err(OpenRgbError::Timeout {
                operation: "shutdown",
                after: self.config.write_timeout,
            }),
        }
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
        let bytes = encode_client_packet_with_policy(
            device_index,
            packet_id,
            payload,
            self.config.packet_policy(),
        )?;
        timeout(self.config.write_timeout, self.stream.write_all(&bytes))
            .await
            .map_err(|_| OpenRgbError::Timeout {
                operation: "write",
                after: self.config.write_timeout,
            })??;
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

    fn take_pending_device_list_update(&mut self) -> bool {
        let before = self.pending_packets.len();
        self.pending_packets
            .retain(|packet| packet.header.packet_id != PacketId::DeviceListUpdated);
        before != self.pending_packets.len()
    }

    async fn read_packet_until(&mut self, deadline: Instant) -> Result<Option<Packet>> {
        loop {
            if let Some(packet) = self.decoder.next_packet()? {
                return Ok(Some(packet));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let mut buf = [0_u8; 4096];
            let Ok(read) = timeout(remaining, self.stream.read(&mut buf)).await else {
                return Ok(None);
            };
            let read = read?;
            if read == 0 {
                return Err(OpenRgbError::ConnectionClosed);
            }
            self.decoder.push(&buf[..read]);
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
                .map_err(|_| OpenRgbError::Timeout {
                    operation: "read",
                    after: self.config.read_timeout,
                })??;
            if read == 0 {
                return Err(OpenRgbError::ConnectionClosed);
            }
            self.decoder.push(&buf[..read]);
        }
    }
}
