use base64::Engine;

const HEADER: u8 = 0xBB;
const FRAME_SUBCOMMAND: u8 = 0xB0;
const MODE_SUBCOMMAND: u8 = 0xB1;
const GRADIENT_OFF: u8 = 0x01;
const MAX_RAZER_LEDS: usize = 255;

pub const RAZER_ENABLE: [u8; 6] = [HEADER, 0x00, 0x01, MODE_SUBCOMMAND, 0x01, 0x0A];
pub const RAZER_DISABLE: [u8; 6] = [HEADER, 0x00, 0x01, MODE_SUBCOMMAND, 0x00, 0x0B];

#[must_use]
pub fn encode_razer_frame(colors: &[[u8; 3]]) -> Option<Vec<u8>> {
    if colors.is_empty() {
        return None;
    }

    let count = colors.len().min(MAX_RAZER_LEDS);
    let payload_len = 2 + (3 * count);
    let mut packet = Vec::with_capacity(7 + (3 * count));
    packet.push(HEADER);
    packet.push(((payload_len >> 8) & 0xFF) as u8);
    packet.push((payload_len & 0xFF) as u8);
    packet.push(FRAME_SUBCOMMAND);
    packet.push(GRADIENT_OFF);
    packet.push(count as u8);

    for [red, green, blue] in colors.iter().take(count) {
        packet.push(*red);
        packet.push(*green);
        packet.push(*blue);
    }

    packet.push(xor_checksum(&packet));
    Some(packet)
}

#[must_use]
pub fn encode_razer_frame_base64(colors: &[[u8; 3]]) -> Option<String> {
    encode_razer_frame(colors)
        .map(|packet| base64::engine::general_purpose::STANDARD.encode(packet))
}

#[must_use]
pub fn encode_razer_mode_base64(enabled: bool) -> String {
    let packet = if enabled {
        RAZER_ENABLE.as_slice()
    } else {
        RAZER_DISABLE.as_slice()
    };
    base64::engine::general_purpose::STANDARD.encode(packet)
}

#[must_use]
pub fn xor_checksum(bytes: &[u8]) -> u8 {
    bytes
        .iter()
        .copied()
        .fold(0, |checksum, byte| checksum ^ byte)
}
