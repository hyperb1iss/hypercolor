//! Manual backups add account-wide choices to a rig's authored setup.

use super::{PortableBlob, PortableSetupError, PortableSetupV1};
use crate::config::{EffectErrorFallbackPolicy, HypercolorConfig, ShutdownBehavior};
use crate::library::FavoriteEffect;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// The manifest inside a portable backup. Binary content travels in separate
/// archive entries, deduplicated by SHA-256 across assets and effect files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableBackupV1 {
    pub schema_version: u32,
    pub setup: PortableSetupV1,
    pub preferences: Option<PortablePreferences>,
    pub favorites: Vec<FavoriteEffect>,
}

impl PortableBackupV1 {
    pub fn validate(&self) -> Result<(), PortableSetupError> {
        if self.schema_version != 1 {
            return Err(invalid("schema_version", "unsupported backup version"));
        }
        self.setup.validate()?;
        if let Some(preferences) = &self.preferences {
            preferences.validate()?;
        }
        let mut favorites = HashSet::new();
        for favorite in &self.favorites {
            if favorite.effect_id.0.is_nil() || !favorites.insert(favorite.effect_id) {
                return Err(invalid(
                    "favorites",
                    "effect identity is empty or duplicated",
                ));
            }
        }
        self.blobs()?;
        Ok(())
    }

    /// One descriptor per immutable object. Conflicting lengths for the same
    /// digest are rejected before reading or admitting any archive contents.
    pub fn blobs(&self) -> Result<BTreeMap<String, PortableBlob>, PortableSetupError> {
        let mut blobs = BTreeMap::new();
        let asset_blobs = self.setup.assets.values().map(|asset| &asset.content);
        let effect_blobs = self
            .setup
            .effects
            .values()
            .flat_map(|effect| effect.files.values());
        for blob in asset_blobs.chain(effect_blobs) {
            blob.validate()?;
            if let Some(previous) = blobs.insert(blob.sha256.clone(), blob.clone())
                && previous != *blob
            {
                return Err(invalid("blobs", "one digest has conflicting byte lengths"));
            }
        }
        Ok(blobs)
    }
}

/// Explicit portable settings. No field can contain credentials, filesystem
/// paths, source selections, driver switches, or arbitrary extension data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePreferences {
    pub shutdown: Option<PortableShutdownPreferences>,
    pub effect_error_fallback: Option<EffectErrorFallbackPolicy>,
    pub audio: Option<PortableAudioPreferences>,
    pub capture: Option<PortableCapturePreferences>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableShutdownPreferences {
    pub behavior: ShutdownBehavior,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableAudioPreferences {
    pub fft_size: u32,
    pub smoothing: f32,
    pub noise_gate: f32,
    pub beat_sensitivity: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableCapturePreferences {
    pub smoothing: f32,
    pub scene_cut_threshold: f32,
    pub letterbox: bool,
    pub letterbox_threshold: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub gamma: f32,
}

impl PortablePreferences {
    /// Project only the engine's finite portable fields.
    #[must_use]
    pub fn from_config(config: &HypercolorConfig) -> Self {
        Self {
            shutdown: Some(PortableShutdownPreferences {
                behavior: config.daemon.shutdown_behavior.clone(),
                color: config.daemon.shutdown_color.clone(),
            }),
            effect_error_fallback: Some(config.effect_engine.effect_error_fallback),
            audio: Some(PortableAudioPreferences {
                fft_size: config.audio.fft_size,
                smoothing: config.audio.smoothing,
                noise_gate: config.audio.noise_gate,
                beat_sensitivity: config.audio.beat_sensitivity,
            }),
            capture: Some(PortableCapturePreferences {
                smoothing: config.capture.smoothing,
                scene_cut_threshold: config.capture.scene_cut_threshold,
                letterbox: config.capture.letterbox,
                letterbox_threshold: config.capture.letterbox_threshold,
                saturation: config.capture.saturation,
                brightness: config.capture.brightness,
                gamma: config.capture.gamma,
            }),
        }
    }

    /// Apply to a candidate configuration before its ordinary transaction.
    /// Missing sections retain destination settings.
    pub fn apply_to_config(&self, config: &mut HypercolorConfig) -> Result<(), PortableSetupError> {
        self.validate()?;
        if let Some(shutdown) = &self.shutdown {
            config.daemon.shutdown_behavior = shutdown.behavior.clone();
            config.daemon.shutdown_color.clone_from(&shutdown.color);
        }
        if let Some(fallback) = self.effect_error_fallback {
            config.effect_engine.effect_error_fallback = fallback;
        }
        if let Some(audio) = &self.audio {
            config.audio.fft_size = audio.fft_size;
            config.audio.smoothing = audio.smoothing;
            config.audio.noise_gate = audio.noise_gate;
            config.audio.beat_sensitivity = audio.beat_sensitivity;
        }
        if let Some(capture) = &self.capture {
            config.capture.smoothing = capture.smoothing;
            config.capture.scene_cut_threshold = capture.scene_cut_threshold;
            config.capture.letterbox = capture.letterbox;
            config.capture.letterbox_threshold = capture.letterbox_threshold;
            config.capture.saturation = capture.saturation;
            config.capture.brightness = capture.brightness;
            config.capture.gamma = capture.gamma;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), PortableSetupError> {
        if let Some(shutdown) = &self.shutdown
            && !(shutdown.color.len() == 7
                && shutdown.color.starts_with('#')
                && shutdown.color.as_bytes()[1..]
                    .iter()
                    .all(u8::is_ascii_hexdigit))
        {
            return Err(invalid("preferences.shutdown.color", "expected #RRGGBB"));
        }
        if let Some(audio) = &self.audio {
            if !(32..=16384).contains(&audio.fft_size) {
                return Err(invalid(
                    "preferences.audio.fft_size",
                    "expected 32 through 16384",
                ));
            }
            bounded("audio.smoothing", audio.smoothing, 0.0, 1.0)?;
            bounded("audio.noise_gate", audio.noise_gate, 0.0, 1.0)?;
            bounded("audio.beat_sensitivity", audio.beat_sensitivity, 0.0, 2.0)?;
        }
        if let Some(capture) = &self.capture {
            bounded("capture.smoothing", capture.smoothing, 0.0, 1.0)?;
            bounded(
                "capture.scene_cut_threshold",
                capture.scene_cut_threshold,
                0.0,
                765.0,
            )?;
            bounded(
                "capture.letterbox_threshold",
                capture.letterbox_threshold,
                0.0,
                1.0,
            )?;
            bounded("capture.saturation", capture.saturation, 0.0, 4.0)?;
            bounded("capture.brightness", capture.brightness, 0.0, 4.0)?;
            bounded("capture.gamma", capture.gamma, 0.2, 5.0)?;
        }
        Ok(())
    }
}

fn bounded(path: &str, value: f32, min: f32, max: f32) -> Result<(), PortableSetupError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(invalid(
            format!("preferences.{path}"),
            "value is outside its supported range",
        ))
    }
}

fn invalid(path: impl Into<String>, message: &str) -> PortableSetupError {
    PortableSetupError {
        path: path.into(),
        message: message.into(),
    }
}
