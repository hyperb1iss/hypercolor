//! Built-in native effect renderers.
//!
//! These renderers produce real [`Canvas`](hypercolor_types::canvas::Canvas) frames
//! entirely in Rust, with no GPU shaders or web engines required. They serve as the
//! always-available utility layer for fallback visuals, diagnostics, and basic scenes.
//!
//! Each effect lives in its own submodule with its struct, [`EffectRenderer`]
//! impl, per-effect helpers, and a `metadata()` constructor that builds the
//! [`EffectMetadata`] entry exposed to the registry. Shared control and preset
//! constructors live in [`common`].
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
//! | `screen_cast`   | Utility        | Live screen crop with aspect-fit controls        |
//! | `calibration`   | Utility        | High-contrast layout calibration patterns        |

mod audio_pulse;
mod breathing;
mod calibration;
mod color_wave;
mod color_zones;
mod common;
mod gradient;
mod rainbow;
mod screen_cast;
mod solid_color;
#[cfg(feature = "servo")]
mod web_viewport;

use std::time::SystemTime;

use hypercolor_types::effect::{EffectMetadata, EffectState};

pub use self::audio_pulse::AudioPulseRenderer;
pub use self::breathing::BreathingRenderer;
pub use self::calibration::CalibrationRenderer;
pub use self::color_wave::ColorWaveRenderer;
pub use self::color_zones::ColorZonesRenderer;
pub use self::gradient::GradientRenderer;
pub use self::rainbow::RainbowRenderer;
pub use self::screen_cast::ScreenCastRenderer;
pub use self::solid_color::SolidColorRenderer;
#[cfg(feature = "servo")]
pub use self::web_viewport::WebViewportRenderer;
use super::registry::{EffectEntry, EffectRegistry};
use super::traits::EffectRenderer;

/// Collect metadata entries for every built-in effect.
fn builtin_metadata() -> Vec<EffectMetadata> {
    vec![
        solid_color::metadata(),
        gradient::metadata(),
        rainbow::metadata(),
        breathing::metadata(),
        audio_pulse::metadata(),
        color_wave::metadata(),
        color_zones::metadata(),
        screen_cast::metadata(),
        #[cfg(feature = "servo")]
        web_viewport::metadata(),
        calibration::metadata(),
    ]
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
        "screen_cast" => Some(Box::new(ScreenCastRenderer::new())),
        #[cfg(feature = "servo")]
        "web_viewport" => Some(Box::new(WebViewportRenderer::new())),
        "calibration" => Some(Box::new(CalibrationRenderer::new())),
        _ => None,
    }
}
