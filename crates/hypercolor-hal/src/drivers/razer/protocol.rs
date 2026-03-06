//! Pure Razer protocol encoder/decoder.

use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use tracing::warn;

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ProtocolZone, ResponseStatus,
};

use super::crc::{RAZER_REPORT_LEN, razer_crc};
use super::types::{
    EFFECT_CUSTOM_FRAME, LED_ID_BACKLIGHT, LED_ID_LOGO, LED_ID_SCROLL_WHEEL, LED_ID_ZERO, NOSTORE,
    RazerLightingCommandSet, RazerMatrixType, RazerProtocolVersion,
};

const STATUS_OFFSET: usize = 0;
const DATA_SIZE_OFFSET: usize = 5;
const COMMAND_CLASS_OFFSET: usize = 6;
const COMMAND_ID_OFFSET: usize = 7;
const ARGS_OFFSET: usize = 8;
const ARGS_LEN: usize = 80;
const CRC_OFFSET: usize = 88;

/// Pure packet encoder/decoder for Razer HID reports.
#[derive(Debug, Clone)]
pub struct RazerProtocol {
    version: RazerProtocolVersion,
    command_set: RazerLightingCommandSet,
    matrix_type: RazerMatrixType,
    matrix_size: (u8, u8),
    reported_matrix_size: Option<(u8, u8)>,
    led_id: u8,
    sends_device_mode_commands: bool,
    standard_storage: u8,
    frame_transaction_id: Option<u8>,
}

impl RazerProtocol {
    /// Construct a protocol encoder for a specific device model.
    #[must_use]
    pub fn new(
        version: RazerProtocolVersion,
        command_set: RazerLightingCommandSet,
        matrix_type: RazerMatrixType,
        matrix_size: (u8, u8),
        led_id: u8,
    ) -> Self {
        Self {
            version,
            command_set,
            matrix_type,
            matrix_size,
            reported_matrix_size: None,
            led_id,
            sends_device_mode_commands: true,
            standard_storage: NOSTORE,
            frame_transaction_id: None,
        }
    }

    /// Override the user-facing matrix dimensions when transport geometry differs.
    #[must_use]
    pub const fn with_reported_matrix_size(mut self, reported_matrix_size: (u8, u8)) -> Self {
        self.reported_matrix_size = Some(reported_matrix_size);
        self
    }

    /// Disable `SET_DEVICE_MODE` init/shutdown packets for devices that do not use them.
    #[must_use]
    pub const fn without_device_mode_commands(mut self) -> Self {
        self.sends_device_mode_commands = false;
        self
    }

    /// Override the storage byte used by standard LED/effect commands.
    #[must_use]
    pub const fn with_standard_storage(mut self, standard_storage: u8) -> Self {
        self.standard_storage = standard_storage;
        self
    }

    /// Override the transaction ID used for frame upload packets only.
    #[must_use]
    pub const fn with_frame_transaction_id(mut self, frame_transaction_id: u8) -> Self {
        self.frame_transaction_id = Some(frame_transaction_id);
        self
    }

    /// Protocol generation.
    #[must_use]
    pub const fn version(&self) -> RazerProtocolVersion {
        self.version
    }

    /// Lighting command family used for non-mode packet generation.
    #[must_use]
    pub const fn command_set(&self) -> RazerLightingCommandSet {
        self.command_set
    }

    /// Matrix addressing mode.
    #[must_use]
    pub const fn matrix_type(&self) -> RazerMatrixType {
        self.matrix_type
    }

    /// Matrix dimensions in `(rows, cols)`.
    #[must_use]
    pub const fn matrix_size(&self) -> (u8, u8) {
        self.matrix_size
    }

    /// Primary LED ID.
    #[must_use]
    pub const fn led_id(&self) -> u8 {
        self.led_id
    }

    fn protocol_name(&self) -> &'static str {
        match (self.version, self.command_set) {
            (RazerProtocolVersion::Legacy, _) => "Razer Legacy",
            (RazerProtocolVersion::Extended, RazerLightingCommandSet::Extended) => "Razer Extended",
            (RazerProtocolVersion::Extended, RazerLightingCommandSet::Standard) => {
                "Razer 0x3F Standard"
            }
            (RazerProtocolVersion::Modern, RazerLightingCommandSet::Extended) => "Razer Modern",
            (RazerProtocolVersion::Modern, RazerLightingCommandSet::Standard) => {
                "Razer 0x1F Standard"
            }
            (RazerProtocolVersion::WirelessKb, RazerLightingCommandSet::Extended) => {
                "Razer Wireless Keyboard"
            }
            (RazerProtocolVersion::WirelessKb, RazerLightingCommandSet::Standard) => {
                "Razer 0x9F Standard"
            }
        }
    }

    fn mode_command(&self, mode: u8, post_delay: Duration) -> Option<ProtocolCommand> {
        // Many Razer devices accept this mode switch as a fire-and-forget
        // feature report and do not return a stable response payload.
        self.build_packet(0x00, 0x04, &[mode, 0x00], false, post_delay)
    }

    fn zone_name(&self) -> &'static str {
        match self.led_id {
            LED_ID_BACKLIGHT => "Backlight",
            LED_ID_SCROLL_WHEEL => "Scroll Wheel",
            LED_ID_LOGO => "Logo",
            LED_ID_ZERO => "Main",
            _ => "Lighting",
        }
    }

    fn normalize_colors(&self, colors: &[[u8; 3]]) -> Vec<[u8; 3]> {
        let expected = usize::try_from(self.total_leds()).unwrap_or(0);
        if expected == 0 {
            return Vec::new();
        }

        if colors.len() == expected {
            return colors.to_vec();
        }

        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = min(colors.len(), expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);

        if colors.len() != expected {
            warn!(
                expected,
                actual = colors.len(),
                "razer frame length mismatch; applying truncate/pad"
            );
        }

        normalized
    }

    fn frame_chunk_capacity(matrix_type: RazerMatrixType) -> usize {
        match matrix_type {
            RazerMatrixType::Linear => 16,
            RazerMatrixType::None
            | RazerMatrixType::Standard
            | RazerMatrixType::Extended
            | RazerMatrixType::ExtendedArgb => 25,
        }
    }

    fn build_packet(
        &self,
        command_class: u8,
        command_id: u8,
        args: &[u8],
        expects_response: bool,
        post_delay: Duration,
    ) -> Option<ProtocolCommand> {
        self.build_packet_with_transaction(
            self.version.transaction_id(),
            command_class,
            command_id,
            args,
            expects_response,
            post_delay,
        )
    }

    fn build_packet_with_transaction(
        &self,
        transaction_id: u8,
        command_class: u8,
        command_id: u8,
        args: &[u8],
        expects_response: bool,
        post_delay: Duration,
    ) -> Option<ProtocolCommand> {
        if args.len() > ARGS_LEN {
            warn!(
                args_len = args.len(),
                "razer command payload exceeds argument field, dropping packet"
            );
            return None;
        }

        let mut packet = [0_u8; RAZER_REPORT_LEN];
        packet[1] = transaction_id;
        packet[DATA_SIZE_OFFSET] = u8::try_from(args.len()).unwrap_or(0);
        packet[COMMAND_CLASS_OFFSET] = command_class;
        packet[COMMAND_ID_OFFSET] = command_id;
        packet[ARGS_OFFSET..ARGS_OFFSET + args.len()].copy_from_slice(args);
        packet[CRC_OFFSET] = razer_crc(&packet);

        Some(ProtocolCommand {
            data: packet.to_vec(),
            expects_response,
            post_delay,
        })
    }

    fn activation_command(&self) -> Option<ProtocolCommand> {
        if matches!(self.command_set, RazerLightingCommandSet::Standard) {
            return self.build_packet(
                0x03,
                0x0A,
                &[0x05, self.standard_storage],
                false,
                Duration::ZERO,
            );
        }

        self.build_packet(
            0x0F,
            0x02,
            &[NOSTORE, LED_ID_ZERO, EFFECT_CUSTOM_FRAME],
            false,
            Duration::ZERO,
        )
    }

    fn encode_scalar(&self, color: [u8; 3]) -> Vec<ProtocolCommand> {
        let (command_class, command_id, args) = match self.command_set {
            RazerLightingCommandSet::Standard => (
                0x03,
                0x01,
                vec![
                    self.standard_storage,
                    self.led_id,
                    color[0],
                    color[1],
                    color[2],
                ],
            ),
            RazerLightingCommandSet::Extended => (
                0x0F,
                0x02,
                vec![
                    NOSTORE,
                    self.led_id,
                    0x01,
                    0x00,
                    0x00,
                    0x01,
                    color[0],
                    color[1],
                    color[2],
                ],
            ),
        };

        self.build_packet(command_class, command_id, &args, false, Duration::ZERO)
            .into_iter()
            .collect()
    }

    fn encode_linear(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        let frame_transaction_id = self
            .frame_transaction_id
            .unwrap_or(self.version.transaction_id());

        let led_count = min(
            colors.len(),
            Self::frame_chunk_capacity(RazerMatrixType::Linear),
        );
        if led_count == 0 {
            return commands;
        }

        let stop_col = u8::try_from(led_count - 1).unwrap_or(0);
        let mut args = Vec::with_capacity(50);
        args.push(0x00);
        args.push(stop_col);

        for color in colors.iter().take(led_count) {
            args.extend_from_slice(color);
        }

        while args.len() < 50 {
            args.push(0x00);
        }

        if let Some(packet) = self.build_packet_with_transaction(
            frame_transaction_id,
            0x03,
            0x0C,
            &args,
            false,
            Duration::from_millis(1),
        ) {
            commands.push(packet);
        }

        if let Some(activation) = self.activation_command() {
            commands.push(activation);
        }

        commands
    }

    fn encode_matrix(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        let frame_transaction_id = self
            .frame_transaction_id
            .unwrap_or(self.version.transaction_id());

        let rows = usize::from(self.matrix_size.0);
        let cols = usize::from(self.matrix_size.1);
        if rows == 0 || cols == 0 {
            return commands;
        }

        let max_chunk = Self::frame_chunk_capacity(self.matrix_type);
        for row in 0..rows {
            let row_start = row.saturating_mul(cols);
            let row_end = row_start.saturating_add(cols);
            let row_colors = &colors[row_start..row_end];

            for chunk_start in (0..cols).step_by(max_chunk) {
                let chunk_end = min(chunk_start + max_chunk, cols);
                let chunk_len = chunk_end.saturating_sub(chunk_start);
                if chunk_len == 0 {
                    continue;
                }

                let mut args = Vec::with_capacity(ARGS_LEN);
                let row_u8 = u8::try_from(row).unwrap_or(0);
                let start_col = u8::try_from(chunk_start).unwrap_or(0);
                let stop_col = u8::try_from(chunk_end - 1).unwrap_or(0);

                let (command_class, command_id) = match self.command_set {
                    RazerLightingCommandSet::Standard => {
                        args.extend_from_slice(&[0xFF, row_u8, start_col, stop_col]);
                        (0x03, 0x0B)
                    }
                    RazerLightingCommandSet::Extended => {
                        args.extend_from_slice(&[0x00, 0x00, row_u8, start_col, stop_col]);
                        (0x0F, 0x03)
                    }
                };

                for color in &row_colors[chunk_start..chunk_end] {
                    args.extend_from_slice(color);
                }

                if let Some(packet) = self.build_packet_with_transaction(
                    frame_transaction_id,
                    command_class,
                    command_id,
                    &args,
                    false,
                    Duration::from_millis(1),
                ) {
                    commands.push(packet);
                }
            }
        }

        if let Some(activation) = self.activation_command() {
            commands.push(activation);
        }

        commands
    }

    fn map_status(byte: u8) -> ResponseStatus {
        match byte {
            0x01 => ResponseStatus::Busy,
            0x02 => ResponseStatus::Ok,
            0x04 => ResponseStatus::Timeout,
            0x05 => ResponseStatus::Unsupported,
            _ => ResponseStatus::Failed,
        }
    }
}

impl Protocol for RazerProtocol {
    fn name(&self) -> &str {
        self.protocol_name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        if !self.sends_device_mode_commands {
            return Vec::new();
        }

        self.mode_command(0x03, Duration::from_millis(7))
            .into_iter()
            .collect()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        if !self.sends_device_mode_commands {
            return Vec::new();
        }

        self.mode_command(0x00, Duration::ZERO)
            .into_iter()
            .collect()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let normalized = self.normalize_colors(colors);
        match self.matrix_type {
            RazerMatrixType::None => {
                let color = normalized.first().copied().unwrap_or([0, 0, 0]);
                self.encode_scalar(color)
            }
            RazerMatrixType::Linear => self.encode_linear(&normalized),
            RazerMatrixType::Standard
            | RazerMatrixType::Extended
            | RazerMatrixType::ExtendedArgb => self.encode_matrix(&normalized),
        }
    }

    fn encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>> {
        let (command_class, command_id) = match self.command_set {
            RazerLightingCommandSet::Standard => (0x03, 0x03),
            RazerLightingCommandSet::Extended => (0x0F, 0x04),
        };

        let storage = match self.command_set {
            RazerLightingCommandSet::Standard => self.standard_storage,
            RazerLightingCommandSet::Extended => NOSTORE,
        };

        self.build_packet(
            command_class,
            command_id,
            &[storage, self.led_id, brightness],
            false,
            Duration::ZERO,
        )
        .map(|command| vec![command])
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        if data.len() < RAZER_REPORT_LEN {
            return Err(ProtocolError::MalformedResponse {
                detail: format!(
                    "expected at least {} bytes, got {}",
                    RAZER_REPORT_LEN,
                    data.len()
                ),
            });
        }

        let mut report = [0_u8; RAZER_REPORT_LEN];
        report.copy_from_slice(&data[..RAZER_REPORT_LEN]);

        let expected_crc = razer_crc(&report);
        let actual_crc = report[CRC_OFFSET];
        if expected_crc != actual_crc {
            return Err(ProtocolError::CrcMismatch {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        let status = Self::map_status(report[STATUS_OFFSET]);
        if status == ResponseStatus::Failed {
            return Err(ProtocolError::DeviceError { status });
        }

        let data_size = usize::from(report[DATA_SIZE_OFFSET]);
        if data_size > ARGS_LEN {
            return Err(ProtocolError::MalformedResponse {
                detail: format!("data size exceeds arguments field: {data_size}"),
            });
        }

        let payload_end = ARGS_OFFSET + data_size;
        let payload = report[ARGS_OFFSET..payload_end].to_vec();

        Ok(ProtocolResponse {
            status,
            data: payload,
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let total_leds = self.total_leds();
        let zone_matrix_size = self.reported_matrix_size.unwrap_or(self.matrix_size);
        let topology = match self.matrix_type {
            RazerMatrixType::None => DeviceTopologyHint::Point,
            RazerMatrixType::Linear => DeviceTopologyHint::Strip,
            RazerMatrixType::Standard
            | RazerMatrixType::Extended
            | RazerMatrixType::ExtendedArgb => DeviceTopologyHint::Matrix {
                rows: u32::from(zone_matrix_size.0),
                cols: u32::from(zone_matrix_size.1),
            },
        };

        vec![ProtocolZone {
            name: self.zone_name().to_owned(),
            led_count: total_leds,
            topology,
            color_format: DeviceColorFormat::Rgb,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let frame_interval = self.frame_interval();
        let max_fps = if frame_interval.is_zero() {
            0
        } else {
            let nanos = frame_interval.as_nanos();
            if nanos == 0 {
                0
            } else {
                let one_second_nanos = 1_000_000_000_u128;
                u32::try_from(one_second_nanos / nanos).unwrap_or(u32::MAX)
            }
        };

        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: true,
            max_fps,
        }
    }

    fn total_leds(&self) -> u32 {
        match self.matrix_type {
            RazerMatrixType::None => 1,
            RazerMatrixType::Linear => {
                if self.matrix_size.1 > 0 {
                    u32::from(self.matrix_size.1)
                } else {
                    1
                }
            }
            RazerMatrixType::Standard
            | RazerMatrixType::Extended
            | RazerMatrixType::ExtendedArgb => {
                u32::from(self.matrix_size.0) * u32::from(self.matrix_size.1)
            }
        }
    }

    fn frame_interval(&self) -> Duration {
        match self.matrix_type {
            RazerMatrixType::None => Duration::from_millis(1),
            RazerMatrixType::Linear => Duration::from_millis(2),
            RazerMatrixType::Standard
            | RazerMatrixType::Extended
            | RazerMatrixType::ExtendedArgb => {
                let rows = u64::from(self.matrix_size.0.max(1));
                Duration::from_millis(rows + 1)
            }
        }
    }
}
