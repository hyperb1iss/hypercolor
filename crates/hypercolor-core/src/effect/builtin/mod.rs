//! Built-in native effect renderers.
//!
//! These renderers produce real [`Canvas`](hypercolor_types::canvas::Canvas) frames
//! entirely in Rust — no GPU shaders or web engines required. They serve as the
//! foundation layer: always available, zero external dependencies, and fast enough
//! to run at 60 fps on any hardware.
//!
//! # Available Effects
//!
//! | Name            | Category       | Description                          |
//! |-----------------|----------------|--------------------------------------|
//! | `solid_color`   | Ambient        | Single color fill                    |
//! | `gradient`      | Ambient        | Animated two-color gradient          |
//! | `rainbow`       | Ambient        | Cycling rainbow hue sweep            |
//! | `breathing`     | Ambient        | Sinusoidal brightness pulsation      |
//! | `audio_pulse`   | Audio          | RMS + beat-reactive color modulation |
//! | `color_wave`    | Ambient        | Traveling sinusoidal wave            |

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
    EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};

// ── Registry Helpers ────────────────────────────────────────────────────────

/// Metadata definitions for all built-in effects.
///
/// Each entry carries a stable name used as the factory key in
/// [`create_builtin_renderer`].
fn builtin_metadata() -> Vec<EffectMetadata> {
    vec![
        EffectMetadata {
            id: builtin_effect_id("solid_color"),
            name: "solid_color".into(),
            author: "hypercolor".into(),
            version: "0.1.0".into(),
            description: "Fills the canvas with a single solid color".into(),
            category: EffectCategory::Ambient,
            tags: vec!["solid".into(), "color".into(), "basic".into()],
            source: EffectSource::Native {
                path: PathBuf::from("builtin/solid_color"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("gradient"),
            name: "gradient".into(),
            author: "hypercolor".into(),
            version: "0.1.0".into(),
            description: "Animated two-color gradient with configurable direction".into(),
            category: EffectCategory::Ambient,
            tags: vec!["gradient".into(), "ambient".into(), "smooth".into()],
            source: EffectSource::Native {
                path: PathBuf::from("builtin/gradient"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("rainbow"),
            name: "rainbow".into(),
            author: "hypercolor".into(),
            version: "0.1.0".into(),
            description: "Cycling rainbow pattern using perceptual hue rotation".into(),
            category: EffectCategory::Ambient,
            tags: vec!["rainbow".into(), "hue".into(), "colorful".into()],
            source: EffectSource::Native {
                path: PathBuf::from("builtin/rainbow"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("breathing"),
            name: "breathing".into(),
            author: "hypercolor".into(),
            version: "0.1.0".into(),
            description: "Smooth sinusoidal brightness pulsation".into(),
            category: EffectCategory::Ambient,
            tags: vec!["breathing".into(), "pulse".into(), "calm".into()],
            source: EffectSource::Native {
                path: PathBuf::from("builtin/breathing"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("audio_pulse"),
            name: "audio_pulse".into(),
            author: "hypercolor".into(),
            version: "0.1.0".into(),
            description: "Audio-reactive effect driven by RMS level and beat detection".into(),
            category: EffectCategory::Audio,
            tags: vec![
                "audio".into(),
                "reactive".into(),
                "beat".into(),
                "pulse".into(),
            ],
            source: EffectSource::Native {
                path: PathBuf::from("builtin/audio_pulse"),
            },
            license: Some("Apache-2.0".into()),
        },
        EffectMetadata {
            id: builtin_effect_id("color_wave"),
            name: "color_wave".into(),
            author: "hypercolor".into(),
            version: "0.1.0".into(),
            description: "Traveling sinusoidal wave of color across the canvas".into(),
            category: EffectCategory::Ambient,
            tags: vec!["wave".into(), "animation".into(), "pattern".into()],
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
        let source_path = PathBuf::from(format!("builtin/{}", metadata.name));
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
