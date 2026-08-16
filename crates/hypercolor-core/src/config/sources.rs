//! The single config load path and the Boot/Live split (Spec 76 §3.1–§3.2).
//!
//! [`ConfigManager::load_with_sources`] is the one pipeline that
//! materializes a config: parse → normalize → seed driver entries
//! (injected — the builtin driver set lives above core) → env overlay
//! → CLI overlay → validate. Precedence is CLI > env > file >
//! defaults, and every overlaid key is recorded as provenance.
//!
//! The Boot/Live split is enforced by ownership, not by a runtime
//! check: [`LoadedConfig::boot`] is consumed **by value** during
//! daemon initialization, so after init no live handle to a
//! [`BootConfig`] exists. Live readers go through
//! [`ConfigManager::live`], which hides the swap machinery behind a
//! plain `Deref` snapshot.
//!
//! Restart reporting derives from the key registry: at load, the
//! manager fingerprints every restart-classified key;
//! [`ConfigManager::pending_restart`] diffs the current persisted
//! state (with sticky overlays re-applied) against that fingerprint,
//! so it never reports a restart that would change nothing and never
//! misses a persisted change an overlay masks.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use hypercolor_types::config::{HypercolorConfig, RenderAccelerationMode, ServoGpuImportMode};
use hypercolor_types::config_registry::{ApplyPolicy, KeyPattern, registry};
use tracing::warn;

use super::ConfigManager;

/// Which layer a key's effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLayer {
    /// Built-in default or the persisted file.
    Persisted,
    /// Process-environment overlay.
    Env,
    /// Command-line overlay.
    Cli,
}

/// Command-line values that overlay the file (CLI > env > file).
///
/// Exactly the flags that genuinely write into the config today; the
/// struct grows a field per new overlay flag.
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    /// `--compositor-acceleration-mode`.
    pub compositor_acceleration_mode: Option<RenderAccelerationMode>,
    /// `--servo-gpu-import-mode`.
    pub servo_gpu_import_mode: Option<ServoGpuImportMode>,
}

/// Environment values that overlay the file (env > file).
///
/// `values` carries dotted-key overlays. The process-env grammar for
/// value overlays is deliberately unminted — no consumer needs one
/// yet — so [`EnvOverrides::from_process_env`] returns an empty
/// overlay today; the mechanism, precedence, and provenance are
/// contract now so growth is additive.
#[derive(Debug, Clone, Default)]
pub struct EnvOverrides {
    /// Dotted-key value overlays.
    pub values: Vec<(String, serde_json::Value)>,
}

impl EnvOverrides {
    /// Capture overlays from the process environment.
    #[must_use]
    pub fn from_process_env() -> Self {
        Self::default()
    }
}

/// Everything that feeds one config load.
pub struct ConfigSources {
    /// Explicit config file path. `Some` = must exist (load fails
    /// otherwise); `None` = the platform default path, falling back
    /// to defaults when absent.
    pub file: Option<PathBuf>,
    /// Command-line overlay.
    pub cli: CliOverrides,
    /// Environment overlay.
    pub env: EnvOverrides,
    /// Driver-entry seeding hook, injected because the builtin driver
    /// set is assembled above core. Runs after normalize, before
    /// overlays.
    pub seed: Option<fn(&mut HypercolorConfig)>,
}

impl ConfigSources {
    /// Sources for the platform-default file with no overlays.
    #[must_use]
    pub fn default_path() -> Self {
        Self {
            file: None,
            cli: CliOverrides::default(),
            env: EnvOverrides::default(),
            seed: None,
        }
    }
}

/// The boot-time config, consumed by value during initialization.
///
/// Holding one of these after init is the bug the type exists to
/// prevent: subsystems either take the values they freeze OUT of it
/// during construction, or read live through the manager.
#[derive(Debug)]
pub struct BootConfig(HypercolorConfig);

impl BootConfig {
    /// Wrap a config the caller already holds, bypassing the pipeline.
    ///
    /// Skips everything [`ConfigManager::load_with_sources`] does around
    /// the config itself: no file resolution, no daemon seeding hook, no
    /// env or CLI overlay, no capture validation, and no provenance. Only
    /// callers that already own a fully materialized config — the tests
    /// that drive initialization directly — have any business with it.
    #[doc(hidden)]
    #[must_use]
    pub fn from_config_unchecked(config: HypercolorConfig) -> Self {
        Self(config)
    }

    /// Consume into the inner config.
    #[must_use]
    pub fn into_inner(self) -> HypercolorConfig {
        self.0
    }
}

impl std::ops::Deref for BootConfig {
    type Target = HypercolorConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Where each overlaid key came from. Keys not listed are
/// [`SourceLayer::Persisted`].
#[derive(Debug, Clone, Default)]
pub struct ConfigProvenance {
    overlays: BTreeMap<String, SourceLayer>,
}

impl ConfigProvenance {
    /// The layer `key`'s effective value came from.
    #[must_use]
    pub fn layer_for(&self, key: &str) -> SourceLayer {
        self.overlays
            .get(key)
            .copied()
            .unwrap_or(SourceLayer::Persisted)
    }

    /// Every overlaid key with its layer.
    #[must_use]
    pub fn overlays(&self) -> &BTreeMap<String, SourceLayer> {
        &self.overlays
    }
}

/// The result of one load: the boot config (consumed by init), the
/// retained manager, and per-key provenance.
pub struct LoadedConfig {
    /// Boot-time values, consumed by value during initialization.
    pub boot: BootConfig,
    /// The retained live authority.
    pub manager: ConfigManager,
    /// Which overlay produced each non-persisted key.
    pub provenance: ConfigProvenance,
}

impl std::fmt::Debug for LoadedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedConfig")
            .field("overlays", &self.provenance.overlays)
            .finish_non_exhaustive()
    }
}

/// Boot-time state the manager retains for restart reporting.
#[derive(Debug, Default)]
pub(super) struct BootState {
    /// Restart-classified values as they were at boot.
    pub fingerprint: BTreeMap<String, serde_json::Value>,
    /// Overlays that will re-apply at the next start (CLI flags and
    /// env vars persist across restarts from the daemon's viewpoint).
    pub sticky_overlays: Vec<(String, serde_json::Value)>,
}

impl ConfigManager {
    /// The one load pipeline (Spec 76 §3.1): parse → normalize →
    /// seed → env overlay → CLI overlay → validate. Config files older
    /// than the current schema are refused by the parser, never
    /// migrated (§0).
    pub fn load_with_sources(sources: ConfigSources) -> Result<LoadedConfig> {
        let (path, mut config) = if let Some(explicit) = &sources.file {
            if !explicit.exists() {
                bail!("config file not found: {}", explicit.display());
            }
            (explicit.clone(), Self::load(explicit)?)
        } else {
            let default_path = Self::config_dir().join("hypercolor.toml");
            let config = if default_path.exists() {
                Self::load(&default_path)?
            } else {
                warn!(
                    path = %default_path.display(),
                    "No config file found, using built-in defaults"
                );
                // Defaults run the same normalize as file loads, so
                // the single-pipeline contract holds even when
                // normalization grows rules the defaults don't
                // already satisfy.
                Self::default_config()
            };
            (default_path, config)
        };

        if let Some(seed) = sources.seed {
            seed(&mut config);
        }

        let mut provenance = ConfigProvenance::default();
        let mut sticky_overlays = Vec::new();

        for (key, value) in &sources.env.values {
            set_dotted(&mut config, key, value.clone())
                .with_context(|| format!("applying env overlay {key}"))?;
            provenance.overlays.insert(key.clone(), SourceLayer::Env);
            sticky_overlays.push((key.clone(), value.clone()));
        }

        apply_cli_overlays(
            &sources.cli,
            &mut config,
            &mut provenance,
            &mut sticky_overlays,
        )?;

        config
            .capture
            .validate()
            .context("capture configuration is invalid")?;

        let manager = Self::with_config(path, config.clone());
        let fingerprint =
            restart_fingerprint(&config).context("boot config does not project to JSON")?;
        manager
            .boot_state
            .set(BootState {
                fingerprint,
                sticky_overlays,
            })
            .expect("boot state is set exactly once, at load");

        Ok(LoadedConfig {
            boot: BootConfig(config),
            manager,
            provenance,
        })
    }

    /// Restart-classified keys whose persisted value (with sticky
    /// overlays re-applied) differs from the value the daemon booted
    /// with. Empty when nothing needs a restart — and empty for
    /// managers created without a boot baseline.
    #[must_use]
    pub fn pending_restart(&self) -> Vec<String> {
        let Some(boot) = self.boot_state.get() else {
            return Vec::new();
        };
        let mut desired = self.live().clone_inner();
        let mut pending = std::collections::BTreeSet::new();
        for (key, value) in &boot.sticky_overlays {
            // A sticky overlay that no longer re-applies means the
            // next start genuinely differs from this one — that IS a
            // pending restart, not a value to compare around.
            if set_dotted(&mut desired, key, value.clone()).is_err() {
                pending.insert(root_of(key).to_owned());
            }
        }
        match restart_fingerprint(&desired) {
            Ok(desired_fingerprint) => {
                for (root, booted) in &boot.fingerprint {
                    if desired_fingerprint.get(root) != Some(booted) {
                        pending.insert(root.clone());
                    }
                }
            }
            // If the desired state no longer projects, every
            // restart-classified root is honestly unknown — report
            // all of them rather than silently none.
            Err(_) => pending.extend(boot.fingerprint.keys().cloned()),
        }
        pending.into_iter().collect()
    }
}

fn apply_cli_overlays(
    cli: &CliOverrides,
    config: &mut HypercolorConfig,
    provenance: &mut ConfigProvenance,
    sticky: &mut Vec<(String, serde_json::Value)>,
) -> Result<()> {
    if let Some(mode) = cli.compositor_acceleration_mode {
        config.effect_engine.compositor_acceleration_mode = mode;
        record_cli(
            provenance,
            sticky,
            "effect_engine.compositor_acceleration_mode",
            serde_json::to_value(mode)?,
        );
    }
    if let Some(mode) = cli.servo_gpu_import_mode {
        config.rendering.servo_gpu_import.mode = mode;
        record_cli(
            provenance,
            sticky,
            "rendering.servo_gpu_import.mode",
            serde_json::to_value(mode)?,
        );
    }
    Ok(())
}

fn record_cli(
    provenance: &mut ConfigProvenance,
    sticky: &mut Vec<(String, serde_json::Value)>,
    key: &str,
    value: serde_json::Value,
) {
    provenance.overlays.insert(key.to_owned(), SourceLayer::Cli);
    sticky.push((key.to_owned(), value));
}

/// Set a dotted key inside the config via its JSON projection,
/// re-validating through deserialization.
fn set_dotted(config: &mut HypercolorConfig, key: &str, value: serde_json::Value) -> Result<()> {
    if !hypercolor_types::config_registry::is_valid_key(key) {
        bail!("malformed config key: {key:?}");
    }
    let mut root = serde_json::to_value(&*config)?;
    let mut cursor = &mut root;
    let mut segments = key.split('.').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            let object = cursor
                .as_object_mut()
                .with_context(|| format!("{key}: parent of {segment} is not an object"))?;
            object.insert(segment.to_owned(), value);
            break;
        }
        let object = cursor
            .as_object_mut()
            .with_context(|| format!("{key}: {segment} is not an object"))?;
        cursor = object
            .entry(segment.to_owned())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    }
    *config = serde_json::from_value(root).with_context(|| format!("overlay {key} rejected"))?;
    Ok(())
}

/// Snapshot the value of every restart-classified registry root.
///
/// A restart section's snapshot excludes nested keys whose own more
/// specific descriptor is not restart-classified (the live render
/// knobs inside the otherwise-frozen `daemon` section), so changing
/// those never flags a restart.
fn restart_fingerprint(config: &HypercolorConfig) -> Result<BTreeMap<String, serde_json::Value>> {
    let root = serde_json::to_value(config).context("config does not project to JSON")?;
    let mut fingerprint = BTreeMap::new();
    for descriptor in registry() {
        if !matches!(descriptor.apply, ApplyPolicy::Restart) {
            continue;
        }
        let section_root = descriptor.pattern.root();
        if section_root.is_empty() {
            continue;
        }
        let Some(value) = lookup_dotted(&root, section_root) else {
            continue;
        };
        let mut value = value.clone();
        if let serde_json::Value::Object(fields) = &mut value {
            fields.retain(|field, _| {
                let leaf = format!("{section_root}.{field}");
                matches!(
                    hypercolor_types::config_registry::descriptor_for(&leaf).apply,
                    ApplyPolicy::Restart
                )
            });
        }
        fingerprint.insert(section_root.to_owned(), value);
    }
    // Exact restart keys nested inside non-restart sections.
    for descriptor in registry() {
        if let (KeyPattern::Exact(key), ApplyPolicy::Restart) =
            (descriptor.pattern, descriptor.apply)
            && let Some(value) = lookup_dotted(&root, key)
        {
            fingerprint.insert(key.to_owned(), value.clone());
        }
    }
    Ok(fingerprint)
}

fn root_of(key: &str) -> &str {
    key.split('.').next().unwrap_or(key)
}

fn lookup_dotted<'v>(root: &'v serde_json::Value, key: &str) -> Option<&'v serde_json::Value> {
    let mut cursor = root;
    for segment in key.split('.') {
        cursor = cursor.as_object()?.get(segment)?;
    }
    Some(cursor)
}
