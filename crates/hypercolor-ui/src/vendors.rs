//! Vendor brand registry — primary identity layer for hardware on device cards
//! and brand surfaces.
//!
//! ## What lives here
//!
//! Each supported vendor has a [`VendorBrand`] entry: a slug, a display name,
//! factual brand colors, an optional embedded SVG mark or PNG image, a fallback
//! typographic wordmark, and the set of `driver_id` strings that resolve to
//! this brand. The [`VendorMark`] component renders the chip used across the
//! UI, tinted with the brand's primary color.
//!
//! ## Asset sources & attribution
//!
//! - **SimpleIcons.org SVGs** — `Razer, ASUS, Corsair, MSI, Alienware, Cooler
//!   Master, HyperX, NZXT, Philips Hue, QMK, Sony, SteelSeries`. The SVG
//!   renditions are licensed CC0 by the SimpleIcons project; the underlying
//!   marks remain trademarks of their respective owners and are used here for
//!   nominative identification of supported hardware.
//! - **Wikimedia Commons SVGs** — `Logitech, Lian Li, ASRock, EVGA, Gigabyte,
//!   Thermaltake, Ableton`. Each file's metadata was individually verified to
//!   carry the `Public domain` license tag (PD-textlogo: artwork below the
//!   originality threshold, with the standard `Restrictions: trademarked`
//!   caveat). Same nominative-use posture as above.
//! - **English Wikipedia uploads** — `Fnatic` (SVG), `Roccat` (PNG),
//!   `Nanoleaf` (PNG). Trademarks of their respective owners; included here
//!   solely to identify supported hardware.
//! - **Vendor CDN** — `Glorious` SVG served from the official Glorious
//!   storefront CDN. Trademark of Glorious LLC, used for nominative
//!   identification of supported hardware.
//! - **WLED PNG** — official brand mark from the WLED web installer
//!   (`install.wled.me`). The WLED firmware is EUPL-1.2-licensed and the
//!   Akemi-derived brand mark is used here for nominative identification of
//!   WLED-compatible hardware.
//! - **Nollie PNG** — Hypercolor's own product, embedded as a Trunk asset.
//! - **Wordmarks** — for vendors without a verified-permissive logo source we
//!   render a compact letterform/wordmark in our own font stack and the
//!   factual brand color. These are generic typography, not logo reproductions.
//!
//! Brand marks are referenced solely to identify devices supported by this
//! software. All trademarks are property of their respective owners.

use leptos::prelude::*;

// ── Data model ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VendorFont {
    /// JetBrains Mono — terminal/dev/keyboard-focused brands
    Mono,
    /// Satoshi (loaded via bunny.net) — default cleanly-cut sans
    Sans,
    /// Orbitron — futuristic / gaming
    Display,
}

impl VendorFont {
    pub fn css_value(self) -> &'static str {
        match self {
            Self::Mono => "'JetBrains Mono', ui-monospace, monospace",
            Self::Sans => "'Satoshi', ui-sans-serif, system-ui, sans-serif",
            Self::Display => "'Orbitron', 'Satoshi', ui-sans-serif, system-ui, sans-serif",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VendorBrand {
    pub slug: &'static str,
    pub display_name: &'static str,
    /// Primary brand color as `"r, g, b"` (factual reference).
    pub primary_rgb: &'static str,
    pub secondary_rgb: &'static str,
    /// Compact letterform/wordmark used as a fallback when no SVG/image is
    /// available, and as a render-failure backstop for the vendors that have
    /// SVGs.
    pub monogram: &'static str,
    pub mark_font: VendorFont,
    /// Embedded SVG content rendered inline with `currentColor` tinting.
    /// Preferred over `image_path` when both are set.
    pub svg_content: Option<&'static str>,
    /// Asset path served by Trunk (e.g. `/assets/vendors/nollie.png`). Used
    /// when the brand has a non-SVG image (Nollie's gradient wordmark PNG).
    pub image_path: Option<&'static str>,
    pub website: &'static str,
    /// `driver_id` / `backend_id` strings that resolve to this brand.
    pub aliases: &'static [&'static str],
}

// ── Vendor list ─────────────────────────────────────────────────────────────

pub const VENDORS: &[VendorBrand] = &[
    VendorBrand {
        slug: "ableton",
        display_name: "Ableton",
        primary_rgb: "250, 100, 0",
        secondary_rgb: "192, 249, 75",
        monogram: "ABL",
        mark_font: VendorFont::Sans,
        svg_content: Some(include_str!("../assets/vendors/ableton.svg")),
        image_path: None,
        website: "https://ableton.com",
        aliases: &["ableton", "push2"],
    },
    VendorBrand {
        slug: "alienware",
        display_name: "Alienware",
        primary_rgb: "0, 174, 239",
        secondary_rgb: "30, 215, 255",
        monogram: "AW",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/alienware.svg")),
        image_path: None,
        website: "https://dell.com/alienware",
        aliases: &["alienware", "dell"],
    },
    VendorBrand {
        slug: "aquacomputer",
        display_name: "Aqua Computer",
        primary_rgb: "0, 121, 193",
        secondary_rgb: "64, 196, 255",
        monogram: "AQUA",
        mark_font: VendorFont::Sans,
        svg_content: None,
        image_path: None,
        website: "https://aquacomputer.de",
        aliases: &["aquacomputer", "aqua"],
    },
    VendorBrand {
        slug: "asrock",
        display_name: "ASRock",
        primary_rgb: "56, 124, 192",
        secondary_rgb: "96, 165, 220",
        monogram: "AR",
        mark_font: VendorFont::Sans,
        svg_content: Some(include_str!("../assets/vendors/asrock.svg")),
        image_path: None,
        website: "https://asrock.com",
        aliases: &["asrock"],
    },
    VendorBrand {
        slug: "asus",
        display_name: "ASUS",
        primary_rgb: "255, 16, 16",
        secondary_rgb: "0, 174, 239",
        monogram: "A",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/asus.svg")),
        image_path: None,
        website: "https://asus.com",
        aliases: &["asus", "rog"],
    },
    VendorBrand {
        slug: "coolermaster",
        display_name: "Cooler Master",
        primary_rgb: "110, 15, 255",
        secondary_rgb: "154, 100, 255",
        monogram: "CM",
        mark_font: VendorFont::Sans,
        svg_content: Some(include_str!("../assets/vendors/coolermaster.svg")),
        image_path: None,
        website: "https://coolermaster.com",
        aliases: &["coolermaster", "cooler_master", "cm"],
    },
    VendorBrand {
        slug: "corsair",
        display_name: "Corsair",
        primary_rgb: "255, 235, 16",
        secondary_rgb: "255, 200, 0",
        monogram: "C",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/corsair.svg")),
        image_path: None,
        website: "https://corsair.com",
        aliases: &["corsair", "icue"],
    },
    VendorBrand {
        slug: "dygma",
        display_name: "Dygma",
        primary_rgb: "255, 0, 140",
        secondary_rgb: "255, 100, 180",
        monogram: "DYG",
        mark_font: VendorFont::Sans,
        svg_content: None,
        image_path: None,
        website: "https://dygma.com",
        aliases: &["dygma"],
    },
    VendorBrand {
        slug: "evga",
        display_name: "EVGA",
        primary_rgb: "43, 111, 182",
        secondary_rgb: "80, 150, 220",
        monogram: "E",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/evga.svg")),
        image_path: None,
        website: "https://evga.com",
        aliases: &["evga"],
    },
    VendorBrand {
        slug: "fnatic",
        display_name: "Fnatic",
        primary_rgb: "255, 89, 0",
        secondary_rgb: "255, 140, 60",
        monogram: "FN",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/fnatic.svg")),
        image_path: None,
        website: "https://fnatic.com",
        aliases: &["fnatic"],
    },
    VendorBrand {
        slug: "gigabyte",
        display_name: "Gigabyte",
        primary_rgb: "110, 51, 204",
        secondary_rgb: "250, 117, 0",
        monogram: "GB",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/gigabyte.svg")),
        image_path: None,
        website: "https://gigabyte.com",
        aliases: &["gigabyte", "aorus"],
    },
    VendorBrand {
        slug: "glorious",
        display_name: "Glorious",
        primary_rgb: "184, 156, 94",
        secondary_rgb: "215, 195, 130",
        monogram: "GLR",
        mark_font: VendorFont::Sans,
        svg_content: Some(include_str!("../assets/vendors/glorious.svg")),
        image_path: None,
        website: "https://gloriousgaming.com",
        aliases: &["glorious"],
    },
    VendorBrand {
        slug: "govee",
        display_name: "Govee",
        primary_rgb: "255, 69, 0",
        secondary_rgb: "255, 130, 60",
        monogram: "GOV",
        mark_font: VendorFont::Sans,
        svg_content: None,
        image_path: None,
        website: "https://govee.com",
        aliases: &["govee"],
    },
    VendorBrand {
        slug: "hyperx",
        display_name: "HyperX",
        primary_rgb: "218, 27, 44",
        secondary_rgb: "255, 80, 90",
        monogram: "HX",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/hyperx.svg")),
        image_path: None,
        website: "https://hyperx.com",
        aliases: &["hyperx", "kingston"],
    },
    VendorBrand {
        slug: "hyte",
        display_name: "HYTE",
        primary_rgb: "0, 230, 230",
        secondary_rgb: "80, 250, 200",
        monogram: "HY",
        mark_font: VendorFont::Display,
        svg_content: None,
        image_path: None,
        website: "https://hyte.com",
        aliases: &["hyte"],
    },
    VendorBrand {
        slug: "lianli",
        display_name: "Lian Li",
        primary_rgb: "220, 20, 60",
        secondary_rgb: "255, 90, 100",
        monogram: "LL",
        mark_font: VendorFont::Sans,
        svg_content: Some(include_str!("../assets/vendors/lianli.svg")),
        image_path: None,
        website: "https://lian-li.com",
        aliases: &["lianli", "lian_li", "lian-li"],
    },
    VendorBrand {
        slug: "logitech",
        display_name: "Logitech",
        primary_rgb: "0, 184, 252",
        secondary_rgb: "80, 220, 255",
        monogram: "L",
        mark_font: VendorFont::Sans,
        svg_content: Some(include_str!("../assets/vendors/logitech.svg")),
        image_path: None,
        website: "https://logitech.com",
        aliases: &["logitech", "logi", "logitech_g"],
    },
    VendorBrand {
        slug: "mountain",
        display_name: "Mountain",
        primary_rgb: "255, 102, 0",
        secondary_rgb: "255, 160, 50",
        monogram: "MTN",
        mark_font: VendorFont::Sans,
        svg_content: None,
        image_path: None,
        website: "https://mountain.gg",
        aliases: &["mountain"],
    },
    VendorBrand {
        slug: "msi",
        display_name: "MSI",
        primary_rgb: "255, 0, 0",
        secondary_rgb: "220, 30, 30",
        monogram: "MSI",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/msi.svg")),
        image_path: None,
        website: "https://msi.com",
        aliases: &["msi"],
    },
    VendorBrand {
        slug: "nanoleaf",
        display_name: "Nanoleaf",
        primary_rgb: "255, 111, 0",
        secondary_rgb: "255, 165, 60",
        monogram: "NANO",
        mark_font: VendorFont::Sans,
        svg_content: None,
        image_path: Some("/assets/vendors/nanoleaf.png"),
        website: "https://nanoleaf.me",
        aliases: &["nanoleaf"],
    },
    VendorBrand {
        slug: "nollie",
        display_name: "Nollie",
        primary_rgb: "128, 200, 255",
        secondary_rgb: "255, 106, 193",
        monogram: "N",
        mark_font: VendorFont::Sans,
        svg_content: None,
        image_path: Some("/assets/vendors/nollie.png"),
        website: "https://nollie.gg",
        aliases: &["nollie"],
    },
    VendorBrand {
        slug: "nzxt",
        display_name: "NZXT",
        primary_rgb: "110, 0, 200",
        secondary_rgb: "165, 80, 255",
        monogram: "NZ",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/nzxt.svg")),
        image_path: None,
        website: "https://nzxt.com",
        aliases: &["nzxt"],
    },
    VendorBrand {
        slug: "philips",
        display_name: "Philips Hue",
        primary_rgb: "0, 181, 226",
        secondary_rgb: "70, 220, 255",
        monogram: "H",
        mark_font: VendorFont::Sans,
        svg_content: Some(include_str!("../assets/vendors/philipshue.svg")),
        image_path: None,
        website: "https://philips-hue.com",
        aliases: &["hue", "philips", "philipshue", "philips_hue", "signify"],
    },
    VendorBrand {
        slug: "prismrgb",
        display_name: "PrismRGB",
        primary_rgb: "225, 53, 255",
        secondary_rgb: "128, 255, 234",
        monogram: "PRSM",
        mark_font: VendorFont::Display,
        svg_content: None,
        image_path: None,
        website: "",
        aliases: &["prismrgb", "prism_rgb", "prism"],
    },
    VendorBrand {
        slug: "qmk",
        display_name: "QMK",
        primary_rgb: "66, 135, 245",
        secondary_rgb: "100, 175, 255",
        monogram: "Q",
        mark_font: VendorFont::Mono,
        svg_content: Some(include_str!("../assets/vendors/qmk.svg")),
        image_path: None,
        website: "https://qmk.fm",
        aliases: &["qmk", "vial"],
    },
    VendorBrand {
        slug: "razer",
        display_name: "Razer",
        primary_rgb: "68, 214, 44",
        secondary_rgb: "100, 240, 80",
        monogram: "R",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/razer.svg")),
        image_path: None,
        website: "https://razer.com",
        aliases: &["razer", "chroma"],
    },
    VendorBrand {
        slug: "roccat",
        display_name: "Roccat",
        primary_rgb: "0, 144, 220",
        secondary_rgb: "60, 180, 240",
        monogram: "ROC",
        mark_font: VendorFont::Display,
        svg_content: None,
        image_path: Some("/assets/vendors/roccat.png"),
        website: "https://roccat.com",
        aliases: &["roccat"],
    },
    VendorBrand {
        slug: "sony",
        display_name: "Sony",
        primary_rgb: "0, 112, 209",
        secondary_rgb: "50, 160, 240",
        monogram: "S",
        mark_font: VendorFont::Sans,
        svg_content: Some(include_str!("../assets/vendors/sony.svg")),
        image_path: None,
        website: "https://sony.com",
        aliases: &["sony", "playstation", "ps"],
    },
    VendorBrand {
        slug: "steelseries",
        display_name: "SteelSeries",
        primary_rgb: "255, 84, 0",
        secondary_rgb: "255, 140, 50",
        monogram: "SS",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/steelseries.svg")),
        image_path: None,
        website: "https://steelseries.com",
        aliases: &["steelseries", "steel_series"],
    },
    VendorBrand {
        slug: "thermaltake",
        display_name: "Thermaltake",
        primary_rgb: "220, 20, 60",
        secondary_rgb: "255, 90, 90",
        monogram: "TT",
        mark_font: VendorFont::Display,
        svg_content: Some(include_str!("../assets/vendors/thermaltake.svg")),
        image_path: None,
        website: "https://thermaltake.com",
        aliases: &["thermaltake", "tt"],
    },
    VendorBrand {
        slug: "wled",
        display_name: "WLED",
        primary_rgb: "0, 188, 212",
        secondary_rgb: "80, 220, 240",
        monogram: "WLED",
        mark_font: VendorFont::Mono,
        svg_content: None,
        image_path: Some("/assets/vendors/wled.png"),
        website: "https://kno.wled.ge",
        aliases: &["wled"],
    },
    VendorBrand {
        slug: "wooting",
        display_name: "Wooting",
        primary_rgb: "255, 95, 0",
        secondary_rgb: "255, 150, 60",
        monogram: "WOOT",
        mark_font: VendorFont::Display,
        svg_content: None,
        image_path: None,
        website: "https://wooting.io",
        aliases: &["wooting"],
    },
];

// ── Lookup ──────────────────────────────────────────────────────────────────

/// Resolve a `driver_id` / `backend_id` to a vendor brand. Case-insensitive.
#[must_use]
pub fn lookup(identifier: &str) -> Option<&'static VendorBrand> {
    let trimmed = identifier.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    VENDORS
        .iter()
        .find(|v| v.aliases.iter().any(|a| a.eq_ignore_ascii_case(&lower)))
}

/// Resolve from the first non-empty identifier in `candidates`. Useful when a
/// device exposes both a `driver_id` and a `backend_id` and we want the most
/// specific match available.
#[must_use]
pub fn resolve_first(candidates: &[&str]) -> Option<&'static VendorBrand> {
    candidates
        .iter()
        .filter(|s| !s.trim().is_empty())
        .find_map(|s| lookup(s))
}

// ── Component ───────────────────────────────────────────────────────────────

/// Visual size variants for [`VendorMark`]. Uses concrete pixel sizes so the
/// component is predictable when nested in dense card grids.
#[derive(Clone, Copy, Debug)]
pub enum VendorMarkSize {
    /// 16px chip — inline meta lines, dense lists.
    Xs,
    /// 24px chip — compact rows, table cells.
    Sm,
    /// 40px chip — device card hero.
    Md,
}

impl VendorMarkSize {
    /// Returns `(chip_px, font_px, radius_px)` for this size.
    pub fn dimensions(self, monogram_len: usize) -> (u32, u32, u32) {
        let (chip, base_font, radius) = match self {
            Self::Xs => (16, 9, 4),
            Self::Sm => (24, 12, 6),
            Self::Md => (40, 18, 10),
        };
        // Shrink font as the wordmark gets longer so it stays inside the chip.
        let font = match monogram_len {
            0 | 1 => base_font,
            2 => (base_font * 4) / 5,
            3 => (base_font * 11) / 18,
            4 => base_font / 2,
            _ => (base_font * 2) / 5,
        };
        (chip, font, radius)
    }

    /// Inner image/SVG size — chip dimensions minus padding.
    pub fn inner_px(self) -> u32 {
        match self {
            Self::Xs => 12,
            Self::Sm => 18,
            Self::Md => 28,
        }
    }
}

/// Vendor brand mark — colored chip with embedded SVG (preferred), embedded
/// image, or wordmark fallback. Tints SVG content via `currentColor` cascade.
#[component]
pub fn VendorMark(
    vendor: VendorBrand,
    #[prop(default = VendorMarkSize::Md)] size: VendorMarkSize,
) -> impl IntoView {
    let primary = vendor.primary_rgb;
    let secondary = vendor.secondary_rgb;
    let display_name = vendor.display_name;
    let (chip_px, font_px, radius_px) = size.dimensions(vendor.monogram.len());
    let inner_px = size.inner_px();

    let chip_style = format!(
        "width: {chip_px}px; height: {chip_px}px; border-radius: {radius_px}px; \
         background: linear-gradient(135deg, rgba({primary}, 0.22) 0%, rgba({secondary}, 0.10) 100%); \
         border: 1px solid rgba({primary}, 0.55); \
         box-shadow: inset 0 0 8px rgba({primary}, 0.10), 0 0 8px rgba({primary}, 0.18)"
    );

    if let Some(svg) = vendor.svg_content {
        let svg_style = format!(
            "width: {inner_px}px; height: {inner_px}px; color: rgb({primary}); \
             display: flex; align-items: center; justify-content: center; \
             filter: drop-shadow(0 0 3px rgba({primary}, 0.35))"
        );
        return view! {
            <div
                class="inline-flex items-center justify-center shrink-0"
                style=chip_style
                title=display_name
            >
                <div class="vendor-mark-svg" style=svg_style inner_html=svg />
            </div>
        }
        .into_any();
    }

    if let Some(image_path) = vendor.image_path {
        let img_style = format!(
            "width: {inner_px}px; height: {inner_px}px; object-fit: contain; \
             filter: drop-shadow(0 0 4px rgba({primary}, 0.45))"
        );
        return view! {
            <div
                class="inline-flex items-center justify-center shrink-0"
                style=chip_style
                title=display_name
            >
                <img src=image_path alt=display_name style=img_style />
            </div>
        }
        .into_any();
    }

    let font_family = vendor.mark_font.css_value();
    let mark = vendor.monogram;
    let label_style = format!(
        "font-family: {font_family}; font-size: {font_px}px; font-weight: 700; \
         letter-spacing: -0.04em; line-height: 1; \
         color: rgb({primary}); \
         text-shadow: 0 0 6px rgba({primary}, 0.45)"
    );

    view! {
        <div
            class="inline-flex items-center justify-center shrink-0 select-none"
            style=chip_style
            title=display_name
        >
            <span style=label_style>{mark}</span>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vendor_has_a_unique_slug() {
        let mut slugs: Vec<&str> = VENDORS.iter().map(|v| v.slug).collect();
        slugs.sort_unstable();
        let initial_len = slugs.len();
        slugs.dedup();
        assert_eq!(slugs.len(), initial_len, "duplicate vendor slug");
    }

    #[test]
    fn aliases_resolve_to_owning_vendor() {
        for vendor in VENDORS {
            for alias in vendor.aliases {
                let resolved = lookup(alias).expect("alias should resolve");
                assert_eq!(
                    resolved.slug, vendor.slug,
                    "alias {alias} resolved to {} instead of {}",
                    resolved.slug, vendor.slug
                );
            }
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert!(lookup("RAZER").is_some());
        assert!(lookup("Razer").is_some());
        assert!(lookup("razer").is_some());
    }

    #[test]
    fn resolve_first_skips_blank_candidates() {
        let v = resolve_first(&["", "  ", "razer"]).expect("should resolve");
        assert_eq!(v.slug, "razer");
    }

    #[test]
    fn unknown_identifier_returns_none() {
        assert!(lookup("definitely_not_a_brand").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn embedded_svgs_are_non_empty() {
        let with_svg = VENDORS.iter().filter(|v| v.svg_content.is_some()).count();
        assert!(
            with_svg >= 18,
            "expected ≥18 vendors with embedded SVGs, got {with_svg}"
        );
        for v in VENDORS {
            if let Some(svg) = v.svg_content {
                assert!(svg.contains("<svg"), "{} svg missing root element", v.slug);
                assert!(
                    svg.contains("currentColor"),
                    "{} svg missing currentColor",
                    v.slug
                );
            }
        }
    }
}
