//! Corsair LCD display streaming protocol.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use hypercolor_types::device::{DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint};

use crate::drivers::corsair::framing::{
    LCD_DATA_PER_PACKET, build_lcd_display_packet, build_lcd_report, pad_to,
};
use crate::protocol::{
    Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse, ProtocolZone,
    ResponseStatus, TransferType,
};

const DEFAULT_TARGET_FPS: u32 = 30;
const LCD_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const STANDARD_LCD_SHUTDOWN: [u8; 8] = [0x03, 0x1E, 0x40, 0x01, 0x43, 0x00, 0x69, 0x00];
const XC7_LCD_SHUTDOWN_PRIMARY: [u8; 7] = [0x03, 0x1E, 0x19, 0x01, 0x04, 0x00, 0xA3];
const XC7_LCD_SHUTDOWN_SECONDARY: [u8; 7] = [0x03, 0x1D, 0x00, 0x01, 0x04, 0x00, 0xA3];
const LCD_VERSION_BYTES: [u8; 7] = [0x32, 0x2E, 0x30, 0x2E, 0x30, 0x2E, 0x33];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CorsairLcdInitMode {
    Standard,
    Xc7,
}

#[derive(Clone, Copy, Debug)]
struct CorsairLcdConfig {
    name: &'static str,
    width: u32,
    height: u32,
    data_zone_byte: u8,
    keepalive_zone_byte: u8,
    circular: bool,
    ring_led_count: u32,
}

/// JPEG streaming protocol for Corsair LCD devices.
pub struct CorsairLcdProtocol {
    name: &'static str,
    width: u32,
    height: u32,
    data_zone_byte: u8,
    keepalive_zone_byte: u8,
    circular: bool,
    ring_led_count: u32,
    init_mode: CorsairLcdInitMode,
    shutdown_reports: Vec<Vec<u8>>,
    last_keepalive_at: RwLock<Option<Instant>>,
}

impl CorsairLcdProtocol {
    /// Create a Corsair LCD protocol instance.
    #[must_use]
    pub fn new(
        name: &'static str,
        width: u32,
        height: u32,
        data_zone_byte: u8,
        keepalive_zone_byte: u8,
        circular: bool,
        ring_led_count: u32,
    ) -> Self {
        Self::with_behavior(
            CorsairLcdConfig {
                name,
                width,
                height,
                data_zone_byte,
                keepalive_zone_byte,
                circular,
                ring_led_count,
            },
            CorsairLcdInitMode::Standard,
            vec![STANDARD_LCD_SHUTDOWN.to_vec()],
        )
    }

    /// Create an XC7 RGB Elite LCD protocol instance.
    #[must_use]
    pub fn new_xc7(name: &'static str) -> Self {
        Self::with_behavior(
            CorsairLcdConfig {
                name,
                width: 480,
                height: 480,
                data_zone_byte: 0x1F,
                keepalive_zone_byte: 0x1C,
                circular: true,
                ring_led_count: 31,
            },
            CorsairLcdInitMode::Xc7,
            vec![
                XC7_LCD_SHUTDOWN_PRIMARY.to_vec(),
                XC7_LCD_SHUTDOWN_SECONDARY.to_vec(),
            ],
        )
    }

    fn with_behavior(
        config: CorsairLcdConfig,
        init_mode: CorsairLcdInitMode,
        shutdown_reports: Vec<Vec<u8>>,
    ) -> Self {
        Self {
            name: config.name,
            width: config.width,
            height: config.height,
            data_zone_byte: config.data_zone_byte,
            keepalive_zone_byte: config.keepalive_zone_byte,
            circular: config.circular,
            ring_led_count: config.ring_led_count,
            init_mode,
            shutdown_reports,
            last_keepalive_at: RwLock::new(None),
        }
    }

    fn hid_report(payload: &[u8], expects_response: bool) -> ProtocolCommand {
        ProtocolCommand {
            data: build_lcd_report(payload),
            expects_response,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::HidReport,
        }
    }

    fn bulk_command(data: Vec<u8>) -> ProtocolCommand {
        ProtocolCommand {
            data,
            expects_response: false,
            response_delay: Duration::ZERO,
            post_delay: Duration::ZERO,
            transfer_type: TransferType::Bulk,
        }
    }

    fn keepalive_due(&self) -> bool {
        self.last_keepalive_at
            .read()
            .expect("LCD keepalive lock should not be poisoned")
            .is_none_or(|last| last.elapsed() >= LCD_KEEPALIVE_INTERVAL)
    }

    fn keepalive_command(
        &self,
        final_packet: u8,
        packets_sent: u8,
        data_length: u16,
    ) -> ProtocolCommand {
        *self
            .last_keepalive_at
            .write()
            .expect("LCD keepalive lock should not be poisoned") = Some(Instant::now());

        Self::hid_report(
            &[
                0x03,
                0x19,
                self.keepalive_zone_byte,
                final_packet,
                packets_sent,
                0x00,
                data_length.to_le_bytes()[0],
                data_length.to_le_bytes()[1],
            ],
            false,
        )
    }

    fn init_device_info_query() -> ProtocolCommand {
        Self::hid_report(&[0x03, 0x1D, 0x01, 0x00], true)
    }

    fn init_status_query() -> ProtocolCommand {
        Self::hid_report(&[0x03, 0x19], true)
    }

    fn init_version_handshake() -> ProtocolCommand {
        let mut payload = vec![0x03, 0x20, 0x00, 0x19, 0x79, 0xE7];
        payload.extend_from_slice(&LCD_VERSION_BYTES);
        Self::hid_report(&payload, true)
    }

    fn init_auth_unlock() -> ProtocolCommand {
        let mut payload = vec![0x03, 0x0B, 0x40, 0x01, 0x79, 0xE7];
        payload.extend_from_slice(&LCD_VERSION_BYTES);
        Self::hid_report(&payload, true)
    }

    fn normalize_ring_colors(&self, colors: &[[u8; 3]]) -> Vec<[u8; 3]> {
        let expected = usize::try_from(self.ring_led_count).unwrap_or_default();
        if expected == 0 {
            return Vec::new();
        }

        let mut normalized = vec![[0_u8; 3]; expected];
        let copy_len = colors.len().min(expected);
        normalized[..copy_len].copy_from_slice(&colors[..copy_len]);
        normalized
    }
}

impl Protocol for CorsairLcdProtocol {
    fn name(&self) -> &'static str {
        self.name
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        let mut commands = vec![Self::init_device_info_query(), Self::init_status_query()];
        if self.init_mode == CorsairLcdInitMode::Standard {
            commands.extend([Self::init_version_handshake(), Self::init_auth_unlock()]);
        }
        commands
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        self.shutdown_reports
            .iter()
            .map(|report| Self::hid_report(report, false))
            .collect()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        if self.ring_led_count == 0 {
            return Vec::new();
        }

        let normalized = self.normalize_ring_colors(colors);
        let mut payload = Vec::with_capacity(normalized.len().saturating_mul(3).saturating_add(3));
        payload.extend_from_slice(&[0x02, 0x07, self.data_zone_byte]);
        payload.extend(
            normalized
                .iter()
                .flat_map(|color| [color[0], color[1], color[2]]),
        );
        vec![Self::bulk_command(pad_to(&payload, 1_024))]
    }

    fn encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
        let chunk_count = jpeg_data.len().div_ceil(LCD_DATA_PER_PACKET);
        let mut commands = jpeg_data
            .chunks(LCD_DATA_PER_PACKET)
            .enumerate()
            .map(|(index, chunk)| {
                Self::bulk_command(build_lcd_display_packet(
                    self.data_zone_byte,
                    index + 1 == chunk_count,
                    u8::try_from(index).unwrap_or(u8::MAX),
                    chunk,
                ))
            })
            .collect::<Vec<_>>();

        if self.keepalive_due() {
            let packets_sent = u8::try_from(chunk_count).unwrap_or(u8::MAX);
            commands.push(self.keepalive_command(
                0x01,
                packets_sent,
                u16::try_from(LCD_DATA_PER_PACKET).unwrap_or(u16::MAX),
            ));
        }

        Some(commands)
    }

    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        Some(ProtocolKeepalive {
            commands: Vec::new(),
            interval: LCD_KEEPALIVE_INTERVAL,
        })
    }

    fn keepalive_commands(&self) -> Vec<ProtocolCommand> {
        if self.keepalive_due() {
            vec![self.keepalive_command(0x01, 0x00, 0x0000)]
        } else {
            Vec::new()
        }
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        let mut zones = vec![ProtocolZone {
            name: "Display".to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width: self.width,
                height: self.height,
                circular: self.circular,
            },
            color_format: DeviceColorFormat::Jpeg,
        }];

        if self.ring_led_count > 0 {
            zones.push(ProtocolZone {
                name: "RGB Ring".to_owned(),
                led_count: self.ring_led_count,
                topology: DeviceTopologyHint::Ring {
                    count: self.ring_led_count,
                },
                color_format: DeviceColorFormat::Rgb,
            });
        }

        zones
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.ring_led_count,
            supports_direct: self.ring_led_count > 0,
            supports_brightness: false,
            has_display: true,
            display_resolution: Some((self.width, self.height)),
            max_fps: DEFAULT_TARGET_FPS,
        }
    }

    fn total_leds(&self) -> u32 {
        self.ring_led_count
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(33)
    }
}
