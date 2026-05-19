//! Native media-player effect.

use std::path::PathBuf;
use std::sync::Arc;

use hypercolor_types::asset::AssetId;
use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, RgbaF32};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource,
};
use hypercolor_types::layer::{LoopMode, MediaPlayback};
use hypercolor_types::viewport::FitMode;
use tokio::sync::RwLock;

use super::common::{
    asset_control, builtin_effect_id, color_control, dropdown_control, slider_control,
};
use crate::asset::AssetLibrary;
use crate::effect::media::MediaProducer;
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

pub struct MediaPlayerRenderer {
    asset: String,
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    asset_dirty: bool,
    producer: Option<MediaProducer>,
    playback: MediaPlayback,
    fit_mode: FitMode,
    brightness: f32,
    tint: [f32; 4],
    tint_strength: f32,
    hue_shift: f32,
}

/// The asset library was momentarily write-locked during resolution.
struct LockContended;

impl MediaPlayerRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            asset: String::new(),
            asset_library: None,
            asset_dirty: false,
            producer: None,
            playback: MediaPlayback::default(),
            fit_mode: FitMode::Cover,
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
        self.producer = None;
        self.asset_dirty = true;
    }

    /// Resolve the untrusted `asset` control value as a content-addressed
    /// asset id. Resolving through the library instead of as a filesystem
    /// path keeps a control value from reading arbitrary local files.
    ///
    /// `Err(LockContended)` means the library was momentarily write-locked, so
    /// the caller retries on a later frame; every other outcome is final.
    fn resolve_producer(&self) -> Result<Option<MediaProducer>, LockContended> {
        let Ok(asset_id) = self.asset.trim().parse::<AssetId>() else {
            return Ok(None);
        };
        let Some(library) = self.asset_library.as_ref() else {
            return Ok(None);
        };
        let Ok(library) = library.try_read() else {
            return Err(LockContended);
        };
        let Some(record) = library.get(asset_id).cloned() else {
            return Ok(None);
        };
        let Ok(object_path) = library.object_path_for_hash(&record.hash_sha256) else {
            return Ok(None);
        };
        let stream_url_policy = library.stream_url_policy().clone();
        // Release the library lock before the potentially slow decode.
        drop(library);
        Ok(MediaProducer::from_path_with_stream_policy(
            &object_path,
            &record.mime_type,
            &stream_url_policy,
        )
        .ok())
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
        if self.asset_dirty
            && let Ok(producer) = self.resolve_producer()
        {
            self.producer = producer;
            self.asset_dirty = false;
        }
        let Some(producer) = &self.producer else {
            canvas.clear();
            return Ok(());
        };

        let rendered = producer.render_frame_with_fit(
            &self.playback,
            time_secs_to_elapsed_ms(input.time_secs),
            input.canvas_width,
            input.canvas_height,
            self.fit_mode,
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
            "fit" => {
                if let ControlValue::Enum(value) | ControlValue::Text(value) = value {
                    self.fit_mode = fit_mode_from_control(value);
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

    fn bind_asset_library(&mut self, library: Arc<RwLock<AssetLibrary>>) {
        self.asset_library = Some(library);
        if !self.asset.is_empty() {
            self.asset_dirty = true;
        }
    }

    fn destroy(&mut self) {}
}

fn loop_mode_from_control(value: &str) -> LoopMode {
    match value {
        "none" | "None" => LoopMode::None,
        "ping_pong" | "PingPong" | "ping-pong" => LoopMode::PingPong,
        _ => LoopMode::Loop,
    }
}

fn fit_mode_from_control(value: &str) -> FitMode {
    match value {
        "contain" | "Contain" => FitMode::Contain,
        "stretch" | "Stretch" => FitMode::Stretch,
        "tile" | "Tile" => FitMode::Tile,
        "mirror" | "Mirror" => FitMode::Mirror,
        _ => FitMode::Cover,
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
