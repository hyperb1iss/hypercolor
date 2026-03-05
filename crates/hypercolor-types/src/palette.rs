//! Unified palette registry — the color language of Hypercolor.
//!
//! All 28 named palettes are defined in `sdk/shared/palettes.json` and
//! loaded at compile time via `include_str!`. This module provides the
//! [`Palette`] enum, color sampling, and mood-based lookup.

use std::f32::consts::PI;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

// ── Raw JSON Palette Data ───────────────────────────────────────────────

/// The palette registry JSON, embedded at compile time.
const PALETTES_JSON: &str = include_str!("../../../sdk/shared/palettes.json");

/// A single palette definition from the JSON registry.
#[derive(Debug, Clone, Deserialize)]
struct PaletteDefinition {
    id: String,
    name: String,
    mood: Vec<String>,
    stops: Vec<String>,
    iq: IqParams,
    accent: String,
    background: String,
}

/// IQ cosine palette parameters.
#[derive(Debug, Clone, Deserialize)]
struct IqParams {
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
}

/// Parsed and ready-to-sample palette data.
#[derive(Debug, Clone)]
struct PaletteData {
    name: String,
    id: String,
    mood: Vec<String>,
    stops: Vec<[f32; 3]>,
    iq: IqParams,
    accent: [f32; 3],
    background: [f32; 3],
}

// ── Lazy Registry ───────────────────────────────────────────────────────

static REGISTRY: LazyLock<Vec<PaletteData>> = LazyLock::new(|| {
    let defs: Vec<PaletteDefinition> =
        serde_json::from_str(PALETTES_JSON).expect("palettes.json is valid");

    defs.into_iter()
        .map(|def| PaletteData {
            name: def.name,
            id: def.id,
            mood: def.mood,
            stops: def.stops.iter().map(|hex| parse_hex_rgb(hex)).collect(),
            iq: def.iq,
            accent: parse_hex_rgb(&def.accent),
            background: parse_hex_rgb(&def.background),
        })
        .collect()
});

// ── Palette Enum ────────────────────────────────────────────────────────

/// Named palette identifier.
///
/// Each variant maps to an entry in `sdk/shared/palettes.json`. The
/// integer value is the index into the registry — safe for use as a
/// GLSL uniform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Palette {
    SilkCircuit,
    Cyberpunk,
    Vaporwave,
    Synthwave,
    NeonFlux,
    Matrix,
    Aurora,
    Ocean,
    Forest,
    Sunset,
    CherryBlossom,
    Fire,
    Lava,
    Ember,
    Solar,
    Ice,
    DeepSea,
    Midnight,
    Frost,
    Viridis,
    Inferno,
    Plasma,
    Magma,
    Candy,
    Pastel,
    CottonCandy,
    Mono,
    Phosphor,
}

/// Total number of named palettes.
pub const PALETTE_COUNT: usize = 28;

impl Palette {
    /// All palette variants in registry order.
    pub const ALL: [Self; PALETTE_COUNT] = [
        Self::SilkCircuit,
        Self::Cyberpunk,
        Self::Vaporwave,
        Self::Synthwave,
        Self::NeonFlux,
        Self::Matrix,
        Self::Aurora,
        Self::Ocean,
        Self::Forest,
        Self::Sunset,
        Self::CherryBlossom,
        Self::Fire,
        Self::Lava,
        Self::Ember,
        Self::Solar,
        Self::Ice,
        Self::DeepSea,
        Self::Midnight,
        Self::Frost,
        Self::Viridis,
        Self::Inferno,
        Self::Plasma,
        Self::Magma,
        Self::Candy,
        Self::Pastel,
        Self::CottonCandy,
        Self::Mono,
        Self::Phosphor,
    ];

    /// Registry index (0-based). Safe for use as a shader uniform.
    #[must_use]
    #[expect(
        clippy::as_conversions,
        reason = "enum discriminant to index is always safe"
    )]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Display name from the palette registry.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        &REGISTRY[self.index()].name
    }

    /// Kebab-case identifier.
    #[must_use]
    pub fn id(self) -> &'static str {
        &REGISTRY[self.index()].id
    }

    /// Mood tags for this palette.
    #[must_use]
    pub fn mood(self) -> &'static [String] {
        &REGISTRY[self.index()].mood
    }

    /// Color stops as linear RGB triplets `[r, g, b]` in 0.0–1.0.
    #[must_use]
    pub fn stops(self) -> &'static [[f32; 3]] {
        &REGISTRY[self.index()].stops
    }

    /// Accent color as linear RGB.
    #[must_use]
    pub fn accent(self) -> [f32; 3] {
        REGISTRY[self.index()].accent
    }

    /// Background color as linear RGB.
    #[must_use]
    pub fn background(self) -> [f32; 3] {
        REGISTRY[self.index()].background
    }

    /// Sample the palette at position `t` (0.0–1.0) using gradient stop interpolation.
    ///
    /// Interpolates linearly between the nearest stops. Values outside
    /// 0.0–1.0 are clamped.
    #[must_use]
    pub fn sample(self, t: f32) -> [f32; 3] {
        let stops = self.stops();
        gradient_sample(stops, t)
    }

    /// Sample using the IQ cosine palette formula for smooth continuous color.
    ///
    /// `color(t) = a + b * cos(2π * (c * t + d))`
    #[must_use]
    pub fn sample_iq(self, t: f32) -> [f32; 3] {
        let iq = &REGISTRY[self.index()].iq;
        iq_sample(&iq.a, &iq.b, &iq.c, &iq.d, t)
    }

    /// All palette display names in registry order.
    #[must_use]
    pub fn names() -> Vec<&'static str> {
        REGISTRY.iter().map(|data| data.name.as_str()).collect()
    }

    /// Find palettes matching a mood tag (case-insensitive).
    #[must_use]
    pub fn by_mood(mood: &str) -> Vec<Self> {
        let lower = mood.to_lowercase();
        Self::ALL
            .into_iter()
            .filter(|palette| {
                REGISTRY[palette.index()]
                    .mood
                    .iter()
                    .any(|m| m.to_lowercase() == lower)
            })
            .collect()
    }

    /// Look up a palette by kebab-case id.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|palette| REGISTRY[palette.index()].id == id)
    }
}

impl std::fmt::Display for Palette {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

// ── Color Math ──────────────────────────────────────────────────────────

/// Linear interpolation between gradient stops.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "t is clamped to 0.0-1.0, idx is always non-negative and within stop count"
)]
fn gradient_sample(stops: &[[f32; 3]], t: f32) -> [f32; 3] {
    if stops.is_empty() {
        return [0.0; 3];
    }
    if stops.len() == 1 {
        return stops[0];
    }

    let t = t.clamp(0.0, 1.0);
    #[expect(
        clippy::cast_precision_loss,
        reason = "stop count is always small (3-8)"
    )]
    let scaled = t * (stops.len() - 1) as f32;
    let idx = scaled.floor() as usize;
    let frac = scaled - scaled.floor();

    if idx >= stops.len() - 1 {
        return stops[stops.len() - 1];
    }

    let a = stops[idx];
    let b = stops[idx + 1];
    [
        a[0] + (b[0] - a[0]) * frac,
        a[1] + (b[1] - a[1]) * frac,
        a[2] + (b[2] - a[2]) * frac,
    ]
}

/// IQ cosine palette: `offset + amplitude * cos(2π(frequency * t + phase))`.
fn iq_sample(
    offset: &[f32; 3],
    amplitude: &[f32; 3],
    frequency: &[f32; 3],
    phase: &[f32; 3],
    t: f32,
) -> [f32; 3] {
    [
        (offset[0] + amplitude[0] * (2.0 * PI * (frequency[0] * t + phase[0])).cos())
            .clamp(0.0, 1.0),
        (offset[1] + amplitude[1] * (2.0 * PI * (frequency[1] * t + phase[1])).cos())
            .clamp(0.0, 1.0),
        (offset[2] + amplitude[2] * (2.0 * PI * (frequency[2] * t + phase[2])).cos())
            .clamp(0.0, 1.0),
    ]
}

/// Parse `#RRGGBB` hex string to `[r, g, b]` in 0.0–1.0 (sRGB).
fn parse_hex_rgb(hex: &str) -> [f32; 3] {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return [0.0; 3];
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    ]
}
