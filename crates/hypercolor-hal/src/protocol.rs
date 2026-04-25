//! Protocol abstraction for pure byte-level driver logic.

use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint, DisplayFrameFormat,
    DisplayFramePayload, ScrollMode,
};

/// Pure byte-level protocol encoder/decoder.
///
/// Implementations keep wire-format logic isolated from transport I/O.
pub trait Protocol: Send + Sync {
    /// Human-readable protocol name.
    fn name(&self) -> &'static str;

    /// Commands to run when a device is first connected.
    fn init_sequence(&self) -> Vec<ProtocolCommand>;

    /// Commands to run before a device disconnects.
    fn shutdown_sequence(&self) -> Vec<ProtocolCommand>;

    /// Encode a device frame into one or more wire-level commands.
    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand>;

    /// Encode a device frame into a reusable command buffer.
    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        commands.clear();
        commands.extend(self.encode_frame(colors));
    }

    /// Encode a hardware brightness change, if the protocol supports it.
    #[must_use]
    fn encode_brightness(&self, _brightness: u8) -> Option<Vec<ProtocolCommand>> {
        None
    }

    /// Encode a hardware scroll wheel mode change, if supported.
    #[must_use]
    fn encode_scroll_mode(&self, _mode: ScrollMode) -> Option<Vec<ProtocolCommand>> {
        None
    }

    /// Encode a Smart Reel toggle, if supported.
    #[must_use]
    fn encode_scroll_smart_reel(&self, _enabled: bool) -> Option<Vec<ProtocolCommand>> {
        None
    }

    /// Encode a scroll acceleration toggle, if supported.
    #[must_use]
    fn encode_scroll_acceleration(&self, _enabled: bool) -> Option<Vec<ProtocolCommand>> {
        None
    }

    /// Optional one-shot commands used to verify a newly connected device.
    ///
    /// This is primarily useful for devices whose normal init/frame traffic is
    /// entirely write-only, where a successful transport send does not confirm
    /// that the device accepted or applied the command stream.
    fn connection_diagnostics(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    /// Background keepalive traffic required to keep the device in direct mode.
    ///
    /// Most devices do not need this. Protocols that do can return a command
    /// sequence and polling interval for the backend to run while connected.
    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        None
    }

    /// Resolve the command sequence to send for a keepalive tick.
    ///
    /// By default this reuses the static command list from [`keepalive`].
    /// Protocols with stateful keepalives can override this to generate
    /// commands from their latest internal state.
    fn keepalive_commands(&self) -> Vec<ProtocolCommand> {
        self.keepalive()
            .map_or_else(Vec::new, |keepalive| keepalive.commands)
    }

    /// Parse a raw device response payload.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the response is malformed or invalid.
    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError>;

    /// Response timeout budget for commands that expect a reply.
    fn response_timeout(&self) -> Duration {
        Duration::from_secs(1)
    }

    /// Encode a display frame from JPEG-compressed image data.
    ///
    /// Only implemented by protocols that drive pixel displays.
    #[must_use]
    fn encode_display_frame(&self, _jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
        None
    }

    /// Encode a display frame into a reusable command buffer.
    fn encode_display_frame_into(
        &self,
        jpeg_data: &[u8],
        commands: &mut Vec<ProtocolCommand>,
    ) -> Option<()> {
        commands.clear();
        commands.extend(self.encode_display_frame(jpeg_data)?);
        Some(())
    }

    /// Encode a display payload into a reusable command buffer.
    fn encode_display_payload_into(
        &self,
        payload: DisplayFramePayload<'_>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Option<()> {
        match payload.format {
            DisplayFrameFormat::Jpeg => self.encode_display_frame_into(payload.data, commands),
            DisplayFrameFormat::Rgb => None,
        }
    }

    /// Zone descriptors for this device.
    fn zones(&self) -> Vec<ProtocolZone>;

    /// Aggregate capabilities for this device.
    fn capabilities(&self) -> DeviceCapabilities;

    /// Total number of addressable LEDs.
    fn total_leds(&self) -> u32;

    /// Minimum interval between frames.
    fn frame_interval(&self) -> Duration;
}

/// Transport path hint for a protocol command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransferType {
    /// Use the transport's default data path.
    #[default]
    Primary,

    /// Use a bulk endpoint path.
    Bulk,

    /// Use HID feature reports over control transfers.
    HidReport,
}

/// One transport-ready command produced by a protocol encoder.
#[derive(Debug, Clone)]
pub struct ProtocolCommand {
    /// Raw command bytes.
    pub data: Vec<u8>,

    /// Whether the caller should read a response after sending.
    pub expects_response: bool,

    /// Minimum delay between sending the command and reading the response.
    pub response_delay: Duration,

    /// Minimum delay after sending this command.
    pub post_delay: Duration,

    /// Transport path hint for this command.
    pub transfer_type: TransferType,
}

impl ProtocolCommand {
    fn empty() -> Self {
        Self {
            data: Vec::new(),
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
        }
    }
}

/// Helper for filling reusable protocol command buffers in place.
pub struct CommandBuffer<'a> {
    commands: &'a mut Vec<ProtocolCommand>,
    used: usize,
}

impl<'a> CommandBuffer<'a> {
    #[must_use]
    pub fn new(commands: &'a mut Vec<ProtocolCommand>) -> Self {
        Self { commands, used: 0 }
    }

    pub fn push_fill<F>(
        &mut self,
        expects_response: bool,
        response_delay: Duration,
        post_delay: Duration,
        transfer_type: TransferType,
        fill: F,
    ) where
        F: FnOnce(&mut Vec<u8>),
    {
        if self.used == self.commands.len() {
            self.commands.push(ProtocolCommand::empty());
        }

        let command = &mut self.commands[self.used];
        self.used += 1;
        command.expects_response = expects_response;
        command.response_delay = response_delay;
        command.post_delay = post_delay;
        command.transfer_type = transfer_type;
        command.data.clear();
        fill(&mut command.data);
    }

    pub fn push_slice(
        &mut self,
        data: &[u8],
        expects_response: bool,
        response_delay: Duration,
        post_delay: Duration,
        transfer_type: TransferType,
    ) {
        self.push_fill(
            expects_response,
            response_delay,
            post_delay,
            transfer_type,
            |buffer| buffer.extend_from_slice(data),
        );
    }

    /// Write a zerocopy-compatible struct directly into the reusable command
    /// buffer, avoiding intermediate allocations.
    pub fn push_struct<T: zerocopy::IntoBytes + zerocopy::Immutable>(
        &mut self,
        value: &T,
        expects_response: bool,
        response_delay: Duration,
        post_delay: Duration,
        transfer_type: TransferType,
    ) {
        self.push_fill(
            expects_response,
            response_delay,
            post_delay,
            transfer_type,
            |buffer| buffer.extend_from_slice(value.as_bytes()),
        );
    }

    pub fn finish(self) {
        self.commands.truncate(self.used);
    }
}

/// A low-frequency protocol command sequence that should be run periodically
/// while a device remains connected.
#[derive(Debug, Clone)]
pub struct ProtocolKeepalive {
    /// Wire-level commands to execute for each keepalive tick.
    pub commands: Vec<ProtocolCommand>,

    /// Delay between keepalive ticks.
    pub interval: Duration,
}

/// Parsed response from a device.
#[derive(Debug, Clone)]
pub struct ProtocolResponse {
    /// Protocol-family-agnostic status.
    pub status: ResponseStatus,

    /// Parsed payload data.
    pub data: Vec<u8>,
}

/// Protocol-family-agnostic response status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseStatus {
    /// Command succeeded.
    Ok,

    /// Device is busy and caller should retry.
    Busy,

    /// Device rejected the command.
    Failed,

    /// Device timed out processing command.
    Timeout,

    /// Device does not support this command.
    Unsupported,
}

/// Zone descriptor emitted by a protocol implementation.
#[derive(Debug, Clone)]
pub struct ProtocolZone {
    /// Zone display name.
    pub name: String,

    /// Number of LEDs in this zone.
    pub led_count: u32,

    /// Physical arrangement hint.
    pub topology: DeviceTopologyHint,

    /// Wire-level color format.
    pub color_format: DeviceColorFormat,
}

/// Protocol-level parse/encode errors.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// CRC mismatch in a response packet.
    #[error("CRC mismatch: expected {expected:#04X}, got {actual:#04X}")]
    CrcMismatch {
        /// Computed checksum from packet content.
        expected: u8,
        /// Checksum byte from the response packet.
        actual: u8,
    },

    /// Response shape or length is invalid.
    #[error("malformed response: {detail}")]
    MalformedResponse {
        /// Human-readable detail.
        detail: String,
    },

    /// Device reported an error status code.
    #[error("device error: {status:?}")]
    DeviceError {
        /// Device status.
        status: ResponseStatus,
    },

    /// Input frame cannot be encoded under protocol limits.
    #[error("encoding error: {detail}")]
    EncodingError {
        /// Human-readable detail.
        detail: String,
    },
}
