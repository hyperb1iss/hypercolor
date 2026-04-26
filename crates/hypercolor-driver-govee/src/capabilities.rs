use std::ops::{BitOr, BitOrAssign};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GoveeCapabilities(u16);

impl GoveeCapabilities {
    pub const ON_OFF: Self = Self(1 << 0);
    pub const BRIGHTNESS: Self = Self(1 << 1);
    pub const COLOR_RGB: Self = Self(1 << 2);
    pub const COLOR_KELVIN: Self = Self(1 << 3);
    pub const SEGMENTS: Self = Self(1 << 4);
    pub const SCENES_DYNAMIC: Self = Self(1 << 5);
    pub const SCENES_MUSIC: Self = Self(1 << 6);
    pub const LAN: Self = Self(1 << 7);
    pub const CLOUD: Self = Self(1 << 8);
    pub const RAZER_STREAMING: Self = Self(1 << 9);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl BitOr for GoveeCapabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for GoveeCapabilities {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkuFamily {
    RgbicStrip,
    RgbicBar,
    RgbicTvBacklight,
    RgbicOutdoor,
    RgbStrip,
    Bulb,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkuProfile {
    pub sku: &'static str,
    pub family: SkuFamily,
    pub capabilities: GoveeCapabilities,
    pub lan_segment_count: Option<u8>,
    pub razer_led_count: Option<u8>,
    pub kelvin_range: Option<(u16, u16)>,
    pub name: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomLanProfile {
    pub sku: &'static str,
    pub local_features: u8,
    pub segments: u8,
}

const LOCAL_RGB: u8 = 1 << 0;
const LOCAL_KELVIN: u8 = 1 << 1;
const LOCAL_BRIGHTNESS: u8 = 1 << 2;
const LOCAL_SCENES: u8 = 1 << 3;
const LOCAL_BASIC: u8 = LOCAL_RGB | LOCAL_KELVIN | LOCAL_BRIGHTNESS | LOCAL_SCENES;

pub static BASIC_LAN_SKUS: &[&str] = &[
    "H1232", "H1250", "H1270", "H1310", "H1401", "H14A1", "H14C0", "H14C1", "H1630", "H16B0",
    "H16C0", "H1A43", "H1A45", "H2A40", "H2A41", "H3200", "H3500", "H3501", "H3510", "H3511",
    "H3751", "H6003", "H6004", "H6006", "H6008", "H6009", "H600A", "H600D", "H601A", "H6020",
    "H6022", "H6038", "H6051", "H6052", "H6054", "H6056", "H6059", "H605D", "H6061", "H6062",
    "H6065", "H6066", "H6067", "H606A", "H6073", "H6076A", "H6078", "H6079", "H608A", "H608C",
    "H608D", "H6094", "H6095", "H609D", "H60A0", "H60B0", "H610A", "H610B", "H6110", "H6117",
    "H6141", "H6143", "H6144", "H6159", "H615A", "H615B", "H615C", "H615D", "H615E", "H6163",
    "H6168", "H616C", "H616E", "H6172", "H6173", "H6175", "H6176", "H6182", "H618G", "H619Z",
    "H61A0", "H61A1", "H61A2", "H61A3", "H61A5", "H61A8", "H61A9", "H61B1", "H61B2", "H61B6",
    "H61B9", "H61BA", "H61BC", "H61BE", "H61C2", "H61C3", "H61C5", "H61D5", "H61D6", "H61E0",
    "H61E1", "H6630", "H6631", "H6671", "H6672", "H6800", "H6811", "H6840", "H6841", "H6860",
    "H6861", "H6870", "H6871", "H7020", "H7021", "H7027", "H7028", "H702B", "H702C", "H7030",
    "H7033", "H7037", "H7038", "H703A", "H703B", "H7041", "H7042", "H7045", "H7046", "H7050",
    "H7051", "H7053", "H7055", "H7056", "H7057", "H7058", "H705A", "H705B", "H705C", "H7060",
    "H7061", "H7062", "H7063", "H7065", "H7066", "H7067", "H706A", "H706B", "H706C", "H7070",
    "H7071", "H7072", "H7073", "H7075", "H707A", "H707B", "H707C", "H7086", "H7087", "H7094",
    "H70B1", "H70B3", "H70B6", "H70B8", "H70BC", "H70C1", "H70C2", "H70C9", "H70D3", "H8015",
    "H801A", "H801B", "H801D", "H8022", "H8025", "H8026", "H802A", "H8048", "H8057", "H805A",
    "H805B", "H8066", "H8069", "H8076", "H8076A", "H807C", "H80A4", "H80C4", "H80D1", "H8630",
    "H8811", "H8840", "H8841",
];

pub static CUSTOM_LAN_PROFILES: &[CustomLanProfile] = &[
    custom("H6039", LOCAL_BASIC, 12),
    custom("H6042", LOCAL_BASIC, 5),
    custom("H6043", LOCAL_RGB | LOCAL_BRIGHTNESS | LOCAL_SCENES, 10),
    custom("H6046", LOCAL_BASIC, 10),
    custom("H6047", LOCAL_BASIC, 10),
    custom("H6048", LOCAL_BASIC, 24),
    custom("H6063", LOCAL_RGB | LOCAL_KELVIN | LOCAL_BRIGHTNESS, 0),
    custom("H6069", LOCAL_RGB | LOCAL_BRIGHTNESS | LOCAL_SCENES, 20),
    custom("H6072", LOCAL_BASIC, 8),
    custom("H6076", LOCAL_BASIC, 7),
    custom("H607C", LOCAL_BASIC, 13),
    custom("H6087", LOCAL_BASIC, 12),
    custom("H6088", LOCAL_BASIC, 6),
    custom("H608B", LOCAL_BASIC, 15),
    custom("H6093", LOCAL_KELVIN | LOCAL_BRIGHTNESS | LOCAL_SCENES, 0),
    custom("H60A1", LOCAL_BASIC, 13),
    custom("H60A4", LOCAL_RGB | LOCAL_KELVIN | LOCAL_BRIGHTNESS, 11),
    custom("H60A6", LOCAL_RGB | LOCAL_KELVIN | LOCAL_BRIGHTNESS, 0),
    custom("H60B1", LOCAL_BASIC, 3),
    custom("H60B2", LOCAL_BASIC, 3),
    custom("H60C1", LOCAL_BASIC, 3),
    custom("H610E", LOCAL_RGB | LOCAL_KELVIN | LOCAL_BRIGHTNESS, 7),
    custom("H610F", LOCAL_RGB | LOCAL_KELVIN | LOCAL_BRIGHTNESS, 7),
    custom("H6167", LOCAL_BASIC, 12),
    custom("H616D", LOCAL_BASIC, 15),
    custom("H618A", LOCAL_BASIC, 15),
    custom("H618C", LOCAL_BASIC, 15),
    custom("H618E", LOCAL_BASIC, 15),
    custom("H618F", LOCAL_BASIC, 15),
    custom("H6199", LOCAL_RGB | LOCAL_BRIGHTNESS, 0),
    custom("H619A", LOCAL_BASIC, 10),
    custom("H619B", LOCAL_BASIC, 10),
    custom("H619C", LOCAL_BASIC, 10),
    custom("H619D", LOCAL_BASIC, 10),
    custom("H619E", LOCAL_BASIC, 10),
    custom("H61B3", LOCAL_BASIC, 15),
    custom("H61B8", LOCAL_BASIC, 40),
    custom("H61D3", LOCAL_BASIC, 15),
    custom("H61E5", LOCAL_BASIC, 12),
    custom("H61E6", LOCAL_BASIC, 15),
    custom("H61F2", LOCAL_BASIC, 4),
    custom("H61F5", LOCAL_BASIC, 10),
    custom("H61F6", LOCAL_BASIC, 20),
    custom("H6609", LOCAL_BASIC, 18),
    custom("H6640", LOCAL_BASIC, 14),
    custom("H6641", LOCAL_BASIC, 14),
    custom("H6810", LOCAL_RGB | LOCAL_KELVIN | LOCAL_BRIGHTNESS, 0),
    custom("H7012", LOCAL_BRIGHTNESS, 0),
    custom("H7013", LOCAL_BRIGHTNESS, 0),
    custom("H7025", LOCAL_RGB | LOCAL_BRIGHTNESS | LOCAL_SCENES, 15),
    custom("H7026", LOCAL_RGB | LOCAL_BRIGHTNESS | LOCAL_SCENES, 30),
    custom("H702A", LOCAL_BASIC, 15),
    custom("H7039", LOCAL_BASIC, 45),
    custom("H7052", LOCAL_BASIC, 15),
    custom("H705D", LOCAL_BASIC, 9),
    custom("H705E", LOCAL_BASIC, 18),
    custom("H705F", LOCAL_BASIC, 27),
    custom("H7076", LOCAL_BASIC, 15),
    custom("H7093", LOCAL_BASIC, 2),
    custom("H70A1", LOCAL_BASIC, 15),
    custom("H70A2", LOCAL_BASIC, 20),
    custom("H70A3", LOCAL_BASIC, 15),
    custom("H70B5", LOCAL_BASIC, 3),
    custom("H70C4", LOCAL_BASIC, 10),
    custom("H70C5", LOCAL_BASIC, 10),
    custom("H70C7", LOCAL_BASIC, 10),
    custom("H70D1", LOCAL_BASIC, 10),
    custom("H70D2", LOCAL_BASIC, 10),
    custom("H805C", LOCAL_BASIC, 9),
    custom("H8072", LOCAL_BASIC, 8),
    custom("H808A", LOCAL_BASIC, 5),
    custom("H80A1", LOCAL_BASIC, 14),
    custom("H80C5", LOCAL_BASIC, 10),
];

#[must_use]
pub fn profile_for_sku(sku: &str) -> Option<SkuProfile> {
    if let Some(&known_sku) = BASIC_LAN_SKUS
        .iter()
        .find(|known_sku| known_sku.eq_ignore_ascii_case(sku))
    {
        return Some(profile_from_parts(known_sku, LOCAL_BASIC, 0));
    }

    CUSTOM_LAN_PROFILES
        .iter()
        .find(|profile| profile.sku.eq_ignore_ascii_case(sku))
        .map(|profile| profile_from_parts(profile.sku, profile.local_features, profile.segments))
}

#[must_use]
pub fn fallback_profile(_sku: &str) -> SkuProfile {
    SkuProfile {
        sku: "UNKNOWN",
        family: SkuFamily::Unknown,
        capabilities: GoveeCapabilities::LAN
            | GoveeCapabilities::COLOR_RGB
            | GoveeCapabilities::BRIGHTNESS
            | GoveeCapabilities::ON_OFF,
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: None,
        name: "Govee Device",
    }
}

#[must_use]
pub const fn known_sku_count() -> usize {
    BASIC_LAN_SKUS.len() + CUSTOM_LAN_PROFILES.len()
}

const fn custom(sku: &'static str, local_features: u8, segments: u8) -> CustomLanProfile {
    CustomLanProfile {
        sku,
        local_features,
        segments,
    }
}

fn profile_from_parts(sku: &'static str, local_features: u8, segments: u8) -> SkuProfile {
    let mut capabilities = GoveeCapabilities::LAN | GoveeCapabilities::ON_OFF;
    if cloud_supported(sku) {
        capabilities |= GoveeCapabilities::CLOUD;
    }
    if local_features & LOCAL_RGB != 0 {
        capabilities |= GoveeCapabilities::COLOR_RGB;
    }
    if local_features & LOCAL_KELVIN != 0 {
        capabilities |= GoveeCapabilities::COLOR_KELVIN;
    }
    if local_features & LOCAL_BRIGHTNESS != 0 {
        capabilities |= GoveeCapabilities::BRIGHTNESS;
    }
    if segments > 0 {
        capabilities |= GoveeCapabilities::SEGMENTS;
    }
    if local_features & LOCAL_SCENES != 0 {
        capabilities |= GoveeCapabilities::SCENES_DYNAMIC;
    }
    let razer_led_count = razer_led_count(sku);
    if razer_led_count.is_some() {
        capabilities |= GoveeCapabilities::RAZER_STREAMING;
    }

    SkuProfile {
        sku,
        family: family_for_sku(sku),
        capabilities,
        lan_segment_count: if segments > 0 { Some(segments) } else { None },
        razer_led_count,
        kelvin_range: if local_features & LOCAL_KELVIN != 0 {
            Some((2000, 9000))
        } else {
            None
        },
        name: display_name(sku),
    }
}

fn cloud_supported(sku: &str) -> bool {
    matches!(
        sku,
        "H6003"
            | "H6008"
            | "H6009"
            | "H6054"
            | "H6056"
            | "H6163"
            | "H6199"
            | "H619A"
            | "H619B"
            | "H619D"
            | "H7020"
    )
}

fn razer_led_count(sku: &str) -> Option<u8> {
    match sku {
        "H619A" | "H70B1" => Some(20),
        _ => None,
    }
}

fn family_for_sku(sku: &str) -> SkuFamily {
    match sku {
        "H6003" | "H6004" | "H6006" | "H6008" | "H6009" | "H600A" | "H600D" | "H601A" | "H6020"
        | "H6022" => SkuFamily::Bulb,
        "H6042" | "H6043" | "H6046" | "H6047" | "H6048" | "H6051" | "H6052" | "H6054" | "H6056"
        | "H6059" | "H605D" => SkuFamily::RgbicBar,
        _ if sku.starts_with("H70") || sku.starts_with("H80") => SkuFamily::RgbicOutdoor,
        _ if sku.starts_with("H60") || sku.starts_with("H61") => SkuFamily::RgbicStrip,
        _ => SkuFamily::Unknown,
    }
}

fn display_name(sku: &str) -> &'static str {
    match sku {
        "H6163" => "RGBIC Strip H6163",
        "H6199" => "Immersion TV Backlight H6199",
        "H6054" => "Flow Plus Light Bars H6054",
        "H6056" => "Flow Pro Light Bars H6056",
        "H619A" => "RGBIC Pro Strip H619A",
        "H7020" => "Outdoor String Lights H7020",
        _ => "Govee Device",
    }
}
