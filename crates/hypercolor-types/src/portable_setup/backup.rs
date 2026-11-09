//! Manual backups add account-wide choices to a rig's authored setup.

use super::{PortableBlob, PortableSetupError, PortableSetupV1};
use crate::config::{EffectErrorFallbackPolicy, HypercolorConfig, ShutdownBehavior};
use crate::library::FavoriteEffect;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// The backup manifest schema this build writes.
pub const PORTABLE_BACKUP_SCHEMA_VERSION: u32 = 1;

/// The oldest backup manifest schema this build reads.
///
/// Every manifest from this floor through [`PORTABLE_BACKUP_SCHEMA_VERSION`]
/// is upgraded in memory before it is decoded, and the floor only moves at a
/// major release. Schema 1 is the first one that shipped.
pub const OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION: u32 = 1;

/// Rewrites a manifest of one schema into the next schema's shape, including
/// its `schema_version`.
type ManifestUpgrade = fn(&mut serde_json::Value) -> Result<(), PortableSetupError>;

/// One upgrade per schema below the current one, starting at the floor: entry
/// `n` lifts schema `OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION + n` by one. The
/// array length is tied to the two constants, so bumping the written schema
/// without adding its upgrade does not compile.
const MANIFEST_UPGRADES: [ManifestUpgrade;
    (PORTABLE_BACKUP_SCHEMA_VERSION - OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION) as usize] = [];

/// Why a backup manifest could not be decoded.
#[derive(Debug, thiserror::Error)]
pub enum PortableBackupDecodeError {
    /// The bytes are not JSON, or do not fit the schema they declare.
    #[error("backup JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The declared schema is missing, malformed, or outside the range this
    /// build reads.
    #[error(transparent)]
    Schema(#[from] PortableSetupError),
}

/// The manifest inside a portable backup. New backups carry configuration only.
/// Older resource descriptors remain readable without restoring their files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableBackupV1 {
    pub schema_version: u32,
    pub setup: PortableSetupV1,
    pub preferences: Option<PortablePreferences>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tui_preferences: Option<crate::client_preferences::TuiPreferences>,
    pub favorites: Vec<FavoriteEffect>,
}

impl PortableBackupV1 {
    /// Decode a manifest written by this build or by any older one.
    ///
    /// The declared `schema_version` is read before the typed decode. A
    /// manifest from a newer build is refused with an upgrade message rather
    /// than an unknown-field error, and an older one is upgraded in memory
    /// first. Decoding does not validate; call [`Self::validate`] next.
    ///
    /// # Errors
    ///
    /// Returns [`PortableBackupDecodeError::Schema`] when the declared schema
    /// is missing, malformed, older than
    /// [`OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION`], or newer than
    /// [`PORTABLE_BACKUP_SCHEMA_VERSION`], and
    /// [`PortableBackupDecodeError::Json`] when the document does not fit
    /// that schema.
    pub fn from_manifest_json(bytes: &[u8]) -> Result<Self, PortableBackupDecodeError> {
        #[derive(Deserialize)]
        struct DeclaredSchema {
            schema_version: Option<serde_json::Value>,
        }

        let declared = serde_json::from_slice::<DeclaredSchema>(bytes)?;
        let schema = readable_backup_schema(declared.schema_version.as_ref())?;
        if schema == PORTABLE_BACKUP_SCHEMA_VERSION {
            return Ok(serde_json::from_slice(bytes)?);
        }
        let mut document = serde_json::from_slice::<serde_json::Value>(bytes)?;
        let first = (schema - OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION) as usize;
        for upgrade in &MANIFEST_UPGRADES[first..] {
            upgrade(&mut document)?;
        }
        Ok(serde_json::from_value(document)?)
    }

    pub fn validate(&self) -> Result<(), PortableSetupError> {
        if self.schema_version != PORTABLE_BACKUP_SCHEMA_VERSION {
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

/// Accept a declared manifest schema this build can read, or explain why not.
fn readable_backup_schema(declared: Option<&serde_json::Value>) -> Result<u32, PortableSetupError> {
    let schema = declared
        .and_then(serde_json::Value::as_u64)
        .and_then(|schema| u32::try_from(schema).ok())
        .ok_or_else(|| {
            invalid(
                "schema_version",
                "a backup manifest must declare schema_version as a non-negative integer",
            )
        })?;
    if schema > PORTABLE_BACKUP_SCHEMA_VERSION {
        return Err(PortableSetupError {
            path: "schema_version".into(),
            message: format!(
                "this backup uses schema {schema}, which a newer Hypercolor wrote; this \
                 build reads backup schemas {OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION} through \
                 {PORTABLE_BACKUP_SCHEMA_VERSION}. Upgrade Hypercolor to restore it."
            ),
        });
    }
    if schema < OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION {
        return Err(PortableSetupError {
            path: "schema_version".into(),
            message: format!(
                "backup schema {schema} is older than any this build reads; this build \
                 reads backup schemas {OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION} through \
                 {PORTABLE_BACKUP_SCHEMA_VERSION}"
            ),
        });
    }
    Ok(schema)
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
