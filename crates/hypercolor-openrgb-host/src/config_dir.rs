//! Hypercolor's managed OpenRGB configuration directory and detector partition.
//!
//! OpenRGB decides which hardware to claim through the `Detectors.detectors`
//! map in `OpenRGB.json`. Hypercolor keeps its own config directory for the
//! headless server and rewrites that map so OpenRGB skips every device a
//! native Hypercolor driver can claim: per USB device where the detector id
//! map knows the detector, per name prefix where it does not. Everything
//! else in the file (SMBus settings, plugin state, the user's own detector
//! toggles) is preserved by a read-modify-write and a durable replace.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tracing::{debug, info};

use crate::detector_ids::{UsbDeviceId, detector_usb_claim, detector_usb_ids};
use crate::error::{HostError, Result};
use crate::types::ManagedConfigDir;

/// Directory name under Hypercolor's data dir that holds the OpenRGB config.
pub const MANAGED_DIR_NAME: &str = "openrgb";

/// The `Detectors` object key in `OpenRGB.json`.
pub const DETECTORS_SECTION: &str = "Detectors";
/// The detector map key inside the `Detectors` object.
pub const DETECTORS_MAP: &str = "detectors";

const EMBEDDED_DETECTORS_TOML: &str = include_str!("../data/detectors.toml");

/// One native Hypercolor driver family and the OpenRGB detectors it owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorFamily {
    /// Hypercolor driver id (`razer`, `lianli`, `corsair`, ...).
    pub driver_id: String,
    /// OpenRGB detector name prefixes, matched case-insensitively.
    pub prefixes: Vec<String>,
    /// Known OpenRGB detector names used to seed a fresh config.
    #[serde(default)]
    pub detectors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DetectorTable {
    #[serde(default)]
    family: Vec<DetectorFamily>,
}

/// What [`write_detector_partition`] produced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorPartition {
    /// Detector names written as `false`.
    pub disabled: Vec<String>,
    /// Detector names written as `true`.
    pub enabled: Vec<String>,
}

static DETECTOR_FAMILIES: LazyLock<Vec<DetectorFamily>> = LazyLock::new(|| {
    parse_detector_table(EMBEDDED_DETECTORS_TOML)
        .expect("embedded data/detectors.toml must parse; run the crate tests")
});

/// Parse a detector table in the `data/detectors.toml` schema.
pub fn parse_detector_table(text: &str) -> Result<Vec<DetectorFamily>> {
    let table: DetectorTable =
        toml::from_str(text).map_err(|error| HostError::DetectorTable(error.to_string()))?;
    Ok(table.family)
}

/// The embedded detector families, one per native driver with OpenRGB overlap.
#[must_use]
pub fn detector_families() -> &'static [DetectorFamily] {
    &DETECTOR_FAMILIES
}

/// Detector prefixes owned by the given native driver ids.
///
/// Unknown ids are ignored. The result is deduplicated and sorted.
#[must_use]
pub fn detector_prefixes_for_drivers<I, S>(driver_ids: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let wanted: BTreeSet<String> = driver_ids
        .into_iter()
        .map(|id| id.as_ref().to_ascii_lowercase())
        .collect();
    let prefixes: BTreeSet<String> = detector_families()
        .iter()
        .filter(|family| wanted.contains(&family.driver_id.to_ascii_lowercase()))
        .flat_map(|family| family.prefixes.iter().cloned())
        .collect();
    prefixes.into_iter().collect()
}

/// Whether `name` starts with any of `prefixes`, ignoring ASCII case.
#[must_use]
pub fn matches_prefix<S: AsRef<str>>(name: &str, prefixes: &[S]) -> bool {
    prefixes.iter().any(|prefix| {
        let prefix = prefix.as_ref();
        name.len() >= prefix.len()
            && name
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })
}

/// The managed OpenRGB config directory under Hypercolor's data dir.
#[must_use]
pub fn managed_config_dir(base_data_dir: &Path) -> ManagedConfigDir {
    ManagedConfigDir {
        root: base_data_dir.join(MANAGED_DIR_NAME),
    }
}

/// How the partition decides each OpenRGB detector (Spec 81 §3.1).
///
/// Prefixes come from the embedded family table; USB ids come from the
/// protocol catalogs the daemon publishes per driver. The default value
/// withholds nothing and hands nothing back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectorRules {
    /// Name prefixes withheld wholesale: every detector under one is
    /// disabled. Used for native families whose driver published no USB
    /// catalog, so nothing finer is known.
    pub disabled_prefixes: Vec<String>,
    /// Name prefixes of withheld families whose driver published its USB
    /// catalog. A detector under one is disabled when it claims a device in
    /// `claimed_usb_ids`, or when the id map does not know it at all; a
    /// mapped detector that claims none of them is handed back to OpenRGB.
    pub id_gated_prefixes: Vec<String>,
    /// Name prefixes handed back to OpenRGB (written `true`).
    pub re_enable_prefixes: Vec<String>,
    /// USB devices the withheld native drivers can claim. A mapped detector
    /// that claims one is disabled whatever its name.
    pub claimed_usb_ids: BTreeSet<UsbDeviceId>,
    /// Every USB device a registered native driver can claim, withheld or
    /// not. A mapped detector that claims one but none of `claimed_usb_ids`
    /// is handed back, so a detector disabled by id outside every family
    /// prefix is released once its native driver lets go.
    pub native_usb_ids: BTreeSet<UsbDeviceId>,
}

impl DetectorRules {
    /// Prefix-only rules: the conservative partition with no USB knowledge.
    #[must_use]
    pub fn by_prefix<S: AsRef<str>, R: AsRef<str>>(
        disabled_prefixes: &[S],
        re_enable_prefixes: &[R],
    ) -> Self {
        Self {
            disabled_prefixes: owned(disabled_prefixes),
            re_enable_prefixes: owned(re_enable_prefixes),
            ..Self::default()
        }
    }

    /// Whether the detector `name` is written enabled. `current` is its value
    /// in the existing file, if any.
    ///
    /// In order:
    ///
    /// 1. a mapped detector that claims a device in `claimed_usb_ids` is
    ///    disabled;
    /// 2. a name under `disabled_prefixes` is disabled;
    /// 3. a name under `id_gated_prefixes` is enabled when the id map knows
    ///    it (step 1 already proved it claims nothing withheld) and disabled
    ///    when it does not;
    /// 4. a name under `re_enable_prefixes` is enabled;
    /// 5. a mapped detector that claims a device in `native_usb_ids` is
    ///    enabled;
    /// 6. anything else keeps `current`, defaulting to enabled.
    #[must_use]
    pub fn detector_enabled(&self, name: &str, current: Option<bool>) -> bool {
        let claim = detector_usb_claim(name);
        if claim.is_some_and(|claim| claim.overlaps(&self.claimed_usb_ids))
            || matches_prefix(name, &self.disabled_prefixes)
        {
            false
        } else if matches_prefix(name, &self.id_gated_prefixes) {
            claim.is_some()
        } else if matches_prefix(name, &self.re_enable_prefixes)
            || claim.is_some_and(|claim| claim.overlaps(&self.native_usb_ids))
        {
            true
        } else {
            current.unwrap_or(true)
        }
    }
}

fn owned<S: AsRef<str>>(items: &[S]) -> Vec<String> {
    items.iter().map(|item| item.as_ref().to_owned()).collect()
}

/// Compute the detector map for a partition without touching the filesystem.
///
/// The universe of names is the union of `existing` keys, `seed_names`,
/// every detector listed in the embedded families, and every mapped
/// detector that claims a device in `rules.claimed_usb_ids` (so a fresh
/// config disables it before OpenRGB ever writes the file). Each name is
/// decided by [`DetectorRules::detector_enabled`].
///
/// Nothing flips an existing `false` to `true` unless a rule hands the name
/// back (a re-enable prefix, an id-gated prefix that proves the device is
/// not natively claimable, or a native catalog id that is no longer
/// withheld), so a user's own toggles for unrelated detectors survive.
#[must_use]
pub fn partition_detectors(
    existing: &Map<String, Value>,
    rules: &DetectorRules,
    seed_names: Option<&[String]>,
) -> BTreeMap<String, bool> {
    let mut universe: BTreeSet<&str> = existing.keys().map(String::as_str).collect();
    universe.extend(seed_names.into_iter().flatten().map(String::as_str));
    universe.extend(
        detector_families()
            .iter()
            .flat_map(|family| family.detectors.iter().map(String::as_str)),
    );
    if !rules.claimed_usb_ids.is_empty() {
        universe.extend(
            detector_usb_ids()
                .iter()
                .filter(|(_, claim)| claim.overlaps(&rules.claimed_usb_ids))
                .map(|(name, _)| name.as_str()),
        );
    }

    universe
        .into_iter()
        .map(|name| {
            let current = existing.get(name).and_then(Value::as_bool);
            (name.to_owned(), rules.detector_enabled(name, current))
        })
        .collect()
}

/// Write `OpenRGB.json` in `dir` with `Detectors.detectors` partitioned.
///
/// Every detector name is decided by `rules` (see [`partition_detectors`]
/// and [`DetectorRules::detector_enabled`]). Callers build `rules` from a
/// [`crate::DetectorPartitionPlan`] with
/// [`crate::DetectorPartitionPlan::detector_rules`]. `known_detectors` seeds
/// names OpenRGB has not written yet (for example a list obtained from a
/// running server). Any other key in an existing file is preserved, and the
/// file is replaced durably.
pub fn write_detector_partition(
    dir: &ManagedConfigDir,
    rules: &DetectorRules,
    known_detectors: Option<&[String]>,
) -> Result<DetectorPartition> {
    std::fs::create_dir_all(&dir.root).map_err(|source| HostError::Io {
        path: dir.root.clone(),
        source,
    })?;
    let path = dir.config_path();

    let mut root = read_existing_config(&path)?;
    let detectors_section = root
        .entry(DETECTORS_SECTION)
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(detectors_section) = detectors_section else {
        return Err(HostError::UnexpectedConfigShape {
            path,
            section: DETECTORS_SECTION,
        });
    };
    let existing_map = match detectors_section.get(DETECTORS_MAP) {
        None => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => {
            return Err(HostError::UnexpectedConfigShape {
                path,
                section: DETECTORS_MAP,
            });
        }
    };

    let partition = partition_detectors(&existing_map, rules, known_detectors);
    let mut report = DetectorPartition::default();
    let mut map = Map::new();
    for (name, enabled) in partition {
        if enabled {
            report.enabled.push(name.clone());
        } else {
            report.disabled.push(name.clone());
        }
        map.insert(name, Value::Bool(enabled));
    }
    detectors_section.insert(DETECTORS_MAP.to_owned(), Value::Object(map));

    let payload = hypercolor_persistence::serialize_json_pretty(&Value::Object(root))?;
    hypercolor_persistence::write_atomic(&path, &payload)?;
    info!(
        path = %path.display(),
        disabled = report.disabled.len(),
        enabled = report.enabled.len(),
        "wrote OpenRGB detector partition"
    );
    Ok(report)
}

fn read_existing_config(path: &Path) -> Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            debug!(path = %path.display(), "no existing OpenRGB.json, starting fresh");
            return Ok(Map::new());
        }
        Err(source) => {
            return Err(HostError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|source| HostError::InvalidExistingConfig {
            path: path.to_path_buf(),
            source,
        })?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(HostError::UnexpectedConfigShape {
            path: path.to_path_buf(),
            section: "root",
        }),
    }
}
