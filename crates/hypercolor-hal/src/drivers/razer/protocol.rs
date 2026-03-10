//! Pure Razer protocol encoder/decoder.

use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};
use tracing::warn;

use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
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
const STANDARD_MATRIX_FRAME_DATA_SIZE: u8 = 0x46;
// Modern custom-effect activation declares a 6-byte payload even though the
// meaningful arguments only consume 5 bytes.
const EXTENDED_CUSTOM_EFFECT_DATA_SIZE: u8 = 0x06;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustomEffectActivationStyle {
    MatchCommandSet,
    LegacyStandard {
        storage: u8,
    },
    StandardLedEffect {
        storage: u8,
        led_id: u8,
        effect: u8,
    },
    ExtendedMatrix {
        declared_data_size: u8,
        args: [u8; 5],
        args_len: u8,
    },
}

/// Pure packet encoder/decoder for Razer HID reports.
#[expect(
    clippy::struct_excessive_bools,
    reason = "protocol quirk flags are independent device capability switches"
)]
#[derive(Debug, Clone)]
pub struct RazerProtocol {
    version: RazerProtocolVersion,
    command_set: RazerLightingCommandSet,
    matrix_type: RazerMatrixType,
    matrix_size: (u8, u8),
    reported_matrix_size: Option<(u8, u8)>,
    led_id: u8,
    brightness_led_id: u8,
    sends_device_mode_commands: bool,
    mode_command_expects_response: bool,
    standard_storage: u8,
    frame_transaction_id: Option<u8>,
    keepalive_interval: Option<Duration>,
    response_delay: Duration,
    activate_custom_effect_in_init: bool,
    activation_style: CustomEffectActivationStyle,
    frame_commands_expect_response: bool,
    activation_expects_response: bool,
    activation_post_delay: Duration,
    supports_brightness: bool,
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
            brightness_led_id: led_id,
            sends_device_mode_commands: true,
            mode_command_expects_response: false,
            standard_storage: NOSTORE,
            frame_transaction_id: None,
            keepalive_interval: None,
            response_delay: Duration::ZERO,
            activate_custom_effect_in_init: false,
            activation_style: CustomEffectActivationStyle::MatchCommandSet,
            frame_commands_expect_response: true,
            activation_expects_response: true,
            activation_post_delay: Duration::ZERO,
            supports_brightness: true,
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

    /// Read and validate responses from `SET_DEVICE_MODE`.
    #[must_use]
    pub const fn with_acknowledged_device_mode_commands(mut self) -> Self {
        self.mode_command_expects_response = true;
        self
    }

    /// Override the storage byte used by standard LED/effect commands.
    #[must_use]
    pub const fn with_standard_storage(mut self, standard_storage: u8) -> Self {
        self.standard_storage = standard_storage;
        self
    }

    /// Override the LED ID used specifically for brightness commands.
    #[must_use]
    pub const fn with_brightness_led_id(mut self, brightness_led_id: u8) -> Self {
        self.brightness_led_id = brightness_led_id;
        self
    }

    /// Override the transaction ID used for frame upload packets only.
    #[must_use]
    pub const fn with_frame_transaction_id(mut self, frame_transaction_id: u8) -> Self {
        self.frame_transaction_id = Some(frame_transaction_id);
        self
    }

    /// Enable a periodic `GET_DEVICE_MODE` keepalive for devices whose RGB
    /// session times out while idle.
    #[must_use]
    pub const fn with_device_mode_keepalive(mut self, interval: Duration) -> Self {
        self.keepalive_interval = Some(interval);
        self
    }

    /// Wait before reading command responses for devices that acknowledge
    /// writes asynchronously.
    #[must_use]
    pub const fn with_response_delay(mut self, response_delay: Duration) -> Self {
        self.response_delay = response_delay;
        self
    }

    /// Send the custom-frame activation command during initialization instead
    /// of appending it to every rendered frame.
    #[must_use]
    pub const fn with_init_custom_effect(mut self) -> Self {
        self.activate_custom_effect_in_init = true;
        self
    }

    /// Force custom-frame activation to use the legacy standard effect packet.
    ///
    /// Some modern mice, including the Basilisk V3, accept extended matrix frame
    /// uploads but still require the legacy `0x03/0x0A` effect switch to enter
    /// software-controlled custom mode.
    #[must_use]
    pub const fn with_legacy_custom_effect_activation(mut self, storage: u8) -> Self {
        self.activation_style = CustomEffectActivationStyle::LegacyStandard { storage };
        self
    }

    /// Activate software control with a per-LED effect command instead of the
    /// matrix-wide custom-mode packet.
    #[must_use]
    pub const fn with_standard_led_effect_activation(
        mut self,
        storage: u8,
        led_id: u8,
        effect: u8,
    ) -> Self {
        self.activation_style = CustomEffectActivationStyle::StandardLedEffect {
            storage,
            led_id,
            effect,
        };
        self
    }

    /// Override the extended custom-effect payload shape for devices with a
    /// vendor-specific apply packet.
    #[must_use]
    pub const fn with_extended_custom_effect_activation(
        mut self,
        declared_data_size: u8,
        args: [u8; 5],
        args_len: u8,
    ) -> Self {
        self.activation_style = CustomEffectActivationStyle::ExtendedMatrix {
            declared_data_size,
            args,
            args_len,
        };
        self
    }

    /// Stream frame uploads as write-only commands instead of request/response
    /// transactions.
    #[must_use]
    pub const fn with_write_only_frame_uploads(mut self) -> Self {
        self.frame_commands_expect_response = false;
        self
    }

    /// Send the custom-effect activation as a fire-and-forget config write.
    #[must_use]
    pub const fn with_write_only_custom_effect_activation(mut self, post_delay: Duration) -> Self {
        self.activation_expects_response = false;
        self.activation_post_delay = post_delay;
        self
    }

    /// Disable brightness support for devices that only expose direct color and
    /// effect control.
    #[must_use]
    pub const fn without_brightness(mut self) -> Self {
        self.supports_brightness = false;
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
            (RazerProtocolVersion::Special08, RazerLightingCommandSet::Extended) => {
                "Razer 0x08 Extended"
            }
            (RazerProtocolVersion::Special08, RazerLightingCommandSet::Standard) => {
                "Razer 0x08 Standard"
            }
            (RazerProtocolVersion::KrakenV4, RazerLightingCommandSet::Extended) => {
                "Razer 0x60 Extended"
            }
            (RazerProtocolVersion::KrakenV4, RazerLightingCommandSet::Standard) => {
                "Razer 0x60 Standard"
            }
        }
    }

    fn mode_command(&self, mode: u8, post_delay: Duration) -> Option<ProtocolCommand> {
        self.build_packet(
            0x00,
            0x04,
            &[mode, 0x00],
            self.mode_command_expects_response,
            post_delay,
        )
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
            | RazerMatrixType::ExtendedArgb => 22,
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
        self.build_packet_with_options(
            self.version.transaction_id(),
            command_class,
            command_id,
            args,
            None,
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
        self.build_packet_with_options(
            transaction_id,
            command_class,
            command_id,
            args,
            None,
            expects_response,
            post_delay,
        )
    }

    fn build_packet_with_declared_size(
        &self,
        command_class: u8,
        command_id: u8,
        args: &[u8],
        declared_data_size: u8,
        expects_response: bool,
        post_delay: Duration,
    ) -> Option<ProtocolCommand> {
        self.build_packet_with_options(
            self.version.transaction_id(),
            command_class,
            command_id,
            args,
            Some(declared_data_size),
            expects_response,
            post_delay,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::unused_self)]
    fn build_packet_with_options(
        &self,
        transaction_id: u8,
        command_class: u8,
        command_id: u8,
        args: &[u8],
        declared_data_size: Option<u8>,
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

        let data_size = declared_data_size.unwrap_or_else(|| u8::try_from(args.len()).unwrap_or(0));
        if usize::from(data_size) > ARGS_LEN {
            warn!(
                data_size,
                "razer command declared data size exceeds argument field, dropping packet"
            );
            return None;
        }

        if args.len() > usize::from(data_size) {
            warn!(
                args_len = args.len(),
                data_size, "razer command arguments exceed declared data size, dropping packet"
            );
            return None;
        }

        let mut packet = [0_u8; RAZER_REPORT_LEN];
        packet[1] = transaction_id;
        packet[DATA_SIZE_OFFSET] = data_size;
        packet[COMMAND_CLASS_OFFSET] = command_class;
        packet[COMMAND_ID_OFFSET] = command_id;
        packet[ARGS_OFFSET..ARGS_OFFSET + args.len()].copy_from_slice(args);
        packet[CRC_OFFSET] = razer_crc(&packet);

        Some(ProtocolCommand {
            data: packet.to_vec(),
            expects_response,
            response_delay: if expects_response {
                self.response_delay
            } else {
                Duration::ZERO
            },
            post_delay,
            transfer_type: TransferType::Primary,
        })
    }

    fn get_device_mode_command(&self) -> Option<ProtocolCommand> {
        self.build_packet_with_declared_size(0x00, 0x84, &[], 0x02, true, Duration::ZERO)
    }

    fn serial_query_command(&self) -> Option<ProtocolCommand> {
        self.build_packet_with_declared_size(0x00, 0x82, &[], 0x02, true, Duration::ZERO)
    }

    fn activation_command(&self) -> Option<ProtocolCommand> {
        match self.activation_style {
            CustomEffectActivationStyle::LegacyStandard { storage } => self.build_packet(
                0x03,
                0x0A,
                &[0x05, storage],
                self.activation_expects_response,
                self.activation_post_delay,
            ),
            CustomEffectActivationStyle::StandardLedEffect {
                storage,
                led_id,
                effect,
            } => self.build_packet(
                0x03,
                0x02,
                &[storage, led_id, effect],
                self.activation_expects_response,
                self.activation_post_delay,
            ),
            CustomEffectActivationStyle::ExtendedMatrix {
                declared_data_size,
                args,
                args_len,
            } => {
                let args_len = min(usize::from(args_len), args.len());
                self.build_packet_with_declared_size(
                    0x0F,
                    0x02,
                    &args[..args_len],
                    declared_data_size,
                    self.activation_expects_response,
                    self.activation_post_delay,
                )
            }
            CustomEffectActivationStyle::MatchCommandSet
                if matches!(self.command_set, RazerLightingCommandSet::Standard) =>
            {
                self.build_packet(
                    0x03,
                    0x0A,
                    &[0x05, self.standard_storage],
                    self.activation_expects_response,
                    self.activation_post_delay,
                )
            }
            CustomEffectActivationStyle::MatchCommandSet => self.build_packet_with_declared_size(
                0x0F,
                0x02,
                &[NOSTORE, LED_ID_ZERO, EFFECT_CUSTOM_FRAME, 0x00, 0x01],
                EXTENDED_CUSTOM_EFFECT_DATA_SIZE,
                self.activation_expects_response,
                self.activation_post_delay,
            ),
        }
    }

    fn should_append_frame_activation(&self) -> bool {
        !self.activate_custom_effect_in_init
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

        self.build_packet(command_class, command_id, &args, true, Duration::ZERO)
            .map(|mut command| {
                command.expects_response = self.frame_commands_expect_response;
                if !self.frame_commands_expect_response {
                    command.response_delay = Duration::ZERO;
                }
                command
            })
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
            self.frame_commands_expect_response,
            Duration::from_millis(1),
        ) {
            commands.push(packet);
        }

        if self.should_append_frame_activation()
            && let Some(activation) = self.activation_command()
        {
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

                let packet = match self.command_set {
                    RazerLightingCommandSet::Standard => self.build_packet_with_options(
                        frame_transaction_id,
                        command_class,
                        command_id,
                        &args,
                        Some(STANDARD_MATRIX_FRAME_DATA_SIZE),
                        self.frame_commands_expect_response,
                        Duration::from_millis(1),
                    ),
                    RazerLightingCommandSet::Extended => self.build_packet_with_options(
                        frame_transaction_id,
                        command_class,
                        command_id,
                        &args,
                        None,
                        self.frame_commands_expect_response,
                        Duration::from_millis(1),
                    ),
                };

                if let Some(packet) = packet {
                    commands.push(packet);
                }
            }
        }

        if self.should_append_frame_activation()
            && let Some(activation) = self.activation_command()
        {
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
    fn name(&self) -> &'static str {
        self.protocol_name()
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();

        if self.sends_device_mode_commands
            && let Some(command) = self.mode_command(0x03, Duration::from_millis(7))
        {
            commands.push(command);
        }

        if self.activate_custom_effect_in_init
            && let Some(command) = self.activation_command()
        {
            commands.push(command);
        }

        commands
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
        if !self.supports_brightness {
            return None;
        }

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
            &[storage, self.brightness_led_id, brightness],
            true,
            Duration::ZERO,
        )
        .map(|command| vec![command])
    }

    fn connection_diagnostics(&self) -> Vec<ProtocolCommand> {
        let init_has_response = self.activation_expects_response
            || (self.sends_device_mode_commands && self.mode_command_expects_response);
        if init_has_response || self.frame_commands_expect_response {
            return Vec::new();
        }

        self.serial_query_command().into_iter().collect()
    }

    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        let interval = self.keepalive_interval?;
        let command = self.get_device_mode_command()?;

        Some(ProtocolKeepalive {
            commands: vec![command],
            interval,
        })
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
            supports_brightness: self.supports_brightness,
            has_display: false,
            display_resolution: None,
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
