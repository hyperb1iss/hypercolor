//! HTML effect discovery and registry loading.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tracing::{debug, warn};
use uuid::Uuid;

use hypercolor_types::effect::{EffectId, EffectMetadata, EffectSource, EffectState};

use super::meta_parser::parse_html_effect_metadata;
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
#[must_use]
pub fn default_effect_search_paths(extra_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let bundled = bundled_effects_root();

    let mut deduped = Vec::new();
    let mut seen = HashSet::new();

    for path in std::iter::once(bundled).chain(extra_dirs.iter().cloned()) {
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
            let source_path = derive_source_path(root, &file);
            let effect_name = if parsed.title == "Unnamed Effect" {
                fallback_effect_name(&file)
            } else {
                parsed.title
            };

            let modified = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or_else(|_| SystemTime::now());

            let metadata = EffectMetadata {
                id: deterministic_html_effect_id(&source_path),
                name: effect_name,
                author: parsed.publisher,
                version: "0.1.0".to_owned(),
                description: parsed.description,
                category: parsed.category,
                tags: parsed.tags,
                source: EffectSource::Html {
                    path: source_path.clone(),
                },
                license: None,
            };

            let entry = EffectEntry {
                metadata,
                source_path: file.clone(),
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

fn derive_source_path(root: &Path, file: &Path) -> PathBuf {
    let relative = file
        .strip_prefix(root)
        .map_or_else(|_| file.to_path_buf(), Path::to_path_buf);

    if root.file_name().is_some_and(|name| name == "effects") {
        return relative;
    }

    root.file_name()
        .map_or(relative.clone(), |name| PathBuf::from(name).join(relative))
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
