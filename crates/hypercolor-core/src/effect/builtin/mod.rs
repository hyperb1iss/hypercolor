//! Built-in native effect renderers.
//!
//! These renderers produce real [`Canvas`](hypercolor_types::canvas::Canvas) frames
//! entirely in Rust, with no GPU shaders or web engines required. They serve as the
//! always-available utility layer for fallback visuals, diagnostics, and basic scenes.
//!
//! # Available Effects
//!
//! | Name            | Category       | Description                                     |
//! |-----------------|----------------|-------------------------------------------------|
//! | `solid_color`   | Ambient        | Solid fills plus split and checker diagnostics   |
//! | `gradient`      | Ambient        | Vivid gradient with Oklch blending and saturation |
//! | `rainbow`       | Ambient        | Cycling rainbow hue sweep                        |
//! | `breathing`     | Ambient        | Sinusoidal brightness pulsation                  |
//! | `audio_pulse`   | Audio          | RMS + beat-reactive color modulation             |
//! | `color_wave`    | Ambient        | Traveling wavefront bands with fade trails       |
//! | `color_zones`   | Ambient        | Multi-zone color grid with per-zone control      |

mod audio_pulse;
mod breathing;
mod color_wave;
mod color_zones;
mod gradient;
mod rainbow;
mod solid_color;

use std::path::PathBuf;
use std::time::SystemTime;

use uuid::Uuid;

pub use self::audio_pulse::AudioPulseRenderer;
pub use self::breathing::BreathingRenderer;
pub use self::color_wave::ColorWaveRenderer;
pub use self::color_zones::ColorZonesRenderer;
pub use self::gradient::GradientRenderer;
pub use self::rainbow::RainbowRenderer;
pub use self::solid_color::SolidColorRenderer;
use super::registry::{EffectEntry, EffectRegistry};
use super::traits::EffectRenderer;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState, PresetTemplate,
};

// ── Registry Helpers ────────────────────────────────────────────────────────

fn color_control(
    id: &str,
    name: &str,
    default_value: [f32; 4],
    group: &str,
    tooltip: &str,
) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: ControlKind::Color,
        control_type: ControlType::ColorPicker,
        default_value: ControlValue::Color(default_value),
        min: None,
        max: None,
        step: None,
        labels: Vec::new(),
        group: Some(group.to_owned()),
        tooltip: Some(tooltip.to_owned()),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "control definitions are constructed from explicit schema fields"
)]
fn slider_control(
    id: &str,
    name: &str,
    default_value: f32,
    min: f32,
    max: f32,
    step: f32,
    group: &str,
    tooltip: &str,
) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: ControlKind::Number,
        control_type: ControlType::Slider,
        default_value: ControlValue::Float(default_value),
        min: Some(min),
        max: Some(max),
        step: Some(step),
        labels: Vec::new(),
        group: Some(group.to_owned()),
        tooltip: Some(tooltip.to_owned()),
    }
}

fn toggle_control(
    id: &str,
    name: &str,
    default_value: bool,
    group: &str,
    tooltip: &str,
) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: ControlKind::Boolean,
        control_type: ControlType::Toggle,
        default_value: ControlValue::Boolean(default_value),
        min: None,
        max: None,
        step: None,
        labels: Vec::new(),
        group: Some(group.to_owned()),
        tooltip: Some(tooltip.to_owned()),
    }
}

fn dropdown_control(
    id: &str,
    name: &str,
    default_value: &str,
    labels: &[&str],
    group: &str,
    tooltip: &str,
) -> ControlDefinition {
    ControlDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: ControlKind::Combobox,
        control_type: ControlType::Dropdown,
        default_value: ControlValue::Enum(default_value.to_owned()),
        min: None,
        max: None,
        step: None,
        labels: labels.iter().map(|label| (*label).to_owned()).collect(),
        group: Some(group.to_owned()),
        tooltip: Some(tooltip.to_owned()),
    }
}

fn solid_color_controls() -> Vec<ControlDefinition> {
    vec![
        dropdown_control(
            "pattern",
            "Pattern",
            "Solid",
            &[
                "Solid",
                "Vertical Split",
                "Horizontal Split",
                "Checker",
                "Quadrants",
            ],
            "Pattern",
            "Switch between a plain fill and simple diagnostic scene layouts.",
        ),
        color_control(
            "color",
            "Primary Color",
            [1.0, 1.0, 1.0, 1.0],
            "Colors",
            "Main fill color for the scene.",
        ),
        color_control(
            "secondary_color",
            "Secondary Color",
            [0.0, 0.0, 0.0, 1.0],
            "Colors",
            "Used for split, checker, and quadrant diagnostic patterns.",
        ),
        slider_control(
            "position",
            "Split Position",
            0.5,
            0.0,
            1.0,
            0.01,
            "Pattern",
            "Boundary position for split and quadrant patterns.",
        ),
        slider_control(
            "softness",
            "Blend Softness",
            0.0,
            0.0,
            0.35,
            0.01,
            "Pattern",
            "Feather the split boundary into a soft scene blend.",
        ),
        slider_control(
            "scale",
            "Pattern Scale",
            6.0,
            1.0,
            16.0,
            1.0,
            "Pattern",
            "Checker cell count across the canvas width.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

#[allow(
    clippy::too_many_lines,
    reason = "the gradient preset control list is intentionally authored inline for readability"
)]
fn gradient_controls() -> Vec<ControlDefinition> {
    vec![
        color_control(
            "color_start",
            "Color A",
            [0.88, 0.08, 1.0, 1.0],
            "Colors",
            "Start color for the gradient.",
        ),
        toggle_control(
            "use_mid_color",
            "Use Middle Color",
            false,
            "Colors",
            "Insert a third color stop between Color A and Color C.",
        ),
        color_control(
            "color_mid",
            "Color B",
            [1.0, 0.25, 0.55, 1.0],
            "Colors",
            "Optional middle color stop for three-color gradients.",
        ),
        slider_control(
            "midpoint",
            "Middle Position",
            0.5,
            0.05,
            0.95,
            0.01,
            "Colors",
            "Placement of the middle color stop along the gradient.",
        ),
        color_control(
            "color_end",
            "Color C",
            [0.0, 1.0, 0.85, 1.0],
            "Colors",
            "End color for the gradient.",
        ),
        dropdown_control(
            "interpolation",
            "Color Blend",
            "Vivid",
            &["Vivid", "Smooth", "Direct"],
            "Colors",
            "Vivid (Oklch) keeps hues vibrant; Smooth (Oklab) blends evenly; Direct mixes RGB.",
        ),
        slider_control(
            "saturation",
            "Saturation",
            1.0,
            0.5,
            1.5,
            0.01,
            "Colors",
            "Boost or reduce color intensity. Values above 1.0 push chroma for vivid output.",
        ),
        dropdown_control(
            "mode",
            "Gradient Type",
            "Linear",
            &["Linear", "Radial"],
            "Shape",
            "Choose a directional sweep or a center-out radial gradient.",
        ),
        slider_control(
            "angle",
            "Angle",
            0.0,
            0.0,
            360.0,
            1.0,
            "Shape",
            "Direction of the linear gradient in degrees.",
        ),
        slider_control(
            "center_x",
            "Center X",
            0.5,
            0.0,
            1.0,
            0.01,
            "Shape",
            "Horizontal origin for radial gradients and linear gradient pivots.",
        ),
        slider_control(
            "center_y",
            "Center Y",
            0.5,
            0.0,
            1.0,
            0.01,
            "Shape",
            "Vertical origin for radial gradients and linear gradient pivots.",
        ),
        slider_control(
            "scale",
            "Scale",
            1.0,
            0.1,
            2.5,
            0.01,
            "Shape",
            "Tighten or spread the gradient without changing the colors.",
        ),
        dropdown_control(
            "easing",
            "Distribution",
            "Linear",
            &["Linear", "Ease In", "Ease Out", "Smooth"],
            "Shape",
            "Curve that controls how colors are distributed along the gradient.",
        ),
        dropdown_control(
            "repeat_mode",
            "Repeat",
            "Clamp",
            &["Clamp", "Repeat", "Mirror"],
            "Motion",
            "Control how the gradient behaves beyond its natural 0-1 range.",
        ),
        slider_control(
            "offset",
            "Offset",
            0.0,
            -1.0,
            1.0,
            0.01,
            "Motion",
            "Static shift along the gradient axis or radius.",
        ),
        slider_control(
            "speed",
            "Scroll Speed",
            0.0,
            -1.0,
            1.0,
            0.01,
            "Motion",
            "Animate the gradient position; negative values reverse direction.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

fn rainbow_controls() -> Vec<ControlDefinition> {
    vec![
        slider_control(
            "speed",
            "Speed",
            60.0,
            -180.0,
            180.0,
            1.0,
            "Motion",
            "Hue rotation speed in degrees per second.",
        ),
        slider_control(
            "scale",
            "Band Density",
            1.0,
            0.1,
            4.0,
            0.01,
            "Shape",
            "Lower values create broad rainbow bands; higher values add more stripes.",
        ),
        slider_control(
            "saturation",
            "Saturation",
            1.0,
            0.0,
            1.0,
            0.01,
            "Colors",
            "Color intensity. Lower values soften the rainbow; 1.0 gives fully saturated hues.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            0.75,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

fn breathing_controls() -> Vec<ControlDefinition> {
    vec![
        color_control(
            "color",
            "Color",
            [1.0, 0.6, 0.2, 1.0],
            "Colors",
            "Base color that breathes in and out.",
        ),
        slider_control(
            "speed",
            "Speed",
            15.0,
            1.0,
            120.0,
            1.0,
            "Motion",
            "Breathing rate in beats per minute.",
        ),
        slider_control(
            "min_brightness",
            "Minimum Brightness",
            0.1,
            0.0,
            1.0,
            0.01,
            "Output",
            "Brightness at the trough of the cycle.",
        ),
        slider_control(
            "max_brightness",
            "Maximum Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Brightness at the peak of the cycle.",
        ),
    ]
}

fn audio_pulse_controls() -> Vec<ControlDefinition> {
    vec![
        color_control(
            "base_color",
            "Base Color",
            [0.0, 0.1, 0.3, 1.0],
            "Colors",
            "Color shown during silence or very quiet audio.",
        ),
        color_control(
            "peak_color",
            "Peak Color",
            [1.0, 0.2, 0.5, 1.0],
            "Colors",
            "Color reached at peak RMS intensity.",
        ),
        slider_control(
            "sensitivity",
            "Sensitivity",
            2.0,
            0.1,
            4.0,
            0.01,
            "Audio",
            "Higher values react harder to quieter input.",
        ),
        slider_control(
            "beat_decay",
            "Beat Decay",
            0.85,
            0.5,
            0.99,
            0.01,
            "Audio",
            "How long the beat flash lingers after a detected beat.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

fn color_wave_controls() -> Vec<ControlDefinition> {
    vec![
        color_control(
            "wave_color",
            "Wave Color",
            [0.5, 1.0, 0.92, 1.0],
            "Colors",
            "Primary color for the traveling wavefront.",
        ),
        color_control(
            "background_color",
            "Background Color",
            [0.0, 0.02, 0.08, 1.0],
            "Colors",
            "Base fill color that the trail fades back toward.",
        ),
        dropdown_control(
            "color_mode",
            "Color Mode",
            "Custom",
            &["Custom", "Random", "Color Cycle"],
            "Colors",
            "Use a fixed color, randomize each wave, or continuously hue-cycle the wavefronts.",
        ),
        slider_control(
            "cycle_speed",
            "Color Cycle Speed",
            50.0,
            0.0,
            100.0,
            1.0,
            "Colors",
            "Hue rotation speed when Color Cycle mode is enabled.",
        ),
        slider_control(
            "speed",
            "Effect Speed",
            85.0,
            0.0,
            100.0,
            1.0,
            "Motion",
            "How quickly each wavefront moves across the canvas.",
        ),
        slider_control(
            "spawn_delay",
            "Wave Spawn Speed",
            50.0,
            0.0,
            100.0,
            1.0,
            "Motion",
            "How often new wavefronts are emitted.",
        ),
        dropdown_control(
            "direction",
            "Wave Direction",
            "Right",
            &[
                "Right",
                "Left",
                "Up",
                "Down",
                "Vertical Pass",
                "Horizontal Pass",
            ],
            "Motion",
            "Direction and pass mode for spawned wavefronts.",
        ),
        slider_control(
            "wave_width",
            "Wave Width",
            50.0,
            1.0,
            100.0,
            1.0,
            "Shape",
            "Thickness of each rectangular wave band.",
        ),
        slider_control(
            "trail",
            "Wave Trail",
            50.0,
            0.0,
            100.0,
            1.0,
            "Output",
            "How much of the previous frame remains visible behind each wave.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

// ── Preset Helpers ────────────────────────────────────────────────────────

fn preset(name: &str, controls: &[(&str, ControlValue)]) -> PresetTemplate {
    PresetTemplate {
        name: name.to_owned(),
        description: None,
        controls: controls
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    }
}

fn preset_with_desc(
    name: &str,
    description: &str,
    controls: &[(&str, ControlValue)],
) -> PresetTemplate {
    PresetTemplate {
        name: name.to_owned(),
        description: Some(description.to_owned()),
        controls: controls
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    }
}

fn breathing_presets() -> Vec<PresetTemplate> {
    vec![
        preset_with_desc(
            "Warm Ember",
            "Slow amber glow like dying embers",
            &[
                ("color", ControlValue::Color([1.0, 0.4, 0.1, 1.0])),
                ("speed", ControlValue::Float(8.0)),
                ("min_brightness", ControlValue::Float(0.05)),
                ("max_brightness", ControlValue::Float(0.8)),
            ],
        ),
        preset_with_desc(
            "Ocean Calm",
            "Deep blue with slow tidal rhythm",
            &[
                ("color", ControlValue::Color([0.1, 0.3, 1.0, 1.0])),
                ("speed", ControlValue::Float(6.0)),
                ("min_brightness", ControlValue::Float(0.08)),
                ("max_brightness", ControlValue::Float(0.7)),
            ],
        ),
        preset(
            "Alert Pulse",
            &[
                ("color", ControlValue::Color([1.0, 0.1, 0.1, 1.0])),
                ("speed", ControlValue::Float(40.0)),
                ("min_brightness", ControlValue::Float(0.2)),
                ("max_brightness", ControlValue::Float(1.0)),
            ],
        ),
    ]
}

fn gradient_presets() -> Vec<PresetTemplate> {
    vec![
        preset_with_desc(
            "Neon Blaze",
            "Electric SilkCircuit palette with vivid hue sweep",
            &[
                ("color_start", ControlValue::Color([0.88, 0.08, 1.0, 1.0])),
                ("color_end", ControlValue::Color([0.0, 1.0, 0.85, 1.0])),
                ("use_mid_color", ControlValue::Boolean(true)),
                ("color_mid", ControlValue::Color([1.0, 0.25, 0.55, 1.0])),
                ("interpolation", ControlValue::Enum("Vivid".to_owned())),
                ("speed", ControlValue::Float(0.2)),
                ("repeat_mode", ControlValue::Enum("Mirror".to_owned())),
            ],
        ),
        preset_with_desc(
            "Sunset",
            "Warm horizon gradient",
            &[
                ("color_start", ControlValue::Color([1.0, 0.3, 0.1, 1.0])),
                ("color_end", ControlValue::Color([0.4, 0.0, 0.6, 1.0])),
                ("use_mid_color", ControlValue::Boolean(true)),
                ("color_mid", ControlValue::Color([1.0, 0.6, 0.2, 1.0])),
                ("interpolation", ControlValue::Enum("Vivid".to_owned())),
                ("angle", ControlValue::Float(0.0)),
            ],
        ),
        preset_with_desc(
            "Aurora",
            "Northern lights with gentle motion",
            &[
                ("color_start", ControlValue::Color([0.0, 1.0, 0.5, 1.0])),
                ("color_end", ControlValue::Color([0.3, 0.0, 1.0, 1.0])),
                ("use_mid_color", ControlValue::Boolean(true)),
                ("color_mid", ControlValue::Color([0.0, 0.8, 1.0, 1.0])),
                ("interpolation", ControlValue::Enum("Vivid".to_owned())),
                ("speed", ControlValue::Float(0.15)),
                ("repeat_mode", ControlValue::Enum("Mirror".to_owned())),
            ],
        ),
        preset_with_desc(
            "Molten Core",
            "Deep orange through red to dark, smooth interpolation",
            &[
                ("color_start", ControlValue::Color([1.0, 0.7, 0.0, 1.0])),
                ("color_end", ControlValue::Color([0.3, 0.0, 0.0, 1.0])),
                ("use_mid_color", ControlValue::Boolean(true)),
                ("color_mid", ControlValue::Color([1.0, 0.15, 0.0, 1.0])),
                ("interpolation", ControlValue::Enum("Smooth".to_owned())),
                ("saturation", ControlValue::Float(1.2)),
                ("easing", ControlValue::Enum("Ease Out".to_owned())),
            ],
        ),
        preset_with_desc(
            "Cyberpunk Skyline",
            "Deep blue to magenta to electric pink",
            &[
                ("color_start", ControlValue::Color([0.0, 0.02, 0.2, 1.0])),
                ("color_end", ControlValue::Color([1.0, 0.08, 0.58, 1.0])),
                ("use_mid_color", ControlValue::Boolean(true)),
                ("color_mid", ControlValue::Color([0.5, 0.0, 0.8, 1.0])),
                ("interpolation", ControlValue::Enum("Vivid".to_owned())),
                ("angle", ControlValue::Float(90.0)),
            ],
        ),
        preset_with_desc(
            "Forest Canopy",
            "Dark green through emerald to golden light",
            &[
                ("color_start", ControlValue::Color([0.0, 0.15, 0.05, 1.0])),
                ("color_end", ControlValue::Color([0.95, 0.85, 0.2, 1.0])),
                ("use_mid_color", ControlValue::Boolean(true)),
                ("color_mid", ControlValue::Color([0.0, 0.7, 0.3, 1.0])),
                ("interpolation", ControlValue::Enum("Vivid".to_owned())),
                ("saturation", ControlValue::Float(1.1)),
            ],
        ),
        preset(
            "Deep Ocean",
            &[
                ("color_start", ControlValue::Color([0.0, 0.02, 0.15, 1.0])),
                ("color_end", ControlValue::Color([0.0, 0.2, 0.5, 1.0])),
                ("mode", ControlValue::Enum("Radial".to_owned())),
                ("interpolation", ControlValue::Enum("Smooth".to_owned())),
                ("speed", ControlValue::Float(0.08)),
            ],
        ),
    ]
}

fn color_wave_presets() -> Vec<PresetTemplate> {
    vec![
        preset_with_desc(
            "Neon Scanner",
            "Fast cyan scan line",
            &[
                ("wave_color", ControlValue::Color([0.5, 1.0, 0.92, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.0, 0.01, 0.04, 1.0]),
                ),
                ("speed", ControlValue::Float(95.0)),
                ("wave_width", ControlValue::Float(20.0)),
                ("trail", ControlValue::Float(30.0)),
                (
                    "direction",
                    ControlValue::Enum("Horizontal Pass".to_owned()),
                ),
            ],
        ),
        preset(
            "Lava Flow",
            &[
                ("wave_color", ControlValue::Color([1.0, 0.3, 0.0, 1.0])),
                (
                    "background_color",
                    ControlValue::Color([0.15, 0.02, 0.0, 1.0]),
                ),
                ("color_mode", ControlValue::Enum("Custom".to_owned())),
                ("speed", ControlValue::Float(30.0)),
                ("wave_width", ControlValue::Float(80.0)),
                ("trail", ControlValue::Float(85.0)),
            ],
        ),
    ]
}

fn audio_pulse_presets() -> Vec<PresetTemplate> {
    vec![
        preset_with_desc(
            "Cyberpunk",
            "Hot pink on dark blue",
            &[
                ("base_color", ControlValue::Color([0.0, 0.02, 0.12, 1.0])),
                ("peak_color", ControlValue::Color([1.0, 0.1, 0.6, 1.0])),
                ("sensitivity", ControlValue::Float(2.5)),
                ("beat_decay", ControlValue::Float(0.88)),
            ],
        ),
        preset(
            "Fire Response",
            &[
                ("base_color", ControlValue::Color([0.08, 0.02, 0.0, 1.0])),
                ("peak_color", ControlValue::Color([1.0, 0.4, 0.0, 1.0])),
                ("sensitivity", ControlValue::Float(3.0)),
                ("beat_decay", ControlValue::Float(0.82)),
            ],
        ),
    ]
}

#[allow(
    clippy::too_many_lines,
    reason = "zone control list is intentionally authored inline for readability"
)]
fn color_zones_controls() -> Vec<ControlDefinition> {
    vec![
        dropdown_control(
            "zone_count",
            "Zone Count",
            "3",
            &["2", "3", "4", "5", "6", "7", "8", "9"],
            "Layout",
            "Number of active color zones.",
        ),
        dropdown_control(
            "layout",
            "Layout",
            "Columns",
            &["Columns", "Rows", "Grid"],
            "Layout",
            "Arrange zones as vertical columns, horizontal rows, or a 2D grid.",
        ),
        slider_control(
            "blend",
            "Blend Softness",
            0.15,
            0.0,
            1.0,
            0.01,
            "Layout",
            "Smoothness of transitions between adjacent zones. 0 = hard edges.",
        ),
        color_control(
            "zone_1",
            "Zone 1",
            [0.88, 0.08, 1.0, 1.0],
            "Zone Colors",
            "Color for zone 1.",
        ),
        color_control(
            "zone_2",
            "Zone 2",
            [0.0, 1.0, 0.85, 1.0],
            "Zone Colors",
            "Color for zone 2.",
        ),
        color_control(
            "zone_3",
            "Zone 3",
            [1.0, 0.25, 0.55, 1.0],
            "Zone Colors",
            "Color for zone 3.",
        ),
        color_control(
            "zone_4",
            "Zone 4",
            [0.31, 0.98, 0.48, 1.0],
            "Zone Colors",
            "Color for zone 4.",
        ),
        color_control(
            "zone_5",
            "Zone 5",
            [0.95, 0.98, 0.55, 1.0],
            "Zone Colors",
            "Color for zone 5.",
        ),
        color_control(
            "zone_6",
            "Zone 6",
            [1.0, 0.39, 0.39, 1.0],
            "Zone Colors",
            "Color for zone 6.",
        ),
        color_control(
            "zone_7",
            "Zone 7",
            [0.0, 0.4, 1.0, 1.0],
            "Zone Colors",
            "Color for zone 7.",
        ),
        color_control(
            "zone_8",
            "Zone 8",
            [1.0, 0.6, 0.0, 1.0],
            "Zone Colors",
            "Color for zone 8.",
        ),
        color_control(
            "zone_9",
            "Zone 9",
            [0.6, 0.0, 1.0, 1.0],
            "Zone Colors",
            "Color for zone 9.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

fn color_zones_presets() -> Vec<PresetTemplate> {
    vec![
        // ── Signature ────────────────────────────────────────────────────
        preset_with_desc(
            "SilkCircuit",
            "Electric purple, neon cyan, and coral",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.88, 0.08, 1.0, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 1.0, 0.85, 1.0])),
                ("zone_3", ControlValue::Color([1.0, 0.25, 0.55, 1.0])),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        preset_with_desc(
            "Fire & Ice",
            "Warm and cool contrast across the system",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.15, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.6, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.3, 1.0, 1.0])),
                ("blend", ControlValue::Float(0.2)),
            ],
        ),
        preset_with_desc(
            "RGB Diagnostic",
            "Pure red, green, blue columns",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.0, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 1.0, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.0, 1.0, 1.0])),
                ("blend", ControlValue::Float(0.0)),
            ],
        ),
        preset_with_desc(
            "Ocean Layers",
            "Horizontal depth bands from surface to deep",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([0.4, 0.85, 1.0, 1.0])),
                ("zone_2", ControlValue::Color([0.1, 0.5, 0.9, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.2, 0.6, 1.0])),
                ("zone_4", ControlValue::Color([0.0, 0.05, 0.2, 1.0])),
                ("blend", ControlValue::Float(0.3)),
            ],
        ),
        preset_with_desc(
            "Neon Matrix",
            "9-zone grid with vibrant neon palette",
            &[
                ("zone_count", ControlValue::Enum("9".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("blend", ControlValue::Float(0.25)),
            ],
        ),
        // ── Nature & Atmosphere ──────────────────────────────────────────
        preset_with_desc(
            "Sunset Boulevard",
            "Golden hour fading into deep twilight",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.85, 0.1, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.4, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([0.9, 0.1, 0.0, 1.0])),
                ("zone_4", ControlValue::Color([0.3, 0.0, 0.4, 1.0])),
                ("blend", ControlValue::Float(0.4)),
            ],
        ),
        preset_with_desc(
            "Arctic Aurora",
            "Northern lights dancing across the sky",
            &[
                ("zone_count", ControlValue::Enum("5".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.9, 0.3, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 0.8, 0.7, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.3, 0.9, 1.0])),
                ("zone_4", ControlValue::Color([0.4, 0.0, 0.8, 1.0])),
                ("zone_5", ControlValue::Color([0.8, 0.1, 0.5, 1.0])),
                ("blend", ControlValue::Float(0.35)),
            ],
        ),
        preset_with_desc(
            "Cherry Blossom",
            "Spring pinks from deep rose to soft bloom",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.4, 0.55, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.7, 0.75, 1.0])),
                ("zone_3", ControlValue::Color([0.85, 0.15, 0.4, 1.0])),
                ("blend", ControlValue::Float(0.35)),
            ],
        ),
        preset_with_desc(
            "Tropical Reef",
            "Coral, turquoise, deep blue, and sandy gold",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.35, 0.25, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 0.85, 0.7, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.2, 0.7, 1.0])),
                ("zone_4", ControlValue::Color([0.95, 0.75, 0.2, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        preset_with_desc(
            "Lava Flow",
            "Molten orange cooling into deep obsidian",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.5, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([0.8, 0.1, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([0.3, 0.02, 0.0, 1.0])),
                ("blend", ControlValue::Float(0.35)),
            ],
        ),
        preset_with_desc(
            "Deep Space",
            "Dark cosmic nebula in a 2x3 grid",
            &[
                ("zone_count", ControlValue::Enum("6".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.02, 0.15, 1.0])),
                ("zone_2", ControlValue::Color([0.15, 0.0, 0.3, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.1, 0.2, 1.0])),
                ("zone_4", ControlValue::Color([0.2, 0.0, 0.15, 1.0])),
                ("zone_5", ControlValue::Color([0.0, 0.05, 0.25, 1.0])),
                ("zone_6", ControlValue::Color([0.1, 0.0, 0.2, 1.0])),
                ("blend", ControlValue::Float(0.4)),
            ],
        ),
        preset_with_desc(
            "Golden Hour",
            "Warm amber through peach to rose gold",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.7, 0.1, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.55, 0.3, 1.0])),
                ("zone_3", ControlValue::Color([0.9, 0.4, 0.4, 1.0])),
                ("blend", ControlValue::Float(0.35)),
            ],
        ),
        preset_with_desc(
            "Blood Moon",
            "Deep crimson split",
            &[
                ("zone_count", ControlValue::Enum("2".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.7, 0.0, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([0.3, 0.0, 0.02, 1.0])),
                ("blend", ControlValue::Float(0.3)),
            ],
        ),
        preset_with_desc(
            "Emerald City",
            "Dark forest, bright emerald, and gold",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.4, 0.1, 1.0])),
                ("zone_2", ControlValue::Color([0.1, 0.9, 0.3, 1.0])),
                ("zone_3", ControlValue::Color([0.9, 0.75, 0.0, 1.0])),
                ("blend", ControlValue::Float(0.15)),
            ],
        ),
        // ── Neon & Cyber ─────────────────────────────────────────────────
        preset_with_desc(
            "Vaporwave",
            "Hot pink, purple, and teal retrowave",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.2, 0.6, 1.0])),
                ("zone_2", ControlValue::Color([0.5, 0.1, 0.9, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.9, 0.8, 1.0])),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        preset_with_desc(
            "Cyberpunk Alley",
            "Neon pink, blue, purple, and toxic green grid",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.0, 0.5, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 0.4, 1.0, 1.0])),
                ("zone_3", ControlValue::Color([0.6, 0.0, 1.0, 1.0])),
                ("zone_4", ControlValue::Color([0.2, 1.0, 0.0, 1.0])),
                ("blend", ControlValue::Float(0.08)),
            ],
        ),
        preset_with_desc(
            "Hacker Terminal",
            "Matrix green and void black",
            &[
                ("zone_count", ControlValue::Enum("2".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.9, 0.1, 1.0])),
                ("zone_2", ControlValue::Color([0.0, 0.15, 0.02, 1.0])),
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Midnight Jazz",
            "Deep navy, purple, gold accent, warm ivory",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.02, 0.02, 0.2, 1.0])),
                ("zone_2", ControlValue::Color([0.35, 0.0, 0.6, 1.0])),
                ("zone_3", ControlValue::Color([0.85, 0.7, 0.0, 1.0])),
                ("zone_4", ControlValue::Color([1.0, 0.9, 0.7, 1.0])),
                ("blend", ControlValue::Float(0.2)),
            ],
        ),
        preset_with_desc(
            "Stealth",
            "Barely-there dim blue and purple",
            &[
                ("zone_count", ControlValue::Enum("2".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.0, 0.02, 0.12, 1.0])),
                ("zone_2", ControlValue::Color([0.05, 0.0, 0.1, 1.0])),
                ("blend", ControlValue::Float(0.4)),
                ("brightness", ControlValue::Float(0.5)),
            ],
        ),
        // ── Pastel & Soft ────────────────────────────────────────────────
        preset_with_desc(
            "Candy Pastel",
            "Bright candy colors in a 2x3 grid",
            &[
                ("zone_count", ControlValue::Enum("6".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.5, 0.6, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.95, 0.4, 1.0])),
                ("zone_3", ControlValue::Color([0.4, 0.65, 1.0, 1.0])),
                ("zone_4", ControlValue::Color([0.4, 1.0, 0.6, 1.0])),
                ("zone_5", ControlValue::Color([0.7, 0.45, 1.0, 1.0])),
                ("zone_6", ControlValue::Color([1.0, 0.65, 0.35, 1.0])),
                ("blend", ControlValue::Float(0.1)),
            ],
        ),
        preset_with_desc(
            "Lavender Dream",
            "Soft purples and rose in layered rows",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Rows".to_owned())),
                ("zone_1", ControlValue::Color([0.7, 0.5, 1.0, 1.0])),
                ("zone_2", ControlValue::Color([0.5, 0.15, 0.85, 1.0])),
                ("zone_3", ControlValue::Color([0.9, 0.3, 0.6, 1.0])),
                ("zone_4", ControlValue::Color([0.75, 0.45, 0.95, 1.0])),
                ("blend", ControlValue::Float(0.35)),
            ],
        ),
        // ── Pride Flags ──────────────────────────────────────────────────
        preset_with_desc(
            "Trans Pride",
            "Light blue, pink, white, pink, blue stripes",
            &[
                ("zone_count", ControlValue::Enum("5".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.35, 0.7, 0.95, 1.0])),
                ("zone_2", ControlValue::Color([0.95, 0.5, 0.65, 1.0])),
                ("zone_3", ControlValue::Color([0.95, 0.95, 0.95, 1.0])),
                ("zone_4", ControlValue::Color([0.95, 0.5, 0.65, 1.0])),
                ("zone_5", ControlValue::Color([0.35, 0.7, 0.95, 1.0])),
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Bi Pride",
            "Magenta, purple, and blue bands",
            &[
                ("zone_count", ControlValue::Enum("3".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.85, 0.0, 0.45, 1.0])),
                ("zone_2", ControlValue::Color([0.6, 0.0, 0.6, 1.0])),
                ("zone_3", ControlValue::Color([0.0, 0.2, 0.85, 1.0])),
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Lesbian Pride",
            "Orange, white, and pink sunset stripes",
            &[
                ("zone_count", ControlValue::Enum("5".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.85, 0.35, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.6, 0.35, 1.0])),
                ("zone_3", ControlValue::Color([0.95, 0.95, 0.95, 1.0])),
                ("zone_4", ControlValue::Color([0.9, 0.4, 0.55, 1.0])),
                ("zone_5", ControlValue::Color([0.65, 0.0, 0.2, 1.0])),
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Non-Binary Pride",
            "Yellow, white, purple, and black stripes",
            &[
                ("zone_count", ControlValue::Enum("4".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.98, 0.95, 0.15, 1.0])),
                ("zone_2", ControlValue::Color([0.95, 0.95, 0.95, 1.0])),
                ("zone_3", ControlValue::Color([0.6, 0.2, 0.85, 1.0])),
                ("zone_4", ControlValue::Color([0.08, 0.08, 0.08, 1.0])),
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        preset_with_desc(
            "Rainbow Pride",
            "Classic six-stripe rainbow flag",
            &[
                ("zone_count", ControlValue::Enum("6".to_owned())),
                ("layout", ControlValue::Enum("Columns".to_owned())),
                ("zone_1", ControlValue::Color([0.9, 0.05, 0.05, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.5, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([1.0, 0.9, 0.0, 1.0])),
                ("zone_4", ControlValue::Color([0.0, 0.75, 0.15, 1.0])),
                ("zone_5", ControlValue::Color([0.0, 0.25, 0.85, 1.0])),
                ("zone_6", ControlValue::Color([0.55, 0.0, 0.55, 1.0])),
                ("blend", ControlValue::Float(0.05)),
            ],
        ),
        // ── Bold Blocks ──────────────────────────────────────────────────
        preset_with_desc(
            "Rainbow Blocks",
            "Full spectrum 3x3 grid with hard edges",
            &[
                ("zone_count", ControlValue::Enum("9".to_owned())),
                ("layout", ControlValue::Enum("Grid".to_owned())),
                ("zone_1", ControlValue::Color([1.0, 0.0, 0.0, 1.0])),
                ("zone_2", ControlValue::Color([1.0, 0.5, 0.0, 1.0])),
                ("zone_3", ControlValue::Color([1.0, 1.0, 0.0, 1.0])),
                ("zone_4", ControlValue::Color([0.0, 1.0, 0.0, 1.0])),
                ("zone_5", ControlValue::Color([0.0, 1.0, 1.0, 1.0])),
                ("zone_6", ControlValue::Color([0.0, 0.0, 1.0, 1.0])),
                ("zone_7", ControlValue::Color([0.3, 0.0, 0.5, 1.0])),
                ("zone_8", ControlValue::Color([0.6, 0.0, 1.0, 1.0])),
                ("zone_9", ControlValue::Color([1.0, 0.0, 0.6, 1.0])),
                ("blend", ControlValue::Float(0.0)),
            ],
        ),
    ]
}

/// Metadata definitions for all built-in effects.
///
/// Each entry carries a human-readable display name while the stable factory
/// key remains in the native source path (`builtin/<key>`).
#[allow(
    clippy::too_many_lines,
    reason = "builtin effect metadata is maintained as a single registry table"
)]
fn builtin_metadata() -> Vec<EffectMetadata> {
    vec![
        EffectMetadata {
            id: builtin_effect_id("solid_color"),
            name: "Solid Color".into(),
            author: "Hypercolor".into(),
            version: "0.1.0".into(),
            description: "Solid fills plus split and checker diagnostic scene patterns".into(),
            category: EffectCategory::Ambient,
            tags: vec![
                "solid".into(),
                "scene".into(),
                "diagnostic".into(),
                "utility".into(),
            ],
            controls: solid_color_controls(),
            presets: Vec::new(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/solid_color"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("gradient"),
            name: "Gradient".into(),
            author: "Hypercolor".into(),
            version: "0.1.0".into(),
            description:
                "Rich configurable gradient with vivid Oklch blending, saturation boost, and motion"
                    .into(),
            category: EffectCategory::Ambient,
            tags: vec![
                "gradient".into(),
                "scene".into(),
                "diagnostic".into(),
                "smooth".into(),
            ],
            controls: gradient_controls(),
            presets: gradient_presets(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/gradient"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("rainbow"),
            name: "Rainbow".into(),
            author: "Hypercolor".into(),
            version: "0.1.0".into(),
            description: "Vivid full-spectrum rainbow cycle with animated hue bands".into(),
            category: EffectCategory::Ambient,
            tags: vec!["rainbow".into(), "hue".into(), "colorful".into()],
            controls: rainbow_controls(),
            presets: Vec::new(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/rainbow"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("breathing"),
            name: "Breathing".into(),
            author: "Hypercolor".into(),
            version: "0.1.0".into(),
            description: "Smooth sinusoidal brightness pulsation".into(),
            category: EffectCategory::Ambient,
            tags: vec!["breathing".into(), "pulse".into(), "calm".into()],
            controls: breathing_controls(),
            presets: breathing_presets(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/breathing"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("audio_pulse"),
            name: "Audio Pulse".into(),
            author: "Hypercolor".into(),
            version: "0.1.0".into(),
            description: "Audio-reactive effect driven by RMS level and beat detection".into(),
            category: EffectCategory::Audio,
            tags: vec![
                "audio".into(),
                "reactive".into(),
                "beat".into(),
                "pulse".into(),
            ],
            controls: audio_pulse_controls(),
            presets: audio_pulse_presets(),
            audio_reactive: true,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/audio_pulse"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("color_wave"),
            name: "Color Wave".into(),
            author: "Hypercolor".into(),
            version: "0.1.0".into(),
            description:
                "Traveling wavefront strips with directional passes and configurable fade trails"
                    .into(),
            category: EffectCategory::Ambient,
            tags: vec!["wave".into(), "animation".into(), "pattern".into()],
            controls: color_wave_controls(),
            presets: color_wave_presets(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/color_wave"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("color_zones"),
            name: "Color Zones".into(),
            author: "Hypercolor".into(),
            version: "0.1.0".into(),
            description:
                "Multi-zone color grid with per-zone colors, flexible layouts, and smooth blending"
                    .into(),
            category: EffectCategory::Ambient,
            tags: vec![
                "zones".into(),
                "grid".into(),
                "static".into(),
                "scene".into(),
            ],
            controls: color_zones_controls(),
            presets: color_zones_presets(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/color_zones"),
            },
            license: Some("Apache-2.0".into()),
        },
    ]
}

/// Generate a deterministic ID for a built-in effect.
///
/// IDs must remain stable across daemon restarts so saved references
/// (profiles/scenes/API clients) continue to resolve.
fn builtin_effect_id(name: &str) -> EffectId {
    let key = format!("hypercolor:builtin:{name}");
    let mut hash: u128 = 0x6c62_69f0_7bb0_14d9_8d4f_1283_7ec6_3b8a;
    for byte in key.bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }

    let mut bytes = hash.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    EffectId::new(Uuid::from_bytes(bytes))
}

/// Register all built-in effects with the given registry.
///
/// Each effect is added as an [`EffectEntry`] with a synthetic source path
/// under `builtin/`. The entries are immediately available for lookup and
/// category filtering.
pub fn register_builtin_effects(registry: &mut EffectRegistry) {
    for metadata in builtin_metadata() {
        let source_path = metadata.source.path().to_path_buf();
        let entry = EffectEntry {
            metadata,
            source_path,
            modified: SystemTime::now(),
            state: EffectState::Loading,
        };
        registry.register(entry);
    }
}

/// Create a renderer instance for the named built-in effect.
///
/// Returns `None` if the name doesn't match any built-in effect.
/// Names must match exactly (e.g. `"solid_color"`, `"audio_pulse"`).
#[must_use]
pub fn create_builtin_renderer(name: &str) -> Option<Box<dyn EffectRenderer>> {
    match name {
        "solid_color" => Some(Box::new(SolidColorRenderer::new())),
        "gradient" => Some(Box::new(GradientRenderer::new())),
        "rainbow" => Some(Box::new(RainbowRenderer::new())),
        "breathing" => Some(Box::new(BreathingRenderer::new())),
        "audio_pulse" => Some(Box::new(AudioPulseRenderer::new())),
        "color_wave" => Some(Box::new(ColorWaveRenderer::new())),
        "color_zones" => Some(Box::new(ColorZonesRenderer::new())),
        _ => None,
    }
}
