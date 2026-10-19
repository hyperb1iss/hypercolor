//! ASUS Aura ENE `SMBus` protocol helpers.

use hypercolor_types::device::SegmentInfo;

use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

use tracing::warn;

use crate::transport::TransportError;
use crate::transport::smbus::{SmBusOperation, decode_operations, encode_operations};

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceFeatures, DeviceTopologyHint,
};

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolResponse, ResponseStatus,
    TransferType,
};

/// ENE indirect address register.
pub const ENE_ADDRESS_REGISTER: u8 = 0x00;

/// ENE indirect write-data register.
pub const ENE_WRITE_REGISTER: u8 = 0x01;

/// ENE indirect block-write register.
pub const ENE_BLOCK_WRITE_REGISTER: u8 = 0x03;

/// ENE indirect read-data register.
pub const ENE_READ_REGISTER: u8 = 0x81;

/// ENE apply register value for transient updates.
pub const ENE_APPLY_VAL: u8 = 0x01;

/// ENE apply register value for persistent writes.
pub const ENE_SAVE_VAL: u8 = 0xAA;

/// ENE direct-mode register.
pub const ENE_DIRECT_MODE_REGISTER: u16 = 0x8020;

/// ENE mode register.
pub const ENE_MODE_REGISTER: u16 = 0x8021;

/// ENE speed register.
pub const ENE_SPEED_REGISTER: u16 = 0x8022;

/// ENE direction register.
pub const ENE_DIRECTION_REGISTER: u16 = 0x8023;

/// ENE apply/save register.
pub const ENE_APPLY_REGISTER: u16 = 0x80A0;

/// ENE DRAM frame-apply register used by Aura DIMMs.
pub const ENE_DRAM_COLOR_APPLY_REGISTER: u16 = 0x802F;

/// ENE DRAM remap slot-index register.
pub const ENE_DRAM_SLOT_INDEX_REGISTER: u16 = 0x80F8;

/// ENE DRAM remap target-address register.
pub const ENE_DRAM_I2C_ADDRESS_REGISTER: u16 = 0x80F9;

/// ENE maximum `SMBus` block-write payload.
pub const ENE_BLOCK_WRITE_LIMIT: usize = 3;

/// Bytes of one LED in the direct-color register range.
const ENE_LED_BYTES: usize = 3;

const _: () = assert!(
    ENE_LED_BYTES == ENE_BLOCK_WRITE_LIMIT,
    "per-LED direct writes rely on one block chunk carrying exactly one LED"
);

/// ENE simple GPU magic word.
pub const AURA_GPU_MAGIC: u16 = 0x1589;

/// Delay between `SMBus` operations recommended by the spec.
pub const ENE_OPERATION_DELAY: Duration = Duration::from_millis(1);

/// Frames between full writes while the controller confirms every frame.
///
/// A controller that resets without failing a write, across host sleep or
/// when other software takes it over, would otherwise keep stale colors on
/// every LED a frame leaves unchanged. Every this-many frames the shown
/// colors are treated as unknown and every LED is written again.
pub const ENE_FULL_WRITE_INTERVAL: u32 = 60;

const ENE_FIRMWARE_NAME_LEN: usize = 16;
const ENE_CONFIG_TABLE_LEN: usize = 64;

/// ENE firmware register layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EneFirmwareVariant {
    /// Direct-mode color register base.
    pub direct_reg: u16,
    /// Effect-mode color register base.
    pub effect_reg: u16,
    /// Config-table offset of the channel-id list.
    pub channel_cfg_offset: usize,
    /// Config-table offset of the LED-count byte.
    pub led_count_offset: usize,
    /// Optional register that must be poked after one direct frame upload.
    pub frame_apply_register: Option<u16>,
    /// Whether this firmware can expose mode 14.
    pub may_support_mode_14: bool,
}

#[derive(Debug, Clone, Default)]
struct AuraSmBusState {
    firmware_name: Option<String>,
    variant: Option<EneFirmwareVariant>,
    led_count: u32,
    supports_mode_14: bool,
}

/// What the controller is known to show, so a frame writes only the LEDs
/// it changes.
///
/// Anything that leaves the shown colors in doubt forgets them: a failed or
/// unreported write, the init sequence, and every
/// [`ENE_FULL_WRITE_INTERVAL`]th frame. A command that changes what the
/// controller displays outside a frame, such as a mode or brightness write,
/// has to forget them too.
#[derive(Debug, Default)]
struct FrameBaseline {
    /// Colors the controller last accepted, `None` when unknown.
    shown: Option<Vec<[u8; 3]>>,
    /// The frame most recently encoded, until its write is reported.
    staged: Option<Vec<[u8; 3]>>,
    /// Pixel bytes the most recently encoded frame writes.
    written_frame_bytes: usize,
    /// Frames encoded since the last one that started from unknown colors.
    frames_since_full_write: u32,
}

impl FrameBaseline {
    fn forget(&mut self) {
        self.shown = None;
        self.staged = None;
    }

    /// Start encoding a frame. A staged frame nobody reported on may have
    /// reached the controller in part, so the shown colors become unknown.
    fn begin_frame(&mut self) {
        if self.staged.take().is_some() {
            self.shown = None;
        }
        self.frames_since_full_write = self.frames_since_full_write.saturating_add(1);
        if self.frames_since_full_write >= ENE_FULL_WRITE_INTERVAL {
            self.shown = None;
        }
        if self.shown.is_none() {
            self.frames_since_full_write = 0;
        }
        self.written_frame_bytes = 0;
    }

    fn needs_write(&self, colors: &[[u8; 3]], index: usize) -> bool {
        self.shown
            .as_ref()
            .filter(|shown| shown.len() == colors.len())
            .is_none_or(|shown| shown.get(index) != colors.get(index))
    }

    fn finish(&mut self, succeeded: bool) {
        match (succeeded, self.staged.take()) {
            (true, Some(staged)) => self.shown = Some(staged),
            (true, None) => {}
            (false, _) => self.shown = None,
        }
    }
}

/// Byte-swap a 16-bit ENE register address for the indirect address port.
#[must_use]
pub const fn ene_byte_swap(register: u16) -> u16 {
    register.rotate_left(8)
}

/// Rearrange RGB into ENE's RBG wire order.
#[must_use]
pub const fn ene_permute_color(r: u8, g: u8, b: u8) -> [u8; 3] {
    [r, b, g]
}

/// Serialize one ENE transaction into the shared `SMBus` transport framing.
///
/// # Errors
///
/// Returns [`ProtocolError`] if an operation cannot be represented by the
/// shared transport format.
pub fn encode_ene_transaction(operations: &[SmBusOperation]) -> Result<Vec<u8>, ProtocolError> {
    encode_operations(operations).map_err(|error| protocol_encoding_error(&error))
}

/// Decode one serialized ENE transaction.
///
/// # Errors
///
/// Returns [`ProtocolError`] when the frame is malformed.
pub fn decode_ene_transaction(data: &[u8]) -> Result<Vec<SmBusOperation>, ProtocolError> {
    decode_operations(data).map_err(|error| protocol_malformed_response(&error))
}

fn protocol_encoding_error(error: &TransportError) -> ProtocolError {
    ProtocolError::EncodingError {
        detail: error.to_string(),
    }
}

fn protocol_malformed_response(error: &TransportError) -> ProtocolError {
    ProtocolError::MalformedResponse {
        detail: error.to_string(),
    }
}

fn decode_ene_firmware_name(data: &[u8]) -> Result<String, ProtocolError> {
    let filtered = data
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .filter(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b' '))
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        return Err(ProtocolError::MalformedResponse {
            detail: "ENE firmware name did not contain any ASCII identifier bytes".to_owned(),
        });
    }

    let firmware =
        String::from_utf8(filtered).map_err(|error| ProtocolError::MalformedResponse {
            detail: format!("ENE firmware name could not be normalized into UTF-8: {error}"),
        })?;
    let firmware = firmware.trim().to_owned();

    if firmware.is_empty() {
        return Err(ProtocolError::MalformedResponse {
            detail: "ENE firmware name normalized to an empty string".to_owned(),
        });
    }

    Ok(firmware)
}

/// Resolve one known ENE firmware string to its register layout.
#[must_use]
pub fn lookup_ene_firmware_variant(name: &str) -> Option<EneFirmwareVariant> {
    match name {
        "LED-0116" | "AUMA0-E8K4-0101" => Some(EneFirmwareVariant {
            direct_reg: 0x8000,
            effect_reg: 0x8010,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
            frame_apply_register: None,
            may_support_mode_14: false,
        }),
        "AUMA0-E6K5-0104" | "AUMA0-E6K5-0105" | "AUMA0-E6K5-0106" | "AUMA0-E6K5-0107"
        | "AUMA0-E6K5-1107" | "AUMA0-E6K5-1110" | "AUMA0-E6K5-1111" | "AUMA0-E6K5-1113" => {
            Some(EneFirmwareVariant {
                direct_reg: 0x8100,
                effect_reg: 0x8160,
                channel_cfg_offset: 0x1B,
                led_count_offset: if name == "AUMA0-E6K5-0107"
                    || name == "AUMA0-E6K5-1107"
                    || name == "AUMA0-E6K5-1110"
                    || name == "AUMA0-E6K5-1111"
                    || name == "AUMA0-E6K5-1113"
                {
                    0x03
                } else {
                    0x02
                },
                frame_apply_register: None,
                may_support_mode_14: false,
            })
        }
        "AUMA0-E6K5-0008" => Some(EneFirmwareVariant {
            direct_reg: 0x8100,
            effect_reg: 0x8160,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x03,
            frame_apply_register: None,
            may_support_mode_14: false,
        }),
        "DIMM_LED-0102" => Some(EneFirmwareVariant {
            direct_reg: 0x8000,
            effect_reg: 0x8010,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
            frame_apply_register: Some(ENE_DRAM_COLOR_APPLY_REGISTER),
            may_support_mode_14: false,
        }),
        "AUDA0-E6K5-0101" => Some(EneFirmwareVariant {
            direct_reg: 0x8100,
            effect_reg: 0x8160,
            channel_cfg_offset: 0x13,
            led_count_offset: 0x02,
            frame_apply_register: Some(ENE_DRAM_COLOR_APPLY_REGISTER),
            may_support_mode_14: true,
        }),
        _ => None,
    }
}

/// Whether the config-table channel list enables ENE hardware mode 14.
#[must_use]
pub fn supports_mode_14(config_table: &[u8], variant: EneFirmwareVariant) -> bool {
    variant.may_support_mode_14
        && config_table
            .get(variant.channel_cfg_offset..)
            .is_some_and(|channels| channels.contains(&0x0E))
}

/// Build the indirect-read sequence for one ENE register range.
///
/// Returns an empty vec if `len` is zero or if the requested range would
/// overflow the 16-bit ENE address space. The overflow case is logged at
/// warn level so a misconfigured caller is observable without panicking.
#[must_use]
pub fn ene_read_register_range(register: u16, len: usize) -> Vec<SmBusOperation> {
    if len == 0 {
        return Vec::new();
    }

    let Ok(len_u16) = u16::try_from(len) else {
        warn!(
            register = %format_args!("{register:#06X}"),
            len,
            "ENE register read length exceeds u16::MAX; dropping request"
        );
        return Vec::new();
    };

    let Some(last_offset) = len_u16.checked_sub(1) else {
        return Vec::new();
    };

    if register.checked_add(last_offset).is_none() {
        warn!(
            register = %format_args!("{register:#06X}"),
            len,
            "ENE register read range overflows u16::MAX; dropping request"
        );
        return Vec::new();
    }

    let mut operations = Vec::with_capacity(len.saturating_mul(3));

    for offset in 0..len_u16 {
        let current = register + offset;
        operations.push(SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(current),
        });
        operations.push(SmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        });
        operations.push(SmBusOperation::ReadByteData {
            register: ENE_READ_REGISTER,
        });
    }

    operations
}

/// Build the indirect-write sequence for one ENE register byte.
#[must_use]
pub fn ene_write_register(register: u16, value: u8) -> Vec<SmBusOperation> {
    vec![
        SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(register),
        },
        SmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        },
        SmBusOperation::WriteByteData {
            register: ENE_WRITE_REGISTER,
            value,
        },
    ]
}

/// Build the indirect block-write sequence for an ENE register range.
///
/// Returns an empty vec if `data` is empty or if the requested range would
/// overflow the 16-bit ENE address space. The overflow case is logged at
/// warn level so a misconfigured caller is observable without panicking.
#[must_use]
pub fn ene_write_register_block(register: u16, data: &[u8]) -> Vec<SmBusOperation> {
    if data.is_empty() {
        return Vec::new();
    }

    let chunk_count = data.len().div_ceil(ENE_BLOCK_WRITE_LIMIT);
    let max_offset_usize = (chunk_count - 1).saturating_mul(ENE_BLOCK_WRITE_LIMIT);

    let Ok(max_offset) = u16::try_from(max_offset_usize) else {
        warn!(
            register = %format_args!("{register:#06X}"),
            data_len = data.len(),
            "ENE block write length exceeds u16::MAX; dropping request"
        );
        return Vec::new();
    };

    if register.checked_add(max_offset).is_none() {
        warn!(
            register = %format_args!("{register:#06X}"),
            data_len = data.len(),
            "ENE block write range overflows u16::MAX; dropping request"
        );
        return Vec::new();
    }

    let mut operations = Vec::with_capacity(chunk_count.saturating_mul(3));

    for (chunk_index, chunk) in data.chunks(ENE_BLOCK_WRITE_LIMIT).enumerate() {
        let chunk_offset = u16::try_from(chunk_index * ENE_BLOCK_WRITE_LIMIT).unwrap_or(max_offset);
        operations.push(SmBusOperation::WriteWordData {
            register: ENE_ADDRESS_REGISTER,
            value: ene_byte_swap(register + chunk_offset),
        });
        operations.push(SmBusOperation::Delay {
            duration: ENE_OPERATION_DELAY,
        });
        operations.push(SmBusOperation::WriteBlockData {
            register: ENE_BLOCK_WRITE_REGISTER,
            data: chunk.to_vec(),
        });
    }

    operations
}

/// Build the direct-color write for one LED using the firmware's register
/// layout.
///
/// Matches the chunk that LED gets in [`ene_direct_color_writes`]. Returns an
/// empty vec, logged at warn level, if the LED's register would overflow the
/// 16-bit ENE address space.
#[must_use]
pub fn ene_direct_led_write(
    variant: EneFirmwareVariant,
    index: usize,
    color: [u8; 3],
) -> Vec<SmBusOperation> {
    let register = index
        .checked_mul(ENE_LED_BYTES)
        .and_then(|offset| u16::try_from(offset).ok())
        .and_then(|offset| variant.direct_reg.checked_add(offset));
    let Some(register) = register else {
        warn!(
            direct_reg = %format_args!("{:#06X}", variant.direct_reg),
            index,
            "ENE LED register overflows u16::MAX; dropping write"
        );
        return Vec::new();
    };

    ene_write_register_block(register, &ene_permute_color(color[0], color[1], color[2]))
}

/// Build a direct-color write sequence using the firmware's register layout.
#[must_use]
pub fn ene_direct_color_writes(
    variant: EneFirmwareVariant,
    colors: &[[u8; 3]],
) -> Vec<SmBusOperation> {
    let payload = colors
        .iter()
        .flat_map(|color| ene_permute_color(color[0], color[1], color[2]))
        .collect::<Vec<_>>();

    ene_write_register_block(variant.direct_reg, &payload)
}

/// Build the DRAM remap write sequence for one slot/address pair.
#[must_use]
pub fn ene_dram_remap_sequence(slot_index: u8, target_address: u8) -> Vec<SmBusOperation> {
    let mut operations = ene_write_register(ENE_DRAM_SLOT_INDEX_REGISTER, slot_index);
    operations.extend(ene_write_register(
        ENE_DRAM_I2C_ADDRESS_REGISTER,
        target_address << 1,
    ));
    operations
}

/// Build the simple Aura GPU magic word from the two probe registers.
#[must_use]
pub const fn simple_gpu_magic(hi: u8, lo: u8) -> u16 {
    u16::from_be_bytes([hi, lo])
}

/// ASUS Aura ENE `SMBus` protocol with runtime firmware/config parsing.
///
/// Frames write only the LEDs that changed since the last frame the
/// controller accepted, which the backend confirms through
/// [`Protocol::frame_write_finished`]. Without that confirmation every frame
/// is written in full, and DRAM frames re-assert direct mode.
pub struct AuraSmBusProtocol {
    state: RwLock<AuraSmBusState>,
    baseline: Mutex<FrameBaseline>,
}

impl AuraSmBusProtocol {
    /// Create a fresh ENE `SMBus` protocol instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RwLock::new(AuraSmBusState::default()),
            baseline: Mutex::new(FrameBaseline::default()),
        }
    }

    fn baseline(&self) -> std::sync::MutexGuard<'_, FrameBaseline> {
        self.baseline.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn command(operations: &[SmBusOperation], expects_response: bool) -> ProtocolCommand {
        ProtocolCommand {
            data: encode_ene_transaction(operations)
                .expect("ENE transaction encoding should succeed for built-in operations"),
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Primary,
            ..Default::default()
        }
    }

    /// Snapshot the currently parsed firmware string.
    #[must_use]
    pub fn firmware_name(&self) -> Option<String> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .firmware_name
            .clone()
    }

    /// Snapshot the currently resolved firmware layout.
    #[must_use]
    pub fn firmware_variant(&self) -> Option<EneFirmwareVariant> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .variant
    }

    /// Whether the parsed config table exposes mode 14.
    #[must_use]
    pub fn supports_mode_14(&self) -> bool {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .supports_mode_14
    }
}

impl Default for AuraSmBusProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol for AuraSmBusProtocol {
    fn name(&self) -> &'static str {
        "ASUS Aura ENE SMBus"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        self.baseline().forget();
        vec![
            Self::command(
                &ene_read_register_range(0x1000, ENE_FIRMWARE_NAME_LEN),
                true,
            ),
            Self::command(&ene_read_register_range(0x1C00, ENE_CONFIG_TABLE_LEN), true),
            Self::command(
                &ene_write_register(ENE_DIRECT_MODE_REGISTER, ENE_APPLY_VAL),
                false,
            ),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let mut baseline = self.baseline();
        baseline.begin_frame();
        let Some(variant) = self.firmware_variant() else {
            commands.clear();
            return;
        };

        let mut operations = Vec::new();
        // While the shown colors are unknown (the first frame of a session,
        // after a failed or unreported write, and on the periodic full
        // write) the DIMM may also have fallen out of direct mode, so the
        // frame re-asserts it.
        if variant.frame_apply_register.is_some() && baseline.shown.is_none() {
            operations.extend(ene_write_register(ENE_DIRECT_MODE_REGISTER, ENE_APPLY_VAL));
        }
        let mut written_leds = 0_usize;
        for (index, color) in colors.iter().enumerate() {
            if baseline.needs_write(colors, index) {
                operations.extend(ene_direct_led_write(variant, index, *color));
                written_leds += 1;
            }
        }
        if written_leds > 0
            && let Some(register) = variant.frame_apply_register
        {
            operations.extend(ene_write_register(register, ENE_APPLY_VAL));
        }
        baseline.written_frame_bytes = written_leds * ENE_LED_BYTES;
        let mut command_buffer = CommandBuffer::new(commands);
        if operations.is_empty() {
            command_buffer.finish();
        } else {
            let data = encode_ene_transaction(&operations)
                .expect("ENE transaction encoding should succeed for built-in operations");
            command_buffer.push_slice(
                &data,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
            command_buffer.finish();
            baseline.staged = Some(colors.to_vec());
        }
    }

    fn written_frame_bytes(&self) -> Option<usize> {
        Some(self.baseline().written_frame_bytes)
    }

    fn frame_write_finished(&self, succeeded: bool) {
        self.baseline().finish(succeeded);
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        match data.len() {
            ENE_FIRMWARE_NAME_LEN => {
                let firmware = decode_ene_firmware_name(data)?;
                let variant = lookup_ene_firmware_variant(&firmware);

                let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
                state.firmware_name = Some(firmware.clone());
                state.variant = variant;

                Ok(ProtocolResponse {
                    status: ResponseStatus::Ok,
                    data: firmware.into_bytes(),
                })
            }
            ENE_CONFIG_TABLE_LEN => {
                let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
                let Some(variant) = state.variant else {
                    return Err(ProtocolError::MalformedResponse {
                        detail: "ENE config table arrived before firmware variant was known"
                            .to_owned(),
                    });
                };

                let led_count = data.get(variant.led_count_offset).copied().ok_or_else(|| {
                    ProtocolError::MalformedResponse {
                        detail: "ENE config table missing LED count byte".to_owned(),
                    }
                })?;
                state.led_count = u32::from(led_count);
                state.supports_mode_14 = supports_mode_14(data, variant);

                Ok(ProtocolResponse {
                    status: ResponseStatus::Ok,
                    data: data.to_vec(),
                })
            }
            _ => Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: data.to_vec(),
            }),
        }
    }

    fn zones(&self) -> Vec<SegmentInfo> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);

        if state.led_count == 0 {
            return Vec::new();
        }

        vec![SegmentInfo {
            name: "Lighting".to_owned(),
            led_count: state.led_count,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .led_count
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16)
    }
}
