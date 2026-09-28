//! Push 2 identity and statistics replies, logged once per connect.
//!
//! Both are already requested: the init sequence opens with a Device
//! Inquiry, and the post-connect probe asks for statistics. Decoding them
//! puts the firmware build, the power source, and the uptime in the log,
//! which a wedge report needs: whether the unit runs on USB bus power alone,
//! and whether it rebooted since the last session.

use tracing::info;

/// The Device Inquiry reply is fixed at 23 bytes.
const IDENTITY_REPLY_LEN: usize = 23;

/// Device Inquiry reply fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Push2Identity {
    pub(super) firmware_major: u8,
    pub(super) firmware_minor: u8,
    pub(super) build: u16,
    pub(super) serial: u32,
    pub(super) board_revision: u8,
}

/// Request Statistics reply fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Push2Statistics {
    /// `true` with the external power supply attached, `false` on USB bus
    /// power alone, where the firmware limits LED and backlight brightness.
    pub(super) external_power: bool,
    pub(super) run_id: u8,
    pub(super) uptime_s: u32,
}

fn seven(byte: u8) -> u32 {
    u32::from(byte & 0x7F)
}

/// Decode a whole Device Inquiry reply, framing bytes included.
pub(super) fn decode_identity(message: &[u8]) -> Option<Push2Identity> {
    if message.len() != IDENTITY_REPLY_LEN {
        return None;
    }
    Some(Push2Identity {
        firmware_major: message[12] & 0x7F,
        firmware_minor: message[13] & 0x7F,
        build: u16::try_from(seven(message[14]) | (seven(message[15]) << 7)).ok()?,
        serial: seven(message[16])
            | (seven(message[17]) << 7)
            | (seven(message[18]) << 14)
            | (seven(message[19]) << 21)
            | ((seven(message[20]) & 0x0F) << 28),
        board_revision: message[21] & 0x7F,
    })
}

/// Decode the argument bytes of a Request Statistics reply.
pub(super) fn decode_statistics(args: &[u8]) -> Option<Push2Statistics> {
    let &[power, run_id, t0, t1, t2, t3, t4] = args else {
        return None;
    };
    Some(Push2Statistics {
        external_power: power == 1,
        run_id: run_id & 0x7F,
        uptime_s: seven(t0)
            | (seven(t1) << 7)
            | (seven(t2) << 14)
            | (seven(t3) << 21)
            | ((seven(t4) & 0x0F) << 28),
    })
}

pub(super) fn log_identity(message: &[u8]) {
    if let Some(identity) = decode_identity(message) {
        info!(
            firmware = %format_args!("{}.{}", identity.firmware_major, identity.firmware_minor),
            build = identity.build,
            serial = identity.serial,
            board_revision = identity.board_revision,
            "Push 2 identified"
        );
    }
}

pub(super) fn log_statistics(args: &[u8]) {
    if let Some(statistics) = decode_statistics(args) {
        info!(
            power = if statistics.external_power {
                "external supply"
            } else {
                "USB bus power only"
            },
            run_id = statistics.run_id,
            uptime_s = statistics.uptime_s,
            "Push 2 reported its power source and uptime"
        );
    }
}
