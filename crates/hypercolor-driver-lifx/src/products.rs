//! Product facts used for capability resolution (Spec 88 §3.1 and §10).
//!
//! The LIFX product registry reserves all rights, so this table is not a
//! copy of it. It records only facts per product id: the light class, the
//! extended multizone rule, the base kelvin range, and the uplight cell.

use crate::protocol::FirmwareVersion;

use ProductKind::{Color, Matrix, Multizone, NonLight, White};

/// LIFX's vendor id in StateVersion.
pub const LIFX_VENDOR_ID: u32 = 1;
/// Kelvin range assumed for product ids the table does not know.
pub const FALLBACK_KELVIN_RANGE: (u16, u16) = (1500, 9000);

/// Broad light class a product belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProductKind {
    /// Single zone, hue and saturation capable.
    Color,
    /// Single zone, white only (kelvin and brightness).
    White,
    /// Linear zones.
    Multizone,
    /// Matrix zones (Tile, Candle, Ceiling, and the rest).
    Matrix,
    /// Not a light (switches); discovery ignores these.
    NonLight,
}

/// When a multizone product understands the extended messages (510 to 512).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtendedMultizone {
    Never,
    Always,
    /// From this `(major, minor)` firmware on.
    SinceFirmware(u16, u16),
}

/// Per-product geometry quirk applied on top of the device chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatrixQuirk {
    None,
    /// A 5 x 6 matrix whose first row drives one top zone from its first cell.
    CandleTopZone,
}

/// Facts about one LIFX product id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifxProduct {
    pub pid: u32,
    pub name: &'static str,
    pub kind: ProductKind,
    pub chain: bool,
    pub extended_multizone: ExtendedMultizone,
    /// Base `[min, max]` kelvin; `None` for non-lights.
    pub kelvin_range: Option<(u16, u16)>,
    /// Matrix cell `(x, y)` that drives an uplight.
    pub uplight: Option<(u8, u8)>,
    pub quirk: MatrixQuirk,
}

impl LifxProduct {
    /// Whether the product renders hue and saturation.
    #[must_use]
    pub const fn color_capable(&self) -> bool {
        !matches!(self.kind, ProductKind::White | ProductKind::NonLight)
    }

    /// Whether extended multizone applies at this firmware.
    #[must_use]
    pub fn supports_extended_multizone(&self, firmware: FirmwareVersion) -> bool {
        if self.kind != ProductKind::Multizone {
            return false;
        }
        match self.extended_multizone {
            ExtendedMultizone::Never => false,
            ExtendedMultizone::Always => true,
            ExtendedMultizone::SinceFirmware(major, minor) => {
                firmware >= FirmwareVersion::new(major, minor)
            }
        }
    }

    /// Kelvin range to clamp output to.
    #[must_use]
    pub fn kelvin_range_or_fallback(&self) -> (u16, u16) {
        self.kelvin_range.unwrap_or(FALLBACK_KELVIN_RANGE)
    }
}

/// Look up a product by id.
#[must_use]
pub fn lookup_product(pid: u32) -> Option<&'static LifxProduct> {
    PRODUCTS
        .binary_search_by_key(&pid, |product| product.pid)
        .ok()
        .and_then(|index| PRODUCTS.get(index))
}

/// Every product id known to the driver, sorted by id.
#[must_use]
pub fn products() -> &'static [LifxProduct] {
    PRODUCTS
}

const NEVER: ExtendedMultizone = ExtendedMultizone::Never;
const ALWAYS: ExtendedMultizone = ExtendedMultizone::Always;
const NO_KELVIN: Option<(u16, u16)> = None;

const fn product(
    pid: u32,
    name: &'static str,
    kind: ProductKind,
    extended_multizone: ExtendedMultizone,
    kelvin_range: Option<(u16, u16)>,
) -> LifxProduct {
    LifxProduct {
        pid,
        name,
        kind,
        chain: false,
        extended_multizone,
        kelvin_range,
        uplight: None,
        quirk: MatrixQuirk::None,
    }
}

impl LifxProduct {
    const fn chainable(mut self) -> Self {
        self.chain = true;
        self
    }

    const fn with_uplight(mut self, x: u8, y: u8) -> Self {
        self.uplight = Some((x, y));
        self
    }

    const fn with_quirk(mut self, quirk: MatrixQuirk) -> Self {
        self.quirk = quirk;
        self
    }
}

#[rustfmt::skip]
static PRODUCTS: &[LifxProduct] = &[
    product(1, "Original", Color, NEVER, Some((2500, 9000))),
    product(2, "Original 1000", Color, NEVER, Some((2500, 9000))),
    product(3, "Color 650", Color, NEVER, Some((2500, 9000))),
    product(10, "LIFX White 800 (LV)", White, NEVER, Some((2700, 6500))),
    product(11, "LIFX White 800 (HV)", White, NEVER, Some((2700, 6500))),
    product(12, "LIFX LCMv4 White OEM (A19 LV)", White, NEVER, Some((2700, 2700))),
    product(13, "LIFX LCMv4 White OEM (A19 HV)", White, NEVER, Some((2700, 2700))),
    product(14, "LIFX Carbon Prototype", Color, NEVER, Some((2500, 9000))),
    product(15, "LIFX LCMv4 Color (A21)", Color, NEVER, Some((2700, 6500))),
    product(18, "LIFX White 900 (BR30)", White, NEVER, Some((2700, 6500))),
    product(19, "LIFX White 900 (BR30)", White, NEVER, Some((2700, 6500))),
    product(20, "LIFX Color 1000 (BR30)", Color, NEVER, Some((2500, 9000))),
    product(21, "LIFX LCMv4 Color (GU10)", Color, NEVER, Some((2500, 9000))),
    product(22, "LIFX Color 1000", Color, NEVER, Some((2500, 9000))),
    product(23, "LIFX (A19)", Color, NEVER, Some((2500, 9000))),
    product(24, "LIFX (BR30)", Color, NEVER, Some((2500, 9000))),
    product(25, "LIFX+ (A19)", Color, NEVER, Some((2500, 9000))),
    product(26, "LIFX+ (BR30)", Color, NEVER, Some((2500, 9000))),
    product(27, "LIFX (A19)", Color, NEVER, Some((2500, 9000))),
    product(28, "LIFX (BR30)", Color, NEVER, Some((2500, 9000))),
    product(29, "LIFX+ (A19)", Color, NEVER, Some((2500, 9000))),
    product(30, "LIFX+ (BR30)", Color, NEVER, Some((2500, 9000))),
    product(31, "LIFX Z", Multizone, NEVER, Some((2500, 9000))),
    product(32, "LIFX Z", Multizone, ExtendedMultizone::SinceFirmware(2, 77), Some((2500, 9000))),
    product(33, "LIFX Downlight", Color, NEVER, Some((2500, 9000))),
    product(36, "LIFX DL", Color, NEVER, Some((2500, 9000))),
    product(37, "LIFX DL", Color, NEVER, Some((2500, 9000))),
    product(38, "LIFX Beam", Multizone, ExtendedMultizone::SinceFirmware(2, 77), Some((2500, 9000))),
    product(39, "LIFX DL WW 700lm", White, NEVER, Some((2500, 9000))),
    product(40, "LIFX DL Colour 700lm", Color, NEVER, Some((2500, 9000))),
    product(43, "LIFX (A19)", Color, NEVER, Some((2500, 9000))),
    product(44, "LIFX (BR30)", Color, NEVER, Some((2500, 9000))),
    product(45, "LIFX+ (A19)", Color, NEVER, Some((2500, 9000))),
    product(46, "LIFX+ (BR30)", Color, NEVER, Some((2500, 9000))),
    product(49, "LIFX Mini C", Color, NEVER, Some((2500, 9000))),
    product(50, "LIFX Mini DD", White, NEVER, Some((1500, 4000))),
    product(51, "LIFX Mini W", White, NEVER, Some((2700, 2700))),
    product(52, "LIFX GU10", Color, NEVER, Some((2500, 9000))),
    product(53, "LIFX GU10", Color, NEVER, Some((2500, 9000))),
    product(54, "LIFX GU10 White", White, NEVER, Some((2700, 2700))),
    product(55, "LIFX Tile", Matrix, NEVER, Some((2500, 9000))).chainable(),
    product(56, "LIFX Beam", Multizone, ALWAYS, Some((2500, 9000))),
    product(57, "LIFX Candle C", Matrix, NEVER, Some((2500, 9000))).with_quirk(MatrixQuirk::CandleTopZone),
    product(59, "LIFX Mini C", Color, NEVER, Some((2500, 9000))),
    product(60, "LIFX Mini DD", White, NEVER, Some((1500, 4000))),
    product(61, "LIFX Mini W", White, NEVER, Some((2700, 2700))),
    product(62, "LIFX (A19) LCM3", Color, NEVER, Some((2500, 9000))),
    product(63, "LIFX (BR30) LCM3", Color, NEVER, Some((2500, 9000))),
    product(64, "LIFX+ (A19) LCM3", Color, NEVER, Some((2500, 9000))),
    product(65, "LIFX+ (BR30) LCM3", Color, NEVER, Some((2500, 9000))),
    product(66, "LIFX Mini W", White, NEVER, Some((2700, 2700))),
    product(67, "LIFX Candle WW", White, NEVER, Some((2200, 6500))),
    product(68, "LIFX Candle C", Matrix, NEVER, Some((2500, 9000))).with_quirk(MatrixQuirk::CandleTopZone),
    product(70, "LIFX Switch", NonLight, NEVER, NO_KELVIN),
    product(71, "LIFX Switch", NonLight, NEVER, NO_KELVIN),
    product(72, "LIFX A19", Color, ALWAYS, Some((1500, 9000))),
    product(81, "LIFX Candle WW", White, NEVER, Some((2200, 6500))),
    product(82, "LIFX Filament Clear", White, NEVER, Some((2200, 2200))),
    product(84, "LIFX Switch", NonLight, NEVER, NO_KELVIN),
    product(85, "LIFX Filament Amber", White, NEVER, Some((2000, 2000))),
    product(86, "LIFX Filament Smokey", White, NEVER, Some((2200, 2200))),
    product(87, "LIFX White", White, NEVER, Some((2700, 2700))),
    product(88, "LIFX White", White, NEVER, Some((2700, 2700))),
    product(89, "LIFX Switch", NonLight, NEVER, NO_KELVIN),
    product(90, "LIFX Clean A19 1100lm", Color, NEVER, Some((2500, 9000))),
    product(91, "LIFX Color 800lm", Color, NEVER, Some((2500, 9000))),
    product(92, "LIFX Colour 1000lm", Color, NEVER, Some((2500, 9000))),
    product(93, "LIFX Color A19 1100lm", Color, NEVER, Some((2500, 9000))),
    product(94, "LIFX Color BR30 1100lm", Color, NEVER, Some((2500, 9000))),
    product(96, "LIFX Candle WW", White, NEVER, Some((2200, 6500))),
    product(97, "LIFX Colour A19 1200lm", Color, NEVER, Some((2500, 9000))),
    product(98, "LIFX Colour BR30 1100lm", Color, NEVER, Some((2500, 9000))),
    product(99, "LIFX Clean A19 1200lm", Color, NEVER, Some((2500, 9000))),
    product(100, "LIFX Filament Clear", White, NEVER, Some((2200, 2200))),
    product(101, "LIFX Filament Amber", White, NEVER, Some((2000, 2000))),
    product(102, "LIFX Filament Smokey", White, NEVER, Some((2200, 2200))),
    product(109, "LIFX NV A19 1100lm", Color, NEVER, Some((2500, 9000))),
    product(110, "LIFX NV BR30 1100lm", Color, NEVER, Some((2500, 9000))),
    product(111, "LIFX NV A19 1200lm", Color, NEVER, Some((2500, 9000))),
    product(112, "LIFX NV BR30 1100lm", Color, NEVER, Some((2500, 9000))),
    product(113, "LIFX WW 800lm", White, NEVER, Some((1500, 4000))),
    product(114, "LIFX WW 1000lm", White, NEVER, Some((1500, 4000))),
    product(115, "LIFX Switch", NonLight, NEVER, NO_KELVIN),
    product(116, "LIFX Switch US", NonLight, NEVER, NO_KELVIN),
    product(117, "LIFX Z", Multizone, ALWAYS, Some((1500, 9000))),
    product(118, "LIFX Z", Multizone, ALWAYS, Some((1500, 9000))),
    product(119, "LIFX Beam", Multizone, ALWAYS, Some((1500, 9000))),
    product(120, "LIFX Beam", Multizone, ALWAYS, Some((1500, 9000))),
    product(121, "LIFX DL", Color, NEVER, Some((1500, 9000))),
    product(122, "LIFX DL", Color, NEVER, Some((1500, 9000))),
    product(123, "LIFX Mini 3.1 Color US", Color, NEVER, Some((1500, 9000))),
    product(124, "LIFX Mini 3.1 Color Intl", Color, NEVER, Some((1500, 9000))),
    product(125, "LIFX Mini 3.1 WW US", White, NEVER, Some((1500, 9000))),
    product(126, "LIFX Mini 3.1 WW Intl", White, NEVER, Some((1500, 9000))),
    product(127, "LIFX Mini 3.1 W US", White, NEVER, Some((2700, 2700))),
    product(128, "LIFX Mini 3.1 W Intl", White, NEVER, Some((2700, 2700))),
    product(129, "LIFX Color 800", Color, NEVER, Some((1500, 9000))),
    product(130, "LIFX Colour 1000", Color, NEVER, Some((1500, 9000))),
    product(131, "LIFX White to Warm 800", White, NEVER, Some((1500, 9000))),
    product(132, "LIFX White to Warm 1000", White, NEVER, Some((1500, 9000))),
    product(133, "LIFX White 650", White, NEVER, Some((2700, 2700))),
    product(134, "LIFX White 800", White, NEVER, Some((2700, 2700))),
    product(135, "LIFX GU10 Color US", Color, NEVER, Some((1500, 9000))),
    product(136, "LIFX GU10 Color Intl", Color, NEVER, Some((1500, 9000))),
    product(137, "LIFX Candle C", Matrix, NEVER, Some((1500, 9000))).with_quirk(MatrixQuirk::CandleTopZone),
    product(138, "LIFX Candle C", Matrix, NEVER, Some((1500, 9000))).with_quirk(MatrixQuirk::CandleTopZone),
    product(141, "LIFX Neon", Multizone, ALWAYS, Some((1500, 9000))),
    product(142, "LIFX Neon", Multizone, ALWAYS, Some((1500, 9000))),
    product(143, "LIFX String", Multizone, ALWAYS, Some((1500, 9000))),
    product(144, "LIFX String", Multizone, ALWAYS, Some((1500, 9000))),
    product(151, "LIFX Neon Outdoor", Multizone, ALWAYS, Some((1500, 9000))),
    product(152, "LIFX Neon Outdoor", Multizone, ALWAYS, Some((1500, 9000))),
    product(153, "LIFX PAR38 US", Color, NEVER, Some((1500, 9000))),
    product(154, "LIFX PAR38 Intl", Color, NEVER, Some((1500, 9000))),
    product(155, "LIFX Sunshine", Color, NEVER, Some((1500, 9000))),
    product(156, "LIFX A21 1000lm Intl", Color, NEVER, Some((1500, 9000))),
    product(161, "LIFX Neon Outdoor", Multizone, ALWAYS, Some((1500, 9000))),
    product(162, "LIFX Neon Outdoor", Multizone, ALWAYS, Some((1500, 9000))),
    product(163, "LIFX A19", Color, NEVER, Some((1500, 9000))),
    product(164, "LIFX BR30", Color, NEVER, Some((1500, 9000))),
    product(165, "LIFX A19 Intl", Color, NEVER, Some((1500, 9000))),
    product(166, "LIFX BR30 Intl", Color, NEVER, Some((1500, 9000))),
    product(167, "LIFX DL AU", Color, NEVER, Some((1500, 9000))),
    product(168, "LIFX DL US", Color, NEVER, Some((1500, 9000))),
    product(169, "LIFX A21", Color, NEVER, Some((1500, 9000))),
    product(170, "LIFX A21", Color, NEVER, Some((1500, 9000))),
    product(171, "LIFX Spot", Matrix, NEVER, Some((1500, 9000))),
    product(172, "LIFX Spot", Matrix, NEVER, Some((1500, 9000))),
    product(173, "LIFX Path", Matrix, NEVER, Some((1500, 9000))),
    product(174, "LIFX Path", Matrix, NEVER, Some((1500, 9000))),
    product(175, "LIFX PAR38", Color, NEVER, Some((1500, 9000))),
    product(176, "LIFX Ceiling", Matrix, NEVER, Some((1500, 9000))).with_uplight(7, 7),
    product(177, "LIFX Ceiling", Matrix, NEVER, Some((1500, 9000))).with_uplight(7, 7),
    product(178, "LIFX Downlight US", Color, NEVER, Some((1500, 9000))),
    product(179, "LIFX Downlight US", Color, NEVER, Some((1500, 9000))),
    product(180, "LIFX Downlight US", Color, NEVER, Some((1500, 9000))),
    product(181, "LIFX Mini", Color, NEVER, Some((1500, 9000))),
    product(182, "LIFX Mini", Color, NEVER, Some((1500, 9000))),
    product(185, "LIFX Candle C", Matrix, NEVER, Some((1500, 9000))).with_quirk(MatrixQuirk::CandleTopZone),
    product(186, "LIFX Candle C Intl", Matrix, NEVER, Some((1500, 9000))).with_quirk(MatrixQuirk::CandleTopZone),
    product(187, "LIFX Candle", Color, NEVER, Some((1500, 9000))),
    product(188, "LIFX Candle Intl", Color, NEVER, Some((1500, 9000))),
    product(191, "LIFX Everyday A19", Color, NEVER, Some((2700, 6500))),
    product(192, "LIFX Everyday A19 Intl", Color, NEVER, Some((2700, 6500))),
    product(201, "LIFX Ceiling 13x26", Matrix, NEVER, Some((1500, 9000))).with_uplight(15, 7),
    product(202, "LIFX Ceiling 13x26 Intl", Matrix, NEVER, Some((1500, 9000))).with_uplight(15, 7),
    product(203, "LIFX String", Multizone, ALWAYS, Some((1500, 9000))),
    product(204, "LIFX String", Multizone, ALWAYS, Some((1500, 9000))),
    product(205, "LIFX Neon", Multizone, ALWAYS, Some((1500, 9000))),
    product(206, "LIFX Neon", Multizone, ALWAYS, Some((1500, 9000))),
    product(207, "LIFX Everyday Lightstrip", Multizone, ALWAYS, Some((2500, 6500))),
    product(208, "LIFX Everyday Lightstrip Intl", Multizone, ALWAYS, Some((2500, 6500))),
    product(211, "LIFX Everyday Permanent Outdoor US", Multizone, ALWAYS, Some((2700, 6500))),
    product(213, "LIFX Permanent Outdoor", Multizone, ALWAYS, Some((1500, 9000))),
    product(214, "LIFX Permanent Outdoor Intl", Multizone, ALWAYS, Some((1500, 9000))),
    product(215, "LIFX Candle C", Matrix, NEVER, Some((1500, 9000))).with_quirk(MatrixQuirk::CandleTopZone),
    product(216, "LIFX Candle C Intl", Matrix, NEVER, Some((1500, 9000))).with_quirk(MatrixQuirk::CandleTopZone),
    product(217, "LIFX Tube", Matrix, NEVER, Some((1500, 9000))),
    product(218, "LIFX Tube Intl", Matrix, NEVER, Some((1500, 9000))),
    product(219, "LIFX Luna", Matrix, NEVER, Some((1500, 9000))),
    product(220, "LIFX Luna Intl", Matrix, NEVER, Some((1500, 9000))),
    product(221, "LIFX Spot Intl", Matrix, NEVER, Some((1500, 9000))),
    product(222, "LIFX Path Intl", Matrix, NEVER, Some((1500, 9000))),
    product(223, "LIFX DL", Color, NEVER, Some((1500, 9000))),
    product(224, "LIFX DL Intl", Color, NEVER, Some((1500, 9000))),
    product(225, "LIFX PAR38 Intl", Color, NEVER, Some((1500, 9000))),
    product(226, "LIFX Dimmer Switch", NonLight, NEVER, NO_KELVIN),
    product(229, "LIFX Path Intl", Matrix, NEVER, Some((1000, 10000))),
    product(265, "LIFX Ceiling 13\"", Matrix, NEVER, Some((1500, 9000))).with_uplight(7, 7),
    product(266, "LIFX Ceiling 13\" Intl", Matrix, NEVER, Some((1500, 9000))).with_uplight(7, 7),
    product(267, "LIFX Mirror", Matrix, NEVER, Some((1500, 9000))),
    product(268, "LIFX Mirror Intl", Matrix, NEVER, Some((1500, 9000))),
    product(300, "LIFX Everyday Permanent Outdoor US", Multizone, NEVER, Some((2700, 6500))),
];
