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

    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
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

const BASIC_LAN: GoveeCapabilities = GoveeCapabilities::LAN
    .union(GoveeCapabilities::CLOUD)
    .union(GoveeCapabilities::COLOR_RGB)
    .union(GoveeCapabilities::COLOR_KELVIN)
    .union(GoveeCapabilities::BRIGHTNESS)
    .union(GoveeCapabilities::ON_OFF);

const CLOUD_RGB: GoveeCapabilities = GoveeCapabilities::CLOUD
    .union(GoveeCapabilities::COLOR_RGB)
    .union(GoveeCapabilities::BRIGHTNESS)
    .union(GoveeCapabilities::ON_OFF);

const RGBIC_PRO: GoveeCapabilities = BASIC_LAN
    .union(GoveeCapabilities::SEGMENTS)
    .union(GoveeCapabilities::SCENES_DYNAMIC)
    .union(GoveeCapabilities::RAZER_STREAMING);

pub static SKU_PROFILES: &[SkuProfile] = &[
    SkuProfile {
        sku: "H6163",
        family: SkuFamily::RgbicStrip,
        capabilities: BASIC_LAN.union(GoveeCapabilities::SCENES_DYNAMIC),
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: Some((2000, 9000)),
        name: "RGBIC Strip H6163",
    },
    SkuProfile {
        sku: "H6199",
        family: SkuFamily::RgbicTvBacklight,
        capabilities: CLOUD_RGB,
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: None,
        name: "Immersion TV Backlight H6199",
    },
    SkuProfile {
        sku: "H6054",
        family: SkuFamily::RgbicBar,
        capabilities: BASIC_LAN,
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: Some((2000, 9000)),
        name: "Flow Plus Light Bars H6054",
    },
    SkuProfile {
        sku: "H6056",
        family: SkuFamily::RgbicBar,
        capabilities: BASIC_LAN.union(GoveeCapabilities::SCENES_DYNAMIC),
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: Some((2000, 9000)),
        name: "Flow Pro Light Bars H6056",
    },
    SkuProfile {
        sku: "H619A",
        family: SkuFamily::RgbicStrip,
        capabilities: RGBIC_PRO,
        lan_segment_count: Some(10),
        razer_led_count: Some(20),
        kelvin_range: Some((2000, 9000)),
        name: "RGBIC Pro Strip H619A",
    },
    SkuProfile {
        sku: "H6003",
        family: SkuFamily::Bulb,
        capabilities: BASIC_LAN,
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: Some((2000, 9000)),
        name: "Smart Bulb H6003",
    },
    SkuProfile {
        sku: "H6008",
        family: SkuFamily::Bulb,
        capabilities: BASIC_LAN,
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: Some((2000, 9000)),
        name: "Smart Bulb H6008",
    },
    SkuProfile {
        sku: "H6009",
        family: SkuFamily::Bulb,
        capabilities: BASIC_LAN,
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: Some((2000, 9000)),
        name: "Smart Bulb H6009",
    },
    SkuProfile {
        sku: "H7020",
        family: SkuFamily::RgbicOutdoor,
        capabilities: BASIC_LAN.union(GoveeCapabilities::SCENES_DYNAMIC),
        lan_segment_count: None,
        razer_led_count: None,
        kelvin_range: Some((2000, 9000)),
        name: "Outdoor String Lights H7020",
    },
];

#[must_use]
pub fn profile_for_sku(sku: &str) -> Option<&'static SkuProfile> {
    SKU_PROFILES
        .iter()
        .find(|profile| profile.sku.eq_ignore_ascii_case(sku))
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
