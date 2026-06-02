use crate::error::{OpenRgbError, Result};
use crate::packet::{CLIENT_MAX_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION};
use crate::types::{
    ColorMode, ControllerData, ControllerMode, ControllerZone, DeviceType, LedData, MatrixMap,
    RgbColor, SegmentData, ZoneType,
};

/// Parse an OpenRGB controller data payload for an approved protocol version.
///
/// # Errors
///
/// Returns an error when the protocol version is unsupported or the payload is
/// malformed.
pub fn parse_controller_data(payload: &[u8], protocol_version: u32) -> Result<ControllerData> {
    if !(MIN_PROTOCOL_VERSION..=CLIENT_MAX_PROTOCOL_VERSION).contains(&protocol_version) {
        return Err(OpenRgbError::UnsupportedProtocolVersion {
            version: protocol_version,
            min: MIN_PROTOCOL_VERSION,
            max: CLIENT_MAX_PROTOCOL_VERSION,
        });
    }

    let mut cursor = Cursor::new(payload);
    let advertised =
        usize::try_from(cursor.read_u32()?).map_err(|_| OpenRgbError::DataSizeMismatch {
            advertised: usize::MAX,
            actual: payload.len(),
        })?;
    if advertised != payload.len() {
        return Err(OpenRgbError::DataSizeMismatch {
            advertised,
            actual: payload.len(),
        });
    }

    let device_type = DeviceType::from_raw(cursor.read_i32()?);
    let name = cursor.read_c_string()?;
    let vendor = cursor.read_c_string()?;
    let description = cursor.read_c_string()?;
    let version = cursor.read_c_string()?;
    let serial = cursor.read_c_string()?;
    let location = cursor.read_c_string()?;

    let mode_count = cursor.read_u16()?;
    let active_mode = cursor.read_i32()?;
    let mut modes = Vec::with_capacity(usize::from(mode_count));
    for _ in 0..mode_count {
        modes.push(parse_mode(&mut cursor, protocol_version)?);
    }

    let zone_count = cursor.read_u16()?;
    let mut zones = Vec::with_capacity(usize::from(zone_count));
    for _ in 0..zone_count {
        zones.push(parse_zone(&mut cursor, protocol_version)?);
    }

    let led_count = cursor.read_u16()?;
    let mut leds = Vec::with_capacity(usize::from(led_count));
    for _ in 0..led_count {
        leds.push(LedData {
            name: cursor.read_c_string()?,
            value: cursor.read_u32()?,
        });
    }

    let color_count = cursor.read_u16()?;
    let mut colors = Vec::with_capacity(usize::from(color_count));
    for _ in 0..color_count {
        colors.push(cursor.read_rgb_color()?);
    }

    let led_alt_names = if protocol_version >= 5 {
        let alt_count = cursor.read_u16()?;
        let mut names = Vec::with_capacity(usize::from(alt_count));
        for _ in 0..alt_count {
            names.push(cursor.read_c_string()?);
        }
        names
    } else {
        Vec::new()
    };

    let flags = if protocol_version >= 5 {
        Some(cursor.read_u32()?)
    } else {
        None
    };
    cursor.finish()?;

    Ok(ControllerData {
        device_type,
        name,
        vendor,
        description,
        version,
        serial,
        location,
        active_mode,
        modes,
        zones,
        leds,
        colors,
        led_alt_names,
        flags,
    })
}

fn parse_mode(cursor: &mut Cursor<'_>, protocol_version: u32) -> Result<ControllerMode> {
    let name = cursor.read_c_string()?;
    let value = cursor.read_i32()?;
    let flags = cursor.read_u32()?;
    let speed_min = cursor.read_u32()?;
    let speed_max = cursor.read_u32()?;
    let (brightness_min, brightness_max) = if protocol_version >= 3 {
        (Some(cursor.read_u32()?), Some(cursor.read_u32()?))
    } else {
        (None, None)
    };
    let colors_min = cursor.read_u32()?;
    let colors_max = cursor.read_u32()?;
    let speed = cursor.read_u32()?;
    let brightness = if protocol_version >= 3 {
        Some(cursor.read_u32()?)
    } else {
        None
    };
    let direction = cursor.read_u32()?;
    let color_mode = ColorMode::from_raw(cursor.read_u32()?);
    let color_count = cursor.read_u16()?;
    let mut colors = Vec::with_capacity(usize::from(color_count));
    for _ in 0..color_count {
        colors.push(cursor.read_rgb_color()?);
    }

    Ok(ControllerMode {
        name,
        value,
        flags,
        speed_min,
        speed_max,
        brightness_min,
        brightness_max,
        colors_min,
        colors_max,
        speed,
        brightness,
        direction,
        color_mode,
        colors,
    })
}

fn parse_zone(cursor: &mut Cursor<'_>, protocol_version: u32) -> Result<ControllerZone> {
    let name = cursor.read_c_string()?;
    let zone_type = ZoneType::from_raw(cursor.read_i32()?);
    let leds_min = cursor.read_u32()?;
    let leds_max = cursor.read_u32()?;
    let leds_count = cursor.read_u32()?;
    let matrix_len = usize::from(cursor.read_u16()?);
    let matrix = if matrix_len == 0 {
        None
    } else {
        if matrix_len < 8 || (matrix_len - 8) % 4 != 0 {
            return Err(OpenRgbError::InvalidMatrixLength(matrix_len));
        }
        let height = cursor.read_u32()?;
        let width = cursor.read_u32()?;
        let value_count = (matrix_len - 8) / 4;
        let mut values = Vec::with_capacity(value_count);
        for _ in 0..value_count {
            values.push(cursor.read_u32()?);
        }
        Some(MatrixMap {
            height,
            width,
            values,
        })
    };

    let segments = if protocol_version >= 4 {
        let segment_count = cursor.read_u16()?;
        let mut segments = Vec::with_capacity(usize::from(segment_count));
        for _ in 0..segment_count {
            segments.push(SegmentData {
                name: cursor.read_c_string()?,
                segment_type: ZoneType::from_raw(cursor.read_i32()?),
                start_index: cursor.read_u32()?,
                leds_count: cursor.read_u32()?,
            });
        }
        segments
    } else {
        Vec::new()
    };

    let flags = if protocol_version >= 5 {
        Some(cursor.read_u32()?)
    } else {
        None
    };

    Ok(ControllerZone {
        name,
        zone_type,
        leds_min,
        leds_max,
        leds_count,
        matrix,
        segments,
        flags,
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or(OpenRgbError::Truncated {
            needed: len,
            remaining: self.remaining(),
        })?;
        if end > self.bytes.len() {
            return Err(OpenRgbError::Truncated {
                needed: len,
                remaining: self.remaining(),
            });
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16> {
        let bytes = self.read_exact(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32> {
        let bytes = self.read_exact(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_i32(&mut self) -> Result<i32> {
        let bytes = self.read_exact(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_rgb_color(&mut self) -> Result<RgbColor> {
        let bytes = self.read_exact(RgbColor::WIRE_SIZE)?;
        Ok(RgbColor::from_wire_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
        ]))
    }

    fn read_c_string(&mut self) -> Result<String> {
        let len = usize::from(self.read_u16()?);
        let bytes = self.read_exact(len)?;
        let Some((&0, content)) = bytes.split_last() else {
            return Err(OpenRgbError::StringMissingNul);
        };
        String::from_utf8(content.to_vec()).map_err(|_| OpenRgbError::InvalidUtf8)
    }

    fn finish(&self) -> Result<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(OpenRgbError::DataSizeMismatch {
                advertised: self.pos,
                actual: self.bytes.len(),
            })
        }
    }
}
