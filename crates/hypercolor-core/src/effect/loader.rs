//! HTML effect discovery and registry loading.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::{debug, warn};
use uuid::Uuid;

use hypercolor_types::canvas::srgb_to_linear;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, ControlValue, EffectId, EffectMetadata,
    EffectSource, EffectState, PresetTemplate,
};

use super::meta_parser::{
    HtmlControlKind, HtmlControlMetadata, HtmlPresetMetadata, parse_html_effect_metadata,
};
use super::paths::bundled_effects_root;
use super::{EffectEntry, EffectRegistry};

const HTML_EXTENSION: &str = "html";

/// Discovery error for a single file/path during HTML scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlDiscoveryError {
    pub path: PathBuf,
    pub message: String,
}

/// Summary report for one HTML discovery pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HtmlDiscoveryReport {
    pub scanned_files: usize,
    pub loaded_effects: usize,
    pub replaced_effects: usize,
    pub skipped_files: usize,
    pub errors: Vec<HtmlDiscoveryError>,
}

impl HtmlDiscoveryReport {
    /// Number of files that failed to parse/load.
    #[must_use]
    pub fn failed_files(&self) -> usize {
        self.errors.len()
    }
}

/// Returns the default effect search roots plus any extra config roots.
///
/// Search order:
/// 1. Bundled effects (`$XDG_DATA_HOME/hypercolor/effects/bundled/` or repo `effects/`)
/// 2. User effects (`$XDG_DATA_HOME/hypercolor/effects/user/`)
/// 3. Any extra directories from `[effect_engine] extra_effect_dirs` in config
#[must_use]
pub fn default_effect_search_paths(extra_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let bundled = bundled_effects_root();
    let user = super::paths::user_effects_dir();

    let mut deduped = Vec::new();
    let mut seen = HashSet::new();

    for path in [bundled, user]
        .into_iter()
        .chain(extra_dirs.iter().cloned())
    {
        let normalized = normalize_path(&path);
        if seen.insert(normalized) {
            deduped.push(path);
        }
    }

    deduped
}

/// Scan HTML effects under `search_paths` and register them into `registry`.
#[must_use]
pub fn register_html_effects(
    registry: &mut EffectRegistry,
    search_paths: &[PathBuf],
) -> HtmlDiscoveryReport {
    let mut report = HtmlDiscoveryReport::default();
    let mut visited_files = HashSet::new();

    for root in search_paths {
        if !root.exists() {
            debug!(path = %root.display(), "Skipping missing HTML effect root");
            continue;
        }

        let files = match collect_html_files(root) {
            Ok(files) => files,
            Err(error) => {
                report.errors.push(HtmlDiscoveryError {
                    path: root.clone(),
                    message: format!("failed to scan directory: {error}"),
                });
                continue;
            }
        };

        for file in files {
            report.scanned_files += 1;

            let normalized = normalize_path(&file);
            if !visited_files.insert(normalized) {
                report.skipped_files += 1;
                continue;
            }

            let raw_html = match fs::read_to_string(&file) {
                Ok(content) => content,
                Err(error) => {
                    report.errors.push(HtmlDiscoveryError {
                        path: file.clone(),
                        message: format!("failed to read file: {error}"),
                    });
                    continue;
                }
            };

            let parsed = parse_html_effect_metadata(&raw_html);
            let source_path = normalize_path(&file);
            let effect_name = if parsed.title == "Unnamed Effect" {
                fallback_effect_name(&file)
            } else {
                parsed.title
            };

            let modified = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or_else(|_| SystemTime::now());

            let controls: Vec<ControlDefinition> = parsed
                .controls
                .iter()
                .filter_map(control_definition_from_html)
                .collect();
            let mut presets: Vec<_> = parsed
                .presets
                .iter()
                .filter_map(|hp| preset_template_from_html(hp, &controls))
                .collect();
            presets.sort_by(|a, b| a.name.cmp(&b.name));

            let metadata = EffectMetadata {
                id: deterministic_html_effect_id(&source_path),
                name: effect_name,
                author: parsed.publisher,
                version: "0.1.0".to_owned(),
                description: parsed.description,
                category: parsed.category,
                tags: parsed.tags,
                controls,
                presets,
                audio_reactive: parsed.audio_reactive,
                screen_reactive: false,
                source: EffectSource::Html {
                    path: source_path.clone(),
                },
                license: None,
            };

            let entry = EffectEntry {
                metadata,
                source_path: source_path.clone(),
                modified,
                state: EffectState::Loading,
            };

            if registry.register(entry).is_some() {
                report.replaced_effects += 1;
            } else {
                report.loaded_effects += 1;
            }
        }
    }

    if report.failed_files() > 0 {
        for failure in &report.errors {
            warn!(
                path = %failure.path.display(),
                error = %failure.message,
                "Failed to load HTML effect"
            );
        }
    }

    report
}

fn collect_html_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut html_files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for child in fs::read_dir(&dir)? {
            let child = child?;
            let file_type = child.file_type()?;
            let path = child.path();

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            if file_type.is_file() && is_html_file(&path) {
                html_files.push(path);
            }
        }
    }

    html_files.sort();
    Ok(html_files)
}

fn is_html_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case(HTML_EXTENSION))
}

fn fallback_effect_name(file: &Path) -> String {
    file.file_stem()
        .and_then(OsStr::to_str)
        .map_or_else(|| "unnamed-effect".to_owned(), ToOwned::to_owned)
}

fn deterministic_html_effect_id(source_path: &Path) -> EffectId {
    let key = format!("hypercolor:html:{}", source_path.display());
    let mut hash: u128 = 0x6c62_69f0_7bb0_14d9_8d4f_1283_7ec6_3b8b;

    for byte in key.bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }

    let mut bytes = hash.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    EffectId::new(Uuid::from_bytes(bytes))
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn control_definition_from_html(raw: &HtmlControlMetadata) -> Option<ControlDefinition> {
    let id = raw.property.trim().to_owned();
    if id.is_empty() {
        return None;
    }

    let name = decode_html_entities(raw.label.trim());
    let labels: Vec<String> = raw
        .values
        .iter()
        .map(|value| decode_html_entities(value.trim()))
        .filter(|value| !value.is_empty())
        .collect();

    let (kind, control_type, default_value) = match raw.kind {
        HtmlControlKind::Number => (
            ControlKind::Number,
            ControlType::Slider,
            numeric_default(raw.default.as_deref()),
        ),
        HtmlControlKind::Boolean => (
            ControlKind::Boolean,
            ControlType::Toggle,
            ControlValue::Boolean(bool_default(raw.default.as_deref())),
        ),
        HtmlControlKind::Color => (
            ControlKind::Color,
            ControlType::ColorPicker,
            color_default(raw.default.as_deref()),
        ),
        HtmlControlKind::Combobox => (
            ControlKind::Combobox,
            ControlType::Dropdown,
            enum_default(raw.default.as_deref(), labels.first()),
        ),
        HtmlControlKind::Sensor => (
            ControlKind::Sensor,
            ControlType::TextInput,
            text_default(raw.default.as_deref(), ""),
        ),
        HtmlControlKind::Hue => (
            ControlKind::Hue,
            ControlType::Slider,
            numeric_default(raw.default.as_deref()),
        ),
        HtmlControlKind::Area => (
            ControlKind::Area,
            ControlType::Slider,
            numeric_default(raw.default.as_deref()),
        ),
        HtmlControlKind::Text => (
            ControlKind::Text,
            ControlType::TextInput,
            text_default(raw.default.as_deref(), ""),
        ),
        HtmlControlKind::Other(ref raw_kind) => (
            ControlKind::Other(raw_kind.clone()),
            ControlType::TextInput,
            text_default(raw.default.as_deref(), ""),
        ),
    };

    Some(ControlDefinition {
        id,
        name: if name.is_empty() {
            raw.property.clone()
        } else {
            name
        },
        kind,
        control_type,
        default_value,
        min: raw.min,
        max: raw.max,
        step: raw.step,
        labels,
        group: raw.group.clone(),
        tooltip: raw
            .tooltip
            .as_ref()
            .map(|tooltip| decode_html_entities(tooltip)),
        binding: None,
    })
}

fn numeric_default(raw: Option<&str>) -> ControlValue {
    let parsed = raw
        .map(str::trim)
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(0.0);
    ControlValue::Float(parsed)
}

fn bool_default(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn enum_default(raw: Option<&str>, first_option: Option<&String>) -> ControlValue {
    let fallback = first_option.map(String::as_str).unwrap_or_default();
    let selected = raw.map(str::trim).filter(|value| !value.is_empty());
    let decoded = decode_html_entities(selected.unwrap_or(fallback));
    ControlValue::Enum(decoded)
}

fn color_default(raw: Option<&str>) -> ControlValue {
    let hex = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("#ffffff");
    parse_hex_color(hex).unwrap_or(ControlValue::Color([1.0, 1.0, 1.0, 1.0]))
}

fn parse_hex_color(hex: &str) -> Option<ControlValue> {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(ControlValue::Color([
            srgb_to_linear(f32::from(r) / 255.0),
            srgb_to_linear(f32::from(g) / 255.0),
            srgb_to_linear(f32::from(b) / 255.0),
            1.0,
        ]))
    } else {
        None
    }
}

fn text_default(raw: Option<&str>, fallback: &str) -> ControlValue {
    let selected = raw.map(str::trim).filter(|value| !value.is_empty());
    let decoded = decode_html_entities(selected.unwrap_or(fallback));
    ControlValue::Text(decoded)
}

/// Convert a parsed HTML preset into a typed `PresetTemplate`.
///
/// Control values in the HTML preset are raw strings — this function resolves
/// each one against the effect's control definitions so the preset uses the
/// correct typed [`ControlValue`] for each entry.
fn preset_template_from_html(
    raw: &HtmlPresetMetadata,
    control_defs: &[ControlDefinition],
) -> Option<PresetTemplate> {
    if raw.name.is_empty() {
        return None;
    }

    let mut controls = std::collections::HashMap::new();
    for (key, raw_value) in &raw.controls {
        if let Some(def) = control_defs
            .iter()
            .find(|c| c.control_id().eq_ignore_ascii_case(key))
            && let Some(typed) = parse_raw_control_value(&def.kind, raw_value)
        {
            controls.insert(key.clone(), typed);
        }
    }

    Some(PresetTemplate {
        name: raw.name.clone(),
        description: raw.description.clone(),
        controls,
    })
}

/// Parse a raw string control value using the control's kind for type guidance.
fn parse_raw_control_value(kind: &ControlKind, raw: &str) -> Option<ControlValue> {
    match kind {
        ControlKind::Number | ControlKind::Hue | ControlKind::Area => {
            raw.parse::<f32>().ok().map(ControlValue::Float)
        }
        ControlKind::Boolean => Some(ControlValue::Boolean(matches!(
            raw.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ))),
        ControlKind::Color => {
            if raw.starts_with('#') {
                parse_hex_color(raw)
            } else {
                Some(ControlValue::Text(raw.to_owned()))
            }
        }
        ControlKind::Combobox => Some(ControlValue::Enum(raw.to_owned())),
        ControlKind::Sensor | ControlKind::Text | ControlKind::Other(_) => {
            Some(ControlValue::Text(raw.to_owned()))
        }
    }
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}
