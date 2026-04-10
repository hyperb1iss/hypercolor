//! Shared helpers, constants, and wire-format structs for Lian Li hub protocols.
//!
//! The ENE (`Ene*Protocol`) and TL (`TlProtocol`) families are fundamentally
//! different wire formats but share the hub variant taxonomy, report IDs, and
//! a handful of utility functions (white limiting, firmware parsing, report-id
//! stripping). Those live here so the per-family modules can stay focused on
//! their own encoding path.

use std::time::Duration;

use hypercolor_types::device::DeviceColorFormat;
use zerocopy::{FromZeros, Immutable, IntoBytes, KnownLayout};

/// Shared report ID for ENE 6K77 feature/output/input reports.
pub const ENE_REPORT_ID: u8 = 0xE0;
/// Shared report ID for TL output/input reports.
pub const TL_REPORT_ID: u8 = 0x01;
/// Recommended inter-command delay for ENE hubs.
pub const ENE_COMMAND_DELAY: Duration = Duration::from_millis(20);

pub(super) const WHITE_LIMIT_THRESHOLD: u16 = 460;
pub(super) const AL_WHITE_LIMIT: u8 = 153;

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub(super) struct EnePacket11 {
    pub(super) report_id: u8,
    pub(super) command: u8,
    pub(super) subcommand: u8,
    pub(super) arg0: u8,
    pub(super) padding: [u8; 7],
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub(super) struct EnePacket65 {
    pub(super) report_id: u8,
    pub(super) command: u8,
    pub(super) subcommand: u8,
    pub(super) arg0: u8,
    pub(super) arg1: u8,
    pub(super) padding: [u8; 60],
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub(super) struct EneOutputPacket146 {
    pub(super) report_id: u8,
    pub(super) port: u8,
    pub(super) payload: [u8; 144],
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub(super) struct EneOutputPacket353 {
    pub(super) report_id: u8,
    pub(super) port: u8,
    pub(super) payload: [u8; 351],
}

const _: () = assert!(
    std::mem::size_of::<EnePacket11>() == 11,
    "EnePacket11 must match the 11-byte SL report size"
);
const _: () = assert!(
    std::mem::size_of::<EnePacket65>() == 65,
    "EnePacket65 must match the 65-byte ENE feature report size"
);
const _: () = assert!(
    std::mem::size_of::<EneOutputPacket146>() == 146,
    "EneOutputPacket146 must match the 146-byte AL output report size"
);
const _: () = assert!(
    std::mem::size_of::<EneOutputPacket353>() == 353,
    "EneOutputPacket353 must match the 353-byte V2/Infinity output report size"
);

/// Supported Lian Li hub families currently wired into the HAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LianLiHubVariant {
    /// UNI FAN SL hub (`0xA100`) — single ring, 11-byte feature packets.
    Sl,
    /// UNI FAN AL hub (`0xA101`) — dual ring, 65-byte feature packets.
    Al,
    /// UNI FAN SL V2 hub (`0xA103`, `0xA105`) — V2 single ring.
    SlV2,
    /// UNI FAN AL V2 hub (`0xA104`) — V2 dual ring.
    AlV2,
    /// UNI FAN SL Infinity hub (`0xA102`) — 4 physical groups, 8 logical channels.
    SlInfinity,
    /// UNI FAN SL Redragon hub (`0xA106`) — SL-compatible single ring.
    SlRedragon,
    /// TL Fan hub (`0x7372`) — framed output/input HID packets.
    TlFan,
}

impl LianLiHubVariant {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sl => "Lian Li UNI Hub SL",
            Self::Al => "Lian Li UNI Hub AL",
            Self::SlV2 => "Lian Li UNI Hub SL V2",
            Self::AlV2 => "Lian Li UNI Hub AL V2",
            Self::SlInfinity => "Lian Li UNI Hub SL Infinity",
            Self::SlRedragon => "Lian Li UNI Hub SL Redragon",
            Self::TlFan => "Lian Li TL Fan Hub",
        }
    }

    #[must_use]
    pub const fn activate_subcommand(self) -> u8 {
        match self {
            Self::Sl | Self::SlRedragon => 0x32,
            Self::Al => 0x40,
            Self::SlV2 | Self::AlV2 | Self::SlInfinity => 0x60,
            Self::TlFan => 0x00,
        }
    }

    #[must_use]
    pub const fn group_count(self) -> u8 {
        4
    }

    #[must_use]
    pub const fn logical_channel_count(self) -> u8 {
        match self {
            Self::SlInfinity => 8,
            Self::TlFan => 0,
            _ => 4,
        }
    }

    #[must_use]
    pub const fn uses_double_port(self) -> bool {
        matches!(self, Self::Al | Self::AlV2 | Self::SlInfinity)
    }

    #[must_use]
    pub const fn is_v2(self) -> bool {
        matches!(self, Self::SlV2 | Self::AlV2)
    }

    #[must_use]
    pub const fn max_fans_per_group(self) -> u8 {
        match self {
            Self::SlV2 | Self::AlV2 => 6,
            _ => 4,
        }
    }

    #[must_use]
    pub const fn leds_per_fan(self) -> u8 {
        match self {
            Self::Al | Self::AlV2 | Self::SlInfinity => 20,
            Self::TlFan => super::tl::LEDS_PER_FAN_U8,
            _ => 16,
        }
    }

    #[must_use]
    pub const fn color_format(self) -> DeviceColorFormat {
        match self {
            Self::TlFan => DeviceColorFormat::Rgb,
            _ => DeviceColorFormat::Rbg,
        }
    }

    #[must_use]
    pub const fn default_fan_counts(self) -> [u8; 8] {
        let max = self.max_fans_per_group();
        match self {
            Self::SlInfinity => [max, max, max, max, max, max, max, max],
            Self::TlFan => [0; 8],
            _ => [max, max, max, max, 0, 0, 0, 0],
        }
    }

    #[must_use]
    pub(super) const fn feature_packet_len(self) -> usize {
        match self {
            Self::Sl | Self::SlRedragon => 11,
            _ => 65,
        }
    }
}

/// Apply the recommended sum-based white limiter used by V2 and Infinity hubs.
#[must_use]
pub fn apply_sum_white_limit([r, g, b]: [u8; 3]) -> [u8; 3] {
    let sum = u16::from(r) + u16::from(g) + u16::from(b);
    if sum <= WHITE_LIMIT_THRESHOLD {
        return [r, g, b];
    }

    let threshold = u32::from(WHITE_LIMIT_THRESHOLD);
    let sum = u32::from(sum);
    [
        u8::try_from(u32::from(r) * threshold / sum).expect("scaled red should fit in u8"),
        u8::try_from(u32::from(g) * threshold / sum).expect("scaled green should fit in u8"),
        u8::try_from(u32::from(b) * threshold / sum).expect("scaled blue should fit in u8"),
    ]
}

/// Apply the AL-family equal-channel white clamp.
#[must_use]
pub fn apply_al_white_limit([r, g, b]: [u8; 3]) -> [u8; 3] {
    if r > AL_WHITE_LIMIT && r == g && g == b {
        [AL_WHITE_LIMIT, AL_WHITE_LIMIT, AL_WHITE_LIMIT]
    } else {
        [r, g, b]
    }
}

/// Convert the ENE firmware `fine_tune` byte into a user-facing version.
#[must_use]
pub fn firmware_version_from_fine_tune(fine_tune: u8) -> String {
    if fine_tune < 8 {
        "1.0".to_owned()
    } else {
        let version = f32::from((fine_tune >> 4) * 10 + (fine_tune & 0x0F) + 2) / 10.0;
        format!("{version:.1}")
    }
}

pub(super) fn strip_optional_report_id(data: &[u8], report_id: u8) -> &[u8] {
    if data.first().copied() == Some(report_id) {
        &data[1..]
    } else {
        data
    }
}
