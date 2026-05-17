//! Native media-player effect.

use std::path::{Path, PathBuf};

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, RgbaF32};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource,
};
use hypercolor_types::layer::{LoopMode, MediaPlayback};

use super::common::{
    asset_control, builtin_effect_id, color_control, dropdown_control, slider_control,
};
use crate::effect::media::MediaProducer;
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

pub struct MediaPlayerRenderer {
    asset: String,
    producer: Option<MediaProducer>,
    playback: MediaPlayback,
    brightness: f32,
    tint: [f32; 4],
    tint_strength: f32,
    hue_shift: f32,
}

impl MediaPlayerRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            asset: String::new(),
            producer: None,
            playback: MediaPlayback::default(),
            brightness: 1.0,
            tint: [1.0, 1.0, 1.0, 1.0],
            tint_strength: 0.0,
            hue_shift: 0.0,
        }
    }

    fn set_asset(&mut self, value: &str) {
        if self.asset == value {
            return;
        }
        value.clone_into(&mut self.asset);
        self.producer = media_producer_from_control_value(value);
    }
}

impl Default for MediaPlayerRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for MediaPlayerRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        let Some(producer) = &self.producer else {
            canvas.clear();
            return Ok(());
        };

        let rendered = producer.render_frame(
            &self.playback,
            time_secs_to_elapsed_ms(input.time_secs),
            input.canvas_width,
            input.canvas_height,
        );
        *canvas = rendered;
        apply_output_adjustments(
            canvas,
            self.brightness,
            self.tint,
            self.tint_strength,
            self.hue_shift,
        );
        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "asset" => match value {
                ControlValue::Text(asset) | ControlValue::Enum(asset) => self.set_asset(asset),
                _ => {}
            },
            "loop_mode" => {
                if let ControlValue::Enum(value) | ControlValue::Text(value) = value {
                    self.playback.loop_mode = loop_mode_from_control(value);
                }
            }
            "speed" => {
                if let Some(value) = value.as_f32() {
                    self.playback.speed = value.clamp(0.0, 4.0);
                }
            }
            "brightness" => {
                if let Some(value) = value.as_f32() {
                    self.brightness = value.clamp(0.0, 2.0);
                }
            }
            "tint" => {
                if let ControlValue::Color(value) = value {
                    self.tint = *value;
                }
            }
            "tint_strength" => {
                if let Some(value) = value.as_f32() {
                    self.tint_strength = value.clamp(0.0, 1.0);
                }
            }
            "hue_shift" => {
                if let Some(value) = value.as_f32() {
                    self.hue_shift = value;
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}

fn media_producer_from_control_value(value: &str) -> Option<MediaProducer> {
    let path = Path::new(value.trim());
    if !path.exists() {
        return None;
    }
    MediaProducer::from_path(path, mime_type_for_path(path)?).ok()
}

fn mime_type_for_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "gif" => Some("image/gif"),
        "apng" => Some("image/apng"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn loop_mode_from_control(value: &str) -> LoopMode {
    match value {
        "none" | "None" => LoopMode::None,
        "ping_pong" | "PingPong" | "ping-pong" => LoopMode::PingPong,
        _ => LoopMode::Loop,
    }
}

fn apply_output_adjustments(
    canvas: &mut Canvas,
    brightness: f32,
    tint: [f32; 4],
    tint_strength: f32,
    _hue_shift: f32,
) {
    let tint_strength = tint_strength.clamp(0.0, 1.0);
    for pixel in canvas.as_rgba_bytes_mut().chunks_exact_mut(BYTES_PER_PIXEL) {
        let mut color = RgbaF32::from_srgb_u8(pixel[0], pixel[1], pixel[2], pixel[3]);
        color.r *= brightness;
        color.g *= brightness;
        color.b *= brightness;
        if tint_strength > 0.0 {
            color.r = color
                .r
                .mul_add(1.0 - tint_strength, tint[0] * tint_strength);
            color.g = color
                .g
                .mul_add(1.0 - tint_strength, tint[1] * tint_strength);
            color.b = color
                .b
                .mul_add(1.0 - tint_strength, tint[2] * tint_strength);
        }
        let rgba = color.to_srgba();
        pixel[0] = rgba.r;
        pixel[1] = rgba.g;
        pixel[2] = rgba.b;
        pixel[3] = rgba.a;
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "media playback receives bounded render-loop elapsed seconds"
)]
fn time_secs_to_elapsed_ms(time_secs: f32) -> u32 {
    (time_secs * 1_000.0).round().max(0.0) as u32
}

fn controls() -> Vec<ControlDefinition> {
    vec![
        asset_control(
            "asset",
            "Asset",
            "Media",
            "Image, GIF, APNG, or image-sequence asset to render.",
        ),
        dropdown_control(
            "fit",
            "Fit",
            "Cover",
            &["Contain", "Cover", "Stretch", "Tile", "Mirror"],
            "Layout",
            "How the media maps onto the output canvas.",
        ),
        dropdown_control(
            "loop_mode",
            "Loop Mode",
            "Loop",
            &["None", "Loop", "PingPong"],
            "Playback",
            "End-of-stream behavior for animated media.",
        ),
        slider_control(
            "speed",
            "Speed",
            1.0,
            0.0,
            4.0,
            0.05,
            "Playback",
            "Playback speed multiplier.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            2.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
        color_control(
            "tint",
            "Tint",
            [1.0, 1.0, 1.0, 1.0],
            "Output",
            "Tint color blended into the media output.",
        ),
        slider_control(
            "tint_strength",
            "Tint Strength",
            0.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Amount of tint color applied to the media output.",
        ),
        slider_control(
            "hue_shift",
            "Hue Shift",
            0.0,
            -std::f32::consts::PI,
            std::f32::consts::PI,
            0.01,
            "Output",
            "Reserved hue rotation control for media output.",
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("media_player"),
        name: "Media Player".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description: "Play an image or animated media asset on a render group".into(),
        category: EffectCategory::Source,
        tags: vec![
            "media".into(),
            "gif".into(),
            "asset".into(),
            "source".into(),
        ],
        controls: controls(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/media_player"),
        },
        license: Some("Apache-2.0".into()),
    }
}
