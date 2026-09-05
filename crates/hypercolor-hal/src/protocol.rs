//! Protocol abstraction for pure byte-level driver logic.

use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DisplayFramePayload, ScrollMode, SegmentInfo};

use crate::display::DisplayEncodeError;

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

    /// Encode a display payload into a reusable command buffer.
    ///
    /// The one display seam. `commands` is rewritten from the start and
    /// holds exactly this frame's wire commands on success; on failure its
    /// contents are unspecified and the caller must not send them.
    ///
    /// # Errors
    ///
    /// Returns [`DisplayEncodeError::Unsupported`] when the protocol drives no
    /// display or none that takes `payload.format`, and the engine's own
    /// errors when the frame cannot be expressed on the wire.
    fn encode_display_payload_into(
        &self,
        payload: DisplayFramePayload<'_>,
        commands: &mut Vec<ProtocolCommand>,
    ) -> Result<(), DisplayEncodeError> {
        let _ = commands;
        Err(DisplayEncodeError::Unsupported {
            format: payload.format,
        })
    }

    /// Zone descriptors for this device.
    fn zones(&self) -> Vec<SegmentInfo>;

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

    /// How the reply is read when `expects_response` is true.
    pub response: ResponsePlan,
}

impl Default for ProtocolCommand {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
            response: ResponsePlan::default(),
        }
    }
}

impl ProtocolCommand {
    /// Read `count` response reports instead of one.
    #[must_use]
    pub const fn with_response_count(mut self, count: u8) -> Self {
        self.response.count = count;
        self
    }

    /// Wait `timeout` for this command's response instead of the
    /// protocol-wide budget.
    #[must_use]
    pub const fn with_response_timeout(mut self, timeout: Duration) -> Self {
        self.response.timeout = Some(timeout);
        self
    }

    /// Accumulate up to `capacity` response bytes across transport packets.
    #[must_use]
    pub const fn with_response_capacity(mut self, capacity: usize) -> Self {
        self.response.capacity = Some(capacity);
        self
    }

    /// Treat a reply that never arrives as a normal outcome.
    #[must_use]
    pub const fn with_optional_response(mut self) -> Self {
        self.response.tolerance = ResponseTolerance::Optional;
        self
    }
}

/// How the backend reads the reply to a responding command.
///
/// Every field is a per-command override; the default plan reads one report
/// at the protocol-wide timeout and treats its absence as an error, which is
/// what every command did before plans existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponsePlan {
    /// Reports to read for this command, each handed to `parse_response` in
    /// arrival order. Parsing is ordinal-sensitive: a parser that treats every
    /// report alike lets a later report overwrite state from an earlier one.
    pub count: u8,

    /// Timeout for each read, overriding [`Protocol::response_timeout`].
    /// Init reads and steady-state reads on one device routinely want
    /// different budgets.
    pub timeout: Option<Duration>,

    /// Receive capacity in bytes: an upper bound, not an expected length.
    /// `None` reads once at the transport default; set this when one logical
    /// reply spans more packets than a single transport read returns.
    pub capacity: Option<usize>,

    /// Whether a report that never arrives fails the command.
    pub tolerance: ResponseTolerance,
}

impl Default for ResponsePlan {
    fn default() -> Self {
        Self {
            count: 1,
            timeout: None,
            capacity: None,
            tolerance: ResponseTolerance::Required,
        }
    }
}

/// What a missing report means for the command that expected it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseTolerance {
    /// The reply is part of the contract; a timeout fails the command.
    #[default]
    Required,

    /// The device may or may not answer. A timeout completes the command with
    /// whatever reports arrived, logged at debug. This is the shape of a
    /// status packet a firmware sends most of the time, and of a trailing
    /// report some units skip.
    Optional,
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

    /// Fill the next slot and hand it back, so a caller can adjust its
    /// response plan after the bytes are in place.
    pub fn push_fill<F>(
        &mut self,
        expects_response: bool,
        response_delay: Duration,
        post_delay: Duration,
        transfer_type: TransferType,
        fill: F,
    ) -> &mut ProtocolCommand
    where
        F: FnOnce(&mut Vec<u8>),
    {
        if self.used == self.commands.len() {
            self.commands.push(ProtocolCommand::default());
        }

        let command = &mut self.commands[self.used];
        self.used += 1;
        command.expects_response = expects_response;
        command.response_delay = response_delay;
        command.post_delay = post_delay;
        command.transfer_type = transfer_type;
        // Slots are reused across frames, so every field of a recycled
        // command must be rewritten or the previous frame's response plan
        // leaks into this one.
        command.response = ResponsePlan::default();
        command.data.clear();
        fill(&mut command.data);
        command
    }

    pub fn push_slice(
        &mut self,
        data: &[u8],
        expects_response: bool,
        response_delay: Duration,
        post_delay: Duration,
        transfer_type: TransferType,
    ) -> &mut ProtocolCommand {
        self.push_fill(
            expects_response,
            response_delay,
            post_delay,
            transfer_type,
            |buffer| buffer.extend_from_slice(data),
        )
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
    ) -> &mut ProtocolCommand {
        self.push_fill(
            expects_response,
            response_delay,
            post_delay,
            transfer_type,
            |buffer| buffer.extend_from_slice(value.as_bytes()),
        )
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
