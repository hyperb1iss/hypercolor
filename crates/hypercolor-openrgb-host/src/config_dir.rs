//! Hypercolor's managed OpenRGB configuration directory and detector partition.
//!
//! OpenRGB decides which hardware to claim through the `Detectors.detectors`
//! map in `OpenRGB.json`. Hypercolor keeps its own config directory for the
//! headless server and rewrites that map so OpenRGB skips every device
//! family a native Hypercolor driver already owns. Everything else in the
//! file (SMBus settings, plugin state, the user's own detector toggles) is
//! preserved by a read-modify-write and a durable replace.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tracing::{debug, info};

use crate::error::{HostError, Result};
use crate::types::ManagedConfigDir;

/// Directory name under Hypercolor's data dir that holds the OpenRGB config.
pub const MANAGED_DIR_NAME: &str = "openrgb";

/// The `Detectors` object key in `OpenRGB.json`.
pub const DETECTORS_SECTION: &str = "Detectors";
/// The detector map key inside the `Detectors` object.
pub const DETECTORS_MAP: &str = "detectors";

const EMBEDDED_DETECTORS_TOML: &str = include_str!("../../../data/openrgb/detectors.toml");

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
        .expect("embedded data/openrgb/detectors.toml must parse; run the crate tests")
});

/// Parse a detector table in the `data/openrgb/detectors.toml` schema.
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

/// Compute the detector map for a partition without touching the filesystem.
///
/// The universe of names is the union of `existing` keys, `seed_names`, and
/// every detector listed in the embedded families. For each name:
///
/// 1. a match on `disabled_prefixes` writes `false`;
/// 2. otherwise a match on any embedded family prefix writes `true`, so a
///    family Hypercolor stops claiming is handed back to OpenRGB;
/// 3. otherwise the existing value is preserved, defaulting to `true`.
#[must_use]
pub fn partition_detectors<S: AsRef<str>>(
    existing: &Map<String, Value>,
    disabled_prefixes: &[S],
    seed_names: Option<&[String]>,
) -> BTreeMap<String, bool> {
    let managed_prefixes: Vec<&str> = detector_families()
        .iter()
        .flat_map(|family| family.prefixes.iter().map(String::as_str))
        .collect();
    let mut universe: BTreeSet<&str> = existing.keys().map(String::as_str).collect();
    universe.extend(seed_names.into_iter().flatten().map(String::as_str));
    universe.extend(
        detector_families()
            .iter()
            .flat_map(|family| family.detectors.iter().map(String::as_str)),
    );

    universe
        .into_iter()
        .map(|name| {
            let enabled = if matches_prefix(name, disabled_prefixes) {
                false
            } else if matches_prefix(name, &managed_prefixes) {
                true
            } else {
                existing.get(name).and_then(Value::as_bool).unwrap_or(true)
            };
            (name.to_owned(), enabled)
        })
        .collect()
}

/// Write `OpenRGB.json` in `dir` with `Detectors.detectors` partitioned.
///
/// Detector names matching `disabled_prefixes` (case-insensitive) become
/// `false`; see [`partition_detectors`] for the full rule. `known_detectors`
/// seeds names OpenRGB has not written yet (for example a list obtained from
/// a running server). Any other key in an existing file is preserved, and the
/// file is replaced durably.
pub fn write_detector_partition<S: AsRef<str>>(
    dir: &ManagedConfigDir,
    disabled_prefixes: &[S],
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

    let partition = partition_detectors(&existing_map, disabled_prefixes, known_detectors);
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
