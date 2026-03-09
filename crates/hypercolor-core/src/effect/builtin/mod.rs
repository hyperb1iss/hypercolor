//! Built-in native effect renderers.
//!
//! These renderers produce real [`Canvas`](hypercolor_types::canvas::Canvas) frames
//! entirely in Rust, with no GPU shaders or web engines required. They serve as the
//! always-available utility layer for fallback visuals, diagnostics, and basic scenes.
//!
//! # Available Effects
//!
//! | Name            | Category       | Description                                   |
//! |-----------------|----------------|-----------------------------------------------|
//! | `solid_color`   | Ambient        | Solid fills plus split and checker diagnostics |
//! | `gradient`      | Ambient        | Configurable linear/radial gradient utility    |
//! | `rainbow`       | Ambient        | Cycling rainbow hue sweep                      |
//! | `breathing`     | Ambient        | Sinusoidal brightness pulsation                |
//! | `audio_pulse`   | Audio          | RMS + beat-reactive color modulation           |
//! | `color_wave`    | Ambient        | Traveling sinusoidal wave                      |

mod audio_pulse;
mod breathing;
mod color_wave;
mod gradient;
mod rainbow;
mod solid_color;

use std::path::PathBuf;
use std::time::SystemTime;

use uuid::Uuid;

pub use self::audio_pulse::AudioPulseRenderer;
pub use self::breathing::BreathingRenderer;
pub use self::color_wave::ColorWaveRenderer;
pub use self::gradient::GradientRenderer;
pub use self::rainbow::RainbowRenderer;
pub use self::solid_color::SolidColorRenderer;
use super::registry::{EffectEntry, EffectRegistry};
use super::traits::EffectRenderer;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory, EffectId,
    EffectMetadata, EffectSource, EffectState,
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
            [0.88, 0.21, 1.0, 1.0],
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
            [1.0, 0.42, 0.76, 1.0],
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
            [0.5, 1.0, 0.92, 1.0],
            "Colors",
            "End color for the gradient.",
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
            "color",
            "Color",
            [0.5, 1.0, 0.92, 1.0],
            "Colors",
            "Wave color at peak intensity.",
        ),
        slider_control(
            "speed",
            "Speed",
            1.0,
            0.0,
            6.0,
            0.01,
            "Motion",
            "Wave travel speed in cycles per second.",
        ),
        slider_control(
            "wave_count",
            "Wave Count",
            3.0,
            1.0,
            12.0,
            1.0,
            "Shape",
            "How many wave peaks are visible across the canvas.",
        ),
        dropdown_control(
            "direction",
            "Direction",
            "Right",
            &["Right", "Left"],
            "Motion",
            "Travel direction of the wave.",
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
                "Configurable linear or radial gradient with motion, tiling, and output controls"
                    .into(),
            category: EffectCategory::Ambient,
            tags: vec![
                "gradient".into(),
                "scene".into(),
                "diagnostic".into(),
                "smooth".into(),
            ],
            controls: gradient_controls(),
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
            description: "Traveling sinusoidal wave of color across the canvas".into(),
            category: EffectCategory::Ambient,
            tags: vec!["wave".into(), "animation".into(), "pattern".into()],
            controls: color_wave_controls(),
            audio_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("builtin/color_wave"),
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
        _ => None,
    }
}
