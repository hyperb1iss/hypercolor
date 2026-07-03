//! Shared vocabulary for display-face composition: how a face layers
//! over the live effect on a screen.
//!
//! One home for the blend-mode labels ("Cutout" for Alpha), their
//! user-facing blurbs, and the quick-look presets, mounted by both the
//! Displays workspace and the Studio composition panel so the two
//! surfaces can never drift. Deliberately leptos-free so the contract
//! is exercisable from `tests/` via a `#[path]` include, mirroring
//! `label_utils.rs`.

use hypercolor_types::scene::DisplayFaceBlendMode;

/// One face blend mode as the UI presents it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FaceBlendOption {
    pub mode: DisplayFaceBlendMode,
    pub label: &'static str,
    pub blurb: &'static str,
}

/// A named blend + opacity combination offered as a one-tap look.
#[derive(Clone, Copy)]
pub struct FaceBlendPreset {
    pub label: &'static str,
    pub mode: DisplayFaceBlendMode,
    pub opacity: f32,
}

pub const FACE_BLEND_OPTIONS: [FaceBlendOption; 11] = [
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Replace,
        label: "Replace",
        blurb: "Render the face on its own. Transparent regions stay empty instead of pulling in the live effect.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Alpha,
        label: "Cutout",
        blurb: "Use face transparency as a clean reveal into the live effect layer.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Tint,
        label: "Effect Tint",
        blurb: "Let the effect provide the living color while the face behaves like tinted material.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::LumaReveal,
        label: "Luma Reveal",
        blurb: "Drive bright face details from the effect while darker panels stay anchored to the face artwork.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Screen,
        label: "Screen",
        blurb: "Fuse face highlights with the effect for luminous neon glass.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Add,
        label: "Add",
        blurb: "Push both layers together for hotter, flashier glow.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Multiply,
        label: "Multiply",
        blurb: "Turn the face into tinted glass that darkens and colors the effect.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Overlay,
        label: "Overlay",
        blurb: "Blend contrast-rich UI material that pops without flattening the effect.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::SoftLight,
        label: "Soft Light",
        blurb: "Keep the effect alive under a softer satin face treatment.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::ColorDodge,
        label: "Color Dodge",
        blurb: "Turn bright face areas into intense reactive highlights.",
    },
    FaceBlendOption {
        mode: DisplayFaceBlendMode::Difference,
        label: "Difference",
        blurb: "Create reactive inversions for wilder holographic looks.",
    },
];

pub const FACE_BLEND_PRESETS: [FaceBlendPreset; 6] = [
    FaceBlendPreset {
        label: "Clean Reveal",
        mode: DisplayFaceBlendMode::Alpha,
        opacity: 0.78,
    },
    FaceBlendPreset {
        label: "Neon Glass",
        mode: DisplayFaceBlendMode::Screen,
        opacity: 0.88,
    },
    FaceBlendPreset {
        label: "Signal Mask",
        mode: DisplayFaceBlendMode::LumaReveal,
        opacity: 1.0,
    },
    FaceBlendPreset {
        label: "Tinted HUD",
        mode: DisplayFaceBlendMode::Tint,
        opacity: 0.92,
    },
    FaceBlendPreset {
        label: "Smoked Panel",
        mode: DisplayFaceBlendMode::Multiply,
        opacity: 0.84,
    },
    FaceBlendPreset {
        label: "Hot Bloom",
        mode: DisplayFaceBlendMode::Add,
        opacity: 0.54,
    },
];

/// Resolve the presentation of one blend mode. Falls back to the first
/// option (Replace) for a mode the table somehow misses, so the UI
/// always has a label to show.
#[must_use]
pub fn face_blend_option(mode: DisplayFaceBlendMode) -> FaceBlendOption {
    FACE_BLEND_OPTIONS
        .iter()
        .copied()
        .find(|option| option.mode == mode)
        .unwrap_or(FACE_BLEND_OPTIONS[0])
}

/// Snake-case wire token for a face blend mode — the serde encoding.
#[must_use]
pub fn face_blend_value(mode: DisplayFaceBlendMode) -> &'static str {
    match mode {
        DisplayFaceBlendMode::Replace => "replace",
        DisplayFaceBlendMode::Alpha => "alpha",
        DisplayFaceBlendMode::Tint => "tint",
        DisplayFaceBlendMode::LumaReveal => "luma_reveal",
        DisplayFaceBlendMode::Add => "add",
        DisplayFaceBlendMode::Screen => "screen",
        DisplayFaceBlendMode::Multiply => "multiply",
        DisplayFaceBlendMode::Overlay => "overlay",
        DisplayFaceBlendMode::SoftLight => "soft_light",
        DisplayFaceBlendMode::ColorDodge => "color_dodge",
        DisplayFaceBlendMode::Difference => "difference",
    }
}

/// Parse a wire token back to a blend mode, defaulting to the blended
/// composition for an unknown value.
#[must_use]
pub fn parse_face_blend(value: &str) -> DisplayFaceBlendMode {
    FACE_BLEND_OPTIONS
        .iter()
        .find(|option| face_blend_value(option.mode) == value)
        .map_or(DisplayFaceBlendMode::Alpha, |option| option.mode)
}

/// `(value, label)` options for a `SilkSelect` dropdown, in table order.
#[must_use]
pub fn face_blend_select_options() -> Vec<(String, String)> {
    FACE_BLEND_OPTIONS
        .iter()
        .map(|option| {
            (
                face_blend_value(option.mode).to_owned(),
                option.label.to_owned(),
            )
        })
        .collect()
}
