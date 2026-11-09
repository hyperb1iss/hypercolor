//! Pure Wooting per-key RGB encoder (Spec 90 §4 to §7).

use std::borrow::Cow;
use std::cmp::min;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFeatures, DeviceTopologyHint,
    SegmentInfo,
};
use tracing::warn;
use zerocopy::{FromZeros, IntoBytes};

use crate::protocol::{
    CommandBuffer, FrameBatch, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse,
    ResponseStatus, TransferType,
};

use super::packet::{
    FeatureCommand, V1ColorReport, V2ColorReport, V2SmallChunk, V3ColorReport, fill_matrix,
};
use super::types::{
    COMMAND_COLOR_INIT, COMMAND_RAW_COLORS, COMMAND_RESET_ALL, MAGIC_HEAD, MAGIC_HEAD_MULTI_REPORT,
    MAGIC_TAIL, MULTI_REPORT_COLOR_REPORT_ID, MULTI_REPORT_FEATURE_REPORT_ID, UNNUMBERED_REPORT_ID,
    V1_PART_PAYLOAD_LEN, V2_SMALL_CHUNK_COUNT, V2_SMALL_CHUNK_PAYLOAD, WootingInterface,
    WootingModel,
};
use super::v1::{self, MAX_PARTS};

/// Settle time after the colour-init command, before the first frame.
pub const COLOR_INIT_SETTLE: Duration = Duration::from_millis(50);

/// Frame interval on v2 and v3 interfaces: one report (or four small ones)
/// per frame.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Frame interval on v1 interfaces, which take four or five checksummed
/// reports per frame from an 8-bit controller.
pub const V1_FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Pure encoder for one Wooting keyboard or keypad.
#[derive(Debug, Clone, Copy)]
pub struct WootingProtocol {
    model: WootingModel,
    interface: WootingInterface,
}

impl WootingProtocol {
    /// An encoder for `model` behind `interface`.
    ///
    /// Only the One and Two ever shipped v1 firmware; other models paired
    /// with [`WootingInterface::V1`] encode whatever their matrix cells map
    /// to in the v1 LED table.
    #[must_use]
    pub const fn new(model: WootingModel, interface: WootingInterface) -> Self {
        Self { model, interface }
    }

    /// The model this encoder drives.
    #[must_use]
    pub const fn model(&self) -> WootingModel {
        self.model
    }

    /// The wire generation this encoder speaks.
    #[must_use]
    pub const fn interface(&self) -> WootingInterface {
        self.interface
    }

    fn led_count(&self) -> usize {
        self.model.rows() * self.model.cols()
    }

    fn feature_command(&self, command: u8) -> FeatureCommand {
        match self.interface {
            WootingInterface::V1 | WootingInterface::V2 { .. } => {
                FeatureCommand::new(UNNUMBERED_REPORT_ID, MAGIC_HEAD, command)
            }
            WootingInterface::V3 => FeatureCommand::new(
                MULTI_REPORT_FEATURE_REPORT_ID,
                MAGIC_HEAD_MULTI_REPORT,
                command,
            ),
        }
    }

    fn feature(&self, command: u8, post_delay: Duration) -> ProtocolCommand {
        ProtocolCommand {
            data: self.feature_command(command).as_bytes().to_vec(),
            post_delay,
            transfer_type: TransferType::HidReport,
            ..ProtocolCommand::default()
        }
    }

    fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        let expected = self.led_count();
        if colors.len() == expected {
            return Cow::Borrowed(colors);
        }

        warn!(
            expected,
            actual = colors.len(),
            model = ?self.model,
            "wooting frame length mismatch; applying truncate/pad"
        );
        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = min(colors.len(), expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);
        Cow::Owned(normalized)
    }

    fn encode_v1(&self, colors: &[[u8; 3]], buffer: &mut CommandBuffer<'_>) {
        let mut parts = [[0_u8; V1_PART_PAYLOAD_LEN]; MAX_PARTS];
        v1::fill_parts(
            &mut parts,
            colors,
            self.model.first_row(),
            self.model.rows(),
            self.model.cols(),
            self.model.v1_led_index_max(),
        );

        for (part, pwm) in parts.iter().enumerate().take(self.model.v1_part_count()) {
            let (chip, register) = v1::part_address(part);
            let mut report = V1ColorReport::new_zeroed();
            report.report_id = UNNUMBERED_REPORT_ID;
            report.magic_head = MAGIC_HEAD;
            report.magic_tail = MAGIC_TAIL;
            report.command = COMMAND_RAW_COLORS;
            report.chip = chip;
            report.register = register;
            report.pwm = *pwm;
            report.seal();
            buffer.push_struct(
                &report,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
        }
    }

    fn encode_v2(&self, colors: &[[u8; 3]], small_packets: bool, buffer: &mut CommandBuffer<'_>) {
        let mut report = V2ColorReport::new_zeroed();
        report.report_id = UNNUMBERED_REPORT_ID;
        report.magic_head = MAGIC_HEAD;
        report.magic_tail = MAGIC_TAIL;
        report.command = COMMAND_RAW_COLORS;
        fill_matrix(
            &mut report.matrix,
            colors,
            self.model.first_row(),
            self.model.rows(),
            self.model.cols(),
        );

        if !small_packets {
            buffer.push_struct(
                &report,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
            return;
        }

        // The 256 bytes after the report ID go out as four 64-byte reports,
        // each with its own report ID byte in front.
        let body = &report.as_bytes()[1..];
        let (chunks, _) = body.as_chunks::<V2_SMALL_CHUNK_PAYLOAD>();
        for chunk in chunks.iter().take(V2_SMALL_CHUNK_COUNT) {
            let small = V2SmallChunk {
                report_id: UNNUMBERED_REPORT_ID,
                payload: *chunk,
            };
            buffer.push_struct(
                &small,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
        }
    }

    fn encode_v3(&self, colors: &[[u8; 3]], buffer: &mut CommandBuffer<'_>) {
        let mut report = V3ColorReport::new_zeroed();
        report.report_id = MULTI_REPORT_COLOR_REPORT_ID;
        report.magic_head = MAGIC_HEAD_MULTI_REPORT;
        report.magic_tail = MAGIC_TAIL;
        report.command = COMMAND_RAW_COLORS;
        fill_matrix(
            &mut report.matrix,
            colors,
            self.model.first_row(),
            self.model.rows(),
            self.model.cols(),
        );
        buffer.push_struct(
            &report,
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        );
    }
}

impl Protocol for WootingProtocol {
    fn name(&self) -> &'static str {
        match self.interface {
            WootingInterface::V1 => "Wooting RGB (v1)",
            WootingInterface::V2 { .. } => "Wooting RGB (v2)",
            WootingInterface::V3 => "Wooting RGB (multi-report)",
        }
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![self.feature(COMMAND_COLOR_INIT, COLOR_INIT_SETTLE)]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        vec![self.feature(COMMAND_RESET_ALL, Duration::ZERO)]
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(
        &self,
        colors: &[[u8; 3]],
        commands: &mut Vec<ProtocolCommand>,
    ) -> FrameBatch {
        let colors = self.normalize_colors(colors);
        let mut buffer = CommandBuffer::new(commands);
        match self.interface {
            WootingInterface::V1 => self.encode_v1(&colors, &mut buffer),
            WootingInterface::V2 { small_packets } => {
                self.encode_v2(&colors, small_packets, &mut buffer);
            }
            WootingInterface::V3 => self.encode_v3(&colors, &mut buffer),
        }
        buffer.finish();
        FrameBatch::from_commands(commands)
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        // No command reads a reply; anything the transport surfaces is
        // passed through untouched.
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        vec![SegmentInfo {
            name: self.model.zone_name().to_owned(),
            led_count: self.total_leds(),
            topology: DeviceTopologyHint::Matrix {
                rows: u32::try_from(self.model.rows()).unwrap_or(u32::MAX),
                cols: u32::try_from(self.model.cols()).unwrap_or(u32::MAX),
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let interval_ms = self.frame_interval().as_millis().max(1);
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: u32::try_from(1000 / interval_ms).unwrap_or(u32::MAX),
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        u32::try_from(self.led_count()).unwrap_or(u32::MAX)
    }

    fn frame_interval(&self) -> Duration {
        match self.interface {
            WootingInterface::V1 => V1_FRAME_INTERVAL,
            WootingInterface::V2 { .. } | WootingInterface::V3 => FRAME_INTERVAL,
        }
    }
}
